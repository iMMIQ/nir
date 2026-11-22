//! Native file dialogs off the owner thread.
//!
//! The synchronous rfd::FileDialog blocked the winit owner thread for the
//! whole dialog lifetime: no turns, no worker drains, no repaints. Each
//! dialog now runs on a dedicated thread behind AsyncFileDialog (the
//! xdg-desktop-portal backend on Linux and Win32 on Windows both resolve
//! from any thread), and the owner keeps pumping turns at its normal
//! cadence until the receiver yields exactly one outcome. The desktop
//! gates input and pauses the story clock while a dialog is open so
//! engine state cannot change behind the modal picker.
use anyhow::{anyhow, Result};
use nir_player::{AppEvent, SaveEnvelope};
use std::{fs, path::PathBuf, sync::mpsc, thread};

pub(crate) enum DialogTask {
    /// Select an export destination. The bounded storage worker writes it.
    Export { job: u32, json: String },
    /// Import a save file; the picked bytes are parsed on this thread.
    Import,
}
pub(crate) enum DialogOutcome {
    ExportSelected {
        job: u32,
        path: Option<PathBuf>,
        json: String,
    },
    /// A cancelled import feeds no engine event, matching the synchronous
    /// behaviour.
    ImportCancelled,
    /// A picked import file, parsed on the dialog thread.
    Imported(std::result::Result<Box<SaveEnvelope>, String>),
}
#[derive(Clone, Copy)]
pub(crate) enum DialogFailure {
    Export(u32),
    Import,
}
impl DialogFailure {
    pub(crate) fn event(self, message: String) -> AppEvent {
        match self {
            Self::Export(job) => AppEvent::ExportFailed { job, message },
            Self::Import => AppEvent::LoadFailed(message),
        }
    }
}
impl DialogTask {
    pub(crate) fn failure(&self) -> DialogFailure {
        match self {
            Self::Export { job, .. } => DialogFailure::Export(*job),
            Self::Import => DialogFailure::Import,
        }
    }
}
pub(crate) struct PendingDialog {
    pub(crate) replies: mpsc::Receiver<DialogOutcome>,
    pub(crate) failure: DialogFailure,
}
/// Runs `task` on a new thread on Windows/Linux (portal / Win32 allow off-thread
/// dialogs), or synchronously on the calling main thread on macOS (AppKit
/// requires `NSOpenPanel`/`NSSavePanel` on the main thread). In both cases the
/// returned receiver yields one outcome.
pub(crate) fn open(task: DialogTask) -> Result<PendingDialog> {
    let failure = task.failure();
    let (done, replies) = mpsc::channel::<DialogOutcome>();
    #[cfg(target_os = "macos")]
    {
        let outcome = run_sync(task);
        let _ = done.send(outcome);
        Ok(PendingDialog { replies, failure })
    }
    #[cfg(not(target_os = "macos"))]
    {
        thread::Builder::new()
            .name("nir-dialog".into())
            .spawn(move || {
                let outcome = run(task);
                let _ = done.send(outcome);
            })
            .map_err(|_| anyhow!("E_DIALOG_THREAD"))?;
        Ok(PendingDialog { replies, failure })
    }
}
fn parse_imported_file(path: &std::path::Path) -> std::result::Result<Box<SaveEnvelope>, String> {
    (|| -> anyhow::Result<_> {
        use std::io::Read;
        let max = nir_format::MAX_INPUT_BYTES;
        let meta = fs::metadata(path)?;
        anyhow::ensure!(
            meta.len() <= max as u64,
            "E_SAVE_SIZE: imported save exceeds {} bytes",
            max
        );
        let file = fs::File::open(path)?;
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take((max + 1) as u64).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= max,
            "E_SAVE_SIZE: imported save exceeds {} bytes",
            max
        );
        Ok(nir_content::parse(&bytes, "imported save")?)
    })()
    .map(Box::new)
    .map_err(|e| e.to_string())
}
#[cfg(target_os = "macos")]
fn run_sync(task: DialogTask) -> DialogOutcome {
    match task {
        DialogTask::Export { job, json } => {
            let picked = rfd::FileDialog::new()
                .set_file_name("save.nir-save.json")
                .save_file();
            DialogOutcome::ExportSelected {
                job,
                path: picked,
                json,
            }
        }
        DialogTask::Import => match rfd::FileDialog::new()
            .add_filter("NIR save", &["json"])
            .pick_file()
        {
            None => DialogOutcome::ImportCancelled,
            Some(path) => DialogOutcome::Imported(parse_imported_file(&path)),
        },
    }
}
#[cfg(not(target_os = "macos"))]
fn run(task: DialogTask) -> DialogOutcome {
    match task {
        DialogTask::Export { job, json } => {
            let picked = pollster::block_on(
                rfd::AsyncFileDialog::new()
                    .set_file_name("save.nir-save.json")
                    .save_file(),
            );
            DialogOutcome::ExportSelected {
                job,
                path: picked.map(|handle| handle.path().to_owned()),
                json,
            }
        }
        DialogTask::Import => {
            let picked = pollster::block_on(
                rfd::AsyncFileDialog::new()
                    .add_filter("NIR save", &["json"])
                    .pick_file(),
            );
            match picked {
                None => DialogOutcome::ImportCancelled,
                Some(handle) => DialogOutcome::Imported(parse_imported_file(handle.path())),
            }
        }
    }
}
