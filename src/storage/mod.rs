//! Account persistence API. Callers share a Store and select a backend at startup.
//! Add a backend by implementing Backend in this module and constructing a Store
//! with it. Session and admin code use the same Store API for every backend.

mod accounts;
mod file;

pub use accounts::{Account, Store};

use crate::game::home::HomeData;
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum BackendKind {
    #[default]
    File,
}

impl BackendKind {
    pub fn open(&self, accounts_file: PathBuf, initial: HomeData) -> Result<Store> {
        match self {
            Self::File => open_file(accounts_file, initial),
        }
    }
}

/// Persistence operations used by Store. Store serializes calls and holds the
/// same lock across related reads and writes within one server process.
trait Backend: Send {
    /// Return account metadata in ascending ID order.
    fn accounts(&self) -> Result<Vec<Account>>;
    fn account(&self, id: u64) -> Result<Option<Account>>;
    fn next_account_id(&self) -> Result<u64>;

    /// Upsert credentials, initializing a home only when one is missing for a new account.
    fn save_account(&mut self, account: &Account, initial: &HomeData) -> Result<()>;
    /// Load and validate the saved home, applying any format migrations in memory.
    fn load_home(&self, id: u64) -> Result<HomeData>;
    fn save_home(&mut self, id: u64, home: &HomeData) -> Result<()>;

    /// Restore the starter home and clear turn history without replacing credentials.
    /// Persist the cooldown and return false when a recent reset makes this a retry.
    fn reset_home(&mut self, id: u64, initial: &HomeData, now_ms: u64) -> Result<bool>;

    fn append_turn(&mut self, id: u64, row: &Value) -> Result<()>;
    /// Return up to 100 most recent turns in chronological log order.
    fn turns(&self, id: u64) -> Result<Value>;
}

pub fn open_file(path: PathBuf, initial: HomeData) -> Result<Store> {
    Store::new(file::FileBackend::open(path)?, initial)
}

#[cfg(test)]
pub(crate) fn test_store(tag: &str) -> (std::sync::Arc<Store>, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hd-store-{tag}-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let store = open_file(dir.join("accounts.json"), HomeData::default()).unwrap();
    (std::sync::Arc::new(store), dir)
}
