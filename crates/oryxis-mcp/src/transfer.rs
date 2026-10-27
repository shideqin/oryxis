//! `sftp_upload`: a file on the machine the server runs on, sent to a
//! host over SFTP, behind a switch the user turns on in Settings > MCP
//! Server (`mcp_server_allow_transfers`, off by default, read per call
//! like the server switch itself).
//!
//! The switch is its own because the tool READS the local disk: a client
//! that has no file access of its own (a chat app) gains it through here,
//! and the bytes leave for a host the agent names. `ssh_execute` already
//! writes on the host; what this adds is the local read, and a file of
//! any size, which a command line cannot carry.
//!
//! The rules, for a caller that is not a person:
//!
//! - The local path is absolute and names a regular file (a link to one
//!   is followed). It is a path on the SERVER's machine; a Windows server
//!   driven from WSL reads `/mnt/<drive>/…` as that drive and any other
//!   absolute path inside the distro named by `ORYXIS_WSL_DISTRO` (the
//!   app writes it into the WSL entry), see [`local_path`].
//! - A remote path ending in `/`, or naming a directory, receives the
//!   file under its own name.
//! - An existing remote file is replaced only with `overwrite: true`, and
//!   keeps its permission bits (its owner becomes the login's, unless it
//!   was written in place). A failure to READ whether it exists (a
//!   permission error) is a failure, never "absent": that reading would
//!   replace a file the call was not allowed to replace.
//! - The bytes go to a scratch name renamed into place at the end, so a
//!   cancel, a stall or a dropped link leaves the real name untouched and
//!   the scratch is removed. A directory that refuses a new file while
//!   the target itself is writable (a group-writable file in a directory
//!   owned by someone else) takes the bytes in place instead, and the
//!   answer says so (`in_place`).
//! - `timeout_secs` bounds a STALL, not the transfer: the call fails when
//!   no byte moved for that long. Progress goes out as
//!   `notifications/progress` when the call carries a token.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::watch;
use uuid::Uuid;

use oryxis_ssh::{SftpClient, SshError, UploadOptions};
use oryxis_vault::VaultStore;

use crate::handlers::{resolve_dial_plan, PlanError};
use crate::pool::cancelled;
use crate::progress::Progress;
use crate::server::Server;

/// The setting that unlocks the tool.
pub const ALLOW_TRANSFERS_SETTING: &str = "mcp_server_allow_transfers";

use oryxis_core::paths::WSL_DISTRO_ENV;

/// Default and ceiling for `timeout_secs`, a stall bound.
pub const DEFAULT_STALL_SECS: u64 = 60;
pub const MAX_STALL_SECS: u64 = 600;

/// How often the transfer is looked at: progress out, stall checked.
const TICK: Duration = Duration::from_millis(500);

/// Refuse unless the user switched transfers on. Read per call so the
/// toggle takes effect on the next request without a restart.
fn gate(vault: &VaultStore) -> Result<(), String> {
    let on = vault
        .get_setting(ALLOW_TRANSFERS_SETTING)
        .ok()
        .flatten()
        .is_some_and(|v| v == "true");
    if on {
        Ok(())
    } else {
        Err("File transfers over MCP are off. Turn on \"Allow the MCP server to send files to hosts\" \
             in Oryxis Settings > MCP Server."
            .to_string())
    }
}

/// The local file `raw` names, as this process opens it. `windows` is
/// whether the server runs on Windows, `wsl_distro` the distro a Linux
/// path belongs to there. Pure, so both platforms' readings are tested
/// on either.
pub fn local_path(raw: &str, windows: bool, wsl_distro: Option<&str>) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("local_path is empty".to_string());
    }
    if windows && raw.starts_with('/') {
        // `/mnt/<drive>` is WSL's mount of a Windows drive.
        let mut parts = raw.trim_start_matches('/').splitn(3, '/');
        let (root, drive) = (parts.next(), parts.next());
        if let (Some("mnt"), Some(drive)) = (root, drive) {
            if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()) {
                let rest = parts.next().unwrap_or("").replace('/', "\\");
                return Ok(PathBuf::from(format!("{}:\\{rest}", drive.to_ascii_uppercase())));
            }
        }
        // Anything else lives inside the distro the client runs in.
        let Some(distro) = wsl_distro.map(str::trim).filter(|d| !d.is_empty()) else {
            return Err(format!(
                "{raw} is a path inside WSL, and this server does not know which distro: \
                 reinstall the Oryxis MCP entry from Oryxis Settings > MCP Server (it names the distro \
                 in {WSL_DISTRO_ENV}), or pass the file as /mnt/<drive>/... or <drive>:\\..."
            ));
        };
        let inside = raw.trim_start_matches('/').replace('/', "\\");
        return Ok(PathBuf::from(format!("\\\\wsl.localhost\\{distro}\\{inside}")));
    }
    let path = PathBuf::from(raw);
    let absolute = if windows {
        // `C:\...` or a UNC path; `\foo` is relative to a drive nobody named.
        raw.starts_with("\\\\") || (raw.len() >= 3 && raw.as_bytes()[1] == b':' && matches!(raw.as_bytes()[2], b'\\' | b'/'))
    } else {
        raw.starts_with('/')
    };
    if !absolute {
        return Err(format!(
            "local_path must be absolute ({raw}): this server's working directory is not the client's"
        ));
    }
    Ok(path)
}

/// Where the file lands: `remote` itself, or `remote/<name>` when it ends
/// in `/` or names a directory (`is_dir`).
fn remote_target(remote: &str, name: &str, is_dir: bool) -> String {
    if remote.ends_with('/') {
        format!("{remote}{name}")
    } else if is_dir {
        format!("{remote}/{name}")
    } else {
        remote.to_string()
    }
}

/// What a remote `stat` said about a path.
enum Remote {
    Absent,
    File { mode: Option<u32> },
    Dir,
}

/// `stat` read for a write decision: "no such file" is absent, any other
/// failure is an error (never a reason to write).
async fn look(client: &SftpClient, path: &str) -> Result<Remote, String> {
    match client.stat(path).await {
        Ok(st) => Ok(match st.permissions {
            Some(p) if p & 0o170000 == 0o040000 => Remote::Dir,
            p => Remote::File { mode: p.map(|p| p & 0o7777) },
        }),
        Err(SshError::Channel(why)) if why.contains(": No such file") => Ok(Remote::Absent),
        Err(e) => Err(format!("could not check {path} on the host: {e}")),
    }
}

/// `mode` as given: octal digits as a string (`"664"`, `"0644"`).
fn mode_param(params: Option<&Value>) -> Result<Option<u32>, String> {
    match params.and_then(|p| p.get("mode")) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let digits = s.trim().trim_start_matches("0o");
            u32::from_str_radix(digits, 8)
                .ok()
                .filter(|m| *m <= 0o7777 && !digits.is_empty())
                .map(Some)
                .ok_or_else(|| format!("mode must be octal permission digits like \"644\", not {s:?}"))
        }
        Some(_) => Err("mode must be a string of octal digits, like \"644\"".to_string()),
    }
}

fn str_param<'a>(params: Option<&'a Value>, name: &str) -> Result<&'a str, String> {
    params
        .and_then(|p| p.get(name))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Missing required parameter: {name}"))
}

pub async fn handle_sftp_upload(
    server: &Server,
    params: Option<&Value>,
    mut cancel: watch::Receiver<bool>,
    progress: &Progress,
) -> Result<Value, String> {
    let id = Uuid::parse_str(str_param(params, "id")?).map_err(|_| "Invalid UUID".to_string())?;
    let raw_local = str_param(params, "local_path")?;
    let remote = str_param(params, "remote_path")?.trim().to_string();
    let overwrite = match params.and_then(|p| p.get("overwrite")) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("overwrite must be a boolean".to_string()),
    };
    let mode = mode_param(params)?;
    let stall = Duration::from_secs(
        params
            .and_then(|p| p.get("timeout_secs"))
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_STALL_SECS)
            .clamp(1, MAX_STALL_SECS),
    );

    let served = match server.holding_host(params, id) {
        Ok(served) => served,
        Err(e) => {
            for v in server.served() {
                v.pool.forget(id);
            }
            return Err(e);
        }
    };
    gate(&served.vault())?;
    let plan = match resolve_dial_plan(&served.vault(), id, Arc::clone(&served.refusals)) {
        Ok(plan) => plan,
        Err(PlanError::NotFound) => {
            served.pool.forget(id);
            return Err(PlanError::NotFound.into());
        }
        Err(e) => return Err(e.into()),
    };

    let wsl_distro = std::env::var(WSL_DISTRO_ENV).ok();
    let local = local_path(raw_local, cfg!(windows), wsl_distro.as_deref())?;
    let meta = std::fs::metadata(&local).map_err(|e| format!("cannot read {}: {e}", local.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", local.display()));
    }
    let size = meta.len();
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} has no file name", local.display()))?;

    let started = Instant::now();
    served.refusals.clear_endpoints(&plan.endpoints);
    let sftp = match served.pool.sftp(&plan, cancel.clone()).await {
        Ok(sftp) => sftp,
        Err(e) => {
            tracing::info!(
                host = %e.label,
                endpoint = %e.endpoint,
                stage = e.stage,
                error = %e.error,
                "sftp_upload failed"
            );
            return Err(crate::handlers::describe_pool_failure(served, &plan.endpoints, &e));
        }
    };
    let client = &sftp.client;

    let first = look(client, &remote).await?;
    let target = remote_target(&remote, &name, matches!(first, Remote::Dir));
    let existing = if target == remote { first } else { look(client, &target).await? };
    let keep_mode = match existing {
        Remote::Absent => None,
        Remote::Dir => return Err(format!("{target} is a directory on the host")),
        Remote::File { mode: old } if overwrite => old,
        Remote::File { .. } => {
            return Err(format!("{target} already exists on the host; pass overwrite: true to replace it"))
        }
    };
    let replacing = !matches!(existing, Remote::Absent);

    // A scratch name needs a new file in the target's directory. Probed
    // with a name of our own, so the answer is the directory's and not a
    // guess from an upload error.
    let dir = target.rsplit_once('/').map(|(d, _)| if d.is_empty() { "/" } else { d }).unwrap_or(".");
    let probe = format!("{}/.oryxis-probe-{}", dir.trim_end_matches('/'), Uuid::new_v4().simple());
    // Only the directory's own refusal leads to the in-place write, the
    // one path that cannot be taken back: a probe that timed out or lost
    // its link says nothing about the directory.
    let in_place = match client.create_file_exclusive(&probe).await {
        Ok(()) => {
            let _ = client.remove_file(&probe).await;
            false
        }
        Err(SshError::Channel(why)) if replacing && why.contains(": Permission denied") => true,
        Err(e) => return Err(format!("cannot create a file in {dir} on the host: {e}")),
    };

    let done = Arc::new(AtomicU64::new(0));
    // Boxed so the stop below DROPS it (closing its remote handle) before
    // the scratch is removed, rather than at the end of the scope.
    let mut upload = Box::pin(client.upload_from_options(
        &local,
        &target,
        UploadOptions {
            progress: Some(Arc::clone(&done)),
            temp_name: !in_place,
            ..UploadOptions::default()
        },
    ));
    let mut ticker = tokio::time::interval(TICK);
    let (mut last, mut moved_at) = (0u64, Instant::now());
    progress.report(0, size, &format!("Uploading {name}"));
    let outcome: Result<(), String> = loop {
        tokio::select! {
            result = &mut upload => break result.map_err(|e| e.to_string()),
            () = cancelled(&mut cancel) => break Err("cancelled".to_string()),
            _ = ticker.tick() => {
                let now = done.load(Ordering::Relaxed);
                if now != last {
                    last = now;
                    moved_at = Instant::now();
                    progress.report(now.min(size), size, &format!("Uploading {name}"));
                } else if moved_at.elapsed() >= stall {
                    break Err(format!("no byte moved for {}s", stall.as_secs()));
                }
            }
        }
    };
    if let Err(why) = outcome {
        // The upload future is dropped here, which stops it; its scratch
        // file is what is left to remove. An in-place write cannot be
        // taken back, and the answer says the target is incomplete.
        drop(upload);
        if in_place {
            return Err(format!("upload of {target} stopped ({why}); the file there is incomplete"));
        }
        client.discard_upload_scratch(&target).await;
        tracing::info!(host = %sftp.label, target = %target, error = %why, "sftp_upload failed");
        return Err(format!("upload of {target} stopped ({why}); nothing was written under that name"));
    }

    // A replaced file keeps its bits: the scratch was created with the
    // server's defaults. Written in place it never lost them, and it may
    // belong to someone else, whose bits this login cannot set.
    let applied = mode.or(keep_mode.filter(|_| !in_place));
    if let Some(bits) = applied {
        if let Err(e) = client.chmod(&target, bits).await {
            return Err(format!("{target} was uploaded, but its mode could not be set to {bits:o}: {e}"));
        }
    }
    progress.report(size, size, &format!("Uploaded {name}"));
    tracing::info!(
        host = %sftp.label,
        endpoint = %sftp.endpoint,
        reused = sftp.reused,
        dial_ms = sftp.dial_ms,
        elapsed_ms = started.elapsed().as_millis(),
        bytes = size,
        target = %target,
        in_place,
        "sftp_upload done"
    );
    let mut answer = json!({
        "remote_path": target,
        "bytes": size,
        "replaced": replacing,
        "vault": served.slug,
    });
    if let Some(bits) = applied {
        answer["mode"] = json!(format!("{bits:o}"));
    }
    if in_place {
        answer["in_place"] = json!(true);
    }
    Ok(answer)
}

/// `true` for the Windows path shapes `local_path` hands back.
#[cfg(test)]
fn is_windows_shape(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with("\\\\") || s.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wsl_mount_path_reads_as_its_drive_on_windows() {
        let p = local_path("/mnt/c/Users/wilso/Downloads/acf.zip", true, None).unwrap();
        assert_eq!(p, PathBuf::from("C:\\Users\\wilso\\Downloads\\acf.zip"));
        assert!(is_windows_shape(&p));
    }

    #[test]
    fn a_distro_path_reads_through_the_named_distro_on_windows() {
        let p = local_path("/home/wilson/acf.zip", true, Some("Ubuntu")).unwrap();
        assert_eq!(p, PathBuf::from("\\\\wsl.localhost\\Ubuntu\\home\\wilson\\acf.zip"));
    }

    #[test]
    fn a_distro_path_without_a_distro_is_refused_with_the_way_out() {
        let e = local_path("/home/wilson/acf.zip", true, None).unwrap_err();
        assert!(e.contains(WSL_DISTRO_ENV) && e.contains("/mnt/"), "{e}");
        // `/mnt` itself, or a longer first segment, is not a drive.
        assert!(local_path("/mnt/data/x", true, None).is_err());
    }

    #[test]
    fn windows_paths_pass_and_relative_ones_do_not() {
        assert!(local_path("C:\\Users\\x\\a.zip", true, None).is_ok());
        assert!(local_path("\\\\server\\share\\a.zip", true, None).is_ok());
        assert!(local_path("a.zip", true, None).is_err());
        assert!(local_path("\\a.zip", true, None).is_err());
    }

    #[test]
    fn unix_paths_are_taken_as_given_and_must_be_absolute() {
        assert_eq!(local_path("/mnt/c/x", false, None).unwrap(), PathBuf::from("/mnt/c/x"));
        assert_eq!(local_path(" /tmp/a ", false, None).unwrap(), PathBuf::from("/tmp/a"));
        assert!(local_path("tmp/a", false, None).is_err());
        assert!(local_path("", false, None).is_err());
    }

    #[test]
    fn a_folder_target_takes_the_file_under_its_own_name() {
        assert_eq!(remote_target("/srv/up/", "a.zip", false), "/srv/up/a.zip");
        assert_eq!(remote_target("/srv/up", "a.zip", true), "/srv/up/a.zip");
        assert_eq!(remote_target("/srv/up/b.zip", "a.zip", false), "/srv/up/b.zip");
    }

    #[test]
    fn mode_is_octal_digits_only() {
        let m = |v: Value| mode_param(Some(&json!({ "mode": v })));
        assert_eq!(m(json!("664")).unwrap(), Some(0o664));
        assert_eq!(m(json!("0644")).unwrap(), Some(0o644));
        assert_eq!(m(json!("0o755")).unwrap(), Some(0o755));
        assert!(m(json!("rw-r--r--")).is_err());
        assert!(m(json!("99")).is_err());
        assert!(m(json!(644)).is_err());
        assert!(m(json!("")).is_err());
    }
}
