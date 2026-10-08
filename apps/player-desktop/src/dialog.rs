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
/// Runs `task` on a new thread; the returned receiver yields one outcome.
pub(crate) fn open(task: DialogTask) -> Result<PendingDialog> {
    let failure = task.failure();
    let (done, replies) = mpsc::channel::<DialogOutcome>();
    thread::Builder::new()
        .name("nir-dialog".into())
        .spawn(move || {
            let outcome = run(task);
            let _ = done.send(outcome);
        })
        .map_err(|_| anyhow!("E_DIALOG_THREAD"))?;
    Ok(PendingDialog { replies, failure })
}
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
                Some(handle) => DialogOutcome::Imported(
                    fs::read(handle.path())
                        .map_err(anyhow::Error::from)
                        .and_then(|bytes| Ok(nir_content::parse(&bytes, "imported save")?))
                        .map(Box::new)
                        .map_err(|e| e.to_string()),
                ),
            }
        }
    }
}
