//! Well-known endpoints of the Oryxis ssh-agent server.
//!
//! Single source of truth shared by the server side
//! (`oryxis-app::agent_server`, which binds these) and the client side
//! (`oryxis-ssh`, which includes them as fallback candidates when
//! authenticating via agent). Both names are part of the product
//! contract: users point `SSH_AUTH_SOCK` / `IdentityAgent` at them, so
//! they are frozen like a wire format.

/// The fixed Windows named pipe the agent server binds. The standard
/// `\\.\pipe\openssh-ssh-agent` name is only ever taken via the opt-in
/// alias setting; this one is always ours while the server runs.
pub const WINDOWS_AGENT_PIPE: &str = r"\\.\pipe\oryxis-ssh-agent";

/// `~/.oryxis/agent.sock`, the fixed Unix socket path the user points
/// `SSH_AUTH_SOCK` at. `None` only when the home directory cannot be
/// resolved.
///
/// A tree outside the profile (a portable `data` folder, or the
/// `ORYXIS_DATA_DIR` an MCP entry carries for one) usually sits on a
/// USB key formatted FAT or exFAT, which cannot hold a Unix socket: the
/// socket goes to the user's runtime folder instead
/// (`$XDG_RUNTIME_DIR/oryxis/agent.sock` on Linux, `$TMPDIR/oryxis/
/// agent.sock` on macOS, both private to the user), and to the tree only
/// when there is none. The listener makes the folder 0700.
pub fn unix_agent_socket_path() -> Option<std::path::PathBuf> {
    let tree = crate::paths::data_dir()?;
    let off_profile = matches!(
        tree.source,
        crate::paths::DataDirSource::Portable | crate::paths::DataDirSource::DataDirEnv
    );
    let runtime = if cfg!(target_os = "macos") { "TMPDIR" } else { "XDG_RUNTIME_DIR" };
    Some(socket_path(&tree.path, off_profile, std::env::var_os(runtime)))
}

/// Pure core of [`unix_agent_socket_path`].
fn socket_path(tree: &std::path::Path, off_profile: bool, runtime: Option<std::ffi::OsString>) -> std::path::PathBuf {
    match runtime.filter(|r| off_profile && !r.is_empty()) {
        Some(dir) => std::path::PathBuf::from(dir).join("oryxis").join("agent.sock"),
        None => tree.join("agent.sock"),
    }
}

#[cfg(test)]
mod tests {
    use super::socket_path;
    use std::path::{Path, PathBuf};

    /// The profile's tree keeps the frozen path; a tree on a USB key puts
    /// the socket in the runtime folder, and in the tree only without one.
    #[test]
    fn a_portable_tree_puts_the_socket_in_the_runtime_folder() {
        let tree = Path::new("/home/u/.oryxis");
        let run = || Some("/run/user/1000".into());
        assert_eq!(socket_path(tree, false, run()), PathBuf::from("/home/u/.oryxis/agent.sock"));
        let usb = Path::new("/media/u/USB/Oryxis/data");
        assert_eq!(socket_path(usb, true, run()), PathBuf::from("/run/user/1000/oryxis/agent.sock"));
        assert_eq!(socket_path(usb, true, None), PathBuf::from("/media/u/USB/Oryxis/data/agent.sock"));
        assert_eq!(socket_path(usb, true, Some("".into())), PathBuf::from("/media/u/USB/Oryxis/data/agent.sock"));
    }
}
