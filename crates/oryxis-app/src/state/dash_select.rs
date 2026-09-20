//! Multi-selection of host cards on the dashboard, and the drag that
//! carries one or more of them onto a folder (issue #230).
//!
//! Keyed by connection id, never by index: `dashboard_host_order`
//! re-sorts under an auto-save rename or a sync apply, and an index kept
//! across that would select a stranger. `card_context_menu` made the
//! same choice for the same reason.

use uuid::Uuid;

/// The selected host cards. Session-only, cleared by Esc, by a plain
/// click, by every move, and by a view change (`prune` drops ids the
/// list no longer has, so a host deleted elsewhere cannot linger in
/// the count).
#[derive(Debug, Clone, Default)]
pub(crate) struct DashSelection {
    /// Selected ids in the order they were added.
    pub(crate) ids: Vec<Uuid>,
    /// The end a Shift+click extends from: the last card toggled on
    /// by a plain toggle, so a range reads the way file managers do.
    pub(crate) anchor: Option<Uuid>,
}

impl DashSelection {
    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn contains(&self, id: Uuid) -> bool {
        self.ids.contains(&id)
    }

    /// Flip one card; a card toggled ON becomes the anchor.
    pub(crate) fn toggle(&mut self, id: Uuid) {
        if let Some(pos) = self.ids.iter().position(|x| *x == id) {
            self.ids.remove(pos);
            if self.anchor == Some(id) {
                self.anchor = self.ids.last().copied();
            }
        } else {
            self.ids.push(id);
            self.anchor = Some(id);
        }
    }

    /// Add every id of `range` (a Shift+click over the visible order),
    /// keeping what was already selected. The anchor stays where it
    /// was, so a second Shift+click re-ranges from the same end.
    pub(crate) fn extend(&mut self, range: impl IntoIterator<Item = Uuid>) {
        for id in range {
            if !self.ids.contains(&id) {
                self.ids.push(id);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// Drop every id that is no longer a connection.
    pub(crate) fn prune(&mut self, alive: impl Fn(Uuid) -> bool) {
        self.ids.retain(|id| alive(*id));
        if self.anchor.is_some_and(|a| !self.ids.contains(&a)) {
            self.anchor = self.ids.last().copied();
        }
    }
}

/// A press on a host card that may become a drag onto a folder. Armed
/// on the global left press (the card's own `button` captures the press,
/// so a `press_hit_reporter` around the card is what names it), promoted
/// past a movement threshold in `MouseMoved`, resolved on the global
/// release against the folder under the cursor.
#[derive(Debug, Clone)]
pub(crate) struct CardDrag {
    /// The hosts travelling: the whole selection when the pressed card
    /// was part of it, else just that card.
    pub(crate) ids: Vec<Uuid>,
    /// Cursor position at press, for the move threshold.
    pub(crate) start: iced::Point,
    /// Promoted past the threshold (a real drag, not a click).
    pub(crate) active: bool,
    /// What the ghost pill says: the host's label, or "N hosts".
    pub(crate) label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_moves_the_anchor_with_the_last_addition() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut sel = DashSelection::default();
        sel.toggle(a);
        sel.toggle(b);
        assert_eq!(sel.anchor, Some(b));
        sel.toggle(b);
        assert_eq!(sel.anchor, Some(a));
        sel.toggle(a);
        assert!(sel.is_empty());
        assert_eq!(sel.anchor, None);
    }

    #[test]
    fn prune_drops_the_dead_and_re_anchors() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut sel = DashSelection::default();
        sel.toggle(a);
        sel.toggle(b);
        sel.prune(|id| id == a);
        assert_eq!(sel.ids, vec![a]);
        assert_eq!(sel.anchor, Some(a));
    }
}
