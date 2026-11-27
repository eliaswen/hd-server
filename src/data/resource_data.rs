//! Runtime access to the external data tables in `resources/`.
//!
//! The resource files are intentionally not embedded in the executable. They
//! are resolved from the process working directory when first used.

use std::{fs, path::PathBuf, sync::OnceLock};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Set the resource directory. The tables are cached for the life of the
/// process, so a second call must name the same directory (a server
/// restarted in the same process); a different one is refused.
pub(crate) fn configure(root: PathBuf) -> anyhow::Result<()> {
    let current = ROOT.get_or_init(|| root.clone());
    anyhow::ensure!(
        *current == root,
        "runtime resources were already loaded from {}; restart the process to use {}",
        current.display(),
        root.display()
    );
    Ok(())
}

pub(crate) fn load(path: &str) -> String {
    let path = ROOT.get_or_init(|| PathBuf::from("resources")).join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        // Server::start reads every table before serving, so this is reached
        // only if a file disappears while the server runs. Panicking ends the
        // one session instead of the whole process (or app).
        panic!(
            "failed to load runtime resource {}: {error}; regenerate the resources directory with hd-dumper",
            path.display()
        )
    })
}
