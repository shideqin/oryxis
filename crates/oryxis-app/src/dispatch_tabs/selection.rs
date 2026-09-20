//! Host-card multi-selection, "Move to group" and the card drag onto a
//! folder (issue #230): the three doors through which a host changes
//! folder without opening its editor.
//!
//! A move is an EDIT: `group_id` set, `updated_at` stamped,
//! `save_connection(conn, None)` (the password column untouched), the
//! same shape as the folder-delete re-home in `groups.rs`. That is the
//! opposite of the `last_used` stamp on connect, which is deliberately
//! a narrow UPDATE because connecting is not an edit; moving is one,
//! and must out-rank an older copy under last-writer-wins.

use super::*;
use uuid::Uuid;

impl Oryxis {
    pub(super) fn handle_tabs_selection(&mut self, message: TabsMessage) -> Task<Message> {
        match message {
            TabsMessage::CardPressed(idx) => {
                let Some(id) = self.connections.get(idx).map(|c| c.id) else {
                    return Task::none();
                };
                // `button` publishes no modifiers; the app tracks them
                // from the keyboard subscription, the way the SFTP rows
                // read theirs.
                let ctrl = self.modifiers.control() || self.modifiers.command();
                let shift = self.modifiers.shift();
                if ctrl {
                    self.dash_selection.toggle(id);
                    return Task::none();
                }
                if shift && let Some(anchor) = self.dash_selection.anchor {
                    // Range over the order the dashboard is showing.
                    let order: Vec<Uuid> = self
                        .dashboard_host_order()
                        .into_iter()
                        .map(|i| self.connections[i].id)
                        .collect();
                    let a = order.iter().position(|x| *x == anchor);
                    let b = order.iter().position(|x| *x == id);
                    if let (Some(a), Some(b)) = (a, b) {
                        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                        self.dash_selection.extend(order[lo..=hi].iter().copied());
                        return Task::none();
                    }
                }
                // A plain click is what it always was: connect. The
                // selection is a way of acting on several hosts at once,
                // and connecting leaves for a terminal tab, so it ends.
                self.dash_selection.clear();
                self.update(Message::Ssh(SshMessage::ConnectSsh(idx)))
            }
            TabsMessage::CardSelectToggle(id) => {
                self.dash_selection.toggle(id);
                Task::none()
            }
            TabsMessage::SelectionClear => {
                self.dash_selection.clear();
                Task::none()
            }
            TabsMessage::SelectionSelectAll => {
                let visible: Vec<Uuid> = self
                    .dashboard_host_order()
                    .into_iter()
                    .map(|i| self.connections[i].id)
                    .collect();
                self.dash_selection.extend(visible);
                Task::none()
            }
            TabsMessage::MoveHostsPick(ids) => {
                if ids.is_empty() {
                    return Task::none();
                }
                // Replaces the kebab (anchored where it was) or opens at
                // the ringed row / the cursor from the selection bar.
                let anchor = match self.overlay.take() {
                    Some(o) => (o.x, o.y),
                    None => self.keynav_take_menu_anchor(),
                };
                self.card_context_menu = None;
                self.group_picker_search.clear();
                self.move_hosts_pending = ids;
                self.overlay = Some(OverlayState {
                    content: OverlayContent::GroupPicker(
                        crate::state::GroupPickerTarget::MoveHosts,
                    ),
                    x: anchor.0,
                    y: anchor.1,
                });
                Task::none()
            }
            m => crate::dispatch::unrouted(m),
        }
    }

    /// Put `ids` in `target` (`None` = the top level). Refuses a target
    /// that is not an existing MANUAL folder: a dynamic group's contents
    /// come from its query. Hosts already there are left alone, so a
    /// no-op move writes nothing and bumps no `updated_at`.
    pub(crate) fn move_hosts_to_group(&mut self, ids: &[Uuid], target: Option<Uuid>) -> Task<Message> {
        if let Some(gid) = target
            && !self.groups.iter().any(|g| g.id == gid && g.cloud_query.is_none())
        {
            return Task::none();
        }
        let mut moved = 0usize;
        let mut failed = 0usize;
        for conn in self.connections.iter_mut().filter(|c| ids.contains(&c.id)) {
            if conn.group_id == target {
                continue;
            }
            conn.group_id = target;
            conn.updated_at = chrono::Utc::now();
            match &self.vault {
                Some(vault) => match vault.save_connection(conn, None) {
                    Ok(()) => moved += 1,
                    Err(e) => {
                        tracing::error!("move host {}: {e}", conn.id);
                        failed += 1;
                    }
                },
                None => failed += 1,
            }
        }
        self.dash_selection.clear();
        if failed > 0 {
            self.set_toast(crate::i18n::t("hosts_move_failed").to_string());
        } else if moved > 0 {
            let group = match target {
                Some(gid) => oryxis_core::models::Group::path_of(&self.groups, gid),
                None => crate::i18n::t("group_picker_root").to_string(),
            };
            self.set_toast(
                crate::i18n::t("hosts_moved")
                    .replace("{hosts}", &crate::i18n::host_count(moved))
                    .replace("{group}", &group),
            );
        }
        Task::none()
    }

    /// Arm a card drag from the global left press: the whole selection
    /// when the pressed card is part of it, else that card alone.
    ///
    /// The folder hovers are reset here, the way the tab drag clears
    /// `hover.tab` before arming: the press is on a HOST card by
    /// construction, so any folder hover still recorded is stale (a
    /// folder card that scrolled out from under a resting cursor gets
    /// no `on_exit`), and a stale one would aim a drop released over
    /// empty space at a folder nobody pointed at. Only the enters that
    /// arrive DURING the drag name a target.
    pub(crate) fn arm_card_drag(&mut self, id: Uuid) {
        self.hover.folder_card = None;
        self.hover.folder_back = false;
        let ids = if self.dash_selection.contains(id) {
            self.dash_selection.ids.clone()
        } else {
            vec![id]
        };
        let label = if ids.len() == 1 {
            self.connections
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.label.clone())
                .unwrap_or_default()
        } else {
            crate::i18n::host_count(ids.len())
        };
        self.card_drag = Some(crate::state::CardDrag {
            ids,
            start: self.mouse_position,
            active: false,
            label,
        });
    }

    /// The folder a card drag released now would land in, read from the
    /// hover state the drop targets write: a manual folder card or tree
    /// row (`hover.folder_card`), or the folder header's back arrow,
    /// which means the open folder's PARENT. Read by the view to paint
    /// the target and by the release to perform the drop, from the same
    /// inputs, so the two agree by construction. `None` = no target.
    pub(crate) fn card_drop_target(&self) -> Option<Option<Uuid>> {
        if self.hover.folder_back {
            let open = self.active_group?;
            let parent = self
                .groups
                .iter()
                .find(|g| g.id == open)
                .and_then(|g| g.parent_id)
                .filter(|pid| self.groups.iter().any(|g| g.id == *pid && g.cloud_query.is_none()));
            return Some(parent);
        }
        let gid = self.hover.folder_card?;
        self.groups
            .iter()
            .any(|g| g.id == gid && g.cloud_query.is_none())
            .then_some(Some(gid))
    }

    /// The global release with an ACTIVE card drag: move onto whatever
    /// target the cursor is over, or nothing.
    pub(crate) fn drop_dragged_cards(&mut self, drag: crate::state::CardDrag) -> Task<Message> {
        match self.card_drop_target() {
            Some(target) => self.move_hosts_to_group(&drag.ids, target),
            None => Task::none(),
        }
    }
}
