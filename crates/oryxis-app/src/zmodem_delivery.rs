//! Where a ZMODEM download lands when the user is asked, and when.
//!
//! "Ask where to save downloads" used to run the folder dialog BEFORE
//! the driver spawned, so the remote `sz` sat on its opening header
//! waiting for the receiver to answer, and stock lrzsz gives that up
//! after about 30 s (`sz -t` is its own knob, tenths of a second, and
//! nothing on this side can move it). The reporter of issue #230 asked
//! for that timeout to be configurable; the honest answer is to stop
//! racing it. So the driver now starts at once, receiving into a
//! STAGING folder of the app's own, the dialog runs meanwhile, and each
//! finished file is moved to the chosen folder once both are known. The
//! dialog can stay open as long as the user likes.
//!
//! The rules, each one a test below:
//!
//! - Answered before a file completes: that file moves on its
//!   `FileDone`, and the ones already finished move right away.
//! - Answered after the whole transfer completed: the parked files
//!   move, and the app says where they went, because the completion
//!   toast has already fired with no location to name.
//! - Declined while the transfer runs: the driver is asked to abort
//!   (flag plus wake-up, the driver sends the CANCEL sequence itself,
//!   never this module: after the driver has ended the divert is gone
//!   and those bytes would land in the shell), and the part files stay
//!   as the resume anchors they always were.
//! - Declined after it completed: the finished files of THIS transfer
//!   are deleted, by the paths their `FileDone` reported; the folder
//!   is never swept.
//! - A move that fails is reported by name with where the file stayed,
//!   never redirected somewhere the user did not pick.
//!
//! Staging is ONE fixed folder (`~/.oryxis/incoming`), not one per
//! transfer: the driver's `<name>.oryxis-part` in its destination is
//! what a later `sz` of the same file resumes from, and a folder minted
//! per transfer would lose that. It sits outside `runtime/`, which is
//! swept at boot. A window closed with the dialog still open leaves
//! finished files there, so [`sweep_orphans`] delivers them to the
//! default download folder on the next launch (parts are left alone).
//!
//! Uploads are untouched: there is nothing to send before a file is
//! picked, so their picker keeps running first.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oryxis_zmodem::{PART_SUFFIX, Progress, place_file};
use tokio::sync::mpsc;

/// The folder the driver receives into while the user is being asked.
pub(crate) fn staging_dir() -> PathBuf {
    oryxis_core::paths::oryxis_dir()
        .unwrap_or_else(|| PathBuf::from(".").join(".oryxis"))
        .join("incoming")
}

/// The answer to "where do these files go", as the relay sees it.
pub(crate) type DestinationAnswer = Pin<Box<dyn Future<Output = Option<PathBuf>> + Send>>;

/// How a transfer's files reach their destination.
pub(crate) enum Placement {
    /// The driver writes straight into the destination (the toggle is
    /// off, or this is an upload): every `FileDone` path is final.
    Direct,
    /// The driver writes into `staging`; `answer` resolves to the folder
    /// the user picked, or `None` when they declined.
    Staged {
        staging: PathBuf,
        answer: DestinationAnswer,
    },
}

/// What the relay reports upward, one message each in the app.
#[derive(Debug)]
pub(crate) enum DeliveryEvent {
    /// A driver event, forwarded; a `FileDone` carries the path the
    /// file ended up at, which is the moved one once a folder is known.
    Progress(Progress),
    /// The folder was picked AFTER the transfer had completed, and the
    /// parked files were just moved there.
    Delivered { dir: PathBuf, files: Vec<String> },
    /// One file could not be moved into the picked folder; it is still
    /// at `staying`.
    MoveFailed {
        name: String,
        dir: PathBuf,
        staying: PathBuf,
        err: String,
    },
}

/// Relay the driver's progress to `events`, placing each finished file
/// once the destination is known. Runs until the driver's channel has
/// closed AND the answer (if any) has been consumed: a native dialog
/// cannot be closed from here, and a dropped future would leave it on
/// screen with its answer thrown away.
pub(crate) async fn relay(
    mut progress: mpsc::UnboundedReceiver<Progress>,
    placement: Placement,
    abort: Arc<AtomicBool>,
    wire_tx: mpsc::UnboundedSender<Vec<u8>>,
    events: mpsc::UnboundedSender<DeliveryEvent>,
) {
    let (staging, mut answer) = match placement {
        Placement::Direct => (None, None),
        Placement::Staged { staging, answer } => (Some(staging), Some(answer)),
    };
    // `None` until the user answers; then the folder, or `None` again
    // inside for a decline.
    let mut destination: Option<Option<PathBuf>> = None;
    let mut parked: Vec<PathBuf> = Vec::new();
    let mut driver_done = false;
    loop {
        tokio::select! {
            picked = async { answer.as_mut().expect("guarded").await }, if answer.is_some() => {
                answer = None;
                match picked {
                    Some(dir) => {
                        let moved = place_parked(&mut parked, &dir, &events).await;
                        if driver_done && !moved.is_empty() {
                            let _ = events.send(DeliveryEvent::Delivered { dir: dir.clone(), files: moved });
                        }
                        destination = Some(Some(dir));
                    }
                    None => {
                        destination = Some(None);
                        for path in parked.drain(..) {
                            let _ = tokio::fs::remove_file(&path).await;
                        }
                        if !driver_done {
                            // The driver owns the wire: the flag plus
                            // an empty chunk is its documented wake-up,
                            // and it answers with the CANCEL sequence.
                            abort.store(true, Ordering::Relaxed);
                            let _ = wire_tx.send(Vec::new());
                        }
                    }
                }
            }
            event = progress.recv(), if !driver_done => {
                match event {
                    None => driver_done = true,
                    Some(Progress::FileDone { name, path: Some(src) }) if staging.is_some() => {
                        let path = match &destination {
                            Some(Some(dir)) => {
                                match place_into(&src, dir).await {
                                    Ok(moved) => Some(moved),
                                    Err(err) => {
                                        let _ = events.send(DeliveryEvent::MoveFailed {
                                            name: name.clone(),
                                            dir: dir.clone(),
                                            staying: src.clone(),
                                            err,
                                        });
                                        Some(src)
                                    }
                                }
                            }
                            // Declined: a file that completed between
                            // the decline and the abort is not wanted.
                            Some(None) => {
                                let _ = tokio::fs::remove_file(&src).await;
                                None
                            }
                            None => {
                                parked.push(src.clone());
                                Some(src)
                            }
                        };
                        let _ = events.send(DeliveryEvent::Progress(Progress::FileDone { name, path }));
                    }
                    Some(p) => {
                        let _ = events.send(DeliveryEvent::Progress(p));
                    }
                }
            }
        }
        if driver_done && answer.is_none() {
            break;
        }
    }
}

/// Move every parked file into `dir`, reporting each failure; returns
/// the names of the ones that made it. A folder the user typed into the
/// dialog may not exist yet.
async fn place_parked(
    parked: &mut Vec<PathBuf>,
    dir: &Path,
    events: &mpsc::UnboundedSender<DeliveryEvent>,
) -> Vec<String> {
    let mut moved = Vec::new();
    let ready = match tokio::fs::create_dir_all(dir).await {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("create {}: {e}", dir.display())),
    };
    for src in parked.drain(..) {
        let name = file_name(&src);
        let outcome = match &ready {
            Ok(()) => place_into(&src, dir).await.map(|_| ()),
            Err(e) => Err(e.clone()),
        };
        match outcome {
            Ok(()) => moved.push(name),
            Err(err) => {
                let _ = events.send(DeliveryEvent::MoveFailed {
                    name,
                    dir: dir.to_path_buf(),
                    staying: src,
                    err,
                });
            }
        }
    }
    moved
}

/// Move `src` into `dir` under its own name, browser-style " (N)" on a
/// collision, across volumes when it has to be.
async fn place_into(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    place_file(src, dir, &file_name(src)).await
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "received.bin".to_string())
}

/// Deliver what a previous process left finished in `staging` to
/// `default_dir`: every regular file that is not a part file. Parts are
/// the resume anchors and stay. Returns the paths the files landed at.
pub(crate) async fn sweep_orphans(staging: &Path, default_dir: &Path) -> Vec<PathBuf> {
    let mut moved = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(staging).await else {
        return moved;
    };
    let mut created = false;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        let name = file_name(&path);
        if name.ends_with(PART_SUFFIX) {
            continue;
        }
        if !created {
            if tokio::fs::create_dir_all(default_dir).await.is_err() {
                return moved;
            }
            created = true;
        }
        match place_file(&path, default_dir, &name).await {
            Ok(landed) => moved.push(landed),
            Err(e) => tracing::warn!(
                "delivering {} from the staging folder failed: {e}",
                path.display()
            ),
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("oryxis-zm-delivery-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Harness {
        staging: PathBuf,
        dest: PathBuf,
        progress: mpsc::UnboundedSender<Progress>,
        answer: Option<oneshot::Sender<Option<PathBuf>>>,
        abort: Arc<AtomicBool>,
        wire: mpsc::UnboundedReceiver<Vec<u8>>,
        events: mpsc::UnboundedReceiver<DeliveryEvent>,
        task: tokio::task::JoinHandle<()>,
    }

    fn start(tag: &str) -> Harness {
        let root = scratch(tag);
        let staging = root.join("incoming");
        let dest = root.join("chosen");
        std::fs::create_dir_all(&staging).unwrap();
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let (answer_tx, answer_rx) = oneshot::channel::<Option<PathBuf>>();
        let (wire_tx, wire_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let abort = Arc::new(AtomicBool::new(false));
        let placement = Placement::Staged {
            staging: staging.clone(),
            answer: Box::pin(async move { answer_rx.await.unwrap_or(None) }),
        };
        let task = tokio::spawn(relay(
            progress_rx,
            placement,
            abort.clone(),
            wire_tx,
            events_tx,
        ));
        Harness {
            staging,
            dest,
            progress: progress_tx,
            answer: Some(answer_tx),
            abort,
            wire: wire_rx,
            events: events_rx,
            task,
        }
    }

    impl Harness {
        /// The driver finishing a file: it exists in staging under its
        /// final name, and `FileDone` names it.
        fn finish(&self, name: &str, body: &[u8]) -> PathBuf {
            let path = self.staging.join(name);
            std::fs::write(&path, body).unwrap();
            self.progress
                .send(Progress::FileDone {
                    name: name.to_string(),
                    path: Some(path.clone()),
                })
                .unwrap();
            path
        }
        fn complete(&mut self) {
            self.progress
                .send(Progress::Completed {
                    trailing: Vec::new(),
                })
                .unwrap();
        }
        fn answer(&mut self, dir: Option<PathBuf>) {
            self.answer.take().unwrap().send(dir).unwrap();
        }
        async fn next(&mut self) -> DeliveryEvent {
            tokio::time::timeout(std::time::Duration::from_secs(5), self.events.recv())
                .await
                .expect("an event within 5 s")
                .expect("relay still running")
        }
        /// Wait until the relay has acted on the answer, observed on
        /// disk: with the dialog and a `FileDone` both ready, `select!`
        /// picks either first, so a test that needs the answer applied
        /// before the next file waits for its footprint.
        async fn wait_until(&self, cond: impl Fn() -> bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !cond() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "condition not met within 5 s"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        async fn finish_relay(self) {
            drop(self.progress);
            tokio::time::timeout(std::time::Duration::from_secs(5), self.task)
                .await
                .expect("relay ends once the driver and the answer are both done")
                .unwrap();
        }
    }

    fn done_path(ev: DeliveryEvent) -> PathBuf {
        match ev {
            DeliveryEvent::Progress(Progress::FileDone { path: Some(p), .. }) => p,
            other => panic!("expected FileDone, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_folder_picked_first_moves_every_file_as_it_finishes() {
        let mut h = start("early");
        h.answer(Some(h.dest.clone()));
        // The answer is applied once the picked folder exists.
        let dest = h.dest.clone();
        h.wait_until(|| dest.is_dir()).await;
        h.finish("a.txt", b"aaa");
        let a = done_path(h.next().await);
        assert_eq!(a, h.dest.join("a.txt"));
        assert_eq!(std::fs::read(&a).unwrap(), b"aaa");
        assert!(!h.staging.join("a.txt").exists());
        h.finish("b.txt", b"bbb");
        assert_eq!(done_path(h.next().await), h.dest.join("b.txt"));
        h.complete();
        assert!(matches!(
            h.next().await,
            DeliveryEvent::Progress(Progress::Completed { .. })
        ));
        h.finish_relay().await;
    }

    #[tokio::test]
    async fn a_folder_picked_mid_transfer_moves_the_parked_files_and_the_rest_on_arrival() {
        let mut h = start("mid");
        h.finish("first.bin", b"1");
        // Not yet known where: the file stays in staging, reported as is.
        assert_eq!(done_path(h.next().await), h.staging.join("first.bin"));
        h.answer(Some(h.dest.clone()));
        let first = h.dest.join("first.bin");
        h.wait_until(|| first.exists()).await;
        // No Delivered toast mid-transfer: the completion toast is coming.
        h.finish("second.bin", b"2");
        assert_eq!(done_path(h.next().await), h.dest.join("second.bin"));
        assert!(
            h.dest.join("first.bin").exists(),
            "parked file moved on the answer"
        );
        assert!(!h.staging.join("first.bin").exists());
        h.complete();
        assert!(matches!(
            h.next().await,
            DeliveryEvent::Progress(Progress::Completed { .. })
        ));
        h.finish_relay().await;
    }

    #[tokio::test]
    async fn a_folder_picked_after_completion_moves_the_files_and_says_where() {
        let mut h = start("late");
        h.finish("report.pdf", b"pdf");
        assert_eq!(done_path(h.next().await), h.staging.join("report.pdf"));
        h.complete();
        assert!(matches!(
            h.next().await,
            DeliveryEvent::Progress(Progress::Completed { .. })
        ));
        // The driver is gone; the dialog is still up.
        drop(std::mem::replace(
            &mut h.progress,
            mpsc::unbounded_channel().0,
        ));
        tokio::task::yield_now().await;
        // A folder that does not exist yet is created.
        let picked = h.dest.join("deeper");
        h.answer(Some(picked.clone()));
        match h.next().await {
            DeliveryEvent::Delivered { dir, files } => {
                assert_eq!(dir, picked);
                assert_eq!(files, vec!["report.pdf".to_string()]);
            }
            other => panic!("expected Delivered, got {other:?}"),
        }
        assert_eq!(std::fs::read(picked.join("report.pdf")).unwrap(), b"pdf");
        assert!(!h.staging.join("report.pdf").exists());
        h.finish_relay().await;
    }

    #[tokio::test]
    async fn declining_while_running_aborts_the_driver_and_drops_what_finished() {
        let mut h = start("decline-running");
        h.finish("done.bin", b"x");
        assert_eq!(done_path(h.next().await), h.staging.join("done.bin"));
        // A part file of the file in flight: the resume anchor.
        std::fs::write(
            h.staging.join(format!("inflight.bin{PART_SUFFIX}")),
            b"half",
        )
        .unwrap();
        h.answer(None);
        // Cooperative cancel: the flag and the empty wake-up chunk, the
        // CANCEL bytes themselves are the driver's to send.
        let wake = tokio::time::timeout(std::time::Duration::from_secs(5), h.wire.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(wake.is_empty());
        assert!(h.abort.load(Ordering::Relaxed));
        assert!(
            !h.staging.join("done.bin").exists(),
            "finished file discarded"
        );
        assert!(
            h.staging
                .join(format!("inflight.bin{PART_SUFFIX}"))
                .exists(),
            "the part file stays as the resume anchor"
        );
        // The driver honours the flag and ends.
        h.progress.send(Progress::Aborted).unwrap();
        assert!(matches!(
            h.next().await,
            DeliveryEvent::Progress(Progress::Aborted)
        ));
        h.finish_relay().await;
    }

    #[tokio::test]
    async fn declining_after_completion_deletes_only_this_transfers_files() {
        let mut h = start("decline-done");
        // Something else's finished file in the same staging folder.
        std::fs::write(h.staging.join("someone-elses.bin"), b"keep").unwrap();
        h.finish("mine.bin", b"drop");
        assert_eq!(done_path(h.next().await), h.staging.join("mine.bin"));
        h.complete();
        assert!(matches!(
            h.next().await,
            DeliveryEvent::Progress(Progress::Completed { .. })
        ));
        drop(std::mem::replace(
            &mut h.progress,
            mpsc::unbounded_channel().0,
        ));
        tokio::task::yield_now().await;
        h.answer(None);
        let staging = h.staging.clone();
        let abort = h.abort.clone();
        h.finish_relay().await;
        assert!(!staging.join("mine.bin").exists());
        assert!(staging.join("someone-elses.bin").exists());
        // Nothing was aborted: the driver had already ended.
        assert!(!abort.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn a_move_that_fails_is_reported_and_the_file_stays_put() {
        let mut h = start("move-fails");
        h.finish("stuck.bin", b"s");
        assert_eq!(done_path(h.next().await), h.staging.join("stuck.bin"));
        h.complete();
        assert!(matches!(
            h.next().await,
            DeliveryEvent::Progress(Progress::Completed { .. })
        ));
        drop(std::mem::replace(
            &mut h.progress,
            mpsc::unbounded_channel().0,
        ));
        tokio::task::yield_now().await;
        // A destination that cannot be a folder: a regular file.
        let blocked = h.dest.parent().unwrap().join("not-a-folder");
        std::fs::write(&blocked, b"file").unwrap();
        h.answer(Some(blocked.clone()));
        match h.next().await {
            DeliveryEvent::MoveFailed {
                name, dir, staying, ..
            } => {
                assert_eq!(name, "stuck.bin");
                assert_eq!(dir, blocked);
                assert_eq!(staying, h.staging.join("stuck.bin"));
            }
            other => panic!("expected MoveFailed, got {other:?}"),
        }
        assert!(h.staging.join("stuck.bin").exists());
        h.finish_relay().await;
    }

    #[tokio::test]
    async fn direct_placement_forwards_everything_untouched() {
        let root = scratch("direct");
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let (wire_tx, _wire_rx) = mpsc::unbounded_channel();
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let abort = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(relay(
            progress_rx,
            Placement::Direct,
            abort,
            wire_tx,
            events_tx,
        ));
        let path = root.join("direct.bin");
        std::fs::write(&path, b"d").unwrap();
        progress_tx
            .send(Progress::FileDone {
                name: "direct.bin".into(),
                path: Some(path.clone()),
            })
            .unwrap();
        progress_tx
            .send(Progress::Completed {
                trailing: b"$ ".to_vec(),
            })
            .unwrap();
        drop(progress_tx);
        assert_eq!(done_path(events_rx.recv().await.unwrap()), path);
        assert!(matches!(
            events_rx.recv().await.unwrap(),
            DeliveryEvent::Progress(Progress::Completed { trailing }) if trailing == b"$ "
        ));
        assert!(events_rx.recv().await.is_none());
        task.await.unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn the_sweep_delivers_finished_orphans_and_leaves_parts_alone() {
        let root = scratch("sweep");
        let staging = root.join("incoming");
        let dest = root.join("downloads");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("finished.txt"), b"f").unwrap();
        std::fs::write(staging.join(format!("half.bin{PART_SUFFIX}")), b"h").unwrap();
        std::fs::create_dir_all(staging.join("a-folder")).unwrap();
        // The default folder already holds a same-named file: it is not
        // clobbered.
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("finished.txt"), b"original").unwrap();
        let moved = sweep_orphans(&staging, &dest).await;
        assert_eq!(moved, vec![dest.join("finished (1).txt")]);
        assert_eq!(
            std::fs::read(dest.join("finished.txt")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read(dest.join("finished (1).txt")).unwrap(), b"f");
        assert!(staging.join(format!("half.bin{PART_SUFFIX}")).exists());
        assert!(staging.join("a-folder").exists());
        assert!(!staging.join("finished.txt").exists());
        // Nothing to do the second time.
        assert!(sweep_orphans(&staging, &dest).await.is_empty());
    }
}
