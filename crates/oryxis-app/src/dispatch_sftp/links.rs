//! Symbolic links in the dual-pane browser (issue #272): opening a link
//! row, which first asks the server where it points, and the "New
//! symbolic link" dialog. Called from `handle_sftp`.
//!
//! A listing reports a link as a link (the server answers `readdir` with
//! `lstat` attributes), so whether it leads to a folder is not known
//! until the row is opened. Opening it `stat`s the link (which follows
//! it) and acts on the answer: a folder is entered, a file gets the file
//! double-click action, and a link the server cannot follow says where it
//! points instead of failing as a folder listing.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, Oryxis, SftpMessage};
use crate::sftp_helpers::file_basename;
use crate::state::SftpLinkIntent;
use crate::views::sftp::NEW_LINK_NAME_ID;

/// File type bits of a POSIX mode, and the directory type.
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;

impl Oryxis {
    pub(super) fn handle_sftp_links(
        &mut self,
        message: SftpMessage,
    ) -> Result<Task<Message>, SftpMessage> {
        match message {
            SftpMessage::SftpOpenLink(side, path, intent) => {
                let pane = self.cur_sftp().pane(side);
                let Some(client) = pane.client.clone() else {
                    return Ok(Task::none());
                };
                // The answer is applied only while the pane still shows
                // the listing it was asked from: any navigation or
                // refresh since stamps a new seq, and a buffer swapped in
                // by another surface carries its own.
                let seq = pane.remote_list_seq;
                let name = file_basename(&path, true);
                let unreachable = crate::i18n::t("sftp_link_unreachable");
                let target_path = path.clone();
                return Ok(Task::perform(
                    async move {
                        match client.stat(&target_path).await {
                            // A server that sends no mode leaves the
                            // question open; going in is what the row
                            // has always offered, and the listing says
                            // the rest.
                            Ok(st) => Ok(st.permissions.is_none_or(|m| m & S_IFMT == S_IFDIR)),
                            Err(e) => Err(match client.read_link(&target_path).await {
                                Ok(target) => unreachable
                                    .replacen("{name}", &name, 1)
                                    .replacen("{target}", &target, 1)
                                    .replacen("{error}", &e.to_string(), 1),
                                Err(_) => e.to_string(),
                            }),
                        }
                    },
                    move |result| Message::Sftp(SftpMessage::SftpLinkResolved(side, seq, path.clone(), intent, result)),
                ));
            }
            SftpMessage::SftpLinkResolved(side, seq, path, intent, result) => {
                let pane = self.cur_sftp().pane(side);
                if !pane.is_remote || pane.remote_list_seq != seq {
                    return Ok(Task::none());
                }
                let is_dir = match result {
                    Ok(is_dir) => is_dir,
                    Err(msg) => {
                        self.push_sftp_log(
                            crate::state::SftpLogLevel::Error,
                            format!("{} {}", crate::i18n::t("sftp_log_error"), msg),
                        );
                        // The listing is still valid: the message rides
                        // the banner above it.
                        self.sftp.pane_mut(side).notice = Some(msg);
                        return Ok(Task::none());
                    }
                };
                return Ok(match intent {
                    SftpLinkIntent::Descend { focus_parent } if is_dir => {
                        self.sftp.selected_rows.clear();
                        self.sftp.selection_anchor = None;
                        self.sftp.parent_cursor = false;
                        if focus_parent {
                            self.sftp.pending_focus = Some((side, crate::state::SftpPendingFocus::Parent));
                        }
                        Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(side, path)))
                    }
                    // A link to a file has nothing to descend into, the
                    // same answer a file row gives.
                    SftpLinkIntent::Descend { .. } => Task::none(),
                    SftpLinkIntent::Activate { from_keyboard } => {
                        self.sftp_activate_known(side, path, is_dir, from_keyboard, Some(is_dir))
                    }
                });
            }
            SftpMessage::SftpStartNewLink(side, target) => {
                // A browsed archive is read-only.
                if self.cur_sftp().pane(side).zip.is_some() {
                    return Ok(Task::none());
                }
                self.sftp.close_menus();
                self.sftp.new_entry = None;
                self.sftp.new_link = Some(crate::state::SftpNewLink {
                    side,
                    name: String::new(),
                    target: target.unwrap_or_default(),
                });
                return Ok(crate::widgets::focus_input(iced::widget::Id::new(NEW_LINK_NAME_ID)));
            }
            SftpMessage::SftpNewLinkName(s) => {
                if let Some(link) = self.sftp.new_link.as_mut() {
                    link.name = s;
                }
            }
            SftpMessage::SftpNewLinkTarget(s) => {
                if let Some(link) = self.sftp.new_link.as_mut() {
                    link.target = s;
                }
            }
            SftpMessage::SftpNewLinkCommit => {
                let Some(link) = self.cur_sftp().new_link.clone() else {
                    return Ok(Task::none());
                };
                // The Create button is off until both fields are usable;
                // an Enter that arrives anyway keeps the dialog open.
                let Some((name, target)) = link.fields() else {
                    return Ok(Task::none());
                };
                self.sftp.new_link = None;
                let side = link.side;
                let exists_msg = crate::i18n::t("files_entry_exists").replacen("{name}", &name, 1);
                if !self.cur_sftp().pane(side).is_remote {
                    let at = self.cur_sftp().pane(side).local_path.join(&name);
                    // `symlink_metadata`: a link whose target is missing
                    // still occupies the name.
                    let result = if at.symlink_metadata().is_ok() {
                        Err(exists_msg)
                    } else {
                        make_local_symlink(&target, &at)
                    };
                    if let Err(e) = result {
                        self.sftp.pane_mut(side).notice = Some(e);
                    }
                    self.refresh_sftp_local(side);
                    return Ok(Task::none());
                }
                let Some(client) = self.cur_sftp().pane(side).client.clone() else {
                    return Ok(Task::none());
                };
                let reload_path = self.cur_sftp().pane(side).remote_path.clone();
                let parent = reload_path.trim_end_matches('/');
                let at = if parent.is_empty() { format!("/{name}") } else { format!("{parent}/{name}") };
                return Ok(Task::perform(
                    async move {
                        // `lstat`, not `stat`: a dangling link is still
                        // there, and `stat` would call the name free.
                        // The server refuses an existing name anyway;
                        // this turns its opaque failure into words.
                        if client.lstat(&at).await.is_ok() {
                            return Err(exists_msg);
                        }
                        client.symlink(&target, &at).await.map_err(|e| e.to_string())
                    },
                    move |result| match result {
                        Ok(()) => Message::Sftp(SftpMessage::SftpNavigateRemote(side, reload_path.clone())),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpNewLinkCancel => {
                self.sftp.new_link = None;
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }
}

/// Make a symbolic link on this machine at `at`, pointing at `target` as
/// written.
#[cfg(unix)]
fn make_local_symlink(target: &str, at: &std::path::Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, at).map_err(|e| e.to_string())
}

/// Make a symbolic link on this machine at `at`, pointing at `target` as
/// written.
///
/// Windows has two kinds, and the folder kind is the only one a folder
/// opens through, so the target decides: resolved against the link's own
/// folder when relative, a folder link when it is a folder now, a file
/// link otherwise (a target that does not exist yet included, as
/// `mklink` does). Making either takes Developer Mode or an elevated
/// process, which is said in words when it is missing.
#[cfg(windows)]
fn make_local_symlink(target: &str, at: &std::path::Path) -> Result<(), String> {
    /// `ERROR_PRIVILEGE_NOT_HELD`.
    const PRIVILEGE_NOT_HELD: i32 = 1314;
    let resolved = match at.parent() {
        Some(dir) => dir.join(target),
        None => std::path::PathBuf::from(target),
    };
    let result = if std::fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(target, at)
    } else {
        std::os::windows::fs::symlink_file(target, at)
    };
    result.map_err(|e| match e.raw_os_error() {
        Some(PRIVILEGE_NOT_HELD) => crate::i18n::t("sftp_symlink_needs_privilege").to_string(),
        _ => e.to_string(),
    })
}
