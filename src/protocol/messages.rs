//! Typed message bodies with layouts taken from the native decoders.
//! Everything here is plaintext body content; framing and encryption live in
//! `scframe` and `session`.

use std::{collections::BTreeMap, io::Write};

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use serde_json::{Value, json};

use super::stream::{Reader, Writer};

/// Types handled without a typed body.
pub mod ids {
    /// Encrypted login envelope: client public key + sealed challenge/nonce/body.
    pub const LOGIN: u16 = 10101;
    /// Client report of a server message that failed decryption: body is its BE32 type.
    pub const INBOUND_ENVELOPE: u16 = 10099;
}

pub trait Message: Sized {
    const TYPE: u16;
    fn encode(&self, w: &mut Writer);
    fn decode(r: &mut Reader) -> Result<Self>;

    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.into_bytes()
    }
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let message = Self::decode(&mut r)?;
        r.finish()?;
        Ok(message)
    }
}

/// 11489: requested farm name. The client enforces neither emptiness nor
/// `MaxFarmNameLength`, so [`ChangeHomeName::validate`] does that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeHomeName(pub String);

/// `MaxFarmNameLength` in game_config.csv.
pub const MAX_FARM_NAME_CHARS: usize = 25;

impl ChangeHomeName {
    /// The name to store, trimmed, or `None` when it must be rejected.
    pub fn validate(&self) -> Option<String> {
        let name = self.0.trim();
        (!name.is_empty()
            && name.chars().count() <= MAX_FARM_NAME_CHARS
            && !name.chars().any(char::is_control))
        .then(|| name.to_owned())
    }
}

impl Message for ChangeHomeName {
    const TYPE: u16 = 11489;
    fn encode(&self, w: &mut Writer) {
        w.string(&self.0);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(Self(r.string()?.context("missing farm name")?))
    }
}

/// 21608: ChangeHomeNameResponseMessage, one VInt result code (decoder 0x1D7F26).
/// On success the client commits its pending name to avatar +0x10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeHomeNameResponse {
    Failed = 0,
    Ok = 1,
    BadWord = 2,
}

impl Message for ChangeHomeNameResponse {
    const TYPE: u16 = 21608;
    fn encode(&self, w: &mut Writer) {
        w.vint(*self as u32);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(match r.vint()? {
            0 => Self::Failed,
            1 => Self::Ok,
            2 => Self::BadWord,
            code => bail!("unknown ChangeHomeNameResponse code {code}"),
        })
    }
}

/// The captured friend-ID list shared by 14690 and 17887.
#[derive(Debug, PartialEq, Eq)]
pub enum FriendIdReport {
    WithLanguage {
        ids: Vec<u64>,
        language: Option<String>,
    },
    WithFlag {
        ids: Vec<u64>,
        flag: bool,
        trailing_id: u64,
    },
}

impl FriendIdReport {
    pub fn decode(kind: u16, body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        let count = usize::try_from(r.vint()?)?;
        let size = count
            .checked_mul(8)
            .context("friend ID list size overflow")?;
        let ids = r
            .raw(size)?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&id| u64::from_be_bytes(id))
            .collect();
        let report = match kind {
            14690 => Self::WithLanguage {
                ids,
                language: r.string()?,
            },
            17887 => Self::WithFlag {
                ids,
                flag: r.bool()?,
                trailing_id: r.long()?,
            },
            _ => anyhow::bail!("unsupported friend ID report {kind}"),
        };
        r.finish()?;
        Ok(report)
    }
}

/// 20415: maintenance countdown in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceWarning(pub u32);

impl Message for MaintenanceWarning {
    const TYPE: u16 = 20415;
    fn encode(&self, w: &mut Writer) {
        w.int(self.0);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let seconds = r.int()?;
        ensure!(
            seconds <= i32::MAX as u32,
            "maintenance seconds exceed int32"
        );
        Ok(Self(seconds))
    }
}

/// 22220: only kinds 0 and 1 have been observed to be safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSilenceNotice {
    None,
    Silenced,
}

impl Message for ChatSilenceNotice {
    const TYPE: u16 = 22220;
    fn encode(&self, w: &mut Writer) {
        w.vint(u32::from(*self == Self::Silenced));
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        match r.vint()? {
            0 => Ok(Self::None),
            1 => Ok(Self::Silenced),
            kind => anyhow::bail!("unsupported chat silence notice kind {kind}"),
        }
    }
}

/// A message with no body (base decoder `0x615010`, or a wrapper decoder that
/// reads nothing, such as 29997's `0x6efdd0`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fieldless<const ID: u16>;

impl<const ID: u16> Message for Fieldless<ID> {
    const TYPE: u16 = ID;
    fn encode(&self, _: &mut Writer) {}
    fn decode(_: &mut Reader) -> Result<Self> {
        Ok(Self)
    }
}

/// Client → server after LoginOk; empty body (ctor `0xdc646`).
/// : mode 1 (own farm) entered, not a request requiring a fresh snapshot.
pub type PostLoginReady = Fieldless<10366>;
/// : empty friend-list request, unrelated to the current farm mode.
pub type FriendListRequest = Fieldless<19838>;

/// Observed mode 2 visit: selector, then BE32 high/low account ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitPlayer {
    pub selector: u32,
    pub target_account_id: u64,
}
impl Message for VisitPlayer {
    const TYPE: u16 = 18485;
    fn encode(&self, w: &mut Writer) {
        w.vint(self.selector).long(self.target_account_id);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            selector: r.vint()?,
            target_account_id: r.long()?,
        })
    }
}

/// FriendEntry decoder at 0xce10c. Numeric field meanings remain unknown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FriendEntry {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub facebook_id: Option<String>,
    #[serde(default)]
    pub gamecenter_id: Option<String>,
    pub values: [u32; 4],
}

/// Non-null friend list, including the default empty form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FriendList(pub Vec<FriendEntry>);
impl Message for FriendList {
    const TYPE: u16 = 26656;
    fn encode(&self, w: &mut Writer) {
        w.vint(self.0.len() as u32);
        for entry in &self.0 {
            w.long(entry.id).string(&entry.name);
            for value in [&entry.facebook_id, &entry.gamecenter_id] {
                if let Some(value) = value {
                    w.string(value);
                } else {
                    w.int(u32::MAX);
                }
            }
            for value in entry.values {
                w.vint(value);
            }
        }
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let count = r.vint()?;
        ensure!(count <= 10000, "friend list too large");
        let mut entries = Vec::new();
        for _ in 0..count {
            entries.push(FriendEntry {
                id: r.long()?,
                name: r.string()?.context("friend name is null")?,
                facebook_id: r.string()?,
                gamecenter_id: r.string()?,
                values: [r.vint()?, r.vint()?, r.vint()?, r.vint()?],
            });
        }
        Ok(Self(entries))
    }
}

/// Decoder at 0x186306: VInt count followed by FriendEntry records. Only the
/// confirmed empty list is modeled; whether it leaves Greg in the UI is untested.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmptyFriendList;
impl Message for EmptyFriendList {
    const TYPE: u16 = 26656;
    fn encode(&self, w: &mut Writer) {
        w.vint(0);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        ensure!(r.vint()? == 0, "only an empty friend list is supported");
        Ok(Self)
    }
}

/// Server → client; wrapper decoder reads nothing, handler sets the game
/// singleton's ready flag (`0x5522c8` → `0x1bb36c(singleton, 0)`).
pub type SessionReady = Fieldless<29997>;

/// 10100, cleartext. Layout from static analysis of the x86 game library and
/// live observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHello {
    pub protocol: u32,
    pub key_version: u32,
    pub major: u32,
    pub minor: u32,
    pub build: u32,
    /// SHA-1 over the per-file hashes in `assets/fingerprint.json` (its
    /// top-level `"sha"` field): identifies the client's asset set.
    pub fingerprint: String,
    /// Two trailing ints, both 2 in every capture; meaning unknown.
    pub unknown: [u32; 2],
}

impl ClientHello {
    pub const SUPPORTED: (u32, u32, u32, u32, u32) = (3, 14, 1, 43, 150);
    pub fn check_supported(&self) -> Result<()> {
        let got = (
            self.protocol,
            self.key_version,
            self.major,
            self.minor,
            self.build,
        );
        ensure!(
            got == Self::SUPPORTED,
            "unsupported ClientHello {got:?}; expected Hay Day 1.43.150 protocol 3/key 14"
        );
        Ok(())
    }
}

impl Message for ClientHello {
    const TYPE: u16 = 10100;
    fn encode(&self, w: &mut Writer) {
        w.int(self.protocol)
            .int(self.key_version)
            .int(self.major)
            .int(self.minor)
            .int(self.build)
            .string(&self.fingerprint)
            .int(self.unknown[0])
            .int(self.unknown[1]);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            protocol: r.int()?,
            key_version: r.int()?,
            major: r.int()?,
            minor: r.int()?,
            build: r.int()?,
            fingerprint: r.string()?.unwrap_or_default(),
            unknown: [r.int()?, r.int()?],
        })
    }
}

/// 20100, cleartext. Decoder `0x0f1a70`: length-prefixed session challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHello {
    pub challenge: [u8; 24],
}

impl Message for ServerHello {
    const TYPE: u16 = 20100;
    fn encode(&self, w: &mut Writer) {
        w.bytes(&self.challenge);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let bytes = r.bytes()?.unwrap_or_default();
        Ok(Self {
            challenge: bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("challenge must be 24 bytes"))?,
        })
    }
}

/// How LoginOk ends after the field-17 vector (RECONCILIATION.md §3.3/§5).
/// The handler's only data-sized allocation reads the field-18 box
/// (`msg+0x78`, `0x55302e -> 0x1aa91c`), the suspected `bad_alloc` source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginOkTail {
    /// `int 0` + bits `00 00`: byte-identical to the first accepted body.
    #[default]
    Legacy,
    /// End after the vector count; the decoder's EOF guard skips 18–20.
    Trimmed,
    /// A real boxed blob `00000004 00000000`, no trailing bits.
    Boxed,
}

impl std::str::FromStr for LoginOkTail {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "legacy" => Self::Legacy,
            "trimmed" => Self::Trimmed,
            "boxed" => Self::Boxed,
            _ => anyhow::bail!("expected legacy, trimmed or boxed"),
        })
    }
}

/// 29501 body (decoder `0x2021fe`). Native layout per RECONCILIATION.md
/// §3.3: 4 VInt, bit, 2 x int64, 8 strings (the last two capped at 50
/// bytes), u32 (field 16), `vector<u32>` (field 17, VInt count; only empty
/// supported), optional boxed blob (18) and two optional bits (19–20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOk {
    /// Four leading VInts; `1, 43, 150, 0` is accepted (version-like).
    pub leading: [u32; 4],
    pub flag: bool,
    pub account_id: u64,
    pub home_id: u64,
    pub server_time: String,
    pub created_time: String,
    pub pass_token: String,
    /// Four strings between the token and country (social IDs etc.); unmapped.
    pub social: [String; 4],
    pub country: String,
    /// Field 16, a bare u32 (previously mis-modelled as an empty string).
    pub field16: u32,
    pub tail: LoginOkTail,
}

impl LoginOk {
    pub fn development(account_id: u64, pass_token: &str) -> Self {
        Self {
            leading: [1, 43, 150, 0],
            flag: false,
            account_id,
            home_id: account_id,
            server_time: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            created_time: "2026-11-20T00:00:00Z".into(),
            pass_token: pass_token.into(),
            social: Default::default(),
            country: "FR".into(),
            field16: 0,
            tail: LoginOkTail::Legacy,
        }
    }
}

impl Message for LoginOk {
    const TYPE: u16 = 29501;
    fn encode(&self, w: &mut Writer) {
        for v in self.leading {
            w.vint(v);
        }
        w.bool(self.flag)
            .long(self.account_id)
            .long(self.home_id)
            .string(&self.server_time)
            .string(&self.created_time)
            .string(&self.pass_token);
        for s in &self.social {
            w.string(s);
        }
        w.string(&self.country).int(self.field16);
        w.vint(0); // field 17: empty vector<u32>
        match self.tail {
            LoginOkTail::Legacy => {
                w.int(0).raw(&[0, 0]);
            }
            LoginOkTail::Trimmed => {}
            LoginOkTail::Boxed => {
                w.bytes(&[0; 4]);
            }
        }
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let s = |r: &mut Reader| -> Result<String> { Ok(r.string()?.unwrap_or_default()) };
        let leading = [r.vint()?, r.vint()?, r.vint()?, r.vint()?];
        let flag = r.bool()?;
        let account_id = r.long()?;
        let home_id = r.long()?;
        let server_time = s(r)?;
        let created_time = s(r)?;
        let pass_token = s(r)?;
        let social = [s(r)?, s(r)?, s(r)?, s(r)?];
        let country = s(r)?;
        let field16 = r.int()?;
        ensure!(r.vint()? == 0, "field-17 elements not supported");
        let tail = if r.is_empty() {
            LoginOkTail::Trimmed
        } else {
            match r.bytes()? {
                Some([]) => {
                    ensure!(r.raw(2)? == [0, 0], "unsupported trailing bits");
                    LoginOkTail::Legacy
                }
                Some(b) if b == [0; 4] => LoginOkTail::Boxed,
                _ => anyhow::bail!("unsupported field-18 blob"),
            }
        };
        Ok(Self {
            leading,
            flag,
            account_id,
            home_id,
            server_time,
            created_time,
            pass_token,
            social,
            country,
            field16,
            tail,
        })
    }
}

/// 20103 (decoder `0x213844`, object 0x54 bytes, handler `0x550839`).
/// The handler forces world phase 3 and dispatches on `kind`: 1–18 have
/// dedicated arms (jump table at `0x861de8`); 3–6 and out-of-range kinds call
/// the generic error dialog `0x6a7be2(game, 4, text(kind))`. `message`
/// (`+0x40`) is the string the handler takes for kinds 8–10.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusNotice {
    pub kind: u32,
    /// `+0x3c`; role unknown.
    pub text: Option<String>,
    /// `+0x40`.
    pub message: Option<String>,
    /// Optional trailing block (read only while bytes remain).
    pub extra_int: u32,
    pub flag: bool,
    pub extra_text: Option<String>,
    /// Two VInts read unconditionally at the end (`+0x4c`, `+0x50`).
    pub tail: [u32; 2],
}

impl StatusNotice {
    pub fn new(kind: u32, message: &str) -> Self {
        Self {
            kind,
            text: None,
            message: Some(message.into()),
            extra_int: 0,
            flag: false,
            extra_text: None,
            tail: [0, 0],
        }
    }
}

impl Message for StatusNotice {
    const TYPE: u16 = 20103;
    fn encode(&self, w: &mut Writer) {
        let opt = |w: &mut Writer, s: &Option<String>| match s {
            Some(s) => {
                w.string(s);
            }
            None => {
                w.int(u32::MAX);
            }
        };
        w.int(self.kind);
        opt(w, &self.text);
        opt(w, &self.message);
        w.int(self.extra_int).bool(self.flag);
        opt(w, &self.extra_text);
        w.vint(self.tail[0]).vint(self.tail[1]);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let kind = r.int()?;
        let text = r.string()?;
        let message = r.string()?;
        let extra_int = if r.is_empty() { 0 } else { r.int()? };
        let flag = !r.is_empty() && r.bool()?;
        let extra_text = if r.is_empty() { None } else { r.string()? };
        Ok(Self {
            kind,
            text,
            message,
            extra_int,
            flag,
            extra_text,
            tail: [r.vint()?, r.vint()?],
        })
    }
}

/// 23549: home/world sync reply to the 13447 request (decoder `0x588992`,
/// handler `0x550d76` sets `world+0x230 = 2`; its install step requires
/// `world+0x234 == 2`, i.e. a prior 22151). Spec: `HOME_PROTOCOL.md` §2.
/// Only empty lists are encoded: the list-element layout differs between
/// GLM reports (2 VInts vs BE32,BE32,VInt), so it is not guessed here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomeSync {
    /// Trailing VInt (`+0x34`).
    pub trailing: u32,
}

impl Message for HomeSync {
    const TYPE: u16 = 23549;
    fn encode(&self, w: &mut Writer) {
        w.vint(0).vint(0).vint(0).vint(self.trailing);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        for _ in 0..3 {
            ensure!(r.vint()? == 0, "list decoding not implemented");
        }
        Ok(Self {
            trailing: r.vint()?,
        })
    }
}

// These offsets describe the observed client layout. Variable-length
// preceding fields or vector elements would move subsequent fields.
// First map boundary measured at 0x61e34c for the original minimal message.
const OWN_HOME_STATS_OFFSET: usize = 365;
const OWN_HOME_STATS_END: usize = OWN_HOME_STATS_OFFSET + 1;
const OWN_HOME_AVATAR_VECTOR_OFFSET: usize = 372;
const OWN_HOME_AVATAR_VECTOR_END: usize = OWN_HOME_AVATAR_VECTOR_OFFSET + 1;
// Layout helpers derive bitset positions from the fixed phase-B arrays instead
// of treating 26017 bytes as a hand-maintained blob.
const OWN_HOME_AVATAR_START: usize = 24;
/// BE32 length of the avatar's farm-name string (avatar +0x10, read at
/// 0x446ab8). The prefix models it as a 4-byte null-or-empty placeholder; a
/// real name is spliced in on the wire. Null (`ffffffff`) keeps the NameFarm
/// tutorial step (3600017) pending; any non-null string completes it.
const OWN_HOME_NAME_OFFSET: usize = 4;
const OWN_HOME_NAME_END: usize = OWN_HOME_NAME_OFFSET + 4;
fn own_home_bitset_offset(kind: u32) -> usize {
    let mut offset = OWN_HOME_AVATAR_START;
    for (t, _, _, _, count) in crate::game::starter::layout() {
        if *t == kind {
            return offset;
        }
        offset += count * 4;
    }
    panic!("type missing from avatar layout: {kind}");
}
/// Byte offset of the required presence flag in the otherwise opaque prefix.
const OWN_HOME_PRESENCE_OFFSET: usize = 395;
const OWN_HOME_PREFIX: [u8; 411] = {
    let mut prefix = [0; 411];
    prefix[OWN_HOME_PRESENCE_OFFSET] = 1;
    prefix
};
const OTHER_HOME_PREFIX: [u8; 819] = {
    let mut prefix = [0; 819];
    prefix[803] = 1;
    prefix
};

fn write_home(w: &mut Writer, prefix: &[u8], blobs: &[Value; 3]) {
    w.raw(prefix);
    for blob in blobs {
        write_json_blob(w, blob);
    }
}

fn read_home(r: &mut Reader, prefix: &[u8], name: &str) -> Result<[Value; 3]> {
    ensure!(
        r.raw(prefix.len())? == prefix,
        "{name} prefix differs from the verified {}-byte prefix",
        prefix.len()
    );
    let blobs = [read_json_blob(r)?, read_json_blob(r)?, read_json_blob(r)?];
    r.finish()?;
    Ok(blobs)
}
// Match the ByteStream's field bound, including the inflated representation.
const MAX_JSON_BLOB: u64 = 1 << 20;

/// BE32 container length, LE32 JSON length, then level-9 zlib compact UTF-8 JSON.
fn write_json_blob(w: &mut Writer, v: &Value) {
    let json = serde_json::to_vec(v).expect("serializing a JSON Value to Vec cannot fail");
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&json).expect("zlib writing to Vec");
    let compressed = encoder.finish().expect("zlib finishing into Vec");
    w.int((4 + compressed.len()) as u32)
        .raw(&(json.len() as u32).to_le_bytes())
        .raw(&compressed);
}

/// Reads the shared JSON blob container, validating both lengths and the zlib stream.
fn read_json_blob(r: &mut Reader) -> Result<Value> {
    let bytes = r.bytes()?.context("JSON blob must not be null")?;
    let mut blob = Reader::new(bytes);
    let expected = u32::from_le_bytes(blob.array()?) as u64;
    ensure!(
        expected <= MAX_JSON_BLOB,
        "JSON blob inflated length {expected} too large"
    );
    // The remaining bytes are exactly one zlib stream, not another ByteStream field.
    let compressed = blob.raw(bytes.len() - 4)?;
    let mut decoder = Decompress::new(true);
    let mut json = vec![0; expected as usize + 1];
    let status = decoder
        .decompress(compressed, &mut json, FlushDecompress::Finish)
        .context("inflating JSON blob")?;
    json.truncate(decoder.total_out() as usize);
    ensure!(
        status == Status::StreamEnd,
        "JSON blob zlib stream is incomplete"
    );
    ensure!(
        json.len() as u64 == expected,
        "JSON blob length mismatch: declared {expected}, inflated {}",
        json.len()
    );
    ensure!(
        decoder.total_in() == compressed.len() as u64,
        "trailing bytes after JSON blob zlib stream"
    );
    let value: Value = serde_json::from_slice(&json).context("invalid JSON in blob")?;
    ensure!(value.is_object(), "JSON blob root must be an object");
    Ok(value)
}

fn write_avatar_stats(w: &mut Writer, stats: &BTreeMap<u32, u32>) {
    w.vint(stats.len().try_into().expect("stats count fits u32"));
    for (&id, &quantity) in stats {
        w.vint(id).vint(quantity);
    }
}

fn read_avatar_stats(r: &mut Reader) -> Result<BTreeMap<u32, u32>> {
    let count = r.vint()?;
    ensure!(
        count <= MAX_JSON_BLOB as u32,
        "avatar stats count too large"
    );
    let mut stats = BTreeMap::new();
    for _ in 0..count {
        let id = r.vint()?;
        let quantity = r.vint()?;
        ensure!(
            stats.insert(id, quantity).is_none(),
            "duplicate avatar stat ID {id}"
        );
    }
    Ok(stats)
}

/// 26017: own-home data. The 411-byte prefix and A/B = {}, C = the minimal
/// 85 x 71 empty tile map were verified to load a farm on the real client.
/// In this empty-vector layout the prefix contains a VInt, a 408-byte avatar
/// and a 2-byte optional element. The first sparse stats map is modeled; other
/// prefix fields remain opaque. The original byte 395 presence bit is retained
/// in the suffix, which moves as the map grows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnHomeData {
    pub unlocked: BTreeMap<u32, Vec<u32>>,
    /// First sparse map in avatar+0x1c: count followed by VInt ID/value pairs.
    /// Sorted IDs make output deterministic; unrelated entries are retained.
    pub avatar_stats: BTreeMap<u32, u32>,
    /// Row unlock bitmap for fields.csv data type 4.
    pub fields_unlocked: u32,
    /// Six fixed-width BE32 words for tutorial data type 36.
    pub tutorial_words: [u32; 6],
    /// Farm name; `None` is sent as a null string.
    pub farm_name: Option<String>,
    /// Count of the unknown vector at wire avatar +0x408; only zero is supported.
    /// This is not XP and has no confirmed gameplay meaning.
    ///
    /// Hay Day 1.43.150 x86 ELF VAs: 0x446c74 reads the VInt (body offset
    /// 372 in the minimal observed message), and 0x446c86 calls reserve at 0x75aafa.
    /// The client allocates 0x10 bytes per element, constructs at 0x1fd642,
    /// decodes at 0x206d2c, then inserts at 0x74c62c. Nonempty vectors require
    /// implementing that element decoder's wire format first; the allocation
    /// size does not establish an element's serialized size.
    ///
    /// Encoding a nonzero count panics before writing any bytes because the
    /// infallible Message encoder cannot emit elements safely yet.
    pub unknown_avatar_vector_count: u32,
    pub blobs: [Value; 3],
}

impl Default for OwnHomeData {
    fn default() -> Self {
        Self {
            unlocked: BTreeMap::new(),
            avatar_stats: BTreeMap::new(),
            fields_unlocked: 3,
            tutorial_words: [0; 6],
            farm_name: None,
            unknown_avatar_vector_count: 0,
            blobs: [
                json!({}),
                json!({}),
                json!({
                    "TileMapWidth": 85, "TileMapHeight": 71,
                    "ObjectVersion": 2, "Objects": [],
                }),
            ],
        }
    }
}

impl Message for OwnHomeData {
    const TYPE: u16 = 26017;
    fn encode(&self, w: &mut Writer) {
        assert_eq!(
            self.unknown_avatar_vector_count, 0,
            "nonempty unknown avatar vector elements are not supported"
        );
        let mut prefix = OWN_HOME_PREFIX;
        let mut offset = OWN_HOME_AVATAR_START;
        for (kind, _, _, _, count) in crate::game::starter::layout() {
            if let Some(words) = self.unlocked.get(kind) {
                assert_eq!(words.len(), *count);
                for (i, word) in words.iter().enumerate() {
                    prefix[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&word.to_be_bytes());
                }
            }
            offset += count * 4;
        }
        prefix[own_home_bitset_offset(4)..own_home_bitset_offset(4) + 4]
            .copy_from_slice(&self.fields_unlocked.to_be_bytes());
        for (index, word) in self.tutorial_words.iter().enumerate() {
            let start = own_home_bitset_offset(36) + index * 4;
            prefix[start..start + 4].copy_from_slice(&word.to_be_bytes());
        }
        w.raw(&prefix[..OWN_HOME_NAME_OFFSET]);
        match &self.farm_name {
            Some(name) => w.string(name),
            None => w.int(u32::MAX),
        };
        w.raw(&prefix[OWN_HOME_NAME_END..OWN_HOME_STATS_OFFSET]);
        write_avatar_stats(w, &self.avatar_stats);
        w.raw(&prefix[OWN_HOME_STATS_END..OWN_HOME_AVATAR_VECTOR_OFFSET])
            .vint(self.unknown_avatar_vector_count);
        write_home(w, &prefix[OWN_HOME_AVATAR_VECTOR_END..], &self.blobs);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let mut prefix = r.raw(OWN_HOME_NAME_OFFSET)?.to_vec();
        // Empty strings (all-zero fixtures) decode as None as well.
        let farm_name = r.string()?.filter(|name| !name.is_empty());
        prefix.extend_from_slice(&[0; 4]);
        prefix.extend_from_slice(r.raw(OWN_HOME_STATS_OFFSET - OWN_HOME_NAME_END)?);
        ensure!(
            prefix[..OWN_HOME_AVATAR_START] == OWN_HOME_PREFIX[..OWN_HOME_AVATAR_START]
                && prefix[364] == 0,
            "OwnHomeData prefix before unknown avatar vector differs from verified prefix"
        );
        let mut unlocked = BTreeMap::new();
        let mut offset = OWN_HOME_AVATAR_START;
        for (kind, _, _, _, count) in crate::game::starter::layout() {
            let words: Vec<u32> = prefix[offset..offset + count * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_be_bytes(*b))
                .collect();
            if *kind != 4 && *kind != 36 && words.iter().any(|w| *w != 0) {
                unlocked.insert(*kind, words);
            }
            offset += count * 4;
        }
        let mut tutorial_words = [0u32; 6];
        for (index, word) in tutorial_words.iter_mut().enumerate() {
            let start = own_home_bitset_offset(36) + index * 4;
            *word = u32::from_be_bytes(prefix[start..start + 4].try_into().unwrap());
        }
        let fields_unlocked = u32::from_be_bytes(
            prefix[own_home_bitset_offset(4)..own_home_bitset_offset(4) + 4]
                .try_into()
                .unwrap(),
        );
        let avatar_stats = read_avatar_stats(r)?;
        ensure!(
            r.raw(OWN_HOME_AVATAR_VECTOR_OFFSET - OWN_HOME_STATS_END)?
                == &OWN_HOME_PREFIX[OWN_HOME_STATS_END..OWN_HOME_AVATAR_VECTOR_OFFSET],
            "OwnHomeData prefix after stats differs from verified prefix"
        );
        let unknown_avatar_vector_count = r.vint()?;
        ensure!(
            unknown_avatar_vector_count == 0,
            "nonempty unknown avatar vector elements are not supported"
        );
        Ok(Self {
            unlocked,
            avatar_stats,
            fields_unlocked,
            tutorial_words,
            farm_name,
            unknown_avatar_vector_count,
            blobs: read_home(
                r,
                &OWN_HOME_PREFIX[OWN_HOME_AVATAR_VECTOR_END..],
                "OwnHomeData",
            )?,
        })
    }
}

/// 28383: Greg's farm, live-verified with the unchanged APK ai.json in blob C.
/// Shares the opaque 411-byte prefix with OwnHomeData. Game data is supplied
/// externally; it must never replace the default own-home document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiHomeData {
    pub blobs: [Value; 3],
}

impl Message for AiHomeData {
    const TYPE: u16 = 28383;
    fn encode(&self, w: &mut Writer) {
        write_home(w, &OWN_HOME_PREFIX, &self.blobs);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            blobs: read_home(r, &OWN_HOME_PREFIX, "AiHomeData")?,
        })
    }
}

/// 29950: visiting another player. Live-verified opaque 819-byte prefix,
/// with the required presence flag at byte 803, followed by the same blobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherHomeData {
    pub blobs: [Value; 3],
}

impl Message for OtherHomeData {
    const TYPE: u16 = 29950;
    fn encode(&self, w: &mut Writer) {
        write_home(w, &OTHER_HOME_PREFIX, &self.blobs);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            blobs: read_home(r, &OTHER_HOME_PREFIX, "OtherHomeData")?,
        })
    }
}

/// 11292: client turn batch, decoded with the validated command schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndClientTurn {
    pub a: u32,
    /// Raw 32-bit word, including signed VInt representations (state checksum).
    pub b: u32,
    pub n: u32,
    /// Entire original stream after the three header VInts, including any unknown tail.
    pub commands: Vec<u8>,
    pub first_command_type: Option<u32>,
    pub rest: Vec<u8>,
    pub decoded: Vec<TurnCommand>,
    pub raw_remainder: Vec<u8>,
    #[cfg(test)]
    header: Vec<u8>,
}

/// Known semantic commands plus schema-decoded commands with unresolved effects.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnCommand {
    Schema {
        command_type: u32,
        fields: Vec<Value>,
        #[serde(skip)]
        raw: Vec<u8>,
    },
    TutorialCompleted {
        data_id: i32,
        tick: i32,
    },
    TutorialStarted {
        data_id: i32,
        tick: i32,
    },
    Harvest {
        object_id: i32,
        tick: i32,
    },
    FollowUp22 {
        object_id: i32,
        tick: i32,
    },
    FollowUp23 {
        object_id: i32,
        tick: i32,
    },
    Plant {
        object_id: i32,
        data_id: i32,
        tick: i32,
    },
    People {
        data_id: i32,
        tick: i32,
    },
}
impl TurnCommand {
    pub fn command_type(&self) -> u32 {
        match self {
            Self::Schema { command_type, .. } => *command_type,
            Self::Plant { .. } => 1,
            Self::Harvest { .. } => 2,
            Self::FollowUp22 { .. } => 22,
            Self::FollowUp23 { .. } => 23,
            Self::TutorialCompleted { .. } => 34,
            Self::TutorialStarted { .. } => 35,
            Self::People { .. } => 43,
        }
    }
    pub fn tick(&self) -> Option<u32> {
        let tick = match self {
            Self::Schema {
                command_type,
                fields,
                ..
            } => {
                return crate::game::commands::tick(*command_type, fields);
            }
            Self::Plant { tick, .. }
            | Self::Harvest { tick, .. }
            | Self::FollowUp22 { tick, .. }
            | Self::FollowUp23 { tick, .. }
            | Self::TutorialCompleted { tick, .. }
            | Self::TutorialStarted { tick, .. }
            | Self::People { tick, .. } => *tick,
        };
        u32::try_from(tick).ok()
    }
    pub fn meaning(&self) -> &'static str {
        crate::game::commands::name(self.command_type())
    }
    fn read(input: &mut &[u8]) -> Option<Self> {
        let (kind, fields, raw) = crate::game::commands::decode(input)?;
        if ![1, 2, 22, 23, 34, 35, 43].contains(&kind) {
            return Some(Self::Schema {
                command_type: kind,
                fields,
                raw,
            });
        }
        let id = fields.first()?.as_i64()? as i32;
        let data_id = if kind == 1 {
            fields.get(1)?.as_i64()? as i32
        } else {
            id
        };
        let tick = fields.last()?.as_i64()? as i32;
        Some(match kind {
            1 => Self::Plant {
                object_id: id,
                data_id,
                tick,
            },
            2 => Self::Harvest {
                object_id: id,
                tick,
            },
            22 => Self::FollowUp22 {
                object_id: id,
                tick,
            },
            23 => Self::FollowUp23 {
                object_id: id,
                tick,
            },
            34 => Self::TutorialCompleted { data_id, tick },
            35 => Self::TutorialStarted { data_id, tick },
            43 => Self::People { data_id, tick },
            _ => unreachable!(),
        })
    }
    #[cfg(test)]
    fn values(&self) -> Vec<i32> {
        match *self {
            Self::Schema { .. } => unreachable!("schema commands retain their original bytes"),
            Self::Plant {
                object_id,
                data_id,
                tick,
            } => vec![1, object_id, data_id, tick],
            Self::Harvest { object_id, tick } => vec![2, object_id, tick],
            Self::FollowUp22 { object_id, tick } => vec![22, object_id, tick],
            Self::FollowUp23 { object_id, tick } => vec![23, object_id, tick],
            Self::TutorialCompleted { data_id, tick } => vec![34, data_id, tick],
            Self::TutorialStarted { data_id, tick } => vec![35, data_id, tick],
            Self::People { data_id, tick } => vec![43, data_id, tick],
        }
    }
}
impl EndClientTurn {
    /// Decode a known prefix only. Unknown types and truncated commands remain
    /// byte-exact in raw_remainder, starting at the undecoded command type.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let mut remaining = body;
        let a = turn_vint(&mut remaining)?;
        let b = turn_vint(&mut remaining)?;
        let n = turn_vint(&mut remaining)?;
        if n > 1000 {
            return None;
        }
        #[cfg(test)]
        let header = body[..body.len() - remaining.len()].to_vec();
        let commands = remaining.to_vec();
        let mut first = remaining;
        let first_command_type = if n > 0 {
            Some(turn_vint(&mut first)?)
        } else {
            None
        };
        let rest = first.to_vec();
        let mut decoded = Vec::new();
        for _ in 0..n {
            let mut candidate = remaining;
            let Some(command) = TurnCommand::read(&mut candidate) else {
                break;
            };
            decoded.push(command);
            remaining = candidate;
        }
        Some(Self {
            #[cfg(test)]
            header,
            a,
            b,
            n,
            commands,
            first_command_type,
            rest,
            decoded,
            raw_remainder: remaining.to_vec(),
        })
    }
    pub fn completed_tutorial_rows(body: &[u8]) -> Vec<u32> {
        Self::parse(body)
            .map(|turn| {
                turn.decoded
                    .into_iter()
                    .filter_map(|cmd| match cmd {
                        TurnCommand::TutorialCompleted { data_id, .. }
                            if (3600000..3600174).contains(&data_id) =>
                        {
                            Some(data_id as u32 - 3600000)
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Re-encode the typed prefix and untouched tail, retaining declared count.
    #[cfg(test)]
    pub fn to_bytes(&self) -> Vec<u8> {
        fn put(out: &mut Vec<u8>, v: i32) {
            let mut n = v >> 6;
            out.push(
                (v as u8 & 0x3f)
                    | if v < 0 { 0x40 } else { 0 }
                    | if n != 0 && n != -1 { 0x80 } else { 0 },
            );
            while n != 0 && n != -1 {
                let next = n >> 7;
                out.push((n as u8 & 0x7f) | if next != 0 && next != -1 { 0x80 } else { 0 });
                n = next;
            }
        }
        let mut out = self.header.clone();
        for command in &self.decoded {
            if let TurnCommand::Schema { raw, .. } = command {
                out.extend(raw);
            } else {
                for v in command.values() {
                    put(&mut out, v);
                }
            }
        }
        out.extend(&self.raw_remainder);
        out
    }
}

// Local to the turn logger: the shared ByteStream VInt reader remains unchanged.
// Bit 6 carries the sign; return the resulting int32 bit pattern as a u32.
pub(crate) fn turn_vint(input: &mut &[u8]) -> Option<u32> {
    let (&first, rest) = input.split_first()?;
    *input = rest;
    let negative = first & 0x40 != 0;
    let mut value = u64::from(first & 0x3f);
    let mut more = first & 0x80 != 0;
    let mut bits = 6;
    while more {
        if bits >= 32 {
            return None;
        }
        let (&byte, rest) = input.split_first()?;
        *input = rest;
        value |= u64::from(byte & 0x7f) << bits;
        bits += 7;
        more = byte & 0x80 != 0;
    }
    if negative {
        // The client's readVInt treats the fifth byte's signed representation explicitly
        // sets bit 31. Extending above bit 33 would lose that sign on cast.
        if bits >= 32 {
            value |= 1 << 31;
        } else {
            value |= u64::MAX << bits;
        }
    }
    Some(value as u32)
}

/// 20062: reply to 17274 (sets `world+0x238 = 2`; harmless, optional).
/// `home-reply-22151-20062.md`: VInt, boxed pair (2 x BE32), VInt count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionalIdsReply;

impl Message for OptionalIdsReply {
    const TYPE: u16 = 20062;
    fn encode(&self, w: &mut Writer) {
        w.vint(0).int(0).int(0).vint(0);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        r.vint()?;
        r.long()?;
        ensure!(r.vint()? == 0, "list decoding not implemented");
        Ok(Self)
    }
}

/// 29637: account/home summary via listener B (decoder `0x6e04c8`).
/// Only GLM's byte-exact all-zero minimal body (14 bytes,
/// `home-29637-spec.md` §2) is supported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountSummary;

impl Message for AccountSummary {
    const TYPE: u16 = 29637;
    fn encode(&self, w: &mut Writer) {
        w.raw(&[0; 14]);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        ensure!(r.raw(14)? == [0; 14], "only the minimal body is supported");
        Ok(Self)
    }
}

/// 22151: reply to the fieldless 19336 request (decoder `0x658350`, handler
/// `0x54fc84` sets `world+0x234 = 2`). Wire: VInt count (list `+0x3c`),
/// VInt count (list `+0x40`), VInt `+0x38`, string `+0x34` (reader
/// `0x146220`, live-verified as int length + bytes). Lists stay empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PingReply {
    pub value: u32,
    pub text: Option<String>,
}

impl Message for PingReply {
    const TYPE: u16 = 22151;
    fn encode(&self, w: &mut Writer) {
        w.vint(0).vint(0).vint(self.value);
        match &self.text {
            Some(text) => w.string(text),
            None => w.int(u32::MAX),
        };
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        ensure!(
            r.vint()? == 0 && r.vint()? == 0,
            "list decoding not implemented"
        );
        Ok(Self {
            value: r.vint()?,
            text: r.string()?,
        })
    }
}

/// 25491: farm/home entity batch via listener B (decoder `0x71382a`).
/// Three VInts then a VInt entity count; entities are not implemented, so
/// only the empty batch (`HOME_PROTOCOL.md` §3 minimal payload) is encoded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityBatch {
    pub header: [u32; 3],
}

impl Message for EntityBatch {
    const TYPE: u16 = 25491;
    fn encode(&self, w: &mut Writer) {
        for v in self.header {
            w.vint(v);
        }
        w.vint(0);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        let header = [r.vint()?, r.vint()?, r.vint()?];
        ensure!(r.vint()? == 0, "entity decoding not implemented");
        Ok(Self { header })
    }
}

/// 25892: server-initiated disconnect. Decoder `0x2bd160` reads one BE32
/// reason into `msg+0x34` and nothing else, based on x86 library analysis.
/// Live: reason 1 shows "Connection lost — Another device is connecting to
/// this farm." (popup 13); reason 0 and an empty body show nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherDeviceConnected {
    pub reason: u32,
}

impl Message for OtherDeviceConnected {
    const TYPE: u16 = 25892;
    fn encode(&self, w: &mut Writer) {
        w.int(self.reason);
    }
    fn decode(r: &mut Reader) -> Result<Self> {
        Ok(Self { reason: r.int()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn _empty_friend_list_is_one_zero_byte() {
        assert_eq!(EmptyFriendList.to_bytes(), [0]);
        assert_eq!(EmptyFriendList::from_bytes(&[0]).unwrap(), EmptyFriendList);
        assert!(EmptyFriendList::from_bytes(&[1]).is_err());
        assert!(EmptyFriendList::from_bytes(&[0, 0]).is_err());
        assert!(FriendListRequest::from_bytes(&[]).is_ok());
        assert!(FriendListRequest::from_bytes(&[0]).is_err());
    }
    #[test]
    fn friend_reports_match_captured_layouts() {
        let ids = vec![
            0x7fff_ffff_0000_0001,
            0x7fff_ffff_0000_0002,
            0x7fff_ffff_0000_0003,
        ];
        let mut w = Writer::new();
        w.vint(ids.len() as u32);
        for &id in &ids {
            w.long(id);
        }
        let mut request = w.clone();
        request.string("EN");
        let request = request.into_bytes();
        assert_eq!(request.len(), 31);
        assert_eq!(
            FriendIdReport::decode(14690, &request).unwrap(),
            FriendIdReport::WithLanguage {
                ids: ids.clone(),
                language: Some("EN".into())
            }
        );
        assert!(FriendIdReport::decode(14690, &request[..30]).is_err());
        w.bool(true).long(0);
        assert_eq!(
            FriendIdReport::decode(17887, &w.into_bytes()).unwrap(),
            FriendIdReport::WithFlag {
                ids,
                flag: true,
                trailing_id: 0
            }
        );
    }

    #[test]
    fn player_visit_matches_live_target_account_capture() {
        let bytes = [0, 0, 0, 0, 0, 0, 0, 0, 1];
        let visit = VisitPlayer {
            selector: 0,
            target_account_id: 1,
        };
        assert_eq!(visit.to_bytes(), bytes);
        assert_eq!(VisitPlayer::from_bytes(&bytes).unwrap(), visit);
        assert!(VisitPlayer::from_bytes(&bytes[..8]).is_err());
        assert!(VisitPlayer::from_bytes(&[0; 10]).is_err());
    }
    #[test]
    fn one_friend_matches_experiment_and_round_trips() {
        let list = FriendList(vec![FriendEntry {
            id: 1,
            name: "Test".into(),
            facebook_id: None,
            gamecenter_id: None,
            values: [1, 0, 0, 0],
        }]);
        let mut expected = vec![1];
        expected.extend_from_slice(&1u64.to_be_bytes());
        expected.extend_from_slice(&4u32.to_be_bytes());
        expected.extend_from_slice(b"Test");
        expected.extend_from_slice(&[0xff; 8]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        assert_eq!(list.to_bytes(), expected);
        assert_eq!(FriendList::from_bytes(&expected).unwrap(), list);
        assert_eq!(FriendList::default().to_bytes(), vec![0]);
        expected.pop();
        assert!(FriendList::from_bytes(&expected).is_err());
    }
    #[test]
    fn typed_turns_stop_at_unknown_and_truncated_commands() {
        let mut w = Writer::new();
        w.vint(10).vint(20).vint(8);
        for vals in [
            vec![1, 400000, 400001, 3],
            vec![2, 400001, 4],
            vec![22, 400001, 4],
            vec![23, 400001, 4],
            vec![35, 3600000, 5],
            vec![34, 3600001, 6],
            vec![43, 4900000, 6],
            vec![305, 34, 3600002, 7],
        ] {
            for value in vals {
                w.vint(value);
            }
        }
        let bytes = w.into_bytes();
        let turn = EndClientTurn::parse(&bytes).unwrap();
        assert_eq!(turn.decoded.len(), 7);
        assert_eq!(turn.to_bytes(), bytes);
        assert_eq!(EndClientTurn::completed_tutorial_rows(&bytes), vec![1]);
        assert!(matches!(
            turn.decoded[0],
            TurnCommand::Plant {
                object_id: 400000,
                data_id: 400001,
                tick: 3
            }
        ));
        // An unknown command's apparent tutorial payload must never execute.
        let mut w = Writer::new();
        w.vint(0)
            .vint(0)
            .vint(2)
            .vint(305)
            .vint(34)
            .vint(3600017)
            .vint(1);
        let bytes = w.into_bytes();
        let turn = EndClientTurn::parse(&bytes).unwrap();
        assert!(turn.decoded.is_empty());
        assert_eq!(turn.raw_remainder, bytes[3..]);
        assert_eq!(turn.to_bytes(), bytes);
        assert!(EndClientTurn::completed_tutorial_rows(&bytes).is_empty());
        let truncated = [0, 0, 1, 34, 0x80];
        let turn = EndClientTurn::parse(&truncated).unwrap();
        assert!(turn.decoded.is_empty());
        assert_eq!(turn.raw_remainder, [34, 0x80]);
        assert_eq!(turn.to_bytes(), truncated);
        // Signed command ID/tick values are represented as int32, not quantities.
        let signed = [0, 0, 1, 2, 0x7f, 0x40];
        let turn = EndClientTurn::parse(&signed).unwrap();
        assert_eq!(
            turn.decoded,
            vec![TurnCommand::Harvest {
                object_id: -1,
                tick: -64
            }]
        );
        assert_eq!(turn.to_bytes(), signed);
    }

    #[test]
    fn visit_home_layouts_and_prefix_validation() {
        let blobs = [json!({"a":1}), json!({"text":"été"}), json!({"Objects":[]})];
        let own = OwnHomeData {
            unlocked: BTreeMap::new(),
            avatar_stats: BTreeMap::new(),
            fields_unlocked: 0,
            tutorial_words: [0; 6],
            // Greg's prefix carries an empty (non-null) name.
            farm_name: Some(String::new()),
            unknown_avatar_vector_count: 0,
            blobs: blobs.clone(),
        }
        .to_bytes();
        let ai = AiHomeData {
            blobs: blobs.clone(),
        };
        let mut encoded = ai.to_bytes();
        assert_eq!(encoded, own);
        assert_eq!(AiHomeData::from_bytes(&encoded).unwrap(), ai);
        check_home_layout(
            &encoded,
            &OwnHomeData {
                unlocked: BTreeMap::new(),
                avatar_stats: BTreeMap::new(),
                fields_unlocked: 0,
                tutorial_words: [0; 6],
                farm_name: None,
                unknown_avatar_vector_count: 0,
                blobs: blobs.clone(),
            },
        );
        encoded[395] = 0;
        assert!(
            AiHomeData::from_bytes(&encoded)
                .unwrap_err()
                .to_string()
                .contains("AiHomeData prefix")
        );
        let other = OtherHomeData { blobs };
        let mut encoded = other.to_bytes();
        let mut prefix = [0; 819];
        prefix[803] = 1;
        assert_eq!(&encoded[..819], prefix);
        assert_eq!(&encoded[819..], &own[411..]);
        assert_eq!(OtherHomeData::from_bytes(&encoded).unwrap(), other);
        assert!(OwnHomeData::from_bytes(&encoded).is_err());
        encoded[803] = 0;
        assert!(
            OtherHomeData::from_bytes(&encoded)
                .unwrap_err()
                .to_string()
                .contains("OtherHomeData prefix")
        );
        encoded[803] = 1;
        encoded[0] = 1;
        assert!(OtherHomeData::from_bytes(&encoded).is_err());
    }

    #[test]
    fn captured_turn_headers_and_opaque_payloads() {
        let first = [
            0xb0, 0x04, 0xc1, 0xe6, 0xbb, 0xa8, 0x00, 0x02, 0x22, 0x91, 0xba, 0xb7, 0x03, 0xb5,
            0x01, 0x23, 0x80, 0xba, 0xb7, 0x03, 0xb5, 0x01,
        ];
        let second = [0x8d, 0x0e, 0x93, 0x8d, 0x85, 0xad, 0x01, 0x00];
        let mut shared = Reader::new(&first);
        assert_eq!(shared.vint().unwrap(), 304);
        assert!(shared.vint().is_err());
        let mut shared = Reader::new(&second);
        assert_eq!(shared.vint().unwrap(), 909);
        assert_eq!(shared.vint().unwrap(), 181445459);
        let turn = EndClientTurn::parse(&first).unwrap();
        assert_eq!(
            (turn.a, turn.b, turn.n, turn.first_command_type),
            (304, 2_189_916_545, 2, Some(34))
        );
        assert_eq!(EndClientTurn::completed_tutorial_rows(&first), vec![17]);
        assert_eq!(turn.to_bytes(), first);
        assert_eq!(turn.rest, first[9..]);
        let turn = EndClientTurn::parse(&second).unwrap();
        assert_eq!(
            (turn.a, turn.b, turn.n, turn.first_command_type),
            (909, 181445459, 0, None)
        );
        assert!(turn.rest.is_empty());
        assert_eq!(turn_vint(&mut &[0x7f][..]), Some(u32::MAX));
        assert_eq!(turn_vint(&mut &[0xc0, 0x7f][..]), Some((-64_i32) as u32));
        for malformed in [&[][..], &[0x80][..], &[0, 0, 1][..], &[0xff; 8][..]] {
            assert!(EndClientTurn::parse(malformed).is_none());
        }
        // Unknown command payloads are kept byte-for-byte without decoding them.
        assert_eq!(
            EndClientTurn::parse(&[0, 0, 1, 1, 255, 255]).unwrap().rest,
            [255, 255]
        );
    }

    fn check_home_layout(bytes: &[u8], home: &OwnHomeData) {
        let mut r = Reader::new(bytes);
        let mut prefix = [0; 411];
        prefix[395] = 1;
        prefix[own_home_bitset_offset(4)..own_home_bitset_offset(4) + 4]
            .copy_from_slice(&home.fields_unlocked.to_be_bytes());
        for (index, word) in home.tutorial_words.iter().enumerate() {
            let start = own_home_bitset_offset(36) + index * 4;
            prefix[start..start + 4].copy_from_slice(&word.to_be_bytes());
        }
        assert_eq!(r.raw(411).unwrap(), prefix);
        for expected in &home.blobs {
            let total = r.int().unwrap() as usize;
            assert!(total >= 4);
            let size = u32::from_le_bytes(r.array().unwrap()) as usize;
            let compressed = r.raw(total - 4).unwrap();
            let mut json = Vec::new();
            flate2::read::ZlibDecoder::new(compressed)
                .read_to_end(&mut json)
                .unwrap();
            assert_eq!(size, json.len());
            assert_eq!(serde_json::from_slice::<Value>(&json).unwrap(), *expected);
        }
        r.finish().unwrap();
    }

    #[test]
    fn sparse_stats_variable_count_and_unrelated_entries() {
        let home = OwnHomeData {
            avatar_stats: (0..64).map(|id| (1_900_000 + id, 8192 + id)).collect(),
            ..OwnHomeData::default()
        };
        let bytes = home.to_bytes();
        assert_eq!(
            &bytes[OWN_HOME_STATS_OFFSET..OWN_HOME_STATS_OFFSET + 2],
            &[0x80, 1]
        );
        assert_eq!(OwnHomeData::from_bytes(&bytes).unwrap(), home);
        // Duplicate keys and absurd counts are rejected, rather than silently
        // dropping an existing entry or allocating from an untrusted count.
        for fields in [vec![2, 1, 7, 1, 9], vec![0x80, 0x80, 0x80, 1]] {
            let mut bytes = vec![0; OWN_HOME_STATS_OFFSET];
            bytes.extend(fields);
            assert!(OwnHomeData::from_bytes(&bytes).is_err());
        }
    }

    #[test]
    fn own_home_rejects_nonempty_unknown_avatar_vector() {
        for count in [1, 64, u32::MAX] {
            let home = OwnHomeData {
                unknown_avatar_vector_count: count,
                ..OwnHomeData::default()
            };
            let mut w = Writer::new();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                home.encode(&mut w);
            }));
            assert!(result.is_err());
            assert!(w.into_bytes().is_empty());

            // Malformed input is rejected at the count, before reading elements.
            let mut w = Writer::new();
            w.raw(&OWN_HOME_PREFIX[..OWN_HOME_AVATAR_VECTOR_OFFSET])
                .vint(count);
            assert!(
                OwnHomeData::from_bytes(&w.into_bytes())
                    .unwrap_err()
                    .to_string()
                    .contains("nonempty unknown avatar vector")
            );
        }
    }

    #[test]
    fn own_home_rejects_invalid_prefix_and_extra_or_missing_blobs() {
        let mut bytes = OwnHomeData::default().to_bytes();
        bytes[395] = 0;
        assert!(
            OwnHomeData::from_bytes(&bytes)
                .unwrap_err()
                .to_string()
                .contains("prefix")
        );
        bytes[395] = 1;
        bytes[0] = 1;
        assert!(OwnHomeData::from_bytes(&bytes).is_err());
        bytes[0] = 0;
        for len in [0, 410, 411, 429, 447, bytes.len() - 1] {
            assert!(
                OwnHomeData::from_bytes(&bytes[..len]).is_err(),
                "length {len}"
            );
        }
        bytes.extend_from_slice(&[0; 4]);
        assert!(
            OwnHomeData::from_bytes(&bytes)
                .unwrap_err()
                .to_string()
                .contains("trailing")
        );
    }

    #[test]
    fn json_blob_rejects_bad_lengths_streams_and_json() {
        fn container(json: &[u8], size: u32, truncate: bool, extra: bool) -> Vec<u8> {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(json).unwrap();
            let mut compressed = encoder.finish().unwrap();
            if truncate {
                compressed.pop();
            }
            if extra {
                compressed.push(0);
            }
            let mut w = Writer::new();
            w.int((4 + compressed.len()) as u32)
                .raw(&size.to_le_bytes())
                .raw(&compressed);
            w.into_bytes()
        }
        for bytes in [
            vec![0, 0, 0, 3, 0, 0, 0],
            vec![255; 4],
            container(b"{}", 3, false, false),
            container(b"{}", 1, false, false),
            container(b"{}", (MAX_JSON_BLOB + 1) as u32, false, false),
            container(b"{}", 2, true, false),
            container(b"{}", 2, false, true),
            container(b"{", 1, false, false),
            container(b"\xff", 1, false, false),
        ] {
            assert!(read_json_blob(&mut Reader::new(&bytes)).is_err());
        }
        let mut corrupt = container(b"{}", 2, false, false);
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(read_json_blob(&mut Reader::new(&corrupt)).is_err());
    }

    #[test]
    fn home_minimal_payloads_match_spec() {
        // HOME_PROTOCOL.md: 23549 and 25491 minimal payloads are 00 00 00 00.
        assert_eq!(HomeSync::default().to_bytes(), [0, 0, 0, 0]);
        assert_eq!(EntityBatch::default().to_bytes(), [0, 0, 0, 0]);
        assert_eq!(
            PingReply::default().to_bytes(),
            [0, 0, 0, 0xff, 0xff, 0xff, 0xff]
        );
        // GLM states 9 bytes, but its own field list (VInt + 2xBE32 + VInt) is 10.
        assert_eq!(OptionalIdsReply.to_bytes(), [0; 10]);
        assert_eq!(AccountSummary.to_bytes(), [0; 14]);
        let ping = PingReply {
            value: 5,
            text: Some("x".into()),
        };
        assert_eq!(PingReply::from_bytes(&ping.to_bytes()).unwrap(), ping);
    }

    #[test]
    fn other_device_connected_matches_decoder() {
        let d = OtherDeviceConnected { reason: 1 };
        assert_eq!(d.to_bytes(), [0, 0, 0, 1]);
        assert_eq!(OtherDeviceConnected::from_bytes(&d.to_bytes()).unwrap(), d);
    }

    #[test]
    fn status_notice_layout() {
        let notice = StatusNotice::new(3, "hi");
        let bytes = notice.to_bytes();
        assert_eq!(
            crate::net::connection::hex(&bytes),
            "00000003ffffffff00000002686900000000".to_owned() + "00ffffffff0000"
        );
        assert_eq!(StatusNotice::from_bytes(&bytes).unwrap(), notice);
    }

    #[test]
    fn client_hello_round_trips() {
        let original = ClientHello {
            protocol: 3,
            key_version: 14,
            major: 1,
            minor: 43,
            build: 150,
            fingerprint: "synthetic-test-fingerprint".into(),
            unknown: [2, 2],
        };
        let bytes = original.to_bytes();
        let hello = ClientHello::from_bytes(&bytes).unwrap();
        hello.check_supported().unwrap();
        assert_eq!(hello, original);
    }

    #[test]
    fn login_ok_matches_previously_accepted_bytes() {
        let ok = LoginOk {
            server_time: "T".into(),
            ..LoginOk::development(1, "local-development-account")
        };
        // Byte layout the live client accepted (legacy hand-written encoder).
        let mut legacy = vec![0x01, 0x2b, 0x96, 0x02, 0x00, 0x00];
        legacy.extend(1u64.to_be_bytes());
        legacy.extend(1u64.to_be_bytes());
        for s in [
            "T",
            "2026-11-20T00:00:00Z",
            "local-development-account",
            "",
            "",
            "",
            "",
            "FR",
            "",
        ] {
            legacy.extend((s.len() as u32).to_be_bytes());
            legacy.extend(s.as_bytes());
        }
        legacy.extend([0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(ok.to_bytes(), legacy);
        assert_eq!(LoginOk::from_bytes(&legacy).unwrap(), ok);
        // T0b: trimmed drops the last 6 bytes (field-18 int + 2 bit bytes).
        let trimmed = LoginOk {
            tail: LoginOkTail::Trimmed,
            ..ok.clone()
        };
        assert_eq!(trimmed.to_bytes(), &legacy[..legacy.len() - 6]);
        assert_eq!(LoginOk::from_bytes(&trimmed.to_bytes()).unwrap(), trimmed);
        let boxed = LoginOk {
            tail: LoginOkTail::Boxed,
            ..ok
        };
        let mut expected = legacy[..legacy.len() - 6].to_vec();
        expected.extend([0, 0, 0, 4, 0, 0, 0, 0]);
        assert_eq!(boxed.to_bytes(), expected);
    }
}

#[cfg(test)]
mod farm_name_tests {
    use super::*;

    #[test]
    fn own_home_sends_null_name_by_default() {
        let bytes = OwnHomeData::default().to_bytes();
        assert_eq!(bytes[4..8], [0xff; 4]);
        assert_eq!(OwnHomeData::from_bytes(&bytes).unwrap().farm_name, None);
    }

    #[test]
    fn own_home_name_round_trips_and_shifts_layout() {
        let base = OwnHomeData::default();
        let named = OwnHomeData {
            farm_name: Some("Orchard".into()),
            ..base.clone()
        };
        let bytes = named.to_bytes();
        assert_eq!(bytes[4..8], 7u32.to_be_bytes());
        assert_eq!(&bytes[8..15], b"Orchard");
        assert_eq!(bytes.len(), base.to_bytes().len() + 7);
        assert_eq!(OwnHomeData::from_bytes(&bytes).unwrap(), named);
    }

    #[test]
    fn name_response_codes() {
        assert_eq!(ChangeHomeNameResponse::Ok.to_bytes(), [1]);
        assert_eq!(ChangeHomeNameResponse::Failed.to_bytes(), [0]);
        assert_eq!(ChangeHomeNameResponse::BadWord.to_bytes(), [2]);
    }

    #[test]
    fn name_validation() {
        assert_eq!(
            ChangeHomeName(" Orchard ".into()).validate().as_deref(),
            Some("Orchard")
        );
        assert_eq!(ChangeHomeName("  ".into()).validate(), None);
        assert_eq!(ChangeHomeName("a\nb".into()).validate(), None);
        assert_eq!(
            ChangeHomeName("é".repeat(25))
                .validate()
                .map(|n| n.chars().count()),
            Some(25)
        );
        assert_eq!(ChangeHomeName("x".repeat(26)).validate(), None);
    }
}
