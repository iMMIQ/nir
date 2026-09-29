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
use crate::atomic_write;
use anyhow::{anyhow, Result};
use nir_player::SaveEnvelope;
use std::{fs, sync::mpsc, thread};

pub(crate) enum DialogTask {
    /// Export a save as JSON; the chosen path is written on this thread.
    Export { json: String },
    /// Import a save file; the picked bytes are parsed on this thread.
    Import,
}
pub(crate) enum DialogOutcome {
    /// The export finished. A cancelled dialog reports success; write
    /// errors were fatal via `?` before and stay fatal.
    Exported(std::result::Result<(), String>),
    /// A cancelled import feeds no engine event, matching the synchronous
    /// behaviour.
    ImportCancelled,
    /// A picked import file, parsed on the dialog thread.
    Imported(std::result::Result<Box<SaveEnvelope>, String>),
}
/// Runs `task` on a new thread; the returned receiver yields one outcome.
pub(crate) fn open(task: DialogTask) -> Result<mpsc::Receiver<DialogOutcome>> {
    let (done, replies) = mpsc::channel::<DialogOutcome>();
    thread::Builder::new()
        .name("nir-dialog".into())
        .spawn(move || {
            let outcome = run(task);
            let _ = done.send(outcome);
        })
        .map_err(|_| anyhow!("E_DIALOG_THREAD"))?;
    Ok(replies)
}
fn run(task: DialogTask) -> DialogOutcome {
    match task {
        DialogTask::Export { json } => {
            let picked = pollster::block_on(
                rfd::AsyncFileDialog::new()
                    .set_file_name("save.nir-save.json")
                    .save_file(),
            );
            match picked {
                Some(handle) => DialogOutcome::Exported(
                    atomic_write(handle.path(), json.as_bytes()).map_err(|e| e.to_string()),
                ),
                None => DialogOutcome::Exported(Ok(())),
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
