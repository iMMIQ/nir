//! Bounded canonical verification of immutable, untrusted snapshots.
use crate::vm::{validate_dialogue, validate_task_definition};
use crate::{Snapshot, ValidatedProgram};
use nir_format::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug)]
enum DialogueRef {
    Task(u32),
    Pending(String),
}
#[derive(Debug, Default)]
struct ModuleChecks {
    tasks: Vec<u32>,
    dialogues: BTreeMap<String, Vec<DialogueRef>>,
}
#[derive(Debug)]
struct VerificationUnit {
    requirements: Vec<ContentRequirement>,
    tasks: Vec<u32>,
    dialogues: Vec<DialogueRef>,
}

/// Owns the exact snapshot being checked. Neither coverage nor its snapshot can
/// be edited by a caller, and verification never retains package bodies.
#[derive(Debug)]
pub struct RestoreSession {
    root: Arc<RuntimeProgram>,
    snapshot: Box<Snapshot>,
    digest: String,
    release: String,
    units: Vec<VerificationUnit>,
    next: usize,
}

/// An in-process, single-use proof, created only by a complete RestoreSession.
/// It contains no content views, leases, packages, or deserializable capability.
#[derive(Debug)]
pub struct VerifiedSnapshot {
    root: Arc<RuntimeProgram>,
    snapshot: Box<Snapshot>,
    digest: String,
    release: String,
}
fn fail(message: &str) -> Diagnostic {
    Diagnostic::new("E_SNAPSHOT", "restore", message)
}
fn snapshot_digest(snapshot: &Snapshot) -> Result<String> {
    let bytes = serde_json::to_vec(snapshot).map_err(|_| fail("snapshot serialization"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
impl RestoreSession {
    pub fn new(program: &ValidatedProgram, snapshot: Snapshot, release: &str) -> Result<Self> {
        let root = program
            .runtime_root_arc()
            .ok_or_else(|| fail("staged verification requires a runtime root"))?;
        if snapshot.format != SNAPSHOT_VERSION
            || snapshot.game_id != root.game_id
            || snapshot.revision != root.revision
            || snapshot.release != release
            || !root.locales.contains(&snapshot.locale)
            || snapshot.frames.is_empty()
            || snapshot.frames.len() > MAX_FRAMES
            || snapshot.tasks.len() > MAX_TASKS * 2
            || snapshot.scene.len() > MAX_NODES
            || snapshot.draft.len() > MAX_NODES
            || snapshot.history.len() > 1000
            || snapshot.next_id == 0
        {
            return Err(fail("incompatible content or state limits"));
        }
        let mut modules = BTreeMap::<String, ModuleChecks>::new();
        for (id, task) in &snapshot.tasks {
            let module = root
                .task_owners
                .get(&task.name)
                .ok_or_else(|| fail("unknown task definition"))?;
            modules.entry(module.clone()).or_default().tasks.push(*id);
            if let Some(dialogue) = &task.dialogue {
                let module = root
                    .text_owners
                    .get(&dialogue.text_id)
                    .ok_or_else(|| fail("unknown dialogue text"))?;
                modules
                    .entry(module.clone())
                    .or_default()
                    .dialogues
                    .entry(dialogue.locale.clone())
                    .or_default()
                    .push(DialogueRef::Task(*id));
            }
        }
        if let Some(pending) = &snapshot.pending {
            for (name, dialogue) in &pending.dialogues {
                let module = root
                    .text_owners
                    .get(&dialogue.text_id)
                    .ok_or_else(|| fail("unknown pending dialogue text"))?;
                modules
                    .entry(module.clone())
                    .or_default()
                    .dialogues
                    .entry(dialogue.locale.clone())
                    .or_default()
                    .push(DialogueRef::Pending(name.clone()));
            }
        }
        let mut units = Vec::new();
        for (module, mut checks) in modules {
            let static_key = ContentKey::Static {
                module: module.clone(),
            };
            let static_requirement = root
                .content_requirement(&static_key)
                .ok_or_else(|| fail("missing canonical module"))?;
            if checks.dialogues.is_empty() {
                units.push(VerificationUnit {
                    requirements: vec![static_requirement],
                    tasks: checks.tasks,
                    dialogues: Vec::new(),
                });
            } else {
                for (locale, dialogues) in checks.dialogues {
                    let text = root
                        .content_requirement(&ContentKey::Text {
                            module: module.clone(),
                            locale,
                        })
                        .ok_or_else(|| fail("missing canonical dialogue locale"))?;
                    units.push(VerificationUnit {
                        requirements: vec![static_requirement.clone(), text],
                        tasks: std::mem::take(&mut checks.tasks),
                        dialogues,
                    });
                }
            }
        }
        let digest = snapshot_digest(&snapshot)?;
        Ok(Self {
            root,
            snapshot: Box::new(snapshot),
            digest,
            release: release.into(),
            units,
            next: 0,
        })
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    /// Full unit requirements, even if these objects are already in live cache.
    /// The caller installs them into an independent, empty scratch view.
    pub fn next_requirements(&self) -> Option<Vec<ContentRequirement>> {
        self.units
            .get(self.next)
            .map(|unit| unit.requirements.clone())
    }
    pub fn verify_next_unit(&mut self, program: &ValidatedProgram) -> Result<()> {
        if !program
            .runtime_root()
            .is_some_and(|root| std::ptr::eq(root, self.root.as_ref()))
        {
            return Err(fail("verification root mismatch"));
        }
        let unit = self
            .units
            .get(self.next)
            .ok_or_else(|| fail("no pending verification unit"))?;
        let report = program.residency();
        if report.resident_bytes > MAX_INPUT_BYTES as u64 {
            return Err(Diagnostic::new(
                "E_RESIDENCY_BUDGET",
                "restore",
                "canonical verification view exceeds 16 MiB",
            ));
        }
        let mut unit_bytes = 0u64;
        for requirement in &unit.requirements {
            let block = report
                .blocks
                .iter()
                .find(|block| block.key == requirement.key && block.digest == requirement.digest)
                .ok_or_else(|| fail("canonical verification content missing"))?;
            unit_bytes = unit_bytes.saturating_add(block.encoded_bytes);
            if unit_bytes > MAX_INPUT_BYTES as u64 {
                return Err(Diagnostic::new(
                    "E_RESIDENCY_BUDGET",
                    "restore",
                    "canonical verification unit exceeds 16 MiB",
                ));
            }
        }
        for id in &unit.tasks {
            validate_task_definition(&self.snapshot.tasks[id], program.program())?;
        }
        for reference in &unit.dialogues {
            let dialogue = match reference {
                DialogueRef::Task(id) => self.snapshot.tasks[id].dialogue.as_ref().unwrap(),
                DialogueRef::Pending(name) => {
                    &self.snapshot.pending.as_ref().unwrap().dialogues[name]
                }
            };
            validate_dialogue(
                dialogue,
                program.program(),
                self.snapshot.tick_us,
                self.snapshot.next_id,
            )?;
        }
        self.next += 1;
        Ok(())
    }
    pub fn finish(self) -> Result<VerifiedSnapshot> {
        if self.next != self.units.len() {
            return Err(fail("incomplete canonical verification"));
        }
        Ok(VerifiedSnapshot {
            root: self.root,
            snapshot: self.snapshot,
            digest: self.digest,
            release: self.release,
        })
    }
}
impl VerifiedSnapshot {
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub(crate) fn consume(self, program: &ValidatedProgram, release: &str) -> Result<Snapshot> {
        if release != self.release
            || !program
                .runtime_root()
                .is_some_and(|root| std::ptr::eq(root, self.root.as_ref()))
            || snapshot_digest(&self.snapshot)? != self.digest
        {
            return Err(fail("verified snapshot identity mismatch"));
        }
        Ok(*self.snapshot)
    }
}
