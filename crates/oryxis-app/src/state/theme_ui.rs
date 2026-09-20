//! The theme editor's own UI state: the open form, its colour
//! popover, and the paste-a-scheme import modal beside it.
//!
//! Five fields on `Oryxis` until they moved here. They are one screen's
//! worth of state and they open and close together, which is exactly
//! what a struct says and five loose fields do not.


#[derive(Default)]
pub(crate) struct ThemeEditorUi {
    /// Open custom-theme editor modal. `None` = closed.
    pub(crate) editor: Option<crate::state::ThemeEditorForm>,
    /// Open color-picker popover in the theme editor: `(slot, anchor)`.
    /// `None` = closed. Clicking a slot's swatch opens a compact picker
    /// (SV square + hue + hex + presets) anchored at the click.
    pub(crate) color_popover: Option<(crate::state::ThemeColorSlot, iced::Point)>,
    pub(crate) import_content: iced::widget::text_editor::Content,
    pub(crate) import_name: String,
    pub(crate) import_error: Option<String>,
    /// Filter line of the Settings terminal-theme gallery. Cleared on
    /// open so the gallery always starts showing everything.
    pub(crate) gallery_filter: String,
    /// Filter line of the per-host theme picker, same contract.
    pub(crate) picker_filter: String,
    /// Dark / Light chip of the Settings terminal-theme gallery
    /// (`None` = all), cleared on open like the line.
    pub(crate) gallery_tone: Option<oryxis_terminal::ThemeTone>,
    /// Same chip on the per-host picker.
    pub(crate) picker_tone: Option<oryxis_terminal::ThemeTone>,
    /// Filter line and chip of the Settings app-theme gallery, which
    /// had no filter before the terminal one grew tags (issue #230).
    pub(crate) ui_gallery_filter: String,
    pub(crate) ui_gallery_tone: Option<oryxis_terminal::ThemeTone>,
}
