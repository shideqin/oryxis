//! Host-card multi-selection, "Move to group" and the card drag onto a
//! folder (issue #230): the three doors through which a host changes
//! folder without opening its editor.
//!
//! The selection itself is built by Ctrl / Shift + click, by the check
//! on a card, and by Space on a ringed card. The toolbar's multi-select
//! mode is the fourth way in, and the only discoverable one: while it is
//! on, every card click toggles instead of dialling, so a batch is
//! gathered by pointing at cards. Ctrl+click alone is invisible to
//! anyone who does not already know it exists.
//!
//! A move is an EDIT: `group_id` set, `updated_at` stamped,
//! `save_connection(conn, None)` (the password column untouched), the
//! same shape as the folder-delete re-home in `groups.rs`. That is the
//! opposite of the `last_used` stamp on connect, which is deliberately
//! a narrow UPDATE because connecting is not an edit; moving is one,
//! and must out-rank an older copy under last-writer-wins.

use super::*;
use uuid::Uuid;

/// Most host labels the batch-remove dialog spells out before it falls
/// back to a count. Six wrap to about two lines in the dialog, which is
/// as much as anyone reads before deciding; past that a wall of names
/// is worse than the number.
const REMOVE_CONFIRM_NAMES: usize = 6;

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
                // Multi-select mode (issue #230): a mode, not a modifier.
                // While it is on, a click on a card is a way of ADDING it
                // to the selection and never a dial, so a whole batch is
                // built by pointing at cards. The mode deliberately
                // out-ranks the modifiers: a click that lands without a
                // held Ctrl must not dial a host the user was only
                // pointing at, which is the whole reason the mode exists.
                // Shift still ranges, from the anchor a previous click
                // set; without one it is a plain toggle.
                if self.dash_multi_select {
                    if !shift || !self.extend_selection_to(id) {
                        self.dash_selection.toggle(id);
                    }
                    return Task::none();
                }
                if ctrl {
                    self.dash_selection.toggle(id);
                    return Task::none();
                }
                if shift && self.extend_selection_to(id) {
                    return Task::none();
                }
                // A plain click is what it always was: connect. The
                // selection is a way of acting on several hosts at once,
                // and connecting leaves for a terminal tab, so it ends.
                self.dash_selection.clear();
                self.update(Message::Ssh(SshMessage::ConnectSsh(idx)))
            }
            TabsMessage::ToggleMultiSelect => {
                // The mode and the selection are one gesture: leaving the
                // mode hands the cards back to click-connects, and a
                // selection left behind would keep the action bar up over
                // hosts whose cards no longer read as chosen. Turning it
                // ON keeps whatever Ctrl+click had already gathered.
                self.dash_multi_select = !self.dash_multi_select;
                if !self.dash_multi_select {
                    self.dash_selection.clear();
                }
                Task::none()
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
            TabsMessage::SelectionDelete => {
                // Destructive, so it asks first - the same guard the
                // single-host kebab and the host editor go through
                // (`confirm_remove`). A selection is cheap to build and
                // cheap to lose, which is exactly why the dialog is the
                // only thing between a slip of the wrist and a vault
                // with holes in it.
                let ids = self.selected_hosts_in_view_order();
                if ids.is_empty() {
                    return Task::none();
                }
                let body = self.remove_confirm_body(&ids);
                self.confirm_remove_body(
                    body,
                    Message::Tabs(TabsMessage::SelectionDeleteConfirmed(ids)),
                );
                Task::none()
            }
            TabsMessage::SelectionDeleteConfirmed(ids) => {
                // Batch delete. Each host goes exactly the way the single
                // one does: the vault row, plus the saved AI conversations
                // that reference it by id (the command history leaves with
                // the row, per the vault's cascade). The ids rode through
                // the dialog, never indices: the list can re-sort while it
                // is up (an auto-saved rename, a sync apply).
                //
                // A host open in the host editor is never flushed on its
                // way out - resurrecting a row the user just deleted would
                // be worse than dropping its last keystrokes - but a
                // debouncing auto-save on some OTHER host must not die
                // with the batch.
                if self
                    .editor_form
                    .editing_id
                    .is_some_and(|id| !ids.contains(&id))
                {
                    self.editor_flush_pending();
                }
                let mut attempted = false;
                if let Some(vault) = &self.vault {
                    for id in &ids {
                        if !self.connections.iter().any(|c| c.id == *id) {
                            continue;
                        }
                        attempted = true;
                        match vault.delete_connection(id) {
                            Ok(()) => {
                                let _ = vault.delete_chat_conversations_for_connection(id);
                            }
                            Err(e) => tracing::error!("delete host {id}: {e}"),
                        }
                    }
                }
                if attempted {
                    // Re-read the vault whatever the writes did: a delete
                    // that failed leaves its host on screen, which is the
                    // honest account of what the vault holds.
                    //
                    // The drawer closes only when it was showing one of
                    // the removed hosts; an unrelated host stays open, the
                    // way a deleted row leaves the editor alone.
                    if self
                        .editor_form
                        .editing_id
                        .is_some_and(|id| ids.contains(&id))
                    {
                        self.panels.host_panel = false;
                        self.panel_nav_clear();
                        self.editor_form.sweep_secrets();
                    }
                    self.load_data_from_vault();
                }
                self.dash_selection.clear();
                Task::none()
            }
            TabsMessage::SelectionConnect => {
                // Batch connect: one tab per selected host. Read the
                // selection in the order the dashboard shows it, so the
                // tabs land in the order the eye reads them. Each dial is
                // dispatched as its own `ConnectSsh`, so a single host
                // runs the exact pipeline a plain click would (tab push,
                // progress card, active-tab hand-off); batching the
                // messages is what makes them a batch.
                let ids = self.selected_hosts_in_view_order();
                // The card's kebab menu reaches this too (the collapsed
                // selection menu), so any open menu closes with the
                // action, the way `ConnectSsh` closes it for the single
                // host it dials.
                self.card_context_menu = None;
                self.overlay = None;
                // Connecting leaves for terminal tabs, so the selection
                // ends, the way a plain click ends it.
                self.dash_selection.clear();
                let connects: Vec<Task<Message>> = ids
                    .into_iter()
                    .filter_map(|id| {
                        self.connections
                            .iter()
                            .position(|c| c.id == id)
                            .map(|idx| Task::done(Message::Ssh(SshMessage::ConnectSsh(idx))))
                    })
                    .collect();
                Task::batch(connects)
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

    /// The selected hosts, in the order the dashboard is showing them.
    /// Every batch action reads the selection through this - the tabs a
    /// batch connect opens, the hosts a batch delete removes, and the
    /// labels the card menu and the remove dialog spell out - so they all
    /// land in the order the eye reads them.
    pub(crate) fn selected_hosts_in_view_order(&self) -> Vec<Uuid> {
        self.dashboard_host_order()
            .into_iter()
            .map(|i| self.connections[i].id)
            .filter(|id| self.dash_selection.contains(*id))
            .collect()
    }

    /// The confirmation body for a batch removal: the hosts' labels while
    /// the list still reads, the count once it does not. Names are the
    /// point when they fit - they are what the user checks the dialog
    /// against - and the number is what they check when they do not.
    fn remove_confirm_body(&self, ids: &[Uuid]) -> String {
        if ids.len() > REMOVE_CONFIRM_NAMES {
            return crate::i18n::host_count(ids.len());
        }
        let labels: Vec<&str> = ids
            .iter()
            .filter_map(|id| self.connections.iter().find(|c| c.id == *id))
            .map(|c| c.label.as_str())
            .collect();
        if labels.is_empty() {
            return crate::i18n::host_count(ids.len());
        }
        labels.join(", ")
    }

    /// Extend the selection with every visible host between the anchor
    /// and `id` (a Shift+click). `false` when there is nothing to range
    /// from - no anchor, or one that left the list - so the caller can
    /// fall back to a plain toggle instead of doing nothing.
    fn extend_selection_to(&mut self, id: Uuid) -> bool {
        // Range over the order the dashboard is showing.
        let Some(anchor) = self.dash_selection.anchor else {
            return false;
        };
        let order: Vec<Uuid> = self
            .dashboard_host_order()
            .into_iter()
            .map(|i| self.connections[i].id)
            .collect();
        let a = order.iter().position(|x| *x == anchor);
        let b = order.iter().position(|x| *x == id);
        let (Some(a), Some(b)) = (a, b) else {
            return false;
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.dash_selection.extend(order[lo..=hi].iter().copied());
        true
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
