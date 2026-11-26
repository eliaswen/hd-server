//! JSON account index, home snapshots, and append-only turn logs.
use super::{Account, Backend};
use crate::game::home::HomeData;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Default, Serialize, Deserialize)]
struct Index {
    accounts: Vec<Account>,
}

pub(super) struct FileBackend {
    // OS advisory lock is released even if the process exits unexpectedly.
    _lock: fs::File,
    path: PathBuf,
    root: PathBuf,
    accounts: BTreeMap<u64, Account>,
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
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

impl FileBackend {
    pub fn open(path: PathBuf) -> Result<Self> {
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
        Ok(Self {
            _lock: lock,
            path,
            root,
            accounts,
        })
    }
    fn home_path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string()).join("home.json")
    }
    fn turns_path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string()).join("turns.jsonl")
    }
}

impl Backend for FileBackend {
    fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.accounts.values().cloned().collect())
    }

    fn account(&self, id: u64) -> Result<Option<Account>> {
        Ok(self.accounts.get(&id).cloned())
    }

    fn next_account_id(&self) -> Result<u64> {
        let mut next = self
            .accounts
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .context("account IDs exhausted")?;
        while self.root.join(next.to_string()).exists() {
            next = next.checked_add(1).context("account IDs exhausted")?;
        }
        Ok(next)
    }

    fn save_account(&mut self, account: &Account, initial: &HomeData) -> Result<()> {
        if !self.accounts.contains_key(&account.id) {
            let path = self.home_path(account.id);
            if path.exists() {
                HomeData::load(&path)?;
            } else {
                atomic_json(&path, initial)?;
            }
        }
        let mut next = self.accounts.clone();
        next.insert(account.id, account.clone());
        atomic_json(
            &self.path,
            &Index {
                accounts: next.values().cloned().collect(),
            },
        )?;
        self.accounts = next;
        Ok(())
    }

    fn load_home(&self, id: u64) -> Result<HomeData> {
        HomeData::load(&self.home_path(id))
    }

    fn save_home(&mut self, id: u64, home: &HomeData) -> Result<()> {
        atomic_json(&self.home_path(id), home)
    }

    fn reset_home(&mut self, id: u64, initial: &HomeData, now_ms: u64) -> Result<bool> {
        // Outside the deleted directory: survives credential adoption,
        // reconnects and server restarts. The account mutex serializes
        // this check with reset/login, including concurrent sessions.
        let marker = self.root.join(".hd-development-resets.json");
        let mut done: BTreeMap<u64, u64> = match fs::read(&marker) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("invalid development reset timestamps")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        if let Some(&reset_done_at_ms) = done.get(&id) {
            if now_ms.saturating_sub(reset_done_at_ms) < 30_000 {
                tracing::info!(
                    account_id = id,
                    reset_done_at_ms,
                    "development reset retry within 30 seconds; keeping current home for resend"
                );
                return Ok(false);
            }
        }
        // Replace per-account state but retain credentials for this session.
        fs::remove_dir_all(self.root.join(id.to_string()))?;
        atomic_json(&self.home_path(id), initial)?;
        done.retain(|_, time| now_ms.saturating_sub(*time) < 30_000);
        done.insert(id, now_ms);
        atomic_json(&marker, &done)?;
        Ok(true)
    }

    fn append_turn(&mut self, id: u64, row: &Value) -> Result<()> {
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

    fn turns(&self, id: u64) -> Result<Value> {
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
