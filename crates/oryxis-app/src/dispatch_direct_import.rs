//! The save of a planned third-party batch (the import preview's
//! "direct" half: PuTTY, WinSCP, ..., a CSV), and the preview state
//! around it. A CSV row may update or remove a known host and may
//! name a folder that does not exist yet (`importers::csv_plan`), so
//! the save is more than a run of inserts:
//!
//! - folders the ticked rows name are created FIRST, all of them,
//!   inside the batch (hosts reference them);
//! - an update is written only over the host the preview showed: a
//!   host edited or removed since (a sync, another window) is left
//!   alone and said so, never overwritten with the older copy;
//! - removals go through `remove_hosts`, the one delete door, after
//!   the batch commits, and only after their own confirm.

use iced::Task;
use oryxis_core::models::Group;
use uuid::Uuid;

use crate::app::{Message, Oryxis};
use crate::importers::{DirectHost, FolderPlan, HostOp};
use crate::messages::ShareMessage;

impl Oryxis {
    /// Close the import preview and drop everything it held, the
    /// target folder included (every dismiss consumes it).
    pub(crate) fn close_ssh_import(&mut self) {
        let ov = self.vaults.cur_mut();
        ov.panels.ssh_import_dialog = false;
        ov.ssh_import_hosts.clear();
        ov.ssh_import_direct = None;
        ov.ssh_import_selected.clear();
        ov.ssh_import_kinds.clear();
        ov.ssh_import_new_folders.clear();
        ov.import_target_group = None;
    }

    /// Re-read which folders the ticked rows would create, so the
    /// preview names exactly what the save will make.
    pub(crate) fn refresh_import_new_folders(&mut self) {
        let ov = self.vaults.cur_mut();
        ov.ssh_import_new_folders = match &ov.ssh_import_direct {
            Some(direct) => crate::importers::csv_plan::folders_to_create(
                &ov.groups,
                &direct.hosts,
                &ov.ssh_import_selected,
            ),
            None => Vec::new(),
        };
    }

    /// The hosts the ticked removal rows name, in the file's order.
    fn direct_import_removals(&self) -> Vec<Uuid> {
        let ov = self.vaults.cur();
        let Some(direct) = &ov.ssh_import_direct else {
            return Vec::new();
        };
        direct
            .hosts
            .iter()
            .zip(&ov.ssh_import_selected)
            .filter(|(h, on)| **on && h.op == HostOp::Remove)
            .map(|(h, _)| h.conn.id)
            .collect()
    }

    pub(crate) fn direct_import_removes_any(&self) -> bool {
        !self.direct_import_removals().is_empty()
    }

    /// The removal confirm for a pending batch: the dashboard's batch
    /// delete dialog, naming the hosts (or counting them past a few).
    pub(crate) fn confirm_direct_import_removals(&mut self) -> Task<Message> {
        let ids = self.direct_import_removals();
        let body = self.remove_confirm_body(&ids);
        self.confirm_remove_body(body, Message::Share(ShareMessage::SshImportConfirmed));
        Task::none()
    }

    /// Save the ticked rows of the pending direct batch in one
    /// transaction, then run its removals; one toast says what
    /// happened. `target_group` is where a new host that names no
    /// folder lands.
    pub(crate) fn save_direct_import(&mut self, target_group: Option<Uuid>) -> Task<Message> {
        let removals_wanted = self.direct_import_removals();
        let ov = self.vaults.cur_mut();
        let Some(direct) = ov.ssh_import_direct.take() else {
            return Task::none();
        };
        let selected = std::mem::take(&mut ov.ssh_import_selected);
        ov.ssh_import_kinds.clear();
        ov.ssh_import_new_folders.clear();
        ov.panels.ssh_import_dialog = false;
        let Some(vault) = &ov.store else {
            return Task::none();
        };
        // A batch of plain adds keeps the summary it always had; one
        // that updates or removes counts each.
        let maintains = direct.hosts.iter().any(|h| h.op != HostOp::Add);
        let picked: Vec<DirectHost> = direct
            .hosts
            .into_iter()
            .zip(selected)
            .filter(|(_, on)| *on)
            .map(|(h, _)| h)
            .collect();
        let total = picked.len();

        let _ = vault.begin_batch();
        let mut errors: Vec<String> = Vec::new();
        // Every folder first: a failure here leaves nothing half made.
        let mut groups = ov.groups.clone();
        let mut created: Vec<Group> = Vec::new();
        let mut folder_of_path: Vec<(String, Option<Uuid>)> = Vec::new();
        for host in &picked {
            if let FolderPlan::Create(path) = &host.folder
                && !folder_of_path.iter().any(|(p, _)| p == path)
            {
                let gid = Group::create_path(&mut groups, path, &mut created);
                folder_of_path.push((path.clone(), gid));
            }
        }
        if let Some(e) = created.iter().find_map(|g| vault.save_group(g).err()) {
            vault.rollback_batch();
            let msg = format!("{}: {e}", crate::i18n::t("import_error_folder"));
            ov.ssh_config_import_status = Some(Err(msg.clone()));
            return self.show_toast(msg);
        }

        let now = chrono::Utc::now();
        let mut added: Vec<oryxis_core::models::Connection> = Vec::new();
        let mut updated: Vec<oryxis_core::models::Connection> = Vec::new();
        for host in picked {
            let DirectHost { mut conn, password, op, folder, .. } = host;
            match &op {
                HostOp::Remove => continue,
                // Ticked, but there is nothing to write.
                HostOp::Update { changes } if changes.is_empty() => continue,
                HostOp::Update { .. } => {
                    match ov.connections.iter().find(|c| c.id == conn.id) {
                        None => {
                            errors.push(format!("{}: {}", conn.label, crate::i18n::t("import_error_host_gone")));
                            continue;
                        }
                        Some(live) if live.updated_at != conn.updated_at => {
                            errors.push(format!("{}: {}", conn.label, crate::i18n::t("import_error_host_changed")));
                            continue;
                        }
                        Some(_) => conn.updated_at = now,
                    }
                }
                HostOp::Add => {}
            }
            conn.group_id = match folder {
                FolderPlan::Open if op == HostOp::Add => target_group,
                FolderPlan::Open | FolderPlan::Keep => conn.group_id,
                FolderPlan::Root => None,
                FolderPlan::Existing(gid) if groups.iter().any(|g| g.id == gid) => Some(gid),
                FolderPlan::Existing(_) => {
                    errors.push(format!("{}: {}", conn.label, crate::i18n::t("import_error_folder_gone")));
                    continue;
                }
                FolderPlan::Create(path) => folder_of_path
                    .iter()
                    .find(|(p, _)| *p == path)
                    .and_then(|(_, gid)| *gid),
            };
            match vault.save_connection(&conn, password.as_deref()) {
                Ok(()) if op == HostOp::Add => added.push(conn),
                Ok(()) => updated.push(conn),
                Err(e) => errors.push(format!("{}: {e}", conn.label)),
            }
        }
        if let Err(e) = vault.commit_batch() {
            vault.rollback_batch();
            errors.push(format!("commit: {e}"));
            let msg = errors.join("; ");
            ov.ssh_config_import_status = Some(Err(msg.clone()));
            return self.show_toast(msg);
        }
        ov.groups = groups;
        let (n_added, n_updated) = (added.len(), updated.len());
        ov.connections.extend(added);
        for conn in updated {
            if let Some(slot) = ov.connections.iter_mut().find(|c| c.id == conn.id) {
                *slot = conn;
            }
        }
        // `remove_hosts` passes over an id that left meanwhile; count
        // what it will actually remove.
        let removals: Vec<Uuid> = removals_wanted
            .into_iter()
            .filter(|id| ov.connections.iter().any(|c| c.id == *id))
            .collect();
        let n_removed = removals.len();
        if !removals.is_empty() {
            self.remove_hosts(&removals);
        }

        let mut summary = if maintains {
            crate::i18n::t("import_summary_counts")
                .replace("{added}", &n_added.to_string())
                .replace("{updated}", &n_updated.to_string())
                .replace("{removed}", &n_removed.to_string())
        } else {
            format!("{} {} / {}", crate::i18n::t("import_summary_imported"), n_added, total)
        };
        let ov = self.vaults.cur_mut();
        if errors.is_empty() {
            ov.ssh_config_import_status = Some(Ok(summary.clone()));
        } else {
            summary.push_str("; ");
            summary.push_str(&errors.join("; "));
            ov.ssh_config_import_status = Some(Err(summary.clone()));
        }
        self.show_toast(summary)
    }
}
