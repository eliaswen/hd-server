//! Account operations shared by all storage backends.
use super::Backend;
use crate::{game::home::HomeData, protocol::messages::EndClientTurn};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: u64,
    pub pass_token: String,
    pub created_at: String,
}

pub struct Store {
    initial: HomeData,
    // Hold the lock across read/modify/write operations, including replay and login.
    backend: Mutex<Box<dyn Backend>>,
}

impl Store {
    pub(super) fn new(backend: impl Backend + 'static, initial: HomeData) -> Result<Self> {
        initial.validate()?;
        for account in backend.accounts()? {
            backend.load_home(account.id)?;
        }
        Ok(Self {
            initial,
            backend: Mutex::new(Box::new(backend)),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Box<dyn Backend>>> {
        self.backend
            .lock()
            .map_err(|_| anyhow::anyhow!("account store lock poisoned"))
    }

    /// Read-only preflight of session overrides against every stored home.
    pub fn validate_progress(&self, progress: crate::game::progression::Overrides) -> Result<()> {
        let backend = self.lock()?;
        for Account { id, .. } in backend.accounts()? {
            let home = backend.load_home(id)?;
            progress
                .apply(home.message())
                .with_context(|| format!("XP/level overrides for account {id}"))?;
        }
        Ok(())
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
        if id == 0 {
            ensure!(token.is_empty(), "Login token without an account ID");
        } else {
            ensure!(!token.is_empty(), "nonzero Login ID requires a token");
        }
        let mut backend = self.lock()?;
        let existing = if id == 0 { None } else { backend.account(id)? };
        if let Some(account) = &existing
            && account.pass_token == token
        {
            tracing::debug!(
                account_id = id,
                "matched Login ID and token; reusing stored account"
            );
            return Ok(account.clone());
        }
        tracing::debug!(
            presented_id = id,
            existing_id = existing.is_some(),
            new_identity = id == 0,
            "selecting local account; nonzero presented credentials are adopted without resetting an existing home"
        );
        let account = if id == 0 {
            Account {
                id: backend.next_account_id()?,
                pass_token: crate::net::connection::hex(&crate::protocol::crypto::random::<32>()?),
                created_at: chrono::Utc::now().to_rfc3339(),
            }
        } else {
            // Explicit local-server recovery: preserve the client's identity.
            // A token rotation never resets an existing account's saved farm.
            Account {
                id,
                pass_token: token,
                created_at: existing
                    .map(|a| a.created_at)
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            }
        };
        backend.save_account(&account, &self.initial)?;
        Ok(account)
    }

    pub fn list(&self) -> Result<Value> {
        Ok(Value::Array(
            self.lock()?
                .accounts()?
                .into_iter()
                .map(|a| json!({"id":a.id,"created_at":a.created_at}))
                .collect(),
        ))
    }

    pub fn contains(&self, id: u64) -> Result<bool> {
        Ok(self.lock()?.account(id)?.is_some())
    }

    pub fn home(&self, id: u64) -> Result<HomeData> {
        let backend = self.lock()?;
        ensure!(backend.account(id)?.is_some(), "account {id} not found");
        backend.load_home(id)
    }

    pub fn replace_home(&self, id: u64, home: HomeData) -> Result<()> {
        home.validate()?;
        let mut backend = self.lock()?;
        ensure!(backend.account(id)?.is_some(), "account {id} not found");
        backend.save_home(id, &home)
    }

    pub fn rename_home(&self, id: u64, name: String) -> Result<()> {
        let mut backend = self.lock()?;
        ensure!(backend.account(id)?.is_some(), "account {id} not found");
        let mut home = backend.load_home(id)?;
        if home.farm_name.as_ref() != Some(&name) {
            home.farm_name = Some(name);
            backend.save_home(id, &home)?;
        }
        Ok(())
    }

    /// Explicitly requested development action, always scoped to the session owner.
    pub fn development_action(
        &self,
        id: u64,
        action: crate::game::dev_friends::Action,
    ) -> Result<()> {
        self.development_action_at(id, action, crate::replay::now_ms())
    }
    fn development_action_at(
        &self,
        id: u64,
        action: crate::game::dev_friends::Action,
        now_ms: u64,
    ) -> Result<()> {
        let mut backend = self.lock()?;
        ensure!(backend.account(id)?.is_some(), "account {id} not found");
        match action {
            crate::game::dev_friends::Action::Reset => {
                if !backend.reset_home(id, &self.initial, now_ms)? {
                    return Ok(());
                }
            }
            crate::game::dev_friends::Action::Resources => {
                let mut home = backend.load_home(id)?;
                for stat in [1900000, 1900001] {
                    let balance = home.avatar_stats.get(&stat).copied().unwrap_or(0);
                    let updated = balance
                        .checked_add(10000)
                        .filter(|&v| v <= i32::MAX as u32)
                        .context("development balance grant exceeds supported VInt range")?;
                    home.avatar_stats.insert(stat, updated);
                }
                home.validate()?;
                backend.save_home(id, &home)?;
            }
            crate::game::dev_friends::Action::SkipTimers => {
                let mut home = backend.load_home(id)?;
                crate::replay::finish_timers(&mut home);
                home.validate()?;
                backend.save_home(id, &home)?;
            }
            crate::game::dev_friends::Action::SkipTutorial => {
                let mut home = backend.load_home(id)?;
                home.tutorial_words =
                    crate::game::progression::tutorial_words(&(0..174).collect::<Vec<_>>())?;
                crate::game::starter::complete_tutorial_farm(&mut home.c, home.tutorial_words);
                home.validate()?;
                backend.save_home(id, &home)?;
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
        let mut backend = self.lock()?;
        ensure!(backend.account(id)?.is_some(), "account {id} not found");
        let turn = EndClientTurn::parse(body);
        let completed = if persist_tutorials {
            EndClientTurn::completed_tutorial_rows(body)
        } else {
            Vec::new()
        };
        let mut replay_notes = Vec::new();
        if !completed.is_empty() || (replay.is_some() && turn.is_some()) {
            let mut home = backend.load_home(id)?;
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
                backend.save_home(id, &home)?;
                tracing::debug!(account_id=id,turn_tick=?turn.as_ref().map(|t|t.a),"account home snapshot saved");
            } else {
                tracing::debug!(account_id=id,turn_tick=?turn.as_ref().map(|t|t.a),"turn produced no saved-state change; home snapshot left untouched");
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
            persist_tutorials,notes=?replay_notes,"recording client turn and replay outcome in account corpus");
        let row = json!({"time":chrono::Utc::now().to_rfc3339(),"a":turn.as_ref().map(|t|t.a),"b":turn.as_ref().map(|t|t.b),"n":turn.as_ref().map(|t|t.n),"first_command_type":turn.as_ref().and_then(|t|t.first_command_type),"commands":turn.as_ref().map(|t|crate::net::connection::hex(&t.commands)),"decoded_commands":turn.as_ref().map(|t| &t.decoded),"raw_remainder":turn.as_ref().map(|t|crate::net::connection::hex(&t.raw_remainder)),"decoded_count":turn.as_ref().map(|t|t.decoded.len()),"body":crate::net::connection::hex(body),"parsed":turn.is_some(),"replay_notes":replay_notes,"replay_active":replay.is_some()});
        backend.append_turn(id, &row)
    }

    pub fn turns(&self, id: u64) -> Result<Value> {
        let backend = self.lock()?;
        ensure!(backend.account(id)?.is_some(), "account {id} not found");
        backend.turns(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{open_file, test_store};
    use std::{
        collections::BTreeMap,
        fs::{self, OpenOptions},
        io::Write,
    };
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
        let store = open_file(dir.join("accounts.json"), HomeData::default()).unwrap();
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
        let before = fs::read(dir.join(id.to_string()).join("home.json")).unwrap();
        let rotated = store.login(&body(id, "replacement-token")).unwrap();
        assert_eq!(rotated.id, id);
        assert_eq!(rotated.created_at, adopted.created_at);
        assert_eq!(
            fs::read(dir.join(id.to_string()).join("home.json")).unwrap(),
            before
        );
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
        let store = open_file(dir.join("accounts.json"), HomeData::default()).unwrap();
        assert_eq!(store.login(&body(id, "replacement-token")).unwrap().id, id);
        assert_eq!(store.home(id).unwrap(), home);
        drop(store);
        fs::remove_dir_all(&dir).unwrap();
        // Simulate loss of every server-side account, keeping client identity.
        let store = open_file(
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
    fn login_recovers_home_left_without_an_index_entry() {
        let (store, dir) = test_store("unindexed-home");
        let account = store.login(&[0; 12]).unwrap();
        let mut home = store.home(account.id).unwrap();
        home.farm_name = Some("Saved farm".into());
        store.replace_home(account.id, home.clone()).unwrap();
        drop(store);

        let index = dir.join("accounts.json");
        fs::remove_file(&index).unwrap();
        let store = open_file(index, HomeData::default()).unwrap();
        let new_account = store.login(&[0; 12]).unwrap();
        assert!(new_account.id > account.id);

        let mut login = crate::protocol::stream::Writer::new();
        login.long(account.id).string(&account.pass_token);
        assert_eq!(store.login(&login.into_bytes()).unwrap().id, account.id);
        assert_eq!(store.home(account.id).unwrap(), home);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn typed_commands_persist_only_verified_effects_and_leave_old_homes_alone() {
        let (store, dir) = test_store("typed-turns");
        let account = store.login(&[0; 12]).unwrap();
        let path = dir.join(account.id.to_string()).join("home.json");
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
        let reopened = open_file(
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
        assert!(open_file(dir.join("accounts.json"), HomeData::default()).is_err());
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
            .open(dir.join(one.id.to_string()).join("turns.jsonl"))
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
        let reopened = open_file(dir.join("accounts.json"), HomeData::default()).unwrap();
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
