//! ZMODEM in-terminal transfer progress and download-dir settings, wrapped by [`crate::messages::Message::Zmodem`]. Handled by `Oryxis::handle_zmodem`.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ZmodemMessage {
    /// A ZMODEM transfer streamed a progress / outcome event for a pane.
    /// Terminal states (Completed / Aborted / Error) clear the pane's
    /// transfer and resume the terminal.
    ZmodemProgress(Uuid, oryxis_zmodem::Progress),  // (pane_id, progress)
    /// User asked to cancel the pane's in-flight ZMODEM transfer.
    ZmodemCancel(Uuid),  // (pane_id)
    /// The detect window for an OS-drop `rz -y` elapsed. If the pane
    /// still holds pending drop sources, the detector never saw the
    /// remote receiver start (no lrzsz, or the line went into a
    /// full-screen program): clear them and explain. A no-op when the
    /// transfer already started, so this can never abort one, unlike
    /// the mid-transfer watchdog it replaces from #106.
    ZmodemDropRzTimeout(Uuid),  // (pane_id)
    /// Pick the folder ZMODEM downloads are saved into.
    PickZmodemDownloadDir,
    /// ZMODEM download folder chosen (or dialog dismissed with `None`).
    ZmodemDownloadDirPicked(Option<String>),
    /// Reset the ZMODEM download folder to the OS default.
    ClearZmodemDownloadDir,
    /// The folder was picked after the download had already completed,
    /// and its files were just moved there (`zmodem_delivery`): the
    /// completion toast fired with no location, so this one names it.
    ZmodemDelivered { dir: std::path::PathBuf, files: Vec<String> },
    /// A finished download could not be moved into the picked folder
    /// and is still at `staying`.
    ZmodemMoveFailed {
        name: String,
        dir: std::path::PathBuf,
        staying: std::path::PathBuf,
        err: String,
    },
    /// The boot sweep delivered files a previous process left finished
    /// in the staging folder to the default download folder.
    ZmodemRecovered { dir: std::path::PathBuf, files: Vec<std::path::PathBuf> },
}
