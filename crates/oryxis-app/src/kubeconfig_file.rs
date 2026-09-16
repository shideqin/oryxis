//! Kubeconfig files the app keeps for managed clusters whose provider
//! RETURNS the credential instead of writing `~/.kube/config` (ACK on
//! Alibaba Cloud, TKE on Tencent Cloud; `DiscoveredManagedCluster`).
//!
//! Each cluster gets a file of its own under `~/.oryxis/kubeconfig/`,
//! and the Kubernetes account created for it points at that file
//! (`kubeconfig` in the k8s profile config), which the k8s provider and
//! the local `kubectl exec` path already honour. Merging into the user's
//! `~/.kube/config` was considered and rejected: it needs a YAML round
//! trip the workspace has no dependency for, and a bad merge corrupts
//! the one file every other tool on the machine reads. The cost is that
//! the user's own `kubectl` in a terminal does not see the cluster
//! unless they pass `--kubeconfig`; the file's path is what the account
//! shows.
//!
//! The file holds a credential, so it is written 0600 through a
//! temporary sibling and a rename, and removed with the account that
//! owned it. Everything but the write is pure and tested.

use std::io;
use std::path::{Path, PathBuf};

/// Directory the per-cluster files live in.
pub(crate) fn dir() -> Option<PathBuf> {
    oryxis_core::paths::oryxis_dir().map(|d| d.join("kubeconfig"))
}

/// One normal path component out of a provider-supplied id: ASCII
/// letters, digits, `.`, `_` and `-`, never empty, never `.` / `..`.
/// Cluster ids arrive over the plugin boundary from a remote API, so
/// they are confined before they become part of a path (the same
/// traversal class the plugin cache confines a manifest version to).
pub(crate) fn sanitize_component(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t == "." || t == ".." {
        return None;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    Some(t.to_string())
}

/// The file a cluster's kubeconfig is stored in:
/// `<dir>/<family>-<cluster id>.yaml`. Deterministic, so the discovery
/// view can tell an added cluster from a new one without asking the
/// provider, and a re-add overwrites in place.
pub(crate) fn path_for(family: &str, cluster_id: &str) -> Result<PathBuf, String> {
    let family = sanitize_component(family).ok_or_else(|| "invalid cluster family".to_string())?;
    let id = sanitize_component(cluster_id).ok_or_else(|| "invalid cluster id".to_string())?;
    let dir = dir().ok_or_else(|| "no home directory to store the kubeconfig in".to_string())?;
    Ok(dir.join(format!("{family}-{id}.yaml")))
}

/// True when `path` is one of ours, so deleting the Kubernetes account
/// that owns it may remove the file. Anything the user typed into a k8s
/// account by hand (`~/.kube/config`, a company file) is left alone.
pub(crate) fn is_managed_path(path: &str) -> bool {
    let Some(dir) = dir() else {
        return false;
    };
    let p = Path::new(path);
    p.parent() == Some(dir.as_path()) && p.extension().and_then(|e| e.to_str()) == Some("yaml")
}

/// The `current-context` a kubeconfig names, when it names one. A
/// line scan rather than a YAML parse: the files the cluster APIs hand
/// back are machine-generated block YAML with the key at column 0, and
/// the Kubernetes account works with a blank context anyway (the k8s
/// provider then uses the file's current-context), so a miss costs
/// nothing but the dup-check label.
pub(crate) fn current_context(yaml: &str) -> Option<String> {
    yaml.lines().find_map(|line| {
        let rest = line.strip_prefix("current-context:")?;
        let v = rest.trim().trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// True when every `server:` the kubeconfig names is a private address
/// (RFC 1918, link-local or loopback). Drives the note that such a file
/// only works from inside the cluster's VPC; a cluster whose public
/// endpoint is off gets exactly this shape back from ACK. Hostnames
/// are not resolved, so a private endpoint behind a DNS name reads as
/// public here.
pub(crate) fn servers_are_private(yaml: &str) -> bool {
    let mut seen = false;
    for line in yaml.lines() {
        let Some(rest) = line.trim_start().strip_prefix("server:") else {
            continue;
        };
        seen = true;
        if !is_private_server(rest.trim().trim_matches(|c| c == '"' || c == '\'')) {
            return false;
        }
    }
    seen
}

fn is_private_server(url: &str) -> bool {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or("");
    // Strip a port, minding a bracketed IPv6 literal.
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        Ok(std::net::IpAddr::V6(v6)) => v6.is_loopback() || v6.is_unique_local(),
        Err(_) => false,
    }
}

/// Write `contents` to `path` as a 0600 file: the directory is created,
/// the bytes land in a temporary sibling first and are renamed into
/// place, so a crash mid-write cannot leave a truncated credential
/// where a whole one used to be.
pub(crate) fn write_secret_file(path: &Path, contents: &str) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "kubeconfig path has no parent")
    })?;
    std::fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "kubeconfig path has no name")
    })?;
    let tmp = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        io::Write::write_all(&mut f, contents.as_bytes())?;
        io::Write::flush(&mut f)?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_confines_ids_to_one_path_component() {
        assert_eq!(
            sanitize_component("cls-abc12345").as_deref(),
            Some("cls-abc12345")
        );
        assert_eq!(
            sanitize_component(" c3fb96524f9274b4495df0f12a6b50000 ").as_deref(),
            Some("c3fb96524f9274b4495df0f12a6b50000")
        );
        for bad in [
            "", " ", ".", "..", "../x", "a/b", "a\\b", "a b", "c:d", "n\u{e9}",
        ] {
            assert_eq!(sanitize_component(bad), None, "{bad:?} must be rejected");
        }
    }

    #[test]
    fn path_is_deterministic_and_named_after_family_and_id() {
        let a = path_for("tke", "cls-abc12345").unwrap();
        let b = path_for("tke", "cls-abc12345").unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.file_name().unwrap().to_str().unwrap(),
            "tke-cls-abc12345.yaml"
        );
        assert_eq!(a.parent().unwrap(), dir().unwrap());
        assert!(path_for("ack", "../etc").is_err());
        assert!(path_for("", "x").is_err());
    }

    #[test]
    fn managed_path_is_only_our_directory() {
        let ours = path_for("ack", "c123").unwrap();
        assert!(is_managed_path(ours.to_str().unwrap()));
        // A sibling directory with the same file name is not ours.
        let elsewhere = dir().unwrap().parent().unwrap().join("ack-c123.yaml");
        assert!(!is_managed_path(elsewhere.to_str().unwrap()));
        assert!(!is_managed_path("~/.kube/config"));
        assert!(!is_managed_path(""));
    }

    #[test]
    fn current_context_is_read_off_the_top_level_key() {
        let yaml = "apiVersion: v1\nclusters:\n- cluster:\n    server: https://1.2.3.4:6443\n  name: kubernetes\ncontexts:\n- context:\n    cluster: kubernetes\n    user: \"cls-abc12345-admin\"\n  name: cls-abc12345-context-default\ncurrent-context: cls-abc12345-context-default\nkind: Config\n";
        assert_eq!(
            current_context(yaml).as_deref(),
            Some("cls-abc12345-context-default")
        );
        // Quoted value, and an indented `current-context:` inside a
        // nested map must not be mistaken for the top-level key.
        assert_eq!(
            current_context("kind: Config\ncurrent-context: 'kubernetes-admin-c1'\n").as_deref(),
            Some("kubernetes-admin-c1")
        );
        assert_eq!(
            current_context("preferences:\n  current-context: nope\n"),
            None
        );
        assert_eq!(current_context("current-context: \n"), None);
    }

    #[test]
    fn private_servers_are_recognized() {
        assert!(servers_are_private(
            "clusters:\n- cluster:\n    server: https://10.0.12.3:6443\n"
        ));
        assert!(servers_are_private(
            "clusters:\n- cluster:\n    server: \"https://192.168.1.10\"\n"
        ));
        assert!(!servers_are_private(
            "clusters:\n- cluster:\n    server: https://114.55.1.2:6443\n"
        ));
        // A domain is not resolved, so it reads as public.
        assert!(!servers_are_private(
            "clusters:\n- cluster:\n    server: https://cls-abc.ccs.tencent-cloud.com\n"
        ));
        // Mixed: one public server makes the file usable from outside.
        assert!(!servers_are_private(
            "- cluster:\n    server: https://10.0.0.1:6443\n- cluster:\n    server: https://8.8.8.8:6443\n"
        ));
        // No server at all: nothing to call private.
        assert!(!servers_are_private("kind: Config\n"));
        assert!(is_private_server("https://[fd00::1]:6443"));
        assert!(!is_private_server("https://[2001:db8::1]:6443"));
    }

    #[test]
    fn secret_file_is_written_whole_and_private() {
        let tmp = std::env::temp_dir().join(format!(
            "oryxis-kubeconfig-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = tmp.join("nested").join("ack-c1.yaml");
        write_secret_file(&path, "kind: Config\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kind: Config\n");
        // A second write replaces the content in place.
        write_secret_file(&path, "kind: Config\ncurrent-context: x\n").unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("current-context")
        );
        // No temporary sibling survives.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
