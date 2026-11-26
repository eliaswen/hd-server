//! TOML defaults. Relative paths are resolved from the process working directory,
//! exactly like their CLI counterparts.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub resources_dir: Option<PathBuf>,
    pub log_level: Option<String>,
    pub http_listen: Option<String>,
    pub game_listen: Option<String>,
    pub game_host: Option<String>,
    pub asset_url: Option<String>,
    pub key_file: Option<PathBuf>,
    pub dump_dir: Option<PathBuf>,
    pub login_ok_tail: Option<String>,
    #[serde(alias = "home_delay_ms")]
    pub after_ready_delay_ms: Option<u64>,
    pub idle_secs: Option<u64>,
    #[serde(default)]
    pub storage_backend: crate::storage::BackendKind,
    pub accounts_file: Option<PathBuf>,
    pub friends_file: Option<PathBuf>,
    pub dev_friends: Option<bool>,
    pub starter_home_file: Option<PathBuf>,
    pub admin_listen: Option<String>,
    pub admin_enabled: Option<bool>,
    pub env_settings: Option<PathBuf>,
    pub replay_commands: Option<bool>,
    pub cash: Option<u32>,
    pub diamonds: Option<u32>,
    pub stats: Option<Vec<String>>,
    pub xp: Option<u32>,
    pub level: Option<u32>,
    pub tutorial: Option<String>,
    pub tutorial_done: Option<Vec<u32>>,
}
impl FileConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))
    }
}
