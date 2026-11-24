//! Runtime access to the external data tables in `resources/`.
//!
//! The resource files are intentionally not embedded in the executable. They
//! are resolved from the process working directory when first used.

use std::{fs, path::PathBuf, sync::OnceLock};
use tracing::error;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn configure(root: PathBuf) -> anyhow::Result<()> {
    anyhow::ensure!(
        ROOT.set(root).is_ok(),
        "runtime resource directory was already initialized"
    );
    Ok(())
}

pub(crate) fn load(path: &str) -> String {
    let path = ROOT.get_or_init(|| PathBuf::from("resources")).join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        error!("failed to load runtime resource {}: {error}", path.display());
        error!("A file could not be read from the resources directory. Did you copy the required data files into it?");
        error!("The server cannot run without the required resource files. Exiting...");
        std::process::exit(1);
    })
}
