//! Experimental Hay Day 1.43.150 server: HTTP bootstrap, encrypted login and
//! post-login home delivery, account persistence, and raw command capture.
//! Supported gameplay commands are replayed into persistent account homes.
//!
//! The library behind the `hd-server` binary, also used by apps that run the
//! server in-process (on Android, next to the game on the same device):
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! let mut options = hd_server::Options::in_dir("/data/user/0/app/files/server");
//! options.game_host = Some("127.0.0.1".into());
//! options.asset_url = Some("http://game-assets.haydaygame.com/".into());
//! options.no_dump = true;
//! let server = hd_server::Server::start(options).await?;
//! println!("patch the client with {}", server.public_key_hex());
//! server.stop().await?;
//! # Ok(())
//! # }
//! ```
//!
//! The library logs through `tracing` and installs no subscriber; the binary
//! installs one, and an app can install its own to show the log.

mod config;
mod data;
mod game;
mod net;
mod protocol;
mod replay;
mod server;
mod storage;

pub use protocol::messages::LoginOkTail;
pub use server::{Options, Server, TutorialMode, console_filter, public_key};
pub use storage::BackendKind;
