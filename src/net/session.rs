//! Per-connection protocol state machine, independent of I/O.
//!
//! The client's connection layer (`0x2aad40` frame decoder) moves through
//! states 3 (await ServerHello) → 4 (await LoginOk) → 5 (terminal,
//! encrypted). This mirrors it from the server side. Server→client messages
//! after LoginOk are sealed with the session key and the server nonce from
//! LoginOk, advanced by 2 per message; live-verified with a StatusNotice
//! (20103) that the client decoded and displayed.
//!
//! A server frame that fails the client's decryption is reported with 10099
//! (body: BE32 type of that frame), after which the client closes the
//! connection and logs in again. A server-sent 29997 is not decrypted by the
//! client and does not consume a nonce (LIVE_TEST_FEEDBACK.md §6).

use crate::protocol::scframe::Frame;
use anyhow::{Context, Result, bail, ensure};

use crate::{
    protocol::crypto,
    protocol::{
        Message, catalogue,
        messages::{
            AccountSummary, ChangeHomeName, ChatSilenceNotice, ClientHello, EmptyFriendList,
            EndClientTurn, EntityBatch, FriendIdReport, FriendListRequest, HomeSync, LoginOk,
            LoginOkTail, MaintenanceWarning, OptionalIdsReply, OtherDeviceConnected, OwnHomeData,
            PingReply, PostLoginReady, ServerHello, SessionReady, StatusNotice, ids,
        },
    },
};

/// A plaintext server message to send once the client is established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub msg_type: u16,
    pub body: Vec<u8>,
    /// Debugging aid: flip a tag bit so the client must reject the frame.
    pub corrupt: bool,
}

impl Outgoing {
    pub fn new<M: Message>(message: &M) -> Self {
        Self {
            msg_type: M::TYPE,
            body: message.to_bytes(),
            corrupt: false,
        }
    }

    /// Parses `TYPE`, `TYPE:HEXBODY`, `TYPE@FILE` (plaintext body from a
    /// file), a minimal typed message by name
    /// (`session-ready`, `ping-reply`, `home-sync`, `entity-batch`,
    /// `optional-ids-reply`, `account-summary`, `own-home`), `notice:KIND:TEXT`
    /// (20103), `disconnect:REASON` (25892), `maintenance:SECONDS` (20415),
    /// `chat-silence` (22220), or `AREA@HOME_JSON` for a fishing/town snapshot.
    /// A trailing `!` corrupts the auth tag.
    pub fn parse(spec: &str) -> Result<Self> {
        let (spec, corrupt) = match spec.strip_suffix('!') {
            Some(rest) => (rest, true),
            None => (spec, false),
        };
        let named = match spec {
            "session-ready" => Some(Self::new(&SessionReady::default())),
            "ping-reply" => Some(Self::new(&PingReply::default())),
            "home-sync" => Some(Self::new(&HomeSync::default())),
            "own-home" => Some(Self::new(&OwnHomeData::default())),
            "entity-batch" => Some(Self::new(&EntityBatch::default())),
            "optional-ids-reply" => Some(Self::new(&OptionalIdsReply)),
            "account-summary" => Some(Self::new(&AccountSummary)),
            _ => None,
        };
        if let Some(out) = named {
            return Ok(Self { corrupt, ..out });
        }
        if spec == "chat-silence" {
            return Ok(Self {
                corrupt,
                ..Self::new(&ChatSilenceNotice::Silenced)
            });
        }
        if let Some(seconds) = spec.strip_prefix("maintenance:") {
            let message = MaintenanceWarning(seconds.parse().context("maintenance seconds")?);
            let out = Self::new(&message);
            out.validate_body()?;
            return Ok(Self { corrupt, ..out });
        }
        if let Some((area, path)) = spec.split_once('@') {
            if let Some(area) = AreaSnapshot::from_name(area) {
                let home = crate::game::home::HomeData::load(std::path::Path::new(path))?;
                return Ok(Self {
                    corrupt,
                    ..area.snapshot(home)
                });
            }
        }
        if let Some(reason) = spec.strip_prefix("disconnect:") {
            let message = OtherDeviceConnected {
                reason: reason.parse().context("disconnect reason")?,
            };
            return Ok(Self {
                corrupt,
                ..Self::new(&message)
            });
        }
        if let Some(rest) = spec.strip_prefix("notice:") {
            let (kind, text) = rest.split_once(':').unwrap_or((rest, ""));
            let notice = StatusNotice::new(kind.parse().context("notice kind")?, text);
            return Ok(Self {
                corrupt,
                ..Self::new(&notice)
            });
        }
        if let Some((ty, path)) = spec.split_once('@') {
            let msg_type: u16 = ty.trim().parse().context("message type")?;
            let body = std::fs::read(path).with_context(|| format!("reading body {path}"))?;
            let out = Self {
                msg_type,
                body,
                corrupt,
            };
            out.validate_body()?;
            return Ok(out);
        }
        let (ty, hex) = spec.split_once(':').unwrap_or((spec, ""));
        let msg_type: u16 = ty.trim().parse().context("message type")?;
        ensure!(hex.len() % 2 == 0, "hex body must have an even length");
        let body = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<_, _>>()
            .context("hex body")?;
        let out = Self {
            msg_type,
            body,
            corrupt,
        };
        out.validate_body()?;
        Ok(out)
    }

    fn validate_body(&self) -> Result<()> {
        match self.msg_type {
            ChatSilenceNotice::TYPE => {
                ChatSilenceNotice::from_bytes(&self.body)?;
            }
            MaintenanceWarning::TYPE => {
                MaintenanceWarning::from_bytes(&self.body)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum AreaSnapshot {
    OwnFishing = 22852,
    OtherFishing = 28185,
    OwnTown = 26650,
    OtherTown = 26359,
    AiTown = 28557,
}

impl AreaSnapshot {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "own-fishing" => Self::OwnFishing,
            "other-fishing" => Self::OtherFishing,
            "own-town" => Self::OwnTown,
            "other-town" => Self::OtherTown,
            "ai-town" => Self::AiTown,
            _ => return None,
        })
    }

    fn snapshot(self, home: crate::game::home::HomeData) -> Outgoing {
        // The area's blobs come from the provided file. The message ID selects
        // the scene and the corresponding own/other avatar decoder.
        let body = match self {
            Self::OtherFishing | Self::OtherTown => crate::protocol::messages::OtherHomeData {
                blobs: home.blobs(),
            }
            .to_bytes(),
            Self::AiTown => crate::protocol::messages::AiHomeData {
                blobs: home.blobs(),
            }
            .to_bytes(),
            Self::OwnFishing | Self::OwnTown => home.message().to_bytes(),
        };
        Outgoing {
            msg_type: self as u16,
            body,
            corrupt: false,
        }
    }
}

pub struct Config {
    pub secret: [u8; 32],
    pub account_id: u64,
    pub pass_token: String,
    /// Persistent runtime identity; None keeps explicit isolated test configs.
    pub accounts: Option<std::sync::Arc<crate::storage::accounts::Store>>,
    /// Use the account's current stored home instead of an explicit experiment list.
    pub account_home: bool,
    pub replay_commands: bool,
    pub dev_friends: bool,
    /// Nonpersistent overrides, reapplied whenever a stored home is sent.
    pub progress: crate::game::progression::Overrides,
    /// Sent after the client's `PostLoginReady` (10366), in order.
    pub after_ready: Vec<Outgoing>,
    /// How the LoginOk body ends (crash investigation, RECONCILIATION.md T5).
    pub login_ok_tail: LoginOkTail,
    /// Server→client nonce advance per message; the last entry repeats.
    pub s2c_nonce_steps: Vec<u16>,
    /// Delay between receiving 10366 and sending `after_ready`.
    pub after_ready_delay: std::time::Duration,
    /// Request-triggered replies: when the client sends `trigger`, send each
    /// listed message. Each reply TYPE is sent at most once per connection.
    pub replies: Vec<(u16, Vec<Outgoing>)>,
    /// Triggers whose replies are sent on EVERY occurrence (e.g. 13198,
    /// which the client sends at startup and again when visiting Greg).
    pub repeat_triggers: Vec<u16>,
    /// Server policy: re-send on each own-farm mode notification (10366).
    /// : this mode-entry path needs no snapshot; official resend behavior
    /// is unknown. False resumes the retained own world without a new packet.
    pub after_ready_every: bool,
    /// Close a connection after this long without client bytes.
    pub idle: std::time::Duration,
    /// Free-form experiment label written into every transcript line.
    pub scenario: String,
}

impl Config {
    /// Live-verified minimal farm data, sent after 10366 (verified with a 7 s delay).
    pub fn default_after_ready() -> Vec<Outgoing> {
        vec![Outgoing::new(&OwnHomeData::default())]
    }

    /// HOME_PROTOCOL.md: 19336 is released by 22151; 13447 by 23549, whose
    /// install step needs 22151 first.
    pub fn default_replies() -> Vec<(u16, Vec<Outgoing>)> {
        let ping = Outgoing::new(&PingReply::default());
        vec![
            // : marks the client's friend state received, not a farm visit.
            (19838, vec![Outgoing::new(&EmptyFriendList)]),
            (19336, vec![ping.clone()]),
            (13447, vec![ping, Outgoing::new(&HomeSync::default())]),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

/// One message as seen in plaintext, for the transcript.
#[derive(Debug, Clone)]
pub struct Record {
    pub direction: Direction,
    pub msg_type: u16,
    pub version: u16,
    /// Plaintext body (for encrypted login frames, the decrypted envelope).
    pub body: Vec<u8>,
    pub encrypted: bool,
}

#[derive(Default)]
pub struct Step {
    /// Frames to send now; `records` covers them and the inbound frame.
    pub replies: Vec<Frame>,
    pub records: Vec<Record>,
    /// Messages to send after `Config::after_ready_delay`. They are sealed
    /// only when sent ([`Session::seal_deferred`]), keeping nonce order.
    pub deferred: Vec<Outgoing>,
    /// Close after writing the immediate replies; discard queued input/deferred output.
    pub close: bool,
    /// A replacement home invalidates snapshots queued before the reset.
    pub clear_deferred: bool,
}

/// One direction of the session cipher. Nonces advance (little-endian)
/// before every message (native `0x0d45b0` / `0x611730`): by 2 in the
/// verified client→server direction. `steps` overrides the per-message
/// advance (last entry repeats) for the unverified server→client direction.
struct Cipher {
    key: [u8; 32],
    nonce: [u8; 24],
    steps: Vec<u16>,
    count: usize,
}

impl Cipher {
    fn new(key: [u8; 32], nonce: [u8; 24], steps: Vec<u16>) -> Self {
        Self {
            key,
            nonce,
            steps,
            count: 0,
        }
    }
    fn advance(&mut self) {
        let step = self
            .steps
            .get(self.count)
            .or(self.steps.last())
            .copied()
            .unwrap_or(2);
        self.count += 1;
        let mut carry = step as u32;
        for byte in &mut self.nonce {
            carry += *byte as u32;
            *byte = carry as u8;
            carry >>= 8;
        }
    }
    fn open(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        self.advance();
        crypto::open(&self.key, &self.nonce, payload)
    }
    fn seal(&mut self, body: &[u8]) -> Vec<u8> {
        self.advance();
        crypto::seal(&self.key, &self.nonce, body)
    }
}

#[allow(clippy::large_enum_variant)] // one per connection
enum State {
    AwaitHello,
    AwaitLogin {
        challenge: [u8; 24],
    },
    Established {
        rx: Cipher,
        tx: Cipher,
        ready_handled: bool,
        sent: u32,
        acked: u32,
        replied_types: std::collections::HashSet<u16>,
        seen: std::collections::HashMap<u16, u32>,
    },
}

pub struct Session<'a> {
    config: &'a Config,
    state: State,
    account: Option<crate::storage::accounts::Account>,
    replay: Option<crate::replay::Replay>,
    // The client retains its own world while visiting. Without a resent 26017,
    // returning must resume these serials, not invent a fresh loaded world.
    away_replay: Option<crate::replay::Replay>,
    mode: Option<u8>,
}

impl<'a> Session<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            state: State::AwaitHello,
            account: None,
            replay: None,
            away_replay: None,
            mode: None,
        }
    }

    /// : mode entry is a notification. Only a new home packet warrants
    /// rebuilding object IDs; visiting and returning preserve the own world.
    fn enter_mode(
        current: &mut Option<u8>,
        replay: &mut Option<crate::replay::Replay>,
        away: &mut Option<crate::replay::Replay>,
        mode: u8,
    ) {
        let previous = current.replace(mode);
        if mode == 1 {
            if replay.is_none() {
                *replay = away.take();
            }
        } else if replay.is_some() {
            *away = replay.take();
        }
        tracing::debug!(
            ?previous,
            mode,
            replay_active = replay.is_some(),
            area = match mode {
                1 => "own farm",
                2 => "other farm",
                3 => "Greg's farm",
                4 => "own fishing",
                5 => "other fishing",
                6 => "own town",
                7 => "other town",
                8 => "Greg's town",
                _ => "unknown",
            },
            "client entered mode"
        );
    }

    /// Seals a deferred message now (see [`Step::deferred`]).
    pub fn seal_deferred(&mut self, out: &Outgoing) -> Result<(Record, Frame)> {
        let current_home = if self.config.account_home && out.msg_type == OwnHomeData::TYPE {
            if let Some((store, account)) = self.config.accounts.as_ref().zip(self.account.as_ref())
            {
                let home = prepare_own_home(store.home(account.id)?)?;
                let message = self.config.progress.apply(home.message())?;
                self.replay = if (home.command_replay || self.config.replay_commands)
                    && message.avatar_stats == home.avatar_stats
                    && message.blobs == home.blobs()
                {
                    Some(crate::replay::Replay::new(&home)?)
                } else {
                    None
                };
                self.away_replay = None; // This packet replaces the loaded own world.
                log_home_delivery(account.id, &home, &message, self.replay.is_some(), true);
                Some(Outgoing::new(&message))
            } else {
                None
            }
        } else {
            None
        };
        let out = current_home.as_ref().unwrap_or(out);
        match &mut self.state {
            State::Established { tx, .. } => Ok(seal(tx, out)),
            _ => bail!("deferred send before login"),
        }
    }

    /// Per-type count of client messages received after login.
    pub fn seen_counts(&self) -> Option<std::collections::BTreeMap<u16, u32>> {
        match &self.state {
            State::Established { seen, .. } => Some(seen.iter().map(|(k, v)| (*k, *v)).collect()),
            _ => None,
        }
    }

    /// Number of server frames sent after login, and how many 10099
    /// decrypt-failure reports the client sent.
    pub fn post_login_counts(&self) -> Option<(u32, u32)> {
        match self.state {
            State::Established { sent, acked, .. } => Some((sent, acked)),
            _ => None,
        }
    }

    pub fn handle(&mut self, frame: &Frame) -> Result<Step> {
        let mut step = Step::default();
        match &mut self.state {
            State::AwaitHello => {
                ensure!(
                    frame.msg_type == ClientHello::TYPE,
                    "expected ClientHello, got {}",
                    catalogue::label(frame.msg_type)
                );
                step.records
                    .push(record_in(frame, frame.payload.clone(), false));
                let hello = ClientHello::from_bytes(&frame.payload)?;
                ensure!(
                    frame.version == 0,
                    "ClientHello frame version {}",
                    frame.version
                );
                hello.check_supported()?;
                tracing::info!(fingerprint = %hello.fingerprint, "ClientHello accepted");
                let challenge = crypto::random::<24>()?;
                push_clear(&mut step, &ServerHello { challenge });
                self.state = State::AwaitLogin { challenge };
            }
            State::AwaitLogin { challenge } => {
                ensure!(
                    frame.msg_type == ids::LOGIN,
                    "expected Login, got {}",
                    catalogue::label(frame.msg_type)
                );
                let challenge = *challenge;
                self.login(frame, challenge, &mut step)?;
            }
            State::Established {
                rx,
                tx,
                ready_handled,
                sent,
                acked,
                replied_types,
                seen,
            } => {
                let count = seen.entry(frame.msg_type).or_default();
                *count += 1;
                let count = *count;
                let body = rx
                    .open(&frame.payload)
                    .with_context(|| format!("decrypting {}", catalogue::label(frame.msg_type)))?;
                step.records.push(record_in(frame, body.clone(), true));
                if frame.msg_type == 11292 {
                    let turn = EndClientTurn::parse(&body);
                    tracing::debug!(account_id=?self.account.as_ref().map(|a|a.id),count,
                        tick=?turn.as_ref().map(|t|t.a),checksum=?turn.as_ref().map(|t|t.b),
                        command_count=?turn.as_ref().map(|t|t.n),
                        decoded_count=?turn.as_ref().map(|t|t.decoded.len()),
                        raw_remainder_bytes=?turn.as_ref().map(|t|t.raw_remainder.len()),
                        replay_active=self.replay.is_some(),"decoded client turn; commands are captured before replay and no turn acknowledgment is sent");
                    if let Some(turn) = &turn {
                        for (command_index, command) in turn.decoded.iter().enumerate() {
                            tracing::debug!(tick=turn.a,command_index,command_type=command.command_type(),meaning=command.meaning(),
                                details=%serde_json::to_value(command).expect("serializable command"),"decoded turn command");
                        }
                    }
                    // Log sampled INFO before replay can emit a warning. Every
                    // turn still appears at DEBUG and in the plaintext transcript.
                    if (count == 1 || count.is_multiple_of(10))
                        && !self.config.replies.iter().any(|(kind, _)| *kind == 11292)
                    {
                        tracing::info!(label=%catalogue::label(11292),count,len=body.len(),body=%crate::net::connection::hex(&body),turn=?turn,"client end turn");
                    }
                    if let Some((store, account)) =
                        self.config.accounts.as_ref().zip(self.account.as_ref())
                    {
                        if self.config.account_home {
                            store.capture_turn_with_replay(
                                account.id,
                                &body,
                                self.replay.as_mut(),
                            )?;
                        } else {
                            store.capture_turn_log_only(account.id, &body)?;
                        }
                    }
                }
                // ModeManager mappings, not guessed visit requests. 19838
                // is a separate friend-list request and must preserve replay.
                if frame.msg_type == 19838 {
                    FriendListRequest::from_bytes(&body)?;
                }
                if frame.msg_type == ChangeHomeName::TYPE {
                    match ChangeHomeName::from_bytes(&body) {
                        Ok(ChangeHomeName(name)) => {
                            if let Some((store, account)) =
                                self.config.accounts.as_ref().zip(self.account.as_ref())
                            {
                                store.rename_home(account.id, name)?;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "ignoring malformed farm name request")
                        }
                    }
                }
                if frame.msg_type == 18485 {
                    let visit = crate::protocol::messages::VisitPlayer::from_bytes(&body)?;
                    tracing::info!(
                        selector = visit.selector,
                        target_account_id = visit.target_account_id,
                        "client entered player-farm visit mode; no 29950 response implemented"
                    );
                    if self.config.dev_friends {
                        if let Some(action) =
                            crate::game::dev_friends::action(visit.target_account_id)
                        {
                            let (store, account) = self
                                .config
                                .accounts
                                .as_ref()
                                .zip(self.account.as_ref())
                                .context(
                                    "development friends require a signed-in persistent account",
                                )?;
                            store.development_action(account.id, action)?;
                            let home = prepare_own_home(store.home(account.id)?)?;
                            // Development actions refresh the authoritative
                            // home immediately using this connection's cipher.
                            let message = home.message();
                            self.replay = if home.command_replay || self.config.replay_commands {
                                Some(crate::replay::Replay::new(&home)?)
                            } else {
                                None
                            };
                            self.away_replay = None;
                            self.mode = Some(1);
                            *ready_handled = true;
                            step.clear_deferred = true;
                            log_home_delivery(
                                account.id,
                                &home,
                                &message,
                                self.replay.is_some(),
                                false,
                            );
                            let (record, reply) = seal(tx, &Outgoing::new(&message));
                            step.records.push(record);
                            step.replies.push(reply);
                            *sent += 1;
                            return Ok(step);
                        }
                    }
                }
                if let Some(mode) = match frame.msg_type {
                    10366 => Some(1),
                    18485 => Some(2),
                    13198 => Some(3),
                    17534 => Some(4),
                    16986 => Some(5),
                    19214 => Some(6),
                    14581 => Some(7),
                    11833 => Some(8),
                    _ => None,
                } {
                    Self::enter_mode(
                        &mut self.mode,
                        &mut self.replay,
                        &mut self.away_replay,
                        mode,
                    );
                }
                match frame.msg_type {
                    PostLoginReady::TYPE => {
                        PostLoginReady::from_bytes(&body)?;
                        tracing::debug!(
                            delay_ms = self.config.after_ready_delay.as_millis(),
                            account_home = self.config.account_home,
                            already_handled = *ready_handled,
                            resend = self.config.after_ready_every,
                            "own-farm mode entered; selecting configured snapshot delivery policy"
                        );
                        // Retain the existing configurable resend policy. The
                        // mode switch does not require this reply;
                        // official-server resend behavior remains unknown.
                        if !*ready_handled || self.config.after_ready_every {
                            self.replay = None;
                            self.away_replay = None;
                            *ready_handled = true;
                            tracing::info!(
                                count = self.config.after_ready.len(),
                                "own-farm mode entered; sending configured home snapshot"
                            );
                            let account_home = if self.config.account_home {
                                self.config
                                    .accounts
                                    .as_ref()
                                    .zip(self.account.as_ref())
                                    .map(|(store, account)| {
                                        store
                                            .home(account.id)
                                            .and_then(|home| {
                                                let home = prepare_own_home(home)?;
                                                let message =
                                                    self.config.progress.apply(home.message())?;
                                                if self.config.after_ready_delay.is_zero() {
                                                    self.replay = if (home.command_replay
                                                        || self.config.replay_commands)
                                                        && message.avatar_stats == home.avatar_stats
                                                        && message.blobs == home.blobs()
                                                    {
                                                        Some(crate::replay::Replay::new(&home)?)
                                                    } else {
                                                        None
                                                    };
                                                }
                                                log_home_delivery(
                                                    account.id,
                                                    &home,
                                                    &message,
                                                    self.replay.is_some(),
                                                    false,
                                                );
                                                Ok(message)
                                            })
                                            .map(|home| vec![Outgoing::new(&home)])
                                    })
                                    .transpose()?
                            } else {
                                None
                            };
                            for out in account_home.as_ref().unwrap_or(&self.config.after_ready) {
                                if self.config.after_ready_delay.is_zero() {
                                    let (record, frame) = seal(tx, out);
                                    step.records.push(record);
                                    step.replies.push(frame);
                                } else {
                                    tracing::debug!(label=%catalogue::label(out.msg_type),delay_ms=self.config.after_ready_delay.as_millis(),"queued server message; current account home will be reread when sealed");
                                    step.deferred.push(out.clone());
                                }
                                *sent += 1;
                            }
                        }
                    }
                    ids::INBOUND_ENVELOPE => {
                        *acked += 1;
                        let rejected = <[u8; 4]>::try_from(body.as_slice())
                            .map(u32::from_be_bytes)
                            .ok();
                        tracing::error!(
                            rejected = ?rejected.map(|t| catalogue::label(t as u16)),
                            "client failed to decrypt a server message (10099); it will reset the connection"
                        );
                    }
                    other if self.config.replies.iter().any(|(t, _)| *t == other) => {
                        if count == 1 || count.is_power_of_two() {
                            tracing::info!(label = %catalogue::label(other), count, len = body.len(), "client request");
                        }
                        for (_, outs) in self.config.replies.iter().filter(|(t, _)| *t == other) {
                            for out in outs {
                                let repeat = self.config.repeat_triggers.contains(&other);
                                if replied_types.insert(out.msg_type) || repeat {
                                    tracing::debug!(trigger=%catalogue::label(other),response=%catalogue::label(out.msg_type),repeat,"configured request/reply rule matched");
                                    let (record, frame) = seal(tx, out);
                                    step.records.push(record);
                                    step.replies.push(frame);
                                    *sent += 1;
                                } else {
                                    tracing::debug!(trigger=%catalogue::label(other),response=%catalogue::label(out.msg_type),"once-only reply already sent; suppressing duplicate response");
                                }
                            }
                        }
                    }
                    14690 | 17887 => match FriendIdReport::decode(frame.msg_type, &body) {
                        Ok(FriendIdReport::WithLanguage { ids, language }) => {
                            tracing::debug!(
                                friend_count = ids.len(),
                                ?language,
                                "decoded friend ID request"
                            );
                        }
                        Ok(FriendIdReport::WithFlag {
                            ids,
                            flag,
                            trailing_id,
                        }) => {
                            tracing::debug!(
                                friend_count = ids.len(),
                                flag,
                                trailing_id,
                                "decoded friend ID report"
                            );
                        }
                        Err(error) => {
                            tracing::debug!(%error, "friend ID report differs from captured layout")
                        }
                    },
                    16368 => {
                        // The ID pair's account-versus-loaded-home scope remains
                        // unresolved. Preserve its bytes without inventing fields.
                        if count == 1 || count.is_multiple_of(10) {
                            tracing::info!(label = %catalogue::label(16368), count,
                                len = body.len(), body = %crate::net::connection::hex(&body),
                                "client avatar/home identity report");
                        }
                    }
                    11292 => tracing::debug!(
                        "client turn handling finished; capture/replay needs no default reply"
                    ),
                    other if count > 1 => {
                        if count.is_power_of_two() {
                            tracing::info!(label = %catalogue::label(other), count, "repeated client message");
                        }
                    }
                    other => tracing::info!(
                        label = %catalogue::label(other),
                        decoder = ?catalogue::classes(other)
                            .map(|c| format!("{:#x}@{:#x}", c.decoder, c.vtable))
                            .collect::<Vec<_>>(),
                        len = body.len(),
                        "unhandled client message"
                    ),
                }
            }
        }
        Ok(step)
    }

    fn login(&mut self, frame: &Frame, challenge: [u8; 24], step: &mut Step) -> Result<()> {
        if frame.payload.len() < 32 {
            bail!("Login missing public key");
        }
        let client_public: [u8; 32] = frame.payload[..32].try_into()?;
        let key = crypto::shared(&self.config.secret, &client_public)?;
        let server_public =
            x25519_dalek::x25519(self.config.secret, x25519_dalek::X25519_BASEPOINT_BYTES);
        let plain = crypto::open(
            &key,
            &crypto::nonce(&[&client_public, &server_public]),
            &frame.payload[32..],
        )?;
        step.records.push(record_in(frame, plain.clone(), true));
        if plain.len() < 48 || plain[..24] != challenge {
            bail!("Login session challenge mismatch");
        }
        let client_nonce: [u8; 24] = plain[24..48].try_into()?;
        tracing::info!(body_len = plain.len() - 48, "authenticated Login decrypted");

        // Decode the ID high/low + token prefix; preserve presented credentials
        // on this local server, including after an account-data wipe.
        self.account = self
            .config
            .accounts
            .as_ref()
            .map(|store| store.login(&plain[48..]))
            .transpose()?;
        let (account_id, pass_token) = self
            .account
            .as_ref()
            .map(|account| (account.id, account.pass_token.as_str()))
            .unwrap_or((self.config.account_id, self.config.pass_token.as_str()));

        tracing::debug!(
            account_id,
            uses_account_store = self.account.is_some(),
            "login identity selected; LoginOk returns this account ID and its token"
        );
        let server_nonce = crypto::random::<24>()?;
        let session_key = crypto::random::<32>()?;
        let body = LoginOk {
            tail: self.config.login_ok_tail,
            ..LoginOk::development(account_id, pass_token)
        }
        .to_bytes();
        let mut response = server_nonce.to_vec();
        response.extend_from_slice(&session_key);
        response.extend_from_slice(&body);
        let sealed = crypto::seal(
            &key,
            &crypto::nonce(&[&client_nonce, &client_public, &server_public]),
            &response,
        );
        step.records.push(Record {
            direction: Direction::ServerToClient,
            msg_type: LoginOk::TYPE,
            version: 0,
            body,
            encrypted: true,
        });
        step.replies.push(Frame {
            msg_type: LoginOk::TYPE,
            version: 0,
            payload: sealed,
        });
        tracing::info!("sent encrypted LoginOk (29501)");
        self.state = State::Established {
            rx: Cipher::new(session_key, client_nonce, vec![2]),
            tx: Cipher::new(
                session_key,
                server_nonce,
                self.config.s2c_nonce_steps.clone(),
            ),
            ready_handled: false,
            sent: 0,
            acked: 0,
            replied_types: Default::default(),
            seen: Default::default(),
        };
        Ok(())
    }
}

fn prepare_own_home(home: crate::game::home::HomeData) -> Result<crate::game::home::HomeData> {
    crate::replay::orders::prepare(crate::replay::elapsed_home(home, crate::replay::now_ms()))
}

fn message_purpose(kind: u16) -> &'static str {
    match kind {
        10100 => "client version/fingerprint negotiation",
        10101 => "encrypted login and credential adoption/reconnect",
        20100 => "server challenge for encrypted login",
        29501 => "login accepted with account credentials and session setup",
        10366 => "mode 1 entered: on own farm; no snapshot reply required by this code path",
        26017 => "authoritative account snapshot: avatar stats, bitsets and home JSON",
        11292 => "client simulation tick, checksum and queued gameplay commands",
        16368 => "avatar/home identity report; no default reply",
        13198 => "mode 3 entered: visiting a farm; reply with configured Greg data; selector meaning unknown",
        28383 => "Greg AI-home snapshot",
        19838 => "friend-list request; does not change farm mode or own-farm replay",
        26656 => "friend-list reply; empty list marks friend data received; Greg visibility untested",
        18485 => "mode 2 entered: visiting target account; optional development friend action; no home response",
        17534|16986|19214|14581|11833 => "mode-entry notification; this mode's meaning and required response remain unknown",
        29950 => "other-player home snapshot",
        10099 => "client reports failure to decrypt a server message",
        19336 => "request answered by configured 22151 reply",
        13447 => "home-sync request answered by configured 22151/23549 replies",
        22151 => "configured home-request prerequisite reply",
        23549 => "configured home synchronization reply",
        _ => "protocol message; handler or configured reply rule determines its effect",
    }
}
fn log_home_delivery(
    id: u64,
    home: &crate::game::home::HomeData,
    message: &OwnHomeData,
    replay_active: bool,
    deferred: bool,
) {
    tracing::debug!(account_id=id,deferred,replay_active,stored_replay=home.command_replay,
        objects=message.blobs[2].get("Objects").and_then(serde_json::Value::as_array).map_or(0,Vec::len),
        level=message.avatar_stats.get(&1900002).copied().unwrap_or(1),xp=message.avatar_stats.get(&1900003).copied().unwrap_or(0),
        cash=?message.avatar_stats.get(&1900000),diamonds=?message.avatar_stats.get(&1900001),
        stats_overridden=message.avatar_stats!=home.avatar_stats,farm_overridden=message.blobs!=home.blobs(),
        "prepared own-home snapshot; replay is enabled only for an opted-in home matching saved stats and farm");
}
fn record_in(frame: &Frame, body: Vec<u8>, encrypted: bool) -> Record {
    tracing::debug!(direction="c2s",label=%catalogue::label(frame.msg_type),version=frame.version,encrypted,
        wire_payload_bytes=frame.payload.len(),body_bytes=body.len(),purpose=message_purpose(frame.msg_type),"received client message");
    if frame.msg_type != 10101 {
        tracing::trace!(direction="c2s",label=%catalogue::label(frame.msg_type),body=%crate::net::connection::hex(&body),"client plaintext body");
    }
    Record {
        direction: Direction::ClientToServer,
        msg_type: frame.msg_type,
        version: frame.version,
        body,
        encrypted,
    }
}

fn push_clear<M: Message>(step: &mut Step, message: &M) {
    let body = message.to_bytes();
    tracing::debug!(direction="s2c",label=%catalogue::label(M::TYPE),body_bytes=body.len(),purpose=message_purpose(M::TYPE),"prepared cleartext server reply");
    step.records.push(Record {
        direction: Direction::ServerToClient,
        msg_type: M::TYPE,
        version: 0,
        body: body.clone(),
        encrypted: false,
    });
    step.replies.push(Frame {
        msg_type: M::TYPE,
        version: 0,
        payload: body,
    });
}

fn seal(tx: &mut Cipher, out: &Outgoing) -> (Record, Frame) {
    // Live-verified (LIVE_TEST_FEEDBACK.md §6): a server-sent 29997 does not
    // consume a client receive nonce, so it must not advance ours either.
    let mut payload = if out.msg_type == SessionReady::TYPE {
        crypto::seal(&tx.key, &tx.nonce, &out.body)
    } else {
        tx.seal(&out.body)
    };
    if out.corrupt {
        payload[0] ^= 1;
    }
    tracing::debug!(direction="s2c",label=%catalogue::label(out.msg_type),body_bytes=out.body.len(),
        wire_payload_bytes=payload.len(),message_number=tx.count,advances_nonce=out.msg_type!=SessionReady::TYPE,
        purpose=message_purpose(out.msg_type),"encrypted server message ready to send");
    if out.msg_type != LoginOk::TYPE {
        tracing::trace!(direction="s2c",label=%catalogue::label(out.msg_type),body=%crate::net::connection::hex(&out.body),"server plaintext body");
    }
    tracing::info!(label = %catalogue::label(out.msg_type), len = out.body.len(), corrupt = out.corrupt, "sealed post-login message");
    (
        Record {
            direction: Direction::ServerToClient,
            msg_type: out.msg_type,
            version: 0,
            body: out.body.clone(),
            encrypted: true,
        },
        Frame {
            msg_type: out.msg_type,
            version: 0,
            payload,
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Plays the client side of the handshake against a session, returning
    /// the client's (rx, tx) ciphers: rx decrypts server frames.
    pub(crate) struct TestClient {
        pub secret: [u8; 32],
        pub server_public: [u8; 32],
        pub nonce: [u8; 24],
    }

    impl TestClient {
        pub fn new(server_secret: &[u8; 32]) -> Self {
            Self {
                secret: [7; 32],
                server_public: x25519_dalek::x25519(
                    *server_secret,
                    x25519_dalek::X25519_BASEPOINT_BYTES,
                ),
                nonce: [11; 24],
            }
        }
        pub fn hello() -> Frame {
            Frame {
                msg_type: 10100,
                version: 0,
                payload: ClientHello {
                    protocol: 3,
                    key_version: 14,
                    major: 1,
                    minor: 43,
                    build: 150,
                    fingerprint: "synthetic-test-fingerprint".into(),
                    unknown: [2, 2],
                }
                .to_bytes(),
            }
        }
        pub fn login(&self, server_hello: &Frame) -> Frame {
            self.login_body(server_hello, &[0; 12])
        }
        fn login_body(&self, server_hello: &Frame, body: &[u8]) -> Frame {
            let challenge = ServerHello::from_bytes(&server_hello.payload)
                .unwrap()
                .challenge;
            let public = x25519_dalek::x25519(self.secret, x25519_dalek::X25519_BASEPOINT_BYTES);
            let shared = crypto::shared(&self.secret, &self.server_public).unwrap();
            let mut login = challenge.to_vec();
            login.extend_from_slice(&self.nonce);
            login.extend_from_slice(body);
            let mut payload = public.to_vec();
            payload.extend(crypto::seal(
                &shared,
                &crypto::nonce(&[&public, &self.server_public]),
                &login,
            ));
            Frame {
                msg_type: 10101,
                version: 44182,
                payload,
            }
        }
        /// Decrypts LoginOk; returns (body, rx cipher, tx cipher).
        pub fn login_ok(&self, ok: &Frame) -> (LoginOk, ClientCiphers) {
            let public = x25519_dalek::x25519(self.secret, x25519_dalek::X25519_BASEPOINT_BYTES);
            let shared = crypto::shared(&self.secret, &self.server_public).unwrap();
            let plain = crypto::open(
                &shared,
                &crypto::nonce(&[&self.nonce, &public, &self.server_public]),
                &ok.payload,
            )
            .unwrap();
            let key: [u8; 32] = plain[24..56].try_into().unwrap();
            let body = LoginOk::from_bytes(&plain[56..]).unwrap();
            (
                body,
                ClientCiphers {
                    rx: Cipher::new(key, plain[..24].try_into().unwrap(), vec![2]),
                    tx: Cipher::new(key, self.nonce, vec![2]),
                },
            )
        }
    }

    pub(crate) struct ClientCiphers {
        rx: Cipher,
        tx: Cipher,
    }

    impl ClientCiphers {
        pub fn send(&mut self, msg_type: u16, body: &[u8]) -> Frame {
            Frame {
                msg_type,
                version: 44182,
                payload: self.tx.seal(body),
            }
        }
        pub fn receive(&mut self, frame: &Frame) -> Vec<u8> {
            self.rx.open(&frame.payload).unwrap()
        }
        /// Like the real client for 29997: check without consuming a nonce.
        pub fn peek(&self, frame: &Frame) -> Vec<u8> {
            crypto::open(&self.rx.key, &self.rx.nonce, &frame.payload).unwrap()
        }
    }

    pub(crate) fn config() -> Config {
        Config {
            secret: [1; 32],
            account_id: 1,
            pass_token: "token".into(),
            accounts: None,
            account_home: false,
            replay_commands: false,
            dev_friends: false,
            progress: crate::game::progression::Overrides::default(),
            after_ready: vec![Outgoing::new(&SessionReady::default())],
            after_ready_delay: std::time::Duration::ZERO,
            replies: Config::default_replies(),
            repeat_triggers: Vec::new(),
            after_ready_every: false,
            login_ok_tail: LoginOkTail::Legacy,
            s2c_nonce_steps: vec![2],
            idle: std::time::Duration::from_secs(300),
            scenario: "test".into(),
        }
    }

    #[test]
    fn development_friends_act_only_on_session_owner_and_force_reload() {
        use crate::game::dev_friends::{RESET_ID, RESOURCES_ID};
        use crate::protocol::messages::VisitPlayer;
        for enabled in [false, true] {
            for target in [RESET_ID, RESOURCES_ID, 77] {
                let (store, dir) = crate::storage::accounts::test_store("development-friends");
                let cfg = Config {
                    accounts: Some(store.clone()),
                    dev_friends: enabled,
                    ..config()
                };
                let client = TestClient::new(&cfg.secret);
                let mut session = Session::new(&cfg);
                let hello = session
                    .handle(&TestClient::hello())
                    .unwrap()
                    .replies
                    .remove(0);
                let login = session.handle(&client.login(&hello)).unwrap();
                let (account, mut ciphers) = client.login_ok(&login.replies[0]);
                let mut home = store.home(account.account_id).unwrap();
                home.avatar_stats.insert(1900000, 350);
                home.avatar_stats.insert(1900001, 35);
                store
                    .replace_home(account.account_id, home.clone())
                    .unwrap();
                store.capture_turn(account.account_id, &[0, 0, 0]).unwrap();
                let mut writer = crate::protocol::stream::Writer::new();
                writer.long(77).string("other-user");
                store.login(&writer.into_bytes()).unwrap();
                let other = store.home(77).unwrap();
                let visit = VisitPlayer {
                    selector: 0,
                    target_account_id: target,
                };
                let step = session
                    .handle(&ciphers.send(18485, &visit.to_bytes()))
                    .unwrap();
                let acts = enabled && target != 77;
                assert!(!step.close);
                assert!(step.deferred.is_empty());
                assert!(!step.replies.iter().any(|f| f.msg_type == 29950));
                if acts {
                    assert_eq!(step.replies.len(), 1);
                    assert_eq!(step.replies[0].msg_type, 26017);
                    let snapshot =
                        OwnHomeData::from_bytes(&ciphers.receive(&step.replies[0])).unwrap();
                    assert_eq!(
                        snapshot,
                        prepare_own_home(store.home(account.account_id).unwrap())
                            .unwrap()
                            .message()
                    );
                    assert_eq!(session.mode, Some(1));
                    assert!(step.clear_deferred);
                } else {
                    assert!(step.replies.is_empty());
                }
                if acts && target == RESET_ID {
                    assert!(store.contains(account.account_id).unwrap());
                    assert!(dir
                        .join(account.account_id.to_string())
                        .join("home.json")
                        .exists());
                    assert!(!dir
                        .join(account.account_id.to_string())
                        .join("turns.jsonl")
                        .exists());
                    let mut w = crate::protocol::stream::Writer::new();
                    w.long(account.account_id).string(&account.pass_token);
                    let recreated = store.login(&w.into_bytes()).unwrap();
                    assert_eq!(recreated.id, account.account_id);
                    assert_eq!(recreated.pass_token, account.pass_token);
                    assert_eq!(
                        store.home(recreated.id).unwrap(),
                        crate::game::home::HomeData::default()
                    );
                } else {
                    let mut expected = home;
                    if acts {
                        expected.avatar_stats.insert(1900000, 10350);
                        expected.avatar_stats.insert(1900001, 10035);
                    }
                    assert_eq!(store.home(account.account_id).unwrap(), expected);
                }
                assert_eq!(store.home(77).unwrap(), other);
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }
    #[test]
    fn _trash_skip_unlock_and_visitor_registration_on_encrypted_session() {
        let (store, dir) = crate::storage::accounts::test_store("-orders");
        let cfg = Config {
            accounts: Some(store.clone()),
            account_home: true,
            after_ready: Config::default_after_ready(),
            after_ready_every: true,
            ..config()
        };
        let client = TestClient::new(&cfg.secret);
        let mut session = Session::new(&cfg);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let login = session.handle(&client.login(&hello)).unwrap();
        let (account, mut ciphers) = client.login_ok(&login.replies[0]);
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 4);
        home.avatar_stats.insert(1900001, 10);
        store
            .replace_home(account.account_id, home.clone())
            .unwrap();
        let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
        let sent = OwnHomeData::from_bytes(&ciphers.receive(&ready.replies[0])).unwrap();
        let seed = sent.blobs[2]["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ID"] == 3200000)
            .unwrap()["Seed"]
            .clone();
        let trash = crate::replay::tests::turn(10, &[vec![33, 0, 4, 1], vec![27, 0, 2]]);
        assert!(session
            .handle(&ciphers.send(11292, &trash))
            .unwrap()
            .replies
            .is_empty());
        let waiting = store.home(account.account_id).unwrap();
        let board = waiting.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ID"] == 3200000)
            .unwrap();
        assert_eq!(board["Seed"], seed);
        assert!(board["Orders"][0]["Timer"]["TicksLeft"].as_u64().unwrap() > 0);
        assert!(waiting.order_board_unlocked);
        assert_eq!(waiting.avatar_stats, home.avatar_stats);
        let skip = crate::replay::tests::turn(20, &[vec![40, 4900000, 11], vec![29, 0, 12]]);
        session.handle(&ciphers.send(11292, &skip)).unwrap();
        let updated = store.home(account.account_id).unwrap();
        assert_eq!(updated.avatar_stats[&1900001], 7);
        assert_eq!(updated.avatar_stats[&1900003], 0);
        assert_eq!(updated.predefined_orders, 3);
        let notes = store.turns(account.account_id).unwrap();
        let last = notes.as_array().unwrap().last().unwrap();
        assert!(!last["replay_notes"].as_array().unwrap().is_empty());
        assert!(last["replay_notes"][0]
            .as_str()
            .unwrap()
            .contains("visitor deal"));
        let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
        let snapshot = OwnHomeData::from_bytes(&ciphers.receive(&ready.replies[0])).unwrap();
        assert_eq!(snapshot.avatar_stats, updated.avatar_stats);
        assert_eq!(snapshot.unlocked[&33], [3]);
        assert!(store.home(account.account_id).unwrap().order_board_unlocked);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn _orders_are_sent_replayed_and_reloaded_without_login_rewrite() {
        use crate::protocol::messages::OwnHomeData;
        for delay in [0, 7000] {
            let (store, dir) = crate::storage::accounts::test_store("-orders");
            let cfg = Config {
                accounts: Some(store.clone()),
                account_home: true,
                after_ready: Config::default_after_ready(),
                after_ready_every: true,
                after_ready_delay: std::time::Duration::from_millis(delay),
                ..config()
            };
            let client = TestClient::new(&cfg.secret);
            let mut session = Session::new(&cfg);
            let hello = session
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let login = session.handle(&client.login(&hello)).unwrap();
            let (account, mut ciphers) = client.login_ok(&login.replies[0]);
            let mut initial = crate::game::starter::new_home().unwrap();
            initial.avatar_stats.insert(1900002, 4);
            initial.avatar_stats.insert(1500000, 10);
            initial.avatar_stats.insert(400001, 100);
            initial.avatar_stats.insert(400002, 100);
            initial.avatar_stats.insert(2400000, 100);
            store
                .replace_home(account.account_id, initial.clone())
                .unwrap();
            let saved =
                std::fs::read(dir.join(account.account_id.to_string()).join("home.json")).unwrap();
            let home_frame = |session: &mut Session<'_>, ciphers: &mut ClientCiphers| {
                let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
                if delay == 0 {
                    ready.replies[0].clone()
                } else {
                    session.seal_deferred(&ready.deferred[0]).unwrap().1
                }
            };
            let frame = home_frame(&mut session, &mut ciphers);
            let sent = OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap();
            let prepared = prepare_own_home(initial.clone()).unwrap();
            assert_eq!(sent, prepared.message());
            assert_eq!(
                std::fs::read(dir.join(account.account_id.to_string()).join("home.json")).unwrap(),
                saved
            );
            let board = sent.blobs[2]["Objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["ID"] == 3200000)
                .unwrap();
            let order = &board["Orders"][0];
            let before_cash = initial.avatar_stats.get(&1900000).copied().unwrap_or(0);
            let complete = crate::replay::tests::turn(10, &[vec![28, 0, 1], vec![41, 2]]);
            assert!(session
                .handle(&ciphers.send(11292, &complete))
                .unwrap()
                .replies
                .is_empty());
            let stored = store.home(account.account_id).unwrap();
            assert_eq!(stored.predefined_orders, 3); // B follows the sent A.
            assert_eq!(
                stored.avatar_stats[&1900000],
                before_cash + order["Cash"].as_u64().unwrap() as u32
            );
            assert_eq!(
                stored.avatar_stats[&1900003],
                order["Exp"].as_u64().unwrap() as u32
            );
            let frame = home_frame(&mut session, &mut ciphers);
            assert_eq!(
                OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap(),
                stored.message()
            );
            drop(session);
            drop(cfg);
            drop(store);
            let reopened =
                crate::storage::accounts::Store::open(dir.join("accounts.json"), initial).unwrap();
            assert_eq!(reopened.home(account.account_id).unwrap(), stored);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn _encrypted_feed_produce_and_offline_reload_then_collect() {
        for delay in [0, 7000] {
            let (store, dir) = crate::storage::accounts::test_store("-timers");
            let cfg = Config {
                accounts: Some(store.clone()),
                account_home: true,
                after_ready_every: true,
                after_ready_delay: std::time::Duration::from_millis(delay),
                ..config()
            };
            let client = TestClient::new(&cfg.secret);
            let mut session = Session::new(&cfg);
            let hello = session
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let login = session.handle(&client.login(&hello)).unwrap();
            let (account, mut ciphers) = client.login_ok(&login.replies[0]);
            let mut home = crate::game::starter::new_home().unwrap();
            home.avatar_stats
                .extend([(1900002, 3), (1900001, 10), (400001, 10)]);
            home.c["Objects"].as_array_mut().unwrap().extend([
                serde_json::json!({"ID":1300000,"Fed":false,"AnimalHabitatIndex":0}),
                serde_json::json!({"ID":600000,"X":25,"Y":25}),
            ]);
            store.replace_home(account.account_id, home).unwrap();
            let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
            let frame = if delay == 0 {
                ready.replies[0].clone()
            } else {
                session.seal_deferred(&ready.deferred[0]).unwrap().1
            };
            ciphers.receive(&frame);
            let turn = crate::replay::tests::turn(
                304,
                &[vec![9, 1300000, 117], vec![10, 2400000, 600000, 118]],
            );
            session.handle(&ciphers.send(11292, &turn)).unwrap();
            let mut saved = store.home(account.account_id).unwrap();
            assert_eq!(saved.avatar_stats[&1600000], 8);
            assert_eq!(saved.avatar_stats[&400001], 7);
            assert!(saved.timer_saved_at_ms.is_some());
            // Simulate a 20-minute offline period without a blocking test wait.
            saved.timer_saved_at_ms = Some(crate::replay::now_ms() - 1_201_000);
            store.replace_home(account.account_id, saved).unwrap();
            let home_path = dir.join(account.account_id.to_string()).join("home.json");
            let unchanged = std::fs::read(&home_path).unwrap();
            let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
            let frame = if delay == 0 {
                ready.replies[0].clone()
            } else {
                session.seal_deferred(&ready.deferred[0]).unwrap().1
            };
            let packet = OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap();
            let objects = packet.blobs[2]["Objects"].as_array().unwrap();
            let animal = objects.iter().find(|o| o["ID"] == 1300000).unwrap();
            assert_eq!(animal["Fed"], true);
            assert_eq!(animal["Timer"], serde_json::Value::Null);
            let bakery = objects.iter().find(|o| o["ID"] == 600000).unwrap();
            assert_eq!(
                bakery["ProductionList"]["FinishedProductions"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                std::fs::read(&home_path).unwrap(),
                unchanged,
                "home delivery ages a copy, not the account file"
            );
            let turn =
                crate::replay::tests::turn(304, &[vec![14, 1300000, 1], vec![13, 600000, 1]]);
            session.handle(&ciphers.send(11292, &turn)).unwrap();
            let saved = store.home(account.account_id).unwrap();
            assert_eq!(saved.avatar_stats[&1500000], 1);
            assert_eq!(saved.avatar_stats[&2400000], 1);
            assert_eq!(saved.avatar_stats[&1900003], 5);
            drop(session);
            drop(cfg);
            drop(store);
            let reopened = crate::storage::accounts::Store::open(
                dir.join("accounts.json"),
                crate::game::starter::new_home().unwrap(),
            )
            .unwrap();
            assert_eq!(reopened.home(account.account_id).unwrap(), saved);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn encrypted_replay_persists_and_reinitializes_after_home_reload() {
        for delay in [0, 7000] {
            let (store, dir) = crate::storage::accounts::test_store("encrypted-replay");
            let cfg = Config {
                accounts: Some(store.clone()),
                account_home: true,
                after_ready_every: true,
                after_ready_delay: std::time::Duration::from_millis(delay),
                ..config()
            };
            let client = TestClient::new(&cfg.secret);
            let mut session = Session::new(&cfg);
            let hello = session
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let login = session.handle(&client.login(&hello)).unwrap();
            let (account, mut ciphers) = client.login_ok(&login.replies[0]);
            let mut initial = crate::game::starter::new_home().unwrap();
            initial.avatar_stats.insert(1900003, 25);
            store.replace_home(account.account_id, initial).unwrap();
            let step = session.handle(&ciphers.send(10366, &[])).unwrap();
            let frame = if delay == 0 {
                step.replies[0].clone()
            } else {
                session.seal_deferred(&step.deferred[0]).unwrap().1
            };
            let initial = OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap();
            assert_eq!(initial.avatar_stats[&1900003], 25);
            assert!(session.replay.is_some());
            let commands = (400000..400006)
                .flat_map(|id| [vec![2, id, 1], vec![22, id, 1], vec![23, id, 1]])
                .collect::<Vec<_>>();
            let bytes = crate::replay::tests::turn(304, &commands);
            session.handle(&ciphers.send(11292, &bytes)).unwrap();
            let saved = store.home(account.account_id).unwrap();
            assert_eq!(saved.avatar_stats[&400001], 12);
            assert_eq!(saved.avatar_stats[&1900002], 2);
            assert_eq!(saved.avatar_stats[&1900003], 4);
            // Every home load rebuilds serials from the exact Objects order sent.
            let step = session.handle(&ciphers.send(10366, &[])).unwrap();
            let frame = if delay == 0 {
                step.replies[0].clone()
            } else {
                session.seal_deferred(&step.deferred[0]).unwrap().1
            };
            let packet = OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap();
            assert_eq!(packet.avatar_stats, saved.avatar_stats);
            assert_eq!(packet.fields_unlocked, 7);
            let bytes = crate::replay::tests::turn(304, &[vec![1, 400000, 400001, 1]]);
            session.handle(&ciphers.send(11292, &bytes)).unwrap();
            assert_eq!(
                store.home(account.account_id).unwrap().avatar_stats[&400001],
                11
            );
            assert_eq!(
                store.turns(account.account_id).unwrap()[0]["replay_notes"],
                serde_json::json!([])
            );
            // Visiting another home must not replay its instance IDs into ours.
            session.handle(&ciphers.send(13198, &[0])).unwrap();
            assert!(session.replay.is_none());
            let before = store.home(account.account_id).unwrap();
            let bytes = crate::replay::tests::turn(607, &[vec![2, 400006, 305]]);
            session.handle(&ciphers.send(11292, &bytes)).unwrap();
            assert_eq!(store.home(account.account_id).unwrap(), before);
            drop(session);
            drop(cfg);
            drop(store);
            let reopened = crate::storage::accounts::Store::open(
                dir.join("accounts.json"),
                crate::game::starter::new_home().unwrap(),
            )
            .unwrap();
            assert_eq!(reopened.home(account.account_id).unwrap(), before);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn raw_home_override_records_commands_without_mutating_account() {
        let (store, dir) = crate::storage::accounts::test_store("raw-read-only");
        let cfg = Config {
            accounts: Some(store.clone()),
            account_home: false,
            replay_commands: true,
            ..config()
        };
        let client = TestClient::new(&cfg.secret);
        let mut session = Session::new(&cfg);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let login = session.handle(&client.login(&hello)).unwrap();
        let (account, mut ciphers) = client.login_ok(&login.replies[0]);
        let before = store.home(account.account_id).unwrap();
        let bytes = crate::replay::tests::turn(304, &[vec![34, 3600000, 1], vec![2, 400000, 1]]);
        session.handle(&ciphers.send(11292, &bytes)).unwrap();
        assert_eq!(store.home(account.account_id).unwrap(), before);
        assert_eq!(
            store
                .turns(account.account_id)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persistent_login_reconnect_home_resend_and_turn_capture() {
        for progress in [
            crate::game::progression::Overrides::default(),
            crate::game::progression::Overrides {
                level: None,
                xp: Some(7),
                tutorial_words: None,
            },
            crate::game::progression::Overrides {
                level: Some(5),
                xp: Some(30),
                tutorial_words: None,
            },
        ] {
            let (store, dir) = crate::storage::accounts::test_store("session");
            let config = Config {
                accounts: Some(store.clone()),
                account_home: true,
                progress,
                after_ready: Config::default_after_ready(),
                after_ready_every: true,
                login_ok_tail: LoginOkTail::Trimmed,
                ..config()
            };
            let client = TestClient::new(&config.secret);
            let mut session = Session::new(&config);
            let hello = session
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let login = session.handle(&client.login(&hello)).unwrap();
            let (account, mut ciphers) = client.login_ok(&login.replies[0]);
            assert_eq!(account.tail, LoginOkTail::Trimmed);
            assert_ne!(account.pass_token, "token");
            let own = session.handle(&ciphers.send(10366, &[])).unwrap();
            assert_eq!(
                OwnHomeData::from_bytes(&ciphers.receive(&own.replies[0])).unwrap(),
                progress
                    .apply(
                        prepare_own_home(store.home(account.account_id).unwrap())
                            .unwrap()
                            .message()
                    )
                    .unwrap()
            );
            let mut changed = crate::game::home::HomeData::default();
            changed.c["Objects"] = serde_json::json!([{"ID":400001,"X":19,"Y":20}]);
            store
                .replace_home(account.account_id, changed.clone())
                .unwrap();
            let own = session.handle(&ciphers.send(10366, &[])).unwrap();
            assert_eq!(
                OwnHomeData::from_bytes(&ciphers.receive(&own.replies[0])).unwrap(),
                progress
                    .apply(prepare_own_home(changed.clone()).unwrap().message())
                    .unwrap()
            );
            for body in [
                &[0x8d, 0x0e, 0x93, 0x8d, 0x85, 0xad, 0x01, 0x00][..],
                &[255][..],
            ] {
                let step = session.handle(&ciphers.send(11292, body)).unwrap();
                assert!(step.replies.is_empty());
            }
            let turns = store.turns(account.account_id).unwrap();
            assert_eq!(turns[0]["a"], 909);
            assert_eq!(turns[1]["parsed"], false);
            // Reconnect with the actual Login credential prefix.
            let mut reconnect = Session::new(&config);
            let hello = reconnect
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let mut frame = client.login(&hello);
            let public: [u8; 32] = frame.payload[..32].try_into().unwrap();
            let key = crypto::shared(&client.secret, &client.server_public).unwrap();
            let nonce = crypto::nonce(&[&public, &client.server_public]);
            let mut plain = crypto::open(&key, &nonce, &frame.payload[32..]).unwrap();
            let mut body = crate::protocol::stream::Writer::new();
            body.long(account.account_id).string(&account.pass_token);
            plain.truncate(48);
            plain.extend(body.into_bytes());
            frame.payload.truncate(32);
            frame.payload.extend(crypto::seal(&key, &nonce, &plain));
            let login = reconnect.handle(&frame).unwrap();
            let (again, mut ciphers) = client.login_ok(&login.replies[0]);
            assert_eq!(again.account_id, account.account_id);
            assert_eq!(again.pass_token, account.pass_token);
            let own = reconnect.handle(&ciphers.send(10366, &[])).unwrap();
            assert_eq!(
                OwnHomeData::from_bytes(&ciphers.receive(&own.replies[0])).unwrap(),
                progress
                    .apply(prepare_own_home(changed.clone()).unwrap().message())
                    .unwrap()
            );
            assert_eq!(store.list().unwrap().as_array().unwrap().len(), 1);
            assert_eq!(store.home(account.account_id).unwrap(), changed);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn delayed_progress_uses_current_account_home_without_writing_it() {
        let (store, dir) = crate::storage::accounts::test_store("delayed-progress");
        let config = Config {
            accounts: Some(store.clone()),
            account_home: true,
            progress: crate::game::progression::Overrides {
                level: None,
                xp: Some(7),
                tutorial_words: None,
            },
            after_ready: Config::default_after_ready(),
            after_ready_delay: std::time::Duration::from_secs(7),
            after_ready_every: true,
            ..config()
        };
        let client = TestClient::new(&config.secret);
        let mut session = Session::new(&config);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let login = session.handle(&client.login(&hello)).unwrap();
        let (account, mut ciphers) = client.login_ok(&login.replies[0]);
        let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
        assert_eq!(ready.deferred.len(), 1);
        let mut changed = crate::game::home::HomeData {
            avatar_stats: std::collections::BTreeMap::from([
                (1_900_000, 100),
                (1_900_002, 5),
                (1_900_003, 30),
            ]),
            ..crate::game::home::HomeData::default()
        };
        changed.c["Objects"] = serde_json::json!([{"ID":400001,"X":19,"Y":20}]);
        store
            .replace_home(account.account_id, changed.clone())
            .unwrap();
        let path = dir.join(account.account_id.to_string()).join("home.json");
        let saved = std::fs::read(&path).unwrap();
        store.validate_progress(config.progress).unwrap();
        for outgoing in [
            ready.deferred[0].clone(),
            session
                .handle(&ciphers.send(10366, &[]))
                .unwrap()
                .deferred
                .remove(0),
        ] {
            let (_, frame) = session.seal_deferred(&outgoing).unwrap();
            // Framing derives length from the resized encrypted payload.
            let wire = frame.to_wire();
            assert_eq!(
                ((wire[2] as usize) << 16) | ((wire[3] as usize) << 8) | wire[4] as usize,
                frame.payload.len()
            );
            let home = OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap();
            assert_eq!(
                home,
                config
                    .progress
                    .apply(prepare_own_home(changed.clone()).unwrap().message())
                    .unwrap()
            );
            assert_eq!(home.avatar_stats[&1_900_002], 5);
            assert_eq!(home.avatar_stats[&1_900_003], 7);
            assert_eq!(std::fs::read(&path).unwrap(), saved);
        }
        let invalid = crate::game::progression::Overrides {
            level: Some(2),
            xp: None,
            tutorial_words: None,
        };
        assert!(store
            .validate_progress(invalid)
            .unwrap_err()
            .to_string()
            .contains("account"));
        assert_eq!(store.home(account.account_id).unwrap(), changed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn default_own_home_is_deferred_once_and_keeps_ping_reply() {
        let config = Config {
            after_ready: Config::default_after_ready(),
            after_ready_delay: std::time::Duration::from_secs(7),
            ..config()
        };
        assert_eq!(
            config.after_ready,
            vec![Outgoing::parse("own-home").unwrap()]
        );
        let mut session = Session::new(&config);
        let client = TestClient::new(&config.secret);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let step = session.handle(&client.login(&hello)).unwrap();
        assert_eq!(step.replies.len(), 1);
        assert!(step.deferred.is_empty());
        let (_, mut ciphers) = client.login_ok(&step.replies[0]);
        let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
        assert!(ready.replies.is_empty());
        assert_eq!(ready.deferred.len(), 1);
        assert_eq!(ready.deferred[0].msg_type, OwnHomeData::TYPE);
        let ping = session.handle(&ciphers.send(19336, &[])).unwrap();
        assert_eq!(ping.replies.len(), 1);
        assert_eq!(ping.replies[0].msg_type, PingReply::TYPE);
        assert_eq!(
            ciphers.receive(&ping.replies[0]),
            PingReply::default().to_bytes()
        );
        let (record, frame) = session.seal_deferred(&ready.deferred[0]).unwrap();
        assert_eq!(record.msg_type, OwnHomeData::TYPE);
        assert_eq!(
            OwnHomeData::from_bytes(&ciphers.receive(&frame)).unwrap(),
            OwnHomeData::default()
        );
        let repeated = session.handle(&ciphers.send(10366, &[])).unwrap();
        assert!(repeated.replies.is_empty());
        assert!(repeated.deferred.is_empty());
    }

    #[test]
    fn _friend_request_preserves_replay_and_return_without_snapshot_resumes_serials() {
        use crate::protocol::messages::AiHomeData;
        let (store, dir) = crate::storage::accounts::test_store("-mode");
        let ai = AiHomeData {
            blobs: OwnHomeData::default().blobs,
        };
        let mut cfg = Config {
            accounts: Some(store.clone()),
            account_home: true,
            after_ready_every: false,
            repeat_triggers: vec![19838, 13198],
            ..config()
        };
        cfg.replies.push((13198, vec![Outgoing::new(&ai)]));
        let client = TestClient::new(&cfg.secret);
        let mut session = Session::new(&cfg);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let login = session.handle(&client.login(&hello)).unwrap();
        let (account, mut ciphers) = client.login_ok(&login.replies[0]);
        store
            .replace_home(account.account_id, crate::game::starter::new_home().unwrap())
            .unwrap();
        let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
        ciphers.receive(&ready.replies[0]);
        assert_eq!(session.mode, Some(1));
        assert!(session.replay.is_some());
        let harvest = crate::replay::tests::turn(304, &[vec![2, 400000, 1], vec![23, 400000, 1]]);
        session.handle(&ciphers.send(11292, &harvest)).unwrap();
        for _ in 0..2 {
            let friends = session.handle(&ciphers.send(19838, &[])).unwrap();
            assert_eq!(friends.replies.len(), 1);
            assert_eq!(friends.replies[0].msg_type, 26656);
            assert_eq!(ciphers.receive(&friends.replies[0]), [0]);
            assert_eq!(session.mode, Some(1));
            assert!(session.replay.is_some());
        }
        let visit = session.handle(&ciphers.send(13198, &[0])).unwrap();
        assert_eq!(visit.replies.len(), 1);
        assert_eq!(visit.replies[0].msg_type, 28383);
        assert_eq!(
            AiHomeData::from_bytes(&ciphers.receive(&visit.replies[0])).unwrap(),
            ai
        );
        assert_eq!(session.mode, Some(3));
        assert!(session.replay.is_none());
        assert!(session.away_replay.is_some());
        let before = store.home(account.account_id).unwrap();
        let foreign_turn = crate::replay::tests::turn(607, &[vec![2, 400001, 1]]);
        session.handle(&ciphers.send(11292, &foreign_turn)).unwrap();
        assert_eq!(store.home(account.account_id).unwrap(), before);
        let returned = session.handle(&ciphers.send(10366, &[])).unwrap();
        assert!(returned.replies.is_empty());
        assert!(returned.deferred.is_empty());
        assert_eq!(session.mode, Some(1));
        assert!(session.replay.is_some());
        assert!(session.away_replay.is_none());
        // The client retained its EmptyField instance 400006 during the visit.
        let plant = crate::replay::tests::turn(607, &[vec![1, 400006, 400001, 305]]);
        session.handle(&ciphers.send(11292, &plant)).unwrap();
        assert_eq!(
            store.home(account.account_id).unwrap().avatar_stats[&400001],
            1
        );
        for message in [18485, 17534, 16986, 19214, 14581, 11833] {
            let body = if message == 18485 {
                vec![0; 9]
            } else {
                vec![0]
            };
            let unknown = session.handle(&ciphers.send(message, &body)).unwrap();
            assert!(unknown.replies.is_empty());
            assert!(session.replay.is_none());
            assert!(session
                .handle(&ciphers.send(10366, &[]))
                .unwrap()
                .replies
                .is_empty());
            assert!(session.replay.is_some());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn greg_visits_and_return_home_repeat_on_one_connection() {
        use crate::protocol::messages::AiHomeData;
        let ai = AiHomeData {
            blobs: OwnHomeData::default().blobs,
        };
        let mut config = Config {
            after_ready: Config::default_after_ready(),
            after_ready_every: true,
            repeat_triggers: vec![13198],
            ..config()
        };
        config.replies.push((13198, vec![Outgoing::new(&ai)]));
        let mut session = Session::new(&config);
        let client = TestClient::new(&config.secret);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let login = session.handle(&client.login(&hello)).unwrap();
        let (_, mut ciphers) = client.login_ok(&login.replies[0]);
        // The first 13198 arrives during login, before the own home.
        for trigger in [13198, 10366, 19838, 13198, 10213, 10366] {
            let body: &[u8] = if trigger == 13198 { &[0] } else { &[] };
            let step = session.handle(&ciphers.send(trigger, body)).unwrap();
            match trigger {
                13198 => {
                    assert_eq!(step.replies.len(), 1);
                    assert_eq!(step.replies[0].msg_type, 28383);
                    assert_eq!(
                        AiHomeData::from_bytes(&ciphers.receive(&step.replies[0])).unwrap(),
                        ai
                    );
                }
                10366 => {
                    assert_eq!(step.replies.len(), 1);
                    assert_eq!(step.replies[0].msg_type, 26017);
                    assert_eq!(
                        OwnHomeData::from_bytes(&ciphers.receive(&step.replies[0])).unwrap(),
                        OwnHomeData::default()
                    );
                }
                19838 => {
                    assert_eq!(step.replies.len(), 1);
                    assert_eq!(step.replies[0].msg_type, 26656);
                    assert_eq!(ciphers.receive(&step.replies[0]), [0]);
                }
                _ => assert!(step.replies.is_empty()),
            }
        }
        assert!(session
            .handle(&ciphers.send(11292, &[0, 0, 0]))
            .unwrap()
            .replies
            .is_empty());
    }

    #[test]
    fn explicit_unknown_reply_rules_keep_precedence_and_repeat_semantics() {
        for every in [false, true] {
            let config = Config {
                replies: vec![(11292, vec![Outgoing::new(&PingReply::default())])],
                repeat_triggers: if every { vec![11292] } else { vec![] },
                ..config()
            };
            let mut session = Session::new(&config);
            let client = TestClient::new(&config.secret);
            let hello = session
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let login = session.handle(&client.login(&hello)).unwrap();
            let (_, mut ciphers) = client.login_ok(&login.replies[0]);
            for n in 0..2 {
                let step = session.handle(&ciphers.send(11292, &[0, 0, 0])).unwrap();
                assert_eq!(step.replies.len(), usize::from(n == 0 || every));
                for frame in &step.replies {
                    assert_eq!(ciphers.receive(frame), PingReply::default().to_bytes());
                }
            }
        }
    }

    #[test]
    fn unknown_farm_messages_are_sampled_without_replies() {
        use std::sync::{Arc, Mutex};
        struct LogBytes(Arc<Mutex<Vec<u8>>>, std::thread::ThreadId);
        impl std::io::Write for LogBytes {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if std::thread::current().id() == self.1 {
                    self.0.lock().unwrap().extend_from_slice(bytes);
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let logs = Arc::new(Mutex::new(Vec::new()));
        let captured = logs.clone();
        let owner = std::thread::current().id();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || LogBytes(captured.clone(), owner))
            .finish();
        // A process-wide subscriber avoids racing the callsite interest cache
        // against parallel tests with no thread-local subscriber. Capture only
        // this test's thread so other handshakes cannot affect the assertions.
        tracing::subscriber::set_global_default(subscriber).unwrap();
        {
            let config = Config {
                after_ready: Vec::new(),
                // No rule for these unknown types: observed, never answered.
                replies: Config::default_replies(),
                ..config()
            };
            let mut session = Session::new(&config);
            let client = TestClient::new(&config.secret);
            let hello = session
                .handle(&TestClient::hello())
                .unwrap()
                .replies
                .remove(0);
            let step = session.handle(&client.login(&hello)).unwrap();
            let (_, mut ciphers) = client.login_ok(&step.replies[0]);
            for count in 1..=21 {
                for kind in [16368, 11292] {
                    let body = if kind == 11292 && count == 1 {
                        vec![
                            0xb0, 0x04, 0xc1, 0xe6, 0xbb, 0xa8, 0x00, 0x02, 0x22, 0x91, 0xba, 0xb7,
                            0x03, 0xb5, 0x01, 0x23, 0x80, 0xba, 0xb7, 0x03, 0xb5, 0x01,
                        ]
                    } else {
                        vec![0xcd; 8]
                    };
                    let step = session.handle(&ciphers.send(kind, &body)).unwrap();
                    assert!(step.replies.is_empty());
                    assert!(step.deferred.is_empty());
                    assert_eq!(step.records.len(), 1);
                    assert_eq!(step.records[0].body, body);
                }
            }
            let counts = session.seen_counts().unwrap();
            assert_eq!(counts[&16368], 21);
            assert_eq!(counts[&11292], 21);
        }
        let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        for name in ["AvatarHomeIdentityReport", "EndClientTurn"] {
            let lines: Vec<_> = text
                .lines()
                .filter(|l| l.contains(name) && l.contains("INFO"))
                .collect();
            assert_eq!(lines.len(), 3, "{text}");
            for (line, count) in lines.iter().zip([1, 10, 20]) {
                assert!(line.contains("INFO"));
                assert!(line.contains(&format!("count={count} ")), "{line}");
                assert!(line.contains("body="));
            }
        }
        for line in text
            .lines()
            .filter(|line| line.contains("AvatarHomeIdentityReport") && line.contains("INFO"))
        {
            assert!(line.contains("client avatar/home identity report"));
            assert!(!line.contains("unhandled"));
            assert!(!line.contains("turn="));
        }
        assert!(text.contains("b004c1e6bba800022291bab703b5012380bab703b501"));
        assert!(text.contains("a: 304, b: 2189916545, n: 2, commands: [34,"));
        assert!(text
            .lines()
            .any(|line| line.contains("EndClientTurn") && line.contains("turn=None")));
        assert!(text.contains(&"cd".repeat(8)));

        assert_eq!(
            text.lines()
                .filter(|line| line.contains("DEBUG")
                    && line.contains("received client message")
                    && line.contains("EndClientTurn"))
                .count(),
            21
        );
        assert!(text.contains("decoded turn command"));
        assert!(text.contains("complete tutorial row"));
        // The second turn is deliberately not INFO-sampled. Its warning must
        // still identify the offending 11292, command, tick, and exact body.
        let (store, dir) = crate::storage::accounts::test_store("warning-context");
        let config = Config {
            accounts: Some(store.clone()),
            account_home: true,
            ..config()
        };
        let mut session = Session::new(&config);
        let client = TestClient::new(&config.secret);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let login = session.handle(&client.login(&hello)).unwrap();
        let (account, mut ciphers) = client.login_ok(&login.replies[0]);
        store
            .replace_home(account.account_id, crate::game::starter::new_home().unwrap())
            .unwrap();
        let ready = session.handle(&ciphers.send(10366, &[])).unwrap();
        ciphers.receive(&ready.replies[0]);
        let rename = ChangeHomeName("Orchard".into()).to_bytes();
        let step = session
            .handle(&ciphers.send(ChangeHomeName::TYPE, &rename))
            .unwrap();
        assert!(step.replies.is_empty());
        session
            .handle(&ciphers.send(ChangeHomeName::TYPE, &[0xff; 4]))
            .unwrap();
        let first = crate::replay::tests::turn(304, &[]);
        session.handle(&ciphers.send(11292, &first)).unwrap();
        let offending =
            crate::replay::tests::turn(607, &[vec![9, 1300000, 305], vec![2, 400000, 306]]);
        session.handle(&ciphers.send(11292, &offending)).unwrap();
        assert_eq!(
            store.home(account.account_id).unwrap().avatar_stats[&400001],
            2
        );
        let next = crate::replay::tests::turn(910, &[vec![2, 400001, 607]]);
        session.handle(&ciphers.send(11292, &next)).unwrap();
        assert_eq!(
            store.home(account.account_id).unwrap().avatar_stats[&400001],
            4
        );

        assert_eq!(
            store.home(account.account_id).unwrap().farm_name.as_deref(),
            Some("Orchard")
        );
        let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        let warning = text
            .lines()
            .find(|line| {
                line.contains("WARN")
                    && line.contains("command replay has partial or rejected effects")
            })
            .expect("warning must explain the unsampled turn");
        assert!(warning.contains("message_type=11292"), "{warning}");
        assert!(warning.contains("turn_tick=Some(607)"), "{warning}");
        assert!(warning.contains("command_index=0 type=9"), "{warning}");
        assert!(
            warning.contains(&crate::net::connection::hex(&offending)),
            "{warning}"
        );
        assert!(text.contains("feed animal"));
        assert!(text.contains("command effects rolled back"));
        assert!(text.contains("prepared own-home snapshot"));
        assert!(!text.contains(&account.pass_token));
        assert!(!text.contains(&crate::net::connection::hex(account.pass_token.as_bytes())));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn full_handshake_post_login_and_ack() {
        let config = config();
        let mut session = Session::new(&config);
        let client = TestClient::new(&config.secret);

        let step = session.handle(&TestClient::hello()).unwrap();
        assert_eq!(step.replies.len(), 1);
        let server_hello = &step.replies[0];
        assert_eq!(
            (server_hello.msg_type, server_hello.payload.len()),
            (20100, 28)
        );

        let step = session.handle(&client.login(server_hello)).unwrap();
        let (ok, mut ciphers) = client.login_ok(&step.replies[0]);
        assert_eq!((ok.account_id, ok.pass_token.as_str()), (1, "token"));
        assert!(session.post_login_counts().is_some());

        let step = session.handle(&ciphers.send(10366, &[])).unwrap();
        assert_eq!(step.replies.len(), 1);
        assert_eq!(step.replies[0].msg_type, 29997);
        assert!(ciphers.peek(&step.replies[0]).is_empty());
        // A repeated 10366 does not resend.
        assert!(session
            .handle(&ciphers.send(10366, &[]))
            .unwrap()
            .replies
            .is_empty());

        session.handle(&ciphers.send(10099, &[])).unwrap();
        assert_eq!(session.post_login_counts(), Some((1, 1)));

        // With a delay, frames are deferred, still sealed in nonce order.
        let delayed = Config {
            after_ready_delay: std::time::Duration::from_secs(1),
            after_ready: vec![Outgoing::new(&SessionReady::default()); 2],
            ..config
        };
        let mut session = Session::new(&delayed);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let step = session.handle(&client.login(&hello)).unwrap();
        let (_, mut ciphers) = client.login_ok(&step.replies[0]);
        let step = session.handle(&ciphers.send(10366, &[])).unwrap();
        assert!(step.replies.is_empty());
        assert_eq!(step.deferred.len(), 2);
        // A request arriving before the deferred send is sealed first; the
        // deferred frames follow in nonce order.
        let step2 = session.handle(&ciphers.send(13447, &[0])).unwrap();
        let types: Vec<u16> = step2.replies.iter().map(|f| f.msg_type).collect();
        assert_eq!(types, [22151, 23549]);
        assert_eq!(
            ciphers.receive(&step2.replies[0]),
            PingReply::default().to_bytes()
        );
        assert_eq!(
            ciphers.receive(&step2.replies[1]),
            HomeSync::default().to_bytes()
        );
        for out in &step.deferred {
            let (_, frame) = session.seal_deferred(out).unwrap();
            assert!(ciphers.peek(&frame).is_empty());
        }
        // 19336 after 13447: 22151 was already sent, so nothing new.
        let step3 = session.handle(&ciphers.send(19336, &[])).unwrap();
        assert!(step3.replies.is_empty());
        assert_eq!(session.seen_counts().unwrap()[&13447], 1);
    }

    #[test]
    fn rejects_out_of_order_and_tampered_frames() {
        let config = config();
        let mut session = Session::new(&config);
        let login_first = Frame {
            msg_type: 10101,
            version: 0,
            payload: vec![0; 64],
        };
        assert!(session.handle(&login_first).is_err());

        let mut session = Session::new(&config);
        let client = TestClient::new(&config.secret);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let mut login = client.login(&hello);
        *login.payload.last_mut().unwrap() ^= 1;
        assert!(session.handle(&login).is_err());
    }

    #[test]
    fn session_ready_does_not_consume_a_nonce() {
        let config = Config {
            after_ready: vec![
                Outgoing::new(&SessionReady::default()),
                Outgoing::new(&PingReply::default()),
                Outgoing::new(&SessionReady::default()),
                Outgoing::new(&HomeSync::default()),
            ],
            ..config()
        };
        let mut session = Session::new(&config);
        let client = TestClient::new(&config.secret);
        let hello = session
            .handle(&TestClient::hello())
            .unwrap()
            .replies
            .remove(0);
        let step = session.handle(&client.login(&hello)).unwrap();
        let (_, mut ciphers) = client.login_ok(&step.replies[0]);
        let replies = session.handle(&ciphers.send(10366, &[])).unwrap().replies;
        // Mirrors the live client: 22151 and 23549 decrypt at +2 and +4.
        assert!(ciphers.peek(&replies[0]).is_empty());
        assert_eq!(
            ciphers.receive(&replies[1]),
            PingReply::default().to_bytes()
        );
        assert_eq!(ciphers.receive(&replies[3]), HomeSync::default().to_bytes());
    }

    #[test]
    fn nonce_steps_apply_per_message_with_carry() {
        let mut c = Cipher::new([0; 32], [0xff; 24], vec![2, 1]);
        c.advance();
        assert_eq!(c.nonce[..2], [0x01, 0x00]);
        c.advance(); // step 1
        assert_eq!(c.nonce[0], 0x02);
        c.advance(); // last step repeats
        assert_eq!(c.nonce[0], 0x03);
    }

    #[test]
    fn outgoing_spec_parsing() {
        assert_eq!(
            Outgoing::parse("26306:00000001").unwrap(),
            Outgoing {
                msg_type: 26306,
                body: vec![0, 0, 0, 1],
                corrupt: false,
            }
        );
        assert_eq!(Outgoing::parse("29997").unwrap().body, Vec::<u8>::new());
        assert!(Outgoing::parse("29997!").unwrap().corrupt);
        let path = std::env::temp_dir().join(format!("hd-body-{}", std::process::id()));
        std::fs::write(&path, [1, 2, 3]).unwrap();
        let from_file = Outgoing::parse(&format!("29950@{}", path.display())).unwrap();
        assert_eq!((from_file.msg_type, from_file.body), (29950, vec![1, 2, 3]));
        std::fs::remove_file(path).unwrap();
        assert_eq!(Outgoing::parse("disconnect:1").unwrap().body, [0, 0, 0, 1]);
        assert_eq!(Outgoing::parse("entity-batch").unwrap().msg_type, 25491);
        let notice = Outgoing::parse("notice:3:hi").unwrap();
        assert_eq!(notice.msg_type, 20103);
        assert_eq!(notice.body, StatusNotice::new(3, "hi").to_bytes());
        assert!(Outgoing::parse("29997:0").is_err());
        assert!(Outgoing::parse("x").is_err());
        let warning = Outgoing::parse("maintenance:300").unwrap();
        assert_eq!((warning.msg_type, warning.body), (20415, vec![0, 0, 1, 44]));
        let warning = Outgoing::parse("chat-silence").unwrap();
        assert_eq!((warning.msg_type, warning.body), (22220, vec![1]));
        for bad in ["22220", "22220:02", "22220:03", "22220:0100"] {
            assert!(Outgoing::parse(bad).is_err(), "{bad}");
        }

        let path = std::env::temp_dir().join(format!("hd-area-home-{}", std::process::id()));
        let home = crate::game::home::HomeData::default();
        crate::storage::accounts::atomic_json(&path, &home).unwrap();
        for (area, kind) in [
            ("own-fishing", 22852),
            ("other-fishing", 28185),
            ("own-town", 26650),
            ("other-town", 26359),
            ("ai-town", 28557),
        ] {
            let out = Outgoing::parse(&format!("{area}@{}", path.display())).unwrap();
            assert_eq!(out.msg_type, kind);
            let blobs = if matches!(kind, 28185 | 26359) {
                crate::protocol::messages::OtherHomeData::from_bytes(&out.body)
                    .unwrap()
                    .blobs
            } else {
                OwnHomeData::from_bytes(&out.body).unwrap().blobs
            };
            assert_eq!(blobs, home.blobs());
        }
        std::fs::remove_file(path).unwrap();
    }
}
