//! Central resolution of the app's data directory (`~/.oryxis` by
//! default).
//!
//! Four sources, the first that answers wins:
//!
//! 1. `ORYXIS_DATA_DIR` names the data tree ITSELF. The app writes it
//!    into every MCP client entry when its own tree is not the default
//!    one, so `oryxis-mcp` (spawned by the client from `<tree>/bin`)
//!    reads the vaults the app shows.
//! 2. `ORYXIS_HOME` overrides the home directory the `.oryxis` tree
//!    lives under. It exists for the harness sandbox on Windows, where
//!    `dirs::home_dir()` is `SHGetKnownFolderPath` (a WinAPI call that
//!    ignores `$HOME` / `%USERPROFILE%`).
//! 3. A `data` entry beside the running executable (portable mode,
//!    issue #271): unzip the portable build, create `data` next to
//!    `oryxis.exe`, and the whole tree lives there. ANY entry by that
//!    name counts, a dangling junction or a file included: the user
//!    asked for that folder, and a tree that cannot be used there is
//!    reported by whatever fails to open in it, never swapped in silence
//!    for the profile's. Decided ONCE per process, so a folder created
//!    while the app runs does not move the tree under it.
//! 4. `.oryxis` under the OS home.
//!
//! The environment wins over the folder: the harness sandbox has to hold
//! whatever sits beside the binary under test. Every source governs only
//! the app's own data tree: user files outside it (`~/.ssh/config`,
//! `~/.aws`, `~/.Xauthority`, the OS download folder) keep resolving
//! against the real OS home, so a portable vault never relocates the
//! user's own configuration.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The variable naming the data tree itself (source 1 above).
pub const DATA_DIR_ENV: &str = "ORYXIS_DATA_DIR";

/// The variable naming the home the `.oryxis` tree lives under
/// (source 2 above).
pub const HOME_ENV: &str = "ORYXIS_HOME";

/// The entry beside the executable that turns portable mode on.
pub const PORTABLE_DIR_NAME: &str = "data";

/// Where the data tree came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirSource {
    /// `ORYXIS_DATA_DIR`.
    DataDirEnv,
    /// `ORYXIS_HOME`.
    HomeEnv,
    /// The `data` folder beside the executable.
    Portable,
    /// `.oryxis` under the OS home: what every other process of this
    /// user finds with nothing set.
    OsHome,
}

impl DataDirSource {
    /// Short name for the diagnostic report.
    pub fn label(self) -> &'static str {
        match self {
            Self::DataDirEnv => DATA_DIR_ENV,
            Self::HomeEnv => HOME_ENV,
            Self::Portable => "portable",
            Self::OsHome => "profile",
        }
    }
}

/// The data tree and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    pub path: PathBuf,
    pub source: DataDirSource,
}

/// The data tree this process uses, `None` when nothing resolves (no
/// override, no portable folder, no OS home).
pub fn data_dir() -> Option<DataDir> {
    resolve_data_dir(
        std::env::var_os(DATA_DIR_ENV),
        std::env::var_os(HOME_ENV),
        portable_dir(),
        dirs::home_dir(),
    )
}

/// The data directory itself (vault, plugin cache, fonts, logs, tray
/// runtime, agent socket). Not created here; each consumer creates what
/// it needs on demand.
pub fn oryxis_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.path)
}

/// The tree a process spawned from somewhere else (an MCP client) has to
/// be TOLD about: `Some` unless the tree is the OS home's `.oryxis`,
/// which every process of this user finds on its own.
pub fn data_dir_for_children() -> Option<PathBuf> {
    data_dir().filter(|d| d.source != DataDirSource::OsHome).map(|d| d.path)
}

/// The portable folder beside the running executable, probed once per
/// process.
fn portable_dir() -> Option<PathBuf> {
    static PORTABLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    PORTABLE
        .get_or_init(|| std::env::current_exe().ok().and_then(|exe| portable_dir_beside(&exe)))
        .clone()
}

/// The `data` entry beside `exe`, when there is one. `symlink_metadata`
/// rather than `is_dir`: a junction to a volume that is not mounted (a
/// VeraCrypt drive) is still the user's choice of tree, and following it
/// would fall back to the profile in silence.
fn portable_dir_beside(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?.join(PORTABLE_DIR_NAME);
    dir.symlink_metadata().is_ok().then_some(dir)
}

/// Pure core of [`data_dir`], split out so the resolution is
/// unit-testable without mutating the process environment (`set_var` is
/// unsafe under Rust 2024, and test binaries run their tests on
/// parallel threads).
fn resolve_data_dir(
    data_env: Option<OsString>,
    home_env: Option<OsString>,
    portable: Option<PathBuf>,
    os_home: Option<PathBuf>,
) -> Option<DataDir> {
    // An empty export must fall through, matching `dirs_sys`' own `HOME`
    // handling; otherwise the data dir would land in the process working
    // directory.
    let set = |v: Option<OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(path) = set(data_env) {
        return Some(DataDir { path, source: DataDirSource::DataDirEnv });
    }
    if let Some(home) = set(home_env) {
        return Some(DataDir { path: home.join(".oryxis"), source: DataDirSource::HomeEnv });
    }
    if let Some(path) = portable {
        return Some(DataDir { path, source: DataDirSource::Portable });
    }
    os_home.map(|home| DataDir { path: home.join(".oryxis"), source: DataDirSource::OsHome })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(path: &str, source: DataDirSource) -> Option<DataDir> {
        Some(DataDir { path: PathBuf::from(path), source })
    }

    /// `ORYXIS_HOME` overrides the data directory's home. The harness
    /// sandbox depends on this on Windows, where `dirs::home_dir()` is
    /// a WinAPI call that ignores `$HOME` / `%USERPROFILE%`: without
    /// the override a harness run would read and write the REAL
    /// profile's `.oryxis`. Exercised through the pure resolver; the
    /// end-to-end pin lives in `oryxis-vault/tests/oryxis_home.rs`, a
    /// binary with exactly one test and therefore no `getenv` race.
    #[test]
    fn oryxis_home_overrides_home_dir() {
        let sandbox = || Some(OsString::from("/sandbox"));
        let home = || Some(PathBuf::from("/real-home"));
        // The override wins over the OS home.
        assert_eq!(
            resolve_data_dir(None, sandbox(), None, home()),
            tree("/sandbox/.oryxis", DataDirSource::HomeEnv)
        );
        // Unset: the OS home.
        assert_eq!(resolve_data_dir(None, None, None, home()), tree("/real-home/.oryxis", DataDirSource::OsHome));
        // An accidental `export ORYXIS_HOME=` falls through to the real
        // home instead of landing the data dir in the working directory.
        assert_eq!(
            resolve_data_dir(None, Some(OsString::new()), None, home()),
            tree("/real-home/.oryxis", DataDirSource::OsHome)
        );
        // Nothing resolves at all: consumers surface their own errors.
        assert_eq!(resolve_data_dir(Some(OsString::new()), Some(OsString::new()), None, None), None);
    }

    /// The portable folder beats the OS home and loses to both variables:
    /// the harness sandbox (`ORYXIS_HOME`) must hold over a `data` folder
    /// beside the binary under test, and `oryxis-mcp`, which lives in
    /// `<tree>/bin`, finds the app's tree only through `ORYXIS_DATA_DIR`.
    #[test]
    fn portable_folder_sits_between_the_variables_and_the_home() {
        let portable = || Some(PathBuf::from("E:/Oryxis/data"));
        let home = || Some(PathBuf::from("C:/Users/me"));
        assert_eq!(
            resolve_data_dir(None, None, portable(), home()),
            tree("E:/Oryxis/data", DataDirSource::Portable)
        );
        // No OS home at all still finds the portable tree.
        assert_eq!(resolve_data_dir(None, None, portable(), None), tree("E:/Oryxis/data", DataDirSource::Portable));
        assert_eq!(
            resolve_data_dir(None, Some(OsString::from("/sandbox")), portable(), home()),
            tree("/sandbox/.oryxis", DataDirSource::HomeEnv)
        );
        // The tree variable names the folder itself, no `.oryxis` added,
        // and wins over everything.
        assert_eq!(
            resolve_data_dir(
                Some(OsString::from("E:/Oryxis/data")),
                Some(OsString::from("/sandbox")),
                Some(PathBuf::from("F:/elsewhere/data")),
                home()
            ),
            tree("E:/Oryxis/data", DataDirSource::DataDirEnv)
        );
        // An empty `ORYXIS_DATA_DIR` is unset, like an empty `ORYXIS_HOME`.
        assert_eq!(
            resolve_data_dir(Some(OsString::new()), None, portable(), home()),
            tree("E:/Oryxis/data", DataDirSource::Portable)
        );
    }

    /// Portable mode is the `data` entry beside the executable, in any
    /// form: a folder, a link whose target is gone (a VeraCrypt volume
    /// not mounted yet), even a file. Each of those is the user asking
    /// for that tree; what fails to open there reports itself, and the
    /// profile's tree is never used in its place.
    #[test]
    fn portable_folder_is_any_data_entry_beside_the_executable() {
        let root = std::env::temp_dir().join(format!("oryxis-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("oryxis.exe");
        std::fs::write(&exe, b"").unwrap();

        // Nothing beside it: not portable.
        assert_eq!(portable_dir_beside(&exe), None);

        // A folder.
        std::fs::create_dir(root.join(PORTABLE_DIR_NAME)).unwrap();
        assert_eq!(portable_dir_beside(&exe), Some(root.join(PORTABLE_DIR_NAME)));
        std::fs::remove_dir(root.join(PORTABLE_DIR_NAME)).unwrap();

        // A file by that name.
        std::fs::write(root.join(PORTABLE_DIR_NAME), b"").unwrap();
        assert_eq!(portable_dir_beside(&exe), Some(root.join(PORTABLE_DIR_NAME)));
        std::fs::remove_file(root.join(PORTABLE_DIR_NAME)).unwrap();

        // A link to a target that is not there.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("unmounted"), root.join(PORTABLE_DIR_NAME)).unwrap();
            assert_eq!(portable_dir_beside(&exe), Some(root.join(PORTABLE_DIR_NAME)));
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The diagnostic report names the source without the path (which
    /// carries the user's name): the variable, or the mode.
    #[test]
    fn source_labels_name_every_source() {
        assert_eq!(DataDirSource::DataDirEnv.label(), "ORYXIS_DATA_DIR");
        assert_eq!(DataDirSource::HomeEnv.label(), "ORYXIS_HOME");
        assert_eq!(DataDirSource::Portable.label(), "portable");
        assert_eq!(DataDirSource::OsHome.label(), "profile");
    }
}
