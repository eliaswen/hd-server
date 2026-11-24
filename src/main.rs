//! Experimental Hay Day 1.43.150 server: HTTP bootstrap, encrypted login and
//! post-login home delivery, account persistence, and raw command capture.
//! Supported gameplay commands are replayed into persistent account homes.

mod config;
mod data;
mod game;
mod net;
mod protocol;
mod replay;
mod storage;

use tracing::Instrument;

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use tracing::error;

use anyhow::{bail, ensure, Context, Result};
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};
use tokio::net::TcpListener;

use crate::{
    protocol::{catalogue, messages::AiHomeData},
    net::session::{Config, Outgoing},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TutorialMode {
    Completed,
    None,
}

#[derive(Debug, Parser)]
#[command(about = "Experimental Hay Day bootstrap, login and game-message server")]
struct Args {
    /// TOML defaults; explicitly supplied CLI flags override the file.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Directory containing runtime data tables; TOML key: resources_dir.
    #[arg(long, default_value = "resources")]
    resources_dir: PathBuf,
    /// Console verbosity; overrides TOML log_level and RUST_LOG.
    #[arg(long, value_parser = ["off", "error", "warn", "info", "debug", "trace"])]
    log_level: Option<String>,
    /// JSON account index; per-account files live beside it in <id>/.
    #[arg(long, default_value = "data/accounts.json")]
    accounts_file: PathBuf,
    /// Enable the separate admin listener (disabled by default).
    #[arg(long, conflicts_with = "no_admin")]
    admin: bool,
    /// Disable admin even when enabled by the config file.
    #[arg(long, conflicts_with = "admin")]
    no_admin: bool,
    #[arg(long, default_value = "127.0.0.1:8081")]
    admin_listen: String,
    /// Permit an unauthenticated admin listener on a non-loopback IP.
    #[arg(long)]
    admin_insecure: bool,
    #[arg(long, default_value = "127.0.0.1:8080")]
    http_listen: String,
    #[arg(long, default_value = "127.0.0.1:9339")]
    game_listen: String,
    /// Game server hostname/IP (without a port). The client filters local IPv4 ranges.
    #[arg(long)]
    game_host: Option<String>,
    /// Asset base URL; serving game assets is not implemented yet.
    #[arg(long)]
    asset_url: Option<String>,
    /// Override bootstrap JSON; re-read on each request. Explicit --asset-url replaces assetUrlList.
    #[arg(long)]
    env_settings: Option<PathBuf>,
    /// Persistent 32-byte X25519 server secret; created with owner-only permissions.
    #[arg(long, default_value = "dumps/server-key.bin")]
    key_file: PathBuf,
    #[arg(long, default_value = "dumps/server")]
    dump_dir: PathBuf,
    /// Server messages sent after the client's PostLoginReady (10366), as
    /// `TYPE` or `TYPE:HEXBODY`. Repeat or comma-separate; `none` sends nothing.
    /// Default: own-home (26017, verified minimal farm data).
    #[arg(long = "after-ready", value_delimiter = ',')]
    after_ready: Vec<String>,
    /// Send a raw 20103 body after PostLoginReady for client notice testing.
    #[arg(long, value_name = "FILE")]
    test_status_notice: Option<PathBuf>,
    /// A/B/C initializer for new accounts; existing homes persist. Ignored with explicit --after-ready.
    #[arg(long)]
    home_json: Option<PathBuf>,
    /// Explicitly opt existing accounts into supported command replay.
    #[arg(long)]
    replay_commands: bool,
    /// New accounts only: plain cash balance (does not replace FreeCash).
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=i32::MAX as i64))]
    cash: Option<u32>,
    /// New accounts only: total diamond balance (FreeDiamonds is merged into it).
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=i32::MAX as i64))]
    diamonds: Option<u32>,
    /// New accounts only: arbitrary sparse stats, repeat --stat DATA_ID=VALUE.
    #[arg(long = "stat", value_parser = parse_stat)]
    stats: Vec<(u32, u32)>,
    /// Session-only XP within the current level; TOML key: xp. No automatic level-up.
    /// Cannot be combined with explicit --after-ready, including "none" or "own-home".
    #[arg(long, value_name = "XP", value_parser = clap::value_parser!(u32).range(0..=i32::MAX as i64))]
    xp: Option<u32>,
    /// Session-only level (1..=700 for build 1598); TOML key: level.
    /// Omitted XP is preserved. Cannot be combined with explicit --after-ready.
    #[arg(long, value_name = "LEVEL", value_parser = clap::value_parser!(u32).range(1..=700))]
    level: Option<u32>,
    /// Tutorial completion bitmap initialization: completed or none.
    #[arg(long, value_enum, conflicts_with = "tutorial_done")]
    tutorial: Option<TutorialMode>,
    /// Rows of tutorials.csv to mark complete, comma-separated (0..173).
    #[arg(long, value_delimiter = ',', conflicts_with = "tutorial", value_parser = clap::value_parser!(u32).range(0..=173))]
    tutorial_done: Vec<u32>,
    /// Experimental  friend entries JSON; reply to 19838 with this list.
    #[arg(long)]
    friends_file: Option<PathBuf>,
    /// Expose reset/resources development friends; incompatible with --friends-file.
    #[arg(long, conflicts_with = "friends_file")]
    dev_friends: bool,
    /// Delay in milliseconds between 10366 and after-ready messages.
    /// Farm loading was verified with 7000 ms; shorter delays are untested.
    #[arg(long, default_value_t = 7000)]
    after_ready_delay_ms: u64,
    /// Close game connections after this many seconds without client bytes.
    /// Closing after the world is built forces the client into phase 5.
    #[arg(long, default_value_t = 900)]
    idle_secs: u64,
    /// Request-triggered replies `TRIGGER=SPEC[+SPEC...]` (SPEC as in
    /// --after-ready), added to the defaults (19336 → 22151; 13447 → 22151,
    /// 23549). Each reply type is sent at most once per connection.
    #[arg(long = "reply")]
    replies: Vec<String>,
    /// Like --reply, but the reply is sent on every occurrence of TRIGGER.
    #[arg(long = "reply-every")]
    replies_every: Vec<String>,
    /// Send after-ready messages only on the first 10366. By default they
    /// are sent on every 10366, including return home from a visit.
    #[arg(long)]
    after_ready_once: bool,
    /// Do not install the default request-triggered replies.
    #[arg(long)]
    no_default_replies: bool,
    /// LoginOk ending: trimmed (verified; default), legacy or boxed (research
    /// overrides; other tails have caused intermittent client crashes).
    #[arg(long, default_value = "trimmed")]
    login_ok_tail: protocol::messages::LoginOkTail,
    /// Server→client nonce advance per message, comma-separated; the last
    /// value repeats (default 2, matching the client→server direction).
    #[arg(long, value_delimiter = ',', default_value = "2")]
    s2c_nonce_steps: Vec<u16>,
    /// Experiment label recorded in every transcript line.
    #[arg(long, default_value = "default")]
    scenario: String,
}

impl Args {
    fn apply_config(&mut self, matches: &clap::ArgMatches) -> Result<()> {
        let Some(path) = &self.config else {
            return Ok(());
        };
        let file = config::settings::FileConfig::load(path)?;
        let explicit =
            |id| matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine);
        macro_rules! merge {
            ($field:ident, $value:expr) => {
                if !explicit(stringify!($field)) {
                    if let Some(value) = $value {
                        self.$field = value;
                    }
                }
            };
        }
        if !explicit("log_level") {
            self.log_level = file.log_level;
        }
        merge!(resources_dir, file.resources_dir);
        merge!(http_listen, file.http_listen);
        merge!(game_listen, file.game_listen);
        if !explicit("game_host") {
            self.game_host = file.game_host;
        }
        if !explicit("asset_url") {
            self.asset_url = file.asset_url;
        }
        merge!(key_file, file.key_file);
        merge!(dump_dir, file.dump_dir);
        merge!(accounts_file, file.accounts_file);
        merge!(after_ready_delay_ms, file.after_ready_delay_ms);
        merge!(idle_secs, file.idle_secs);
        merge!(admin_listen, file.admin_listen);
        if !explicit("admin") && !explicit("no_admin") {
            self.admin = file.admin_enabled.unwrap_or(false);
        }
        if !explicit("login_ok_tail") {
            if let Some(tail) = file.login_ok_tail {
                self.login_ok_tail = tail.parse().context("config login_ok_tail")?;
            }
        }
        if !explicit("friends_file") {
            self.friends_file = file.friends_file;
        }
        if !explicit("dev_friends") {
            self.dev_friends = file.dev_friends.unwrap_or(false);
        }
        if !explicit("home_json") {
            self.home_json = file.starter_home_file;
        }
        if !explicit("env_settings") {
            self.env_settings = file.env_settings;
        }
        if !explicit("replay_commands") {
            self.replay_commands = file.replay_commands.unwrap_or(false);
        }
        if !explicit("cash") {
            self.cash = file.cash;
        }
        if !explicit("diamonds") {
            self.diamonds = file.diamonds;
        }
        if !explicit("stats") {
            self.stats = file
                .stats
                .unwrap_or_default()
                .iter()
                .map(|s| parse_stat(s))
                .collect::<std::result::Result<_, _>>()
                .map_err(anyhow::Error::msg)?;
        }
        if !explicit("xp") {
            self.xp = file.xp;
        }
        if !explicit("level") {
            self.level = file.level;
        }
        if !explicit("tutorial") {
            if let Some(mode) = file.tutorial {
                self.tutorial = Some(match mode.as_str() {
                    "completed" => TutorialMode::Completed,
                    "none" => TutorialMode::None,
                    other => {
                        return Err(anyhow::anyhow!(
                            "invalid tutorial mode {other:?}; expected completed or none"
                        ))
                    }
                });
            }
        }
        if !explicit("tutorial_done") {
            if let Some(rows) = file.tutorial_done {
                ensure!(
                    rows.iter().all(|&row| row < 174),
                    "tutorial-done rows must be 0..173"
                );
                self.tutorial_done = rows;
            }
        }
        Ok(())
    }

    fn progress(&self) -> Result<game::progression::Overrides> {
        let tutorial_words = if !self.tutorial_done.is_empty() {
            Some(game::progression::tutorial_words(&self.tutorial_done)?)
        } else {
            match self.tutorial {
                Some(TutorialMode::Completed) => {
                    Some(game::progression::tutorial_words(&(0..174).collect::<Vec<_>>())?)
                }
                Some(TutorialMode::None) => Some([0; 6]),
                None => None,
            }
        };
        let progress = game::progression::Overrides {
            level: self.level,
            xp: self.xp,
            tutorial_words,
        };
        progress.check_after_ready(&self.after_ready)?;
        Ok(progress)
    }
}

fn parse_stat(value: &str) -> std::result::Result<(u32, u32), String> {
    let (id, quantity) = value.split_once('=').ok_or("expected DATA_ID=VALUE")?;
    let id = id.parse::<u32>().map_err(|_| "invalid data ID")?;
    let quantity = quantity
        .parse::<u32>()
        .map_err(|_| "invalid stat quantity")?;
    if id > i32::MAX as u32 || quantity > i32::MAX as u32 {
        return Err("stat IDs and quantities must fit nonnegative int32".into());
    }
    Ok((id, quantity))
}

fn initial_home(args: &Args, progress: game::progression::Overrides) -> Result<game::home::HomeData> {
    if !args.after_ready.is_empty() {
        return Ok(game::home::HomeData::default());
    }
    let mut home = match args.home_json.as_deref() {
        Some(path) => game::home::HomeData::load(path)?,
        None => game::starter::new_home()?,
    };
    for &(id, value) in &args.stats {
        home.avatar_stats.insert(id, value);
    }
    for (id, value) in [(1900000, args.cash), (1900001, args.diamonds)] {
        if let Some(value) = value {
            home.avatar_stats.insert(id, value);
        }
    }
    // Persist initial tutorial selection for new accounts; session overrides
    // for existing accounts continue to operate on message copies only.
    if let Some(words) = progress.tutorial_words {
        home.tutorial_words = words;
    }
    game::starter::complete_tutorial_farm(&mut home.c, home.tutorial_words);
    home.validate()?;
    Ok(home)
}

fn load_key(path: &std::path::Path) -> Result<[u8; 32]> {
    match std::fs::File::open(path) {
        Ok(mut file) => {
            let mut key = [0; 32];
            file.read_exact(&mut key)?;
            let mut extra = [0];
            if file.read(&mut extra)? != 0 {
                bail!("server key file must be exactly 32 bytes");
            }
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            use std::os::unix::fs::OpenOptionsExt;
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let key = protocol::crypto::random()?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(&key)?;
            file.sync_all()?;
            Ok(key)
        }
        Err(e) => Err(e.into()),
    }
}

fn after_ready(specs: &[String]) -> Result<Vec<Outgoing>> {
    if specs.is_empty() {
        return Ok(Config::default_after_ready());
    }
    if specs.len() == 1 && specs[0] == "none" {
        return Ok(Vec::new());
    }
    specs
        .iter()
        .map(|spec| {
            let out = Outgoing::parse(spec).with_context(|| format!("--after-ready {spec:?}"))?;
            anyhow::ensure!(!matches!(out.msg_type,29997|20103), "refusing unsafe post-login type {} (29997 desynchronises the client; 20103 forces an error state)",out.msg_type);
            if catalogue::origin(out.msg_type) != Some(catalogue::Origin::Server)
                || !catalogue::is_known(out.msg_type)
            {
                tracing::warn!(
                    msg_type = out.msg_type,
                    "not a server message in the client's factory; the client cannot decode it"
                );
            }
            if out.body.is_empty() && catalogue::classes(out.msg_type).any(|c| c.has_body_decoder())
            {
                tracing::warn!(
                    msg_type = out.msg_type,
                    "empty body for a class with a body decoder; it may fail to decode"
                );
            }
            Ok(out)
        })
        .collect()
}

fn configured_after_ready(
    specs: &[String],
    home_json: Option<&Path>,
    progress: game::progression::Overrides,
) -> Result<Vec<Outgoing>> {
    progress.check_after_ready(specs)?;
    if !specs.is_empty() {
        return after_ready(specs);
    }
    let home = home_json
        .map(|path| game::home::HomeData::load(path).context("--home-json"))
        .transpose()?
        .unwrap_or_default();
    Ok(vec![Outgoing::new(&progress.apply(home.message())?)])
}

/// Greg's AI farm (28383 payload), read from the resources directory when visiting.
const GREG_HOME_FILE: &str = "home-greg.json";

fn load_home_json(path: &Path, flag: &str) -> Result<[serde_json::Value; 3]> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading {flag} {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid {flag} JSON in {}", path.display()))?;
    let blob = |key: &str| -> Result<serde_json::Value> {
        let v = value
            .get(key)
            .with_context(|| format!("{flag} {}: missing key {key:?}", path.display()))?;
        anyhow::ensure!(
            v.is_object(),
            "{flag} {}: key {key:?} must be a JSON object",
            path.display()
        );
        Ok(v.clone())
    };
    Ok([blob("a")?, blob("b")?, blob("c")?])
}

type ReplyRules = Vec<(u16, Vec<Outgoing>)>;

fn configured_replies(args: &Args) -> Result<(ReplyRules, Vec<u16>)> {
    let mut rules = if args.no_default_replies {
        Vec::new()
    } else {
        Config::default_replies()
    };
    let mut repeat = if args.no_default_replies {
        Vec::new()
    } else {
        vec![19838]
    };
    ensure!(
        !args.dev_friends || args.friends_file.is_none(),
        "dev_friends requires disabling friends_file (custom friend list)"
    );
    if args.dev_friends {
        rules.retain(|(kind, _)| *kind != 19838);
        rules.push((
            19838,
            vec![Outgoing::new(&protocol::messages::FriendList(
                game::dev_friends::entries(),
            ))],
        ));
        if !repeat.contains(&19838) {
            repeat.push(19838);
        }
    }
    if let Some(path) = &args.friends_file {
        let entries: Vec<protocol::messages::FriendEntry> = serde_json::from_slice(
            &std::fs::read(path)
                .with_context(|| format!("reading friends file {}", path.display()))?,
        )
        .context("invalid friends JSON")?;
        ensure!(entries.len() <= 10000, "friend list too large");
        ensure!(
            entries
                .iter()
                .all(|e| e.values.iter().all(|&v| v <= i32::MAX as u32)),
            "friend values must be non-negative signed VInts"
        );
        tracing::info!(count=entries.len(),path=%path.display(),"configured experimental friend list; numeric field meanings and friend visits remain unverified");
        rules.retain(|(kind, _)| *kind != 19838);
        rules.push((
            19838,
            vec![Outgoing::new(&protocol::messages::FriendList(entries))],
        ));
        if !repeat.contains(&19838) {
            repeat.push(19838);
        }
    }
    // Greg's AI farm lives beside the other runtime tables, so visits follow --resources-dir.
    let greg_home = args.resources_dir.join(GREG_HOME_FILE);
    if !greg_home
        .try_exists()
        .with_context(|| format!("checking Greg home {}", greg_home.display()))?
    {
        tracing::warn!(path=%greg_home.display(),"Greg home missing; Greg rule disabled");
    } else {
        rules.push((
            13198,
            vec![Outgoing::new(&AiHomeData {
                blobs: load_home_json(&greg_home, "Greg home")?,
            })],
        ));
        repeat.push(13198);
    }
    for (flag, entries, every) in [
        ("--reply", &args.replies, false),
        ("--reply-every", &args.replies_every, true),
    ] {
        for rule in entries {
            let (trigger, specs) = rule
                .split_once('=')
                .with_context(|| format!("{flag} {rule:?}: expected TRIGGER=SPEC"))?;
            let trigger = trigger.parse().with_context(|| format!("{flag} trigger"))?;
            let specs = specs.split('+').map(String::from).collect::<Vec<_>>();
            rules.push((trigger, after_ready(&specs)?));
            if every && !repeat.contains(&trigger) {
                repeat.push(trigger);
            }
        }
    }
    Ok((rules, repeat))
}

fn console_filter(level: Option<&str>, env: Option<&str>) -> Result<tracing_subscriber::EnvFilter> {
    if let Some(level) = level {
        ensure!(
            ["off", "error", "warn", "info", "debug", "trace"].contains(&level),
            "invalid log_level {level:?}; expected off, error, warn, info, debug or trace"
        );
        return tracing_subscriber::EnvFilter::try_new(format!("hd_server={level}"))
            .context("invalid log level");
    }
    Ok(env
        .and_then(|value| tracing_subscriber::EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| "hd_server=info".into()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Args::command().get_matches();
    let mut args = Args::from_arg_matches(&matches)?;
    args.apply_config(&matches)?;

    data::resource_data::configure(args.resources_dir.clone())?;
    let env_log = std::env::var("RUST_LOG").ok();
    tracing_subscriber::fmt()
        .with_env_filter(console_filter(
            args.log_level.as_deref(),
            env_log.as_deref(),
        )?)
        .init();
    // Check if configured resources_dir exists and is a directory
    if !args.resources_dir.is_dir() {
        error!("Could not find resources directory: {}", args.resources_dir.display());
        error!("Please ensure that the resources directory exists and contains the necessary data files.");
        error!("The server cannot run without the required resource files. Exiting...");
        std::process::exit(1);
    }
    let progress = args.progress()?;
    let game_host = args.game_host.clone().context("--game-host is required")?;
    let asset_url = args.asset_url.clone().context("--asset-url is required")?;
    let (replies, repeat_triggers) = configured_replies(&args)?;
    let initial_home = initial_home(&args, progress)?;
    // Validate the starter and every existing account before opening listeners.
    // Overrides are applied to message copies, never to the store's initializer.
    progress.apply(initial_home.message())?;
    let store = Arc::new(storage::accounts::Store::open(
        args.accounts_file.clone(),
        initial_home,
    )?);
    store.validate_progress(progress)?;
    let admin_listener = if args.admin && !args.no_admin {
        Some(TcpListener::bind(net::admin::address(&args.admin_listen, args.admin_insecure)?).await?)
    } else {
        None
    };
    if let Some(listener) = &admin_listener {
        tracing::info!(admin = %listener.local_addr()?, "admin listener enabled");
    }
    tokio::fs::create_dir_all(&args.dump_dir).await?;
    if let Some(path) = &args.env_settings {
        let bytes = tokio::fs::read(path)
            .await
            .context("reading env settings")?;
        let json = serde_json::from_slice::<serde_json::Value>(&bytes)
            .context("invalid env settings JSON")?;
        net::http::Bootstrap::check(&json);
        net::http::Bootstrap::validate(&json)?;
    }
    let secret = load_key(&args.key_file)?;
    let public = x25519_dalek::x25519(secret, x25519_dalek::X25519_BASEPOINT_BYTES);
    tracing::info!(public_key = %net::connection::hex(&public), "server public key; use with hd-patcher --server-public-key");
    let config = Arc::new(Config {
        secret,
        account_id: 0,
        pass_token: String::new(),
        accounts: Some(store.clone()),
        account_home: args.after_ready.is_empty() && args.test_status_notice.is_none(),
        replay_commands: args.replay_commands,
        dev_friends: args.dev_friends,
        progress,
        after_ready: if let Some(path) = &args.test_status_notice {
            vec![Outgoing {
                msg_type: 20103,
                body: std::fs::read(path).with_context(|| format!("reading notice body {}", path.display()))?,
                corrupt: false,
            }]
        } else {
            configured_after_ready(&args.after_ready, args.home_json.as_deref(), progress)?
        },
        after_ready_delay: std::time::Duration::from_millis(args.after_ready_delay_ms),
        login_ok_tail: args.login_ok_tail,
        s2c_nonce_steps: args.s2c_nonce_steps.clone(),
        replies,
        after_ready_every: !args.after_ready_once,
        repeat_triggers,
        idle: std::time::Duration::from_secs(args.idle_secs),
        scenario: args.scenario.clone(),
    });
    let config_summary = serde_json::json!({
        "xp": args.xp,
        "level": args.level,
        "after_ready": config.after_ready.iter().map(|o| serde_json::json!({
            "type": o.msg_type,
            "label": catalogue::label(o.msg_type),
            "body": net::connection::hex(&o.body),
            "corrupt": o.corrupt,
        })).collect::<Vec<_>>(),
        "after_ready_delay_ms": args.after_ready_delay_ms,
        "after_ready_every": config.after_ready_every,
        "repeat_triggers": config.repeat_triggers,
        "login_ok_tail": format!("{:?}", args.login_ok_tail),
        "s2c_nonce_steps": args.s2c_nonce_steps,
        "replies": config.replies.iter().map(|(t, outs)| serde_json::json!({
            "trigger": t,
            "send": outs.iter().map(|o| format!("{}:{}", o.msg_type, net::connection::hex(&o.body))).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "idle_secs": args.idle_secs,
        "env_settings": args.env_settings,
        "game_host": game_host,
        "version": env!("CARGO_PKG_VERSION"),
    });
    tracing::info!(after_ready = ?config.after_ready.iter().map(|o| catalogue::label(o.msg_type)).collect::<Vec<_>>(),
        "post-login messages");
    let bootstrap = Arc::new(net::http::Bootstrap {
        game_host,
        asset_url,
        explicit_asset_url: matches.value_source("asset_url")
            == Some(clap::parser::ValueSource::CommandLine),
        override_path: args.env_settings.clone(),
    });
    let http_listener = TcpListener::bind(&args.http_listen).await?;
    let game = TcpListener::bind(&args.game_listen).await?;
    tracing::info!(http = %http_listener.local_addr()?, game = %game.local_addr()?, "listening");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let mut session = 0u64;

    loop {
        tokio::select! {
            accepted = async { match &admin_listener {Some(listener)=>listener.accept().await,None=>std::future::pending().await} } => {
                let (stream,peer) = accepted?;
                let store = store.clone();
                tokio::spawn(async move { if let Err(error) = net::admin::serve(stream,store).await { tracing::warn!(%peer,%error,"admin connection failed"); } });
            }
            accepted = http_listener.accept() => {
                let (stream, peer) = accepted?;
                let bootstrap = bootstrap.clone();
                tokio::spawn(async move {
                    if let Err(error) = net::http::serve(stream, &bootstrap).await {
                        tracing::warn!(%peer, %error, "HTTP request failed");
                    }
                });
            }
            accepted = game.accept() => {
                let (stream, peer) = accepted?;
                let stem = args.dump_dir.join(format!("{run}-{session}"));
                session += 1;
                let config = config.clone();
                let summary = config_summary.clone();
                let span=tracing::info_span!("game_connection",peer=%peer,connection_id=%stem.display(),scenario=%config.scenario);
                tokio::spawn(async move {
                    tracing::info!(%peer, dump = %stem.display(), scenario = %config.scenario, "game connection accepted");
                    let result = async {
                        let mut dumps = net::connection::Dumps::create(&stem, &config.scenario).await?;
                        dumps.event("start", serde_json::json!({ "peer": peer.to_string(), "config": summary })).await?;
                        net::connection::serve(stream, dumps, &config).await
                    }.await;
                    if let Err(error) = result {
                        tracing::warn!(%peer, error = format!("{error:#}"), "game connection ended");
                    }
                }.instrument(span));
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{messages::OwnHomeData, Message};

    #[test]
    fn log_level_precedence_and_validation() {
        let path = std::env::temp_dir().join(format!("hd-log-config-{}.toml", std::process::id()));
        std::fs::write(&path, "log_level = 'debug'\n").unwrap();
        let matches = Args::command()
            .try_get_matches_from(["hd-server", "--config", path.to_str().unwrap()])
            .unwrap();
        let mut args = Args::from_arg_matches(&matches).unwrap();
        args.apply_config(&matches).unwrap();
        
        assert_eq!(
            console_filter(args.log_level.as_deref(), Some("hd_server=error"))
                .unwrap()
                .to_string(),
            "hd_server=debug"
        );
        let matches = Args::command()
            .try_get_matches_from([
                "hd-server",
                "--config",
                path.to_str().unwrap(),
                "--log-level",
                "info",
            ])
            .unwrap();
        let mut args = Args::from_arg_matches(&matches).unwrap();
        args.apply_config(&matches).unwrap();
        assert_eq!(
            console_filter(args.log_level.as_deref(), Some("hd_server=trace"))
                .unwrap()
                .to_string(),
            "hd_server=info"
        );
        assert_eq!(
            console_filter(None, Some("hd_server::session=trace"))
                .unwrap()
                .to_string(),
            "hd_server::session=trace"
        );
        assert_eq!(
            console_filter(None, None).unwrap().to_string(),
            "hd_server=info"
        );
        assert!(Args::try_parse_from(["hd-server", "--log-level", "verbose"]).is_err());
        assert!(console_filter(Some("verbose"), None).is_err());
        for level in ["off", "error", "warn", "info", "debug", "trace"] {
            assert!(console_filter(Some(level), None).is_ok());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn new_account_defaults_overrides_and_config_precedence() {
        let path =
            std::env::temp_dir().join(format!("hd-starter-config-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "cash = 500\ndiamonds = 20\nstats = ['1600000=12','1900000=7']\n",
        )
        .unwrap();
        let matches = Args::command()
            .try_get_matches_from([
                "hd-server",
                "--config",
                path.to_str().unwrap(),
                "--cash",
                "900",
                "--stat",
                "1600000=15",
                "--tutorial",
                "completed",
            ])
            .unwrap();
        let mut args = Args::from_arg_matches(&matches).unwrap();
        args.apply_config(&matches).unwrap();
        let home = initial_home(&args, args.progress().unwrap()).unwrap();
        assert_eq!(home.avatar_stats[&1900000], 900);
        assert_eq!(home.avatar_stats[&1900001], 20);
        assert_eq!(home.avatar_stats[&1600000], 15);
        assert!(!home.avatar_stats.contains_key(&1900131));
        assert!(home.table_unlocks);
        assert!(!home.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["ID"] == 2100003));
        assert_eq!(
            serde_json::from_value::<game::home::HomeData>(serde_json::to_value(&home).unwrap()).unwrap(),
            home
        );
        for pair in ["-1=0", "1=-1", "2147483648=1", "1=2147483648", "1", "x=2"] {
            assert!(parse_stat(pair).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn config_file_and_explicit_cli_precedence() {
        let path = std::env::temp_dir().join(format!("hd-settings-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"
http_listen = "127.0.0.1:18080"
game_listen = "127.0.0.1:19339"
game_host = "file-host"
asset_url = "http://file/assets/"
key_file = "file-key"
dump_dir = "file-dumps"
login_ok_tail = "trimmed"
home_delay_ms = 7500
idle_secs = 1800
accounts_file = "data/test-accounts.json"
starter_home_file = "home-starter.json"
admin_enabled = true
admin_listen = "127.0.0.1:18081"
level = 5
xp = 30
"#,
        )
        .unwrap();
        let matches = Args::command()
            .try_get_matches_from(["hd-server", "--config", path.to_str().unwrap()])
            .unwrap();
        let mut args = Args::from_arg_matches(&matches).unwrap();
        args.apply_config(&matches).unwrap();
        assert_eq!(args.http_listen, "127.0.0.1:18080");
        assert_eq!((args.level, args.xp), (Some(5), Some(30)));
        args.progress()
            .unwrap()
            .apply(OwnHomeData::default())
            .unwrap();
        assert_eq!(args.game_listen, "127.0.0.1:19339");
        assert_eq!(args.game_host.as_deref(), Some("file-host"));
        assert_eq!(args.after_ready_delay_ms, 7500);
        assert_eq!(args.idle_secs, 1800);
        assert!(args.admin);
        assert_eq!(args.accounts_file, PathBuf::from("data/test-accounts.json"));
        let matches = Args::command()
            .try_get_matches_from([
                "hd-server",
                "--config",
                path.to_str().unwrap(),
                "--http-listen",
                "127.0.0.1:8080",
                "--game-host",
                "cli-host",
                "--after-ready-delay-ms",
                "7000",
                "--idle-secs",
                "900",
                "--accounts-file",
                "data/cli.json",
                "--level",
                "1",
                "--xp",
                "0",
                "--no-admin",
            ])
            .unwrap();
        let mut args = Args::from_arg_matches(&matches).unwrap();
        args.apply_config(&matches).unwrap();
        assert_eq!(args.http_listen, "127.0.0.1:8080");
        assert_eq!(args.game_host.as_deref(), Some("cli-host"));
        assert_eq!((args.level, args.xp), (Some(1), Some(0)));
        args.progress()
            .unwrap()
            .apply(OwnHomeData::default())
            .unwrap();
        assert_eq!(args.after_ready_delay_ms, 7000);
        assert_eq!(args.idle_secs, 900);
        assert!(args.no_admin);
        assert!(!args.admin);
        assert_eq!(args.accounts_file, PathBuf::from("data/cli.json"));
        for (flag, value, expected) in [
            ("--xp", "0", (Some(5), Some(0))),
            ("--level", "6", (Some(6), Some(30))),
        ] {
            let matches = Args::command()
                .try_get_matches_from([
                    "hd-server",
                    "--config",
                    path.to_str().unwrap(),
                    flag,
                    value,
                ])
                .unwrap();
            let mut args = Args::from_arg_matches(&matches).unwrap();
            args.apply_config(&matches).unwrap();
            assert_eq!((args.level, args.xp), expected);
        }
        std::fs::write(&path, "unknown_key = true").unwrap();
        assert!(config::settings::FileConfig::load(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn progress_cli_generates_verified_bytes_and_rejects_invalid_values() {
        for (flags, expected) in [
            (vec!["--xp", "0"], vec![1, 0xa3, 0xf7, 0xe7, 1, 0]),
            (vec!["--xp", "7"], vec![1, 0xa3, 0xf7, 0xe7, 1, 7]),
            (
                vec!["--level", "1", "--xp", "7"],
                vec![2, 0xa2, 0xf7, 0xe7, 1, 1, 0xa3, 0xf7, 0xe7, 1, 7],
            ),
            (
                vec!["--level", "5", "--xp", "30"],
                vec![2, 0xa2, 0xf7, 0xe7, 1, 5, 0xa3, 0xf7, 0xe7, 1, 30],
            ),
            (vec!["--level", "5"], vec![1, 0xa2, 0xf7, 0xe7, 1, 5]),
        ] {
            let args = Args::try_parse_from(std::iter::once("hd-server").chain(flags)).unwrap();
            let messages =
                configured_after_ready(&args.after_ready, None, args.progress().unwrap()).unwrap();
            let original = OwnHomeData::default().to_bytes();
            assert_eq!(&messages[0].body[365..365 + expected.len()], expected);
            assert_eq!(&messages[0].body[365 + expected.len()..], &original[366..]);
        }
        for flags in [
            ["--xp", "-1"],
            ["--xp", "2147483648"],
            ["--xp", "4294967296"],
            ["--xp", "seven"],
            ["--level", "0"],
            ["--level", "701"],
            ["--level", "-1"],
        ] {
            assert!(Args::try_parse_from(std::iter::once("hd-server").chain(flags)).is_err());
        }
        let args = Args::try_parse_from(["hd-server", "--xp", "27"]).unwrap();
        assert!(configured_after_ready(&[], None, args.progress().unwrap())
            .unwrap_err()
            .to_string()
            .contains("below 27"));
    }

    #[test]
    fn starter_home_stats_and_progress_are_merged() {
        let path =
            std::env::temp_dir().join(format!("hd-progress-home-{}.json", std::process::id()));
        let home = game::home::HomeData {
            avatar_stats: std::collections::BTreeMap::from([
                (1_900_000, 100),
                (1_900_002, 5),
                (1_900_003, 30),
            ]),
            ..game::home::HomeData::default()
        };
        let saved = serde_json::to_vec(&home).unwrap();
        std::fs::write(&path, &saved).unwrap();
        let progress = game::progression::Overrides {
            level: None,
            xp: Some(7),
            tutorial_words: None,
        };
        let out = configured_after_ready(&[], Some(&path), progress).unwrap();
        let decoded = OwnHomeData::from_bytes(&out[0].body).unwrap();
        assert_eq!(decoded, progress.apply(home.message()).unwrap());
        assert_eq!(decoded.avatar_stats.len(), 3);
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn progress_rejects_explicit_after_ready_including_config_defaults() {
        for spec in [
            "none",
            "own-home",
            "26017:00",
            "26017@missing-file.bin",
            "ping-reply",
        ] {
            let args =
                Args::try_parse_from(["hd-server", "--xp", "0", "--after-ready", spec]).unwrap();
            assert!(args
                .progress()
                .unwrap_err()
                .to_string()
                .contains("explicit --after-ready"));
            // Conflict is detected before touching a supplied raw file.
            assert!(configured_after_ready(
                &args.after_ready,
                None,
                game::progression::Overrides {
                    level: Some(1),
                    xp: None,
                    tutorial_words: None,
                }
            )
            .unwrap_err()
            .to_string()
            .contains("explicit --after-ready"));
            // Existing raw behavior remains exact without progress overrides.
            if spec == "26017:00" {
                assert_eq!(
                    configured_after_ready(
                        &args.after_ready,
                        None,
                        game::progression::Overrides::default()
                    )
                    .unwrap()[0]
                        .body,
                    [0]
                );
            }
        }
        let path = std::env::temp_dir().join(format!("hd-progress-{}.toml", std::process::id()));
        for content in ["level = 0", "level = 701", "xp = 27", "xp = 2147483648"] {
            std::fs::write(&path, content).unwrap();
            let matches = Args::command()
                .try_get_matches_from(["hd-server", "--config", path.to_str().unwrap()])
                .unwrap();
            let mut args = Args::from_arg_matches(&matches).unwrap();
            args.apply_config(&matches).unwrap();
            assert!(configured_after_ready(&[], None, args.progress().unwrap()).is_err());
        }
        for content in ["xp = -1", "level = -1", "xp = 4294967296"] {
            std::fs::write(&path, content).unwrap();
            assert!(config::settings::FileConfig::load(&path).is_err());
        }
        std::fs::write(&path, "xp = 7").unwrap();
        let matches = Args::command()
            .try_get_matches_from([
                "hd-server",
                "--config",
                path.to_str().unwrap(),
                "--after-ready",
                "26017:00",
            ])
            .unwrap();
        let mut args = Args::from_arg_matches(&matches).unwrap();
        args.apply_config(&matches).unwrap();
        assert!(args.progress().is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_greg_and_unsafe_messages() {
        let dir = std::env::temp_dir().join(format!("hd-no-greg-{}", std::process::id()));
        let args =
            Args::try_parse_from(["hd-server", "--resources-dir", dir.to_str().unwrap()]).unwrap();
        let (rules, repeat) = configured_replies(&args).unwrap();
        assert!(!rules.iter().any(|(trigger, _)| *trigger == 13198));
        assert!(!repeat.contains(&13198));
        for spec in ["29997", "session-ready", "notice:3:unsafe", "20103:0000"] {
            assert!(after_ready(&[spec.into()]).is_err());
        }
    }

    #[test]
    fn default_home_and_delay() {
        let args = Args::try_parse_from([
            "hd-server",
            "--game-host",
            "test",
            "--asset-url",
            "http://test/assets/",
        ])
        .unwrap();
        assert_eq!(args.after_ready_delay_ms, 7000);
        assert!(!args.after_ready_once);
        assert_eq!(
            configured_after_ready(&args.after_ready, None, game::progression::Overrides::default())
                .unwrap(),
            vec![Outgoing::new(&OwnHomeData::default())]
        );
        let minimal = std::env::temp_dir().join(format!("hd-home-minimal-{}.json", std::process::id()));
        std::fs::write(
            &minimal,
            r#"{"a":{},"b":{},"c":{"TileMapWidth":85,"TileMapHeight":71,"ObjectVersion":2,"Objects":[]}}"#,
        )
        .unwrap();
        assert_eq!(
            configured_after_ready(&[], Some(&minimal), game::progression::Overrides::default()).unwrap(),
            Config::default_after_ready()
        );
        std::fs::remove_file(minimal).unwrap();
    }

    #[test]
    fn starter_home_loads_with_six_wheat_fields() {
        let path = std::env::temp_dir().join(format!("hd-home-starter-{}.json", std::process::id()));
        let objects: Vec<_> = [(19, 20), (20, 21), (19, 21), (20, 20), (20, 22), (19, 22)]
            .iter()
            .map(|(x, y)| serde_json::json!({"ID": 400001, "X": x, "Y": y}))
            .collect();
        std::fs::write(
            &path,
            serde_json::json!({
                "a": {},
                "b": {},
                "c": {"ObjectVersion": 2, "TileMapWidth": 85, "TileMapHeight": 71, "Objects": objects}
            })
            .to_string(),
        )
        .unwrap();
        let out =
            configured_after_ready(&[], Some(&path), game::progression::Overrides::default()).unwrap();
        std::fs::remove_file(&path).unwrap();
        let home = OwnHomeData::from_bytes(&out[0].body).unwrap();
        assert_eq!(home.blobs[2]["TileMapWidth"], 85);
        assert_eq!(home.blobs[2]["TileMapHeight"], 71);
        let objects = home.blobs[2]["Objects"].as_array().unwrap();
        assert_eq!(objects.len(), 6);
        for (object, (x, y)) in
            objects
                .iter()
                .zip([(19, 20), (20, 21), (19, 21), (20, 20), (20, 22), (19, 22)])
        {
            assert_eq!(object["ID"], 400001);
            assert_eq!(object["X"], x);
            assert_eq!(object["Y"], y);
        }
    }

    #[test]
    fn development_friends_replace_list_and_reject_custom_configuration() {
        let args = Args::try_parse_from(["hd-server", "--dev-friends"]).unwrap();
        let (rules, repeat) = configured_replies(&args).unwrap();
        let reply = &rules.iter().find(|(kind, _)| *kind == 19838).unwrap().1[0];
        let list = protocol::messages::FriendList::from_bytes(&reply.body).unwrap();
        assert_eq!(list.0, game::dev_friends::entries());
        assert!(repeat.contains(&19838));
        let mut args = args;
        // Also enforce the conflict after TOML merging, not just Clap parsing.
        args.friends_file = Some(PathBuf::from("unused.json"));
        assert!(configured_replies(&args)
            .unwrap_err()
            .to_string()
            .contains("requires disabling friends_file"));
    }
    #[test]
    fn experimental_friend_file_replaces_empty_reply() {
        let path = std::env::temp_dir().join(format!("hd-friends-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"[{"id":1,"name":"Test","facebook_id":null,"gamecenter_id":null,"values":[1,0,0,0]}]"#,
        )
        .unwrap();
        let args =
            Args::try_parse_from(["hd-server", "--friends-file", path.to_str().unwrap()]).unwrap();
        let (rules, repeat) = configured_replies(&args).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(rules.iter().filter(|(kind, _)| *kind == 19838).count(), 1);
        let reply = &rules.iter().find(|(kind, _)| *kind == 19838).unwrap().1[0];
        let list = protocol::messages::FriendList::from_bytes(&reply.body).unwrap();
        assert_eq!(list.0.len(), 1);
        assert_eq!(list.0[0].name, "Test");
        assert!(repeat.contains(&19838));
    }
    #[test]
    fn greg_rule_is_repeated_and_does_not_change_own_home() {
        let dir = std::env::temp_dir().join(format!("hd-greg-rule-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(GREG_HOME_FILE);
        std::fs::write(&path, r#"{"a":{},"b":{},"c":{}}"#).unwrap();
        let args = Args::try_parse_from([
            "hd-server",
            "--game-host",
            "test",
            "--asset-url",
            "http://test/",
            "--resources-dir",
            dir.to_str().unwrap(),
            "--reply-every",
            "16368=ping-reply",
            "--after-ready-once",
        ])
        .unwrap();
        assert!(args.after_ready_once);
        let (rules, repeat) = configured_replies(&args).unwrap();
        assert!(repeat.contains(&19838));
        let friends = &rules.iter().find(|(kind, _)| *kind == 19838).unwrap().1[0];
        assert_eq!(
            (friends.msg_type, friends.body.as_slice()),
            (26656, &[0][..])
        );
        assert!(repeat.contains(&13198));
        assert!(repeat.contains(&16368));
        let ai = &rules.iter().find(|(t, _)| *t == 13198).unwrap().1[0];
        assert_eq!(ai.msg_type, 28383);
        assert_eq!(
            AiHomeData::from_bytes(&ai.body).unwrap().blobs,
            load_home_json(&path, "Greg home").unwrap()
        );
        assert_eq!(
            configured_after_ready(
                &args.after_ready,
                args.home_json.as_deref(),
                game::progression::Overrides::default()
            )
            .unwrap(),
            Config::default_after_ready()
        );
        assert!(rules.iter().any(|(t, _)| *t == 19336));
    }

    #[test]
    fn home_json_validation_and_explicit_override() {
        let path = std::env::temp_dir().join(format!("hd-home-json-{}.json", std::process::id()));
        std::fs::write(&path, br#"{"a":{"x":1},"b":{},"c":{"Objects":[]}}"#).unwrap();
        let out =
            configured_after_ready(&[], Some(&path), game::progression::Overrides::default()).unwrap();
        let home = OwnHomeData::from_bytes(&out[0].body).unwrap();
        assert_eq!(home.blobs[0], serde_json::json!({"x":1}));
        for json in [
            "not json",
            r#"{"a":{},"b":{}}"#,
            r#"{"a":{},"c":{}}"#,
            r#"{"b":{},"c":{}}"#,
            r#"{"a":null,"b":{},"c":{}}"#,
        ] {
            std::fs::write(&path, json).unwrap();
            assert!(
                configured_after_ready(&[], Some(&path), game::progression::Overrides::default())
                    .unwrap_err()
                    .to_string()
                    .contains("--home-json")
            );
        }
        std::fs::remove_file(&path).unwrap();
        assert!(
            configured_after_ready(&[], Some(&path), game::progression::Overrides::default()).is_err()
        );
        // Explicit specs, including own-home and none, never open the file.
        for spec in ["none", "own-home", "ping-reply"] {
            assert_eq!(
                configured_after_ready(
                    &[spec.into()],
                    Some(&path),
                    game::progression::Overrides::default()
                )
                .unwrap(),
                after_ready(&[spec.into()]).unwrap()
            );
        }
    }
}
