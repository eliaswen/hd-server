//! The `hd-server` binary: parse the command line and settings file, log to
//! the console, and serve until Ctrl-C.

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};
use hd_server::{Options, Server};

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Options::command().get_matches();
    let mut options = Options::from_arg_matches(&matches)?;
    options.apply_config(&matches)?;
    options.explicit_asset_url =
        matches.value_source("asset_url") == Some(clap::parser::ValueSource::CommandLine);

    let env_log = std::env::var("RUST_LOG").ok();
    tracing_subscriber::fmt()
        .with_env_filter(hd_server::console_filter(
            options.log_level.as_deref(),
            env_log.as_deref(),
        )?)
        .init();

    let mut server = match Server::start(options).await {
        Ok(server) => server,
        Err(error) => {
            tracing::error!("{error:#}");
            std::process::exit(1);
        }
    };
    tokio::select! {
        result = server.wait() => result,
        _ = tokio::signal::ctrl_c() => server.stop().await,
    }
}
