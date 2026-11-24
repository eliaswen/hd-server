//! Single-process account store. A mutex serializes index, home and turn I/O.
use crate::{game::home::HomeData, protocol::messages::EndClientTurn};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: u64,
    pub pass_token: String,
    pub created_at: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    accounts: Vec<Account>,
}
pub struct Store {
    // OS advisory lock is released even if the process exits unexpectedly.
    _lock: fs::File,
    path: PathBuf,
    root: PathBuf,
    initial: HomeData,
    accounts: Mutex<BTreeMap<u64, Account>>,
}
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".hd-write-{}-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.with_context(|| format!("atomically writing {}", path.display()))
}
impl Store {
    pub fn open(path: PathBuf, initial: HomeData) -> Result<Self> {
        initial.validate()?;
        let root = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        fs::create_dir_all(&root)?;
        use std::os::unix::fs::OpenOptionsExt;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(root.join(".hd-server.lock"))?;
        lock.try_lock()
            .context("account data directory is already in use by another server")?;
        let index: Index = match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).context("invalid accounts JSON")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Index::default(),
            Err(e) => return Err(e.into()),
        };
        let mut accounts = BTreeMap::new();
        for a in index.accounts {
            ensure!(
                a.id > 0 && !a.pass_token.is_empty(),
                "invalid stored account"
            );
            ensure!(accounts.insert(a.id, a).is_none(), "duplicate account id");
        }
        let store = Self {
            _lock: lock,
            path,
            root,
            initial,
            accounts: Mutex::new(accounts),
        };
        for id in store.lock()?.keys() {
            HomeData::load(&store.home_path(*id))?;
        }
        Ok(store)
    }
    fn lock(&self) -> Result<MutexGuard<'_, BTreeMap<u64, Account>>> {
        self.accounts
            .lock()
            .map_err(|_| anyhow::anyhow!("account store lock poisoned"))
    }
    fn home_path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string()).join("home.json")
    }
    /// Read-only preflight of session overrides against every stored home.
    pub fn validate_progress(&self, progress: crate::game::progression::Overrides) -> Result<()> {
        for id in self.lock()?.keys() {
            let home = HomeData::load(&self.home_path(*id))?;
            progress
                .apply(home.message())
                .with_context(|| format!("XP/level overrides for account {id}"))?;
        }
        Ok(())
    }
    fn turns_path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string()).join("turns.jsonl")
    }
    /// Parse only the established Login prefix. The remaining application body
    /// is unrelated to credentials and must never be searched for tokens.
    pub fn login(&self, body: &[u8]) -> Result<Account> {
        let mut reader = crate::protocol::stream::Reader::new(body);
        let high = reader.int().context("Login ID high")?;
        let low = reader.int().context("Login ID low")?;
        let id = (u64::from(high) << 32) | u64::from(low);
        let token = reader.string().context("Login token")?.unwrap_or_default();
        ensure!(
            token.len() <= 4096,
            "Login token exceeds local credential limit"
        );
        let mut accounts = self.lock()?;
        if id != 0 {
            ensure!(!token.is_empty(), "nonzero Login ID requires a token");
            if let Some(account) = accounts.get(&id) {
                if account.pass_token == token {
                    tracing::debug!(
                        account_id = id,
                        "matched Login ID and token; reusing stored account"
                    );
                    return Ok(account.clone());
                }
            }
        } else {
            ensure!(token.is_empty(), "Login token without an account ID");
        }
        tracing::debug!(presented_id=id,existing_id=accounts.contains_key(&id),new_identity=id==0,
            "selecting local account; nonzero presented credentials are adopted without resetting an existing home");
        let account = if id == 0 {
            let mut next = accounts
                .keys()
                .next_back()
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .context("account IDs exhausted")?;
            while self.root.join(next.to_string()).exists() {
                next = next.checked_add(1).context("account IDs exhausted")?;
            }
            Account {
                id: next,
                pass_token: crate::net::connection::hex(&crate::protocol::crypto::random::<32>()?),
                created_at: chrono::Utc::now().to_rfc3339(),
            }
        } else {
            // Explicit local-server recovery: preserve the client's identity.
            // A token rotation never resets an existing account's saved farm.
            Account {
                id,
                pass_token: token,
                created_at: accounts
                    .get(&id)
                    .map(|a| a.created_at.clone())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            }
        };
        if !accounts.contains_key(&account.id) {
            let path = self.home_path(account.id);
            if path.exists() {
                HomeData::load(&path)?;
            } else {
                atomic_json(&path, &self.initial)?;
            }
        }
        let mut next = accounts.clone();
        next.insert(account.id, account.clone());
        atomic_json(
            &self.path,
            &Index {
                accounts: next.values().cloned().collect(),
            },
        )?;
        *accounts = next;
        Ok(account)
    }
    pub fn list(&self) -> Result<Value> {
        Ok(Value::Array(
            self.lock()?
                .values()
                .map(|a| json!({"id":a.id,"created_at":a.created_at}))
                .collect(),
        ))
    }
    pub fn contains(&self, id: u64) -> Result<bool> {
        Ok(self.lock()?.contains_key(&id))
    }
    pub fn home(&self, id: u64) -> Result<HomeData> {
        let accounts = self.lock()?;
        ensure!(accounts.contains_key(&id), "account {id} not found");
        HomeData::load(&self.home_path(id))
    }
    pub fn replace_home(&self, id: u64, home: HomeData) -> Result<()> {
        home.validate()?;
        let accounts = self.lock()?;
        ensure!(accounts.contains_key(&id), "account {id} not found");
        atomic_json(&self.home_path(id), &home)
    }
    /// Explicitly requested development action, always scoped to the session owner.
    pub fn development_action(&self, id: u64, action: crate::game::dev_friends::Action) -> Result<()> {
        self.development_action_at(id, action, crate::replay::now_ms())
    }
    fn development_action_at(
        &self,
        id: u64,
        action: crate::game::dev_friends::Action,
        now_ms: u64,
    ) -> Result<()> {
        let accounts = self.lock()?;
        match action {
            crate::game::dev_friends::Action::Reset => {
                // Outside the deleted directory: survives credential adoption,
                // reconnects and server restarts. The account mutex serializes
                // this check with reset/login, including concurrent sessions.
                let marker = self.root.join(".hd-development-resets.json");
                let mut done: BTreeMap<u64, u64> = match fs::read(&marker) {
                    Ok(bytes) => serde_json::from_slice(&bytes)
                        .context("invalid development reset timestamps")?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                    Err(e) => return Err(e.into()),
                };
                if let Some(&reset_done_at_ms) = done.get(&id) {
                    if now_ms.saturating_sub(reset_done_at_ms) < 30_000 {
                        tracing::info!(account_id=id,reset_done_at_ms,"development reset retry within 30 seconds; keeping current home for resend");
                        return Ok(());
                    }
                }
                ensure!(accounts.contains_key(&id), "account {id} not found");
                // Replace per-account state but retain credentials for this session.
                fs::remove_dir_all(self.root.join(id.to_string()))?;
                atomic_json(&self.home_path(id), &self.initial)?;
                done.retain(|_, time| now_ms.saturating_sub(*time) < 30_000);
                done.insert(id, now_ms);
                atomic_json(&marker, &done)?;
            }
            crate::game::dev_friends::Action::Resources => {
                ensure!(accounts.contains_key(&id), "account {id} not found");
                let mut home = HomeData::load(&self.home_path(id))?;
                for stat in [1900000, 1900001] {
                    let balance = home.avatar_stats.get(&stat).copied().unwrap_or(0);
                    let updated = balance
                        .checked_add(10000)
                        .filter(|&v| v <= i32::MAX as u32)
                        .context("development balance grant exceeds supported VInt range")?;
                    home.avatar_stats.insert(stat, updated);
                }
                home.validate()?;
                atomic_json(&self.home_path(id), &home)?;
            }
            crate::game::dev_friends::Action::SkipTutorial => {
                ensure!(accounts.contains_key(&id), "account {id} not found");
                let mut home = HomeData::load(&self.home_path(id))?;
                home.tutorial_words = crate::game::progression::tutorial_words(
                    &(0..174).collect::<Vec<_>>(),
                )?;
                crate::game::starter::complete_tutorial_farm(
                    &mut home.c,
                    home.tutorial_words,
                );
                home.validate()?;
                atomic_json(&self.home_path(id), &home)?;
            }
        }
        tracing::info!(
            account_id = id,
            ?action,
            "applied development friend action to signed-in account"
        );
        Ok(())
    }
    #[cfg(test)]
    pub fn capture_turn(&self, id: u64, body: &[u8]) -> Result<()> {
        self.capture_turn_with_replay(id, body, None)
    }
    pub fn capture_turn_with_replay(
        &self,
        id: u64,
        body: &[u8],
        replay: Option<&mut crate::replay::Replay>,
    ) -> Result<()> {
        self.capture_turn_impl(id, body, replay, true)
    }
    pub fn capture_turn_log_only(&self, id: u64, body: &[u8]) -> Result<()> {
        self.capture_turn_impl(id, body, None, false)
    }
    fn capture_turn_impl(
        &self,
        id: u64,
        body: &[u8],
        mut replay: Option<&mut crate::replay::Replay>,
        persist_tutorials: bool,
    ) -> Result<()> {
        let accounts = self.lock()?;
        ensure!(accounts.contains_key(&id), "account {id} not found");
        let turn = EndClientTurn::parse(body);
        let completed = if persist_tutorials {
            EndClientTurn::completed_tutorial_rows(body)
        } else {
            Vec::new()
        };
        let mut replay_notes = Vec::new();
        if !completed.is_empty() || (replay.is_some() && turn.is_some()) {
            let mut home = HomeData::load(&self.home_path(id))?;
            let previous = home.clone();
            let mut candidate = replay.as_deref().cloned();
            if let Some((state, turn)) = candidate.as_mut().zip(turn.as_ref()) {
                replay_notes = state.apply(&mut home, turn, body);
            }
            for row in completed {
                tracing::debug!(
                    account_id = id,
                    tutorial_row = row,
                    data_id = 3600000 + row,
                    "persisting completed tutorial bit; active tutorial step is not stored"
                );
                home.tutorial_words[(row >> 5) as usize] |= 1 << (row & 31);
            }
            home.validate()?;
            if home != previous {
                atomic_json(&self.home_path(id), &home)?;
                tracing::debug!(account_id=id,turn_tick=?turn.as_ref().map(|t|t.a),path=%self.home_path(id).display(),"account home snapshot atomically committed");
            } else {
                tracing::debug!(account_id=id,turn_tick=?turn.as_ref().map(|t|t.a),"turn produced no saved-state change; home file left untouched");
            }
            if let Some((state, next)) = replay.as_mut().zip(candidate) {
                **state = next;
            }
        }
        if !replay_notes.is_empty() && replay_notes != ["duplicate turn ignored"] {
            tracing::warn!(account_id=id,message_type=11292,turn_tick=?turn.as_ref().map(|t|t.a),
                checksum=?turn.as_ref().map(|t|t.b),command_count=?turn.as_ref().map(|t|t.n),
                decoded_commands=?turn.as_ref().map(|t|t.decoded.iter().map(|c|(c.command_type(),c.meaning())).collect::<Vec<_>>()),
                body=%crate::net::connection::hex(body),notes=?replay_notes,
                "command replay has partial or rejected effects; known effects applied where possible; replay continues with later commands and turns");
        }
        tracing::debug!(account_id=id,turn_tick=?turn.as_ref().map(|t|t.a),replay_context=replay.is_some(),
            persist_tutorials,notes=?replay_notes,path=%self.turns_path(id).display(),"recording client turn and replay outcome in account corpus");
        let row = json!({"time":chrono::Utc::now().to_rfc3339(),"a":turn.as_ref().map(|t|t.a),"b":turn.as_ref().map(|t|t.b),"n":turn.as_ref().map(|t|t.n),"first_command_type":turn.as_ref().and_then(|t|t.first_command_type),"commands":turn.as_ref().map(|t|crate::net::connection::hex(&t.commands)),"decoded_commands":turn.as_ref().map(|t| &t.decoded),"raw_remainder":turn.as_ref().map(|t|crate::net::connection::hex(&t.raw_remainder)),"decoded_count":turn.as_ref().map(|t|t.decoded.len()),"body":crate::net::connection::hex(body),"parsed":turn.is_some(),"replay_notes":replay_notes,"replay_active":replay.is_some()});
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .mode(0o600)
            .open(self.turns_path(id))?;
        // Isolate any unterminated record left by an interrupted write so a
        // subsequent valid turn does not get concatenated with the partial JSON.
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                file.write_all(b"\n")?;
            }
        }
        file.write_all(format!("{row}\n").as_bytes())?;
        file.sync_data()?;
        Ok(())
    }
    pub fn turns(&self, id: u64) -> Result<Value> {
        let accounts = self.lock()?;
        ensure!(accounts.contains_key(&id), "account {id} not found");
        let file = match fs::File::open(self.turns_path(id)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!([])),
            Err(e) => return Err(e.into()),
        };
        let mut last = VecDeque::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if last.len() == 100 {
                    last.pop_front();
                }
                last.push_back(v);
            }
        }
        Ok(Value::Array(last.into_iter().collect()))
    }
}
#[cfg(test)]
pub(crate) fn test_store(tag: &str) -> (std::sync::Arc<Store>, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "hd-store-{tag}-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let store = Store::open(dir.join("accounts.json"), HomeData::default()).unwrap();
    (std::sync::Arc::new(store), dir)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn development_reset_cooldown_survives_reconnect_and_store_reopen() {
        let (store, dir) = test_store("reset-cooldown");
        let mut w = crate::protocol::stream::Writer::new();
        w.long(39).string("retained-token");
        let credentials = w.into_bytes();
        store.login(&credentials).unwrap();
        store
            .development_action_at(39, crate::game::dev_friends::Action::Reset, 100_000)
            .unwrap();
        assert!(store.contains(39).unwrap());
        assert_eq!(store.home(39).unwrap(), HomeData::default());
        // A retry on the retained session is harmless.
        store
            .development_action_at(39, crate::game::dev_friends::Action::Reset, 100_001)
            .unwrap();
        store.login(&credentials).unwrap();
        let mut home = store.home(39).unwrap();
        home.avatar_stats.insert(1900000, 1234);
        store.replace_home(39, home.clone()).unwrap();
        drop(store);
        let store = Store::open(dir.join("accounts.json"), HomeData::default()).unwrap();
        store
            .development_action_at(39, crate::game::dev_friends::Action::Reset, 129_999)
            .unwrap();
        assert_eq!(store.home(39).unwrap(), home);
        let done: BTreeMap<u64, u64> =
            serde_json::from_slice(&fs::read(dir.join(".hd-development-resets.json")).unwrap())
                .unwrap();
        assert_eq!(done[&39], 100_000); // Retries do not extend the cooldown.
        store
            .development_action_at(39, crate::game::dev_friends::Action::Reset, 130_000)
            .unwrap();
        assert!(store.contains(39).unwrap());
        assert_eq!(store.home(39).unwrap(), HomeData::default());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn login_prefix_adopts_identity_after_wipe_and_never_scans_suffix() {
        fn body(id: u64, token: &str) -> Vec<u8> {
            let mut w = crate::protocol::stream::Writer::new();
            w.long(id).string(token);
            w.into_bytes()
        }
        let (store, dir) = test_store("adopt-prefix");
        let first = store.login(&body(0, "")).unwrap();
        let mut unrelated = body(0, "");
        unrelated.extend(body(first.id, &first.pass_token));
        let second = store.login(&unrelated).unwrap();
        assert_ne!(first.id, second.id);
        let id = (3u64 << 32) | 38;
        let adopted = store.login(&body(id, "client-retained-token")).unwrap();
        assert_eq!(adopted.id, id);
        assert_eq!(adopted.pass_token, "client-retained-token");
        assert_eq!(
            store.login(&body(id, "client-retained-token")).unwrap().id,
            id
        );
        let mut home = store.home(id).unwrap();
        home.avatar_stats.insert(1900000, 123);
        store.replace_home(id, home.clone()).unwrap();
        let before = fs::read(store.home_path(id)).unwrap();
        let rotated = store.login(&body(id, "replacement-token")).unwrap();
        assert_eq!(rotated.id, id);
        assert_eq!(rotated.created_at, adopted.created_at);
        assert_eq!(fs::read(store.home_path(id)).unwrap(), before);
        // Identity is ID + token: identical tokens on another presented ID do
        // not redirect to whichever account happens to share that token.
        assert_eq!(
            store.login(&body(id + 5, "replacement-token")).unwrap().id,
            id + 5
        );
        assert_eq!(store.login(&body(0, "")).unwrap().id, id + 6);
        let count = store.list().unwrap().as_array().unwrap().len();
        for malformed in [vec![], vec![0; 8], body(id, ""), body(0, "token")] {
            assert!(store.login(&malformed).is_err());
        }
        assert_eq!(store.list().unwrap().as_array().unwrap().len(), count);
        drop(store);
        let store = Store::open(dir.join("accounts.json"), HomeData::default()).unwrap();
        assert_eq!(store.login(&body(id, "replacement-token")).unwrap().id, id);
        assert_eq!(store.home(id).unwrap(), home);
        drop(store);
        fs::remove_dir_all(&dir).unwrap();
        // Simulate loss of every server-side account, keeping client identity.
        let store = Store::open(
            dir.join("accounts.json"),
            crate::game::starter::new_home().unwrap(),
        )
        .unwrap();
        assert_eq!(store.login(&body(id, "replacement-token")).unwrap().id, id);
        assert!(store.home(id).unwrap().command_replay);
        assert_eq!(
            store.home(id).unwrap().c["Objects"]
                .as_array()
                .unwrap()
                .len(),
            516
        );
        assert_eq!(store.list().unwrap().as_array().unwrap().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn typed_commands_persist_only_verified_effects_and_leave_old_homes_alone() {
        let (store, dir) = test_store("typed-turns");
        let account = store.login(&[0; 12]).unwrap();
        let path = store.home_path(account.id);
        let original = fs::read(&path).unwrap();
        let mut w = crate::protocol::stream::Writer::new();
        // Plant has two payload values; then unknown bytes mimicking cmd 34.
        for v in [1, 2, 3, 1, 400000, 400001, 5, 305, 34, 3600017, 6] {
            w.vint(v);
        }
        store.capture_turn(account.id, &w.into_bytes()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        let rows = store.turns(account.id).unwrap();
        assert_eq!(rows[0]["decoded_count"], 1);
        assert_eq!(rows[0]["decoded_commands"][0]["kind"], "plant");
        assert!(!rows[0]["raw_remainder"].as_str().unwrap().is_empty());
        let mut w = crate::protocol::stream::Writer::new();
        for v in [
            1, 2, 3, 1, 400000, 400001, 5, 34, 3600032, 6, 35, 3600033, 7,
        ] {
            w.vint(v);
        }
        store.capture_turn(account.id, &w.into_bytes()).unwrap();
        let saved = store.home(account.id).unwrap();
        assert_eq!(saved.tutorial_words, [0, 1, 0, 0, 0, 0]);
        assert_eq!(saved.c, HomeData::default().c);
        drop(store);
        let reopened = Store::open(
            dir.join("accounts.json"),
            crate::game::starter::new_home().unwrap(),
        )
        .unwrap();
        assert_eq!(reopened.home(account.id).unwrap(), saved);
        assert!(!reopened.home(account.id).unwrap().table_unlocks);
        let new_account = reopened.login(&[0; 12]).unwrap();
        assert!(reopened.home(new_account.id).unwrap().table_unlocks);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persistence_tokens_and_homes() {
        let (store, dir) = test_store("persist");
        assert!(Store::open(dir.join("accounts.json"), HomeData::default()).is_err());
        let one = store.login(&[0; 12]).unwrap();
        let two = store.login(&[0; 12]).unwrap();
        assert_ne!(one.id, two.id);
        let mut login = crate::protocol::stream::Writer::new();
        login.long(one.id).string(&one.pass_token);
        assert_eq!(store.login(&login.into_bytes()).unwrap().id, one.id);
        let mut home = HomeData::default();
        home.c["Objects"] = json!([{"ID":400001,"X":19,"Y":20}]);
        store.replace_home(one.id, home.clone()).unwrap();
        let first = [
            0xb0, 0x04, 0xc1, 0xe6, 0xbb, 0xa8, 0x00, 0x02, 0x22, 0x91, 0xba, 0xb7, 0x03, 0xb5,
            0x01, 0x23, 0x80, 0xba, 0xb7, 0x03, 0xb5, 0x01,
        ];
        store.capture_turn(one.id, &first).unwrap();
        let stored = store.home(one.id).unwrap();
        assert_ne!(stored.tutorial_words, [0; 6]);
        assert_ne!(stored.tutorial_words[0] & (1 << 17), 0);
        home.tutorial_words = stored.tutorial_words;
        for _ in 0..105 {
            store.capture_turn(one.id, &[0, 0, 0]).unwrap();
        }
        store.capture_turn(one.id, &[255]).unwrap();
        OpenOptions::new()
            .append(true)
            .open(store.turns_path(one.id))
            .unwrap()
            .write_all(b"{partial")
            .unwrap();
        store.capture_turn(one.id, &[0, 0, 0]).unwrap();
        assert_eq!(
            store
                .turns(one.id)
                .unwrap()
                .as_array()
                .unwrap()
                .last()
                .unwrap()["body"],
            "000000"
        );
        assert_eq!(store.turns(one.id).unwrap().as_array().unwrap().len(), 100);
        drop(store);
        let reopened = Store::open(dir.join("accounts.json"), HomeData::default()).unwrap();
        let mut body = crate::protocol::stream::Writer::new();
        body.long(one.id).string(&one.pass_token);
        assert_eq!(reopened.login(&body.into_bytes()).unwrap().id, one.id);
        assert_eq!(reopened.home(one.id).unwrap(), home);
        let invalid = HomeData {
            a: json!(null),
            ..HomeData::default()
        };
        assert!(reopened.replace_home(one.id, invalid).is_err());
        assert_eq!(reopened.home(one.id).unwrap(), home);
        assert_eq!(reopened.home(two.id).unwrap(), HomeData::default());
        assert_eq!(reopened.list().unwrap().as_array().unwrap().len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}
