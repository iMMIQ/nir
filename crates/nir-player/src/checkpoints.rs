use nir_core::Snapshot;
use std::{io, ops::Index};

const MAX_COUNT: usize = 32;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Immutable rollback snapshots, with their exact JSON sizes measured once.
/// The ledger reserves twice this serialized budget for retained snapshots.
#[derive(Clone, Default)]
pub(super) struct Checkpoints {
    entries: Vec<(Snapshot, usize)>,
    bytes: usize,
}

impl Checkpoints {
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn push(&mut self, snapshot: Snapshot) {
        let bytes = serialized_bytes(&snapshot);
        self.bytes = self.bytes.checked_add(bytes).expect("E_CHECKPOINT_SIZE");
        self.entries.push((snapshot, bytes));
        while self.entries.len() > MAX_COUNT || self.bytes > MAX_BYTES {
            self.bytes -= self.entries.remove(0).1;
        }
    }

    pub(super) fn pop(&mut self) -> Option<Snapshot> {
        self.entries.pop().map(|(snapshot, bytes)| {
            self.bytes -= bytes;
            snapshot
        })
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

impl Index<usize> for Checkpoints {
    type Output = Snapshot;

    fn index(&self, index: usize) -> &Snapshot {
        &self.entries[index].0
    }
}

/// Count the same bytes serde_json would write, without allocating a second
/// copy of the snapshot's JSON solely to enforce the rollback budget.
fn serialized_bytes(snapshot: &Snapshot) -> usize {
    struct Counter(usize);
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| io::Error::other("E_CHECKPOINT_SIZE"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, snapshot).expect("E_CHECKPOINT_SERIALIZE");
    counter.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_core::{Core, ValidatedProgram};
    use nir_format::{Micros, Value};

    fn snapshot(index: u64, payload: String) -> Snapshot {
        let mut program: nir_format::Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program
            .variables
            .insert("payload".into(), Value::String(String::new()));
        let core = Core::new(
            ValidatedProgram::new(program).unwrap(),
            "release".into(),
            "zh-Hans".into(),
        )
        .unwrap();
        let mut snapshot = core.snapshot();
        snapshot.tick_us = Micros(index);
        snapshot
            .variables
            .insert("payload".into(), Value::String(payload));
        snapshot
    }

    #[test]
    fn byte_count_matches_real_json_and_count_eviction_keeps_the_same_snapshots() {
        let mut checkpoints = Checkpoints::default();
        let mut previous = vec![];
        for index in 0..45 {
            let snapshot = snapshot(index, "\"\\\n雨🙂".repeat(index as usize));
            assert_eq!(
                serialized_bytes(&snapshot),
                serde_json::to_vec(&snapshot).unwrap().len()
            );
            previous.push(snapshot.clone());
            // This is the prior retention behavior, used as the oracle.
            while previous.len() > MAX_COUNT
                || previous
                    .iter()
                    .map(|s| serde_json::to_vec(s).unwrap().len())
                    .sum::<usize>()
                    > MAX_BYTES
            {
                previous.remove(0);
            }
            checkpoints.push(snapshot);
            assert_eq!(checkpoints.len(), previous.len());
            assert_eq!(
                checkpoints.bytes,
                previous
                    .iter()
                    .map(|s| serde_json::to_vec(s).unwrap().len())
                    .sum::<usize>()
            );
            for (index, expected) in previous.iter().enumerate() {
                assert_eq!(
                    serde_json::to_value(&checkpoints[index]).unwrap(),
                    serde_json::to_value(expected).unwrap()
                );
            }
        }
    }

    #[test]
    fn byte_eviction_clone_pop_and_clear_keep_independent_accounting() {
        let mut checkpoints = Checkpoints::default();
        for index in 0..12 {
            checkpoints.push(snapshot(index, "x".repeat(2 * 1024 * 1024)));
        }
        assert_eq!(checkpoints.len(), 7);
        assert_eq!(checkpoints[0].tick_us.0, 5);
        assert!(checkpoints.bytes <= MAX_BYTES);
        let mut frozen = checkpoints.clone();
        let previous_bytes = frozen.bytes;
        let last = frozen.pop().unwrap();
        assert_eq!(last.tick_us.0, 11);
        assert_eq!(frozen.bytes, previous_bytes - serialized_bytes(&last));
        checkpoints.clear();
        assert_eq!(checkpoints.len(), 0);
        assert_eq!(checkpoints.bytes, 0);
        assert_eq!(frozen.len(), 6);
        frozen.push(last);
        assert_eq!(frozen.bytes, previous_bytes);
        // The old budget also discarded all older points when an individual
        // snapshot exceeded the entire budget. Preserve that boundary.
        frozen.push(snapshot(12, "x".repeat(MAX_BYTES + 1)));
        assert_eq!(frozen.len(), 0);
        assert_eq!(frozen.bytes, 0);
    }

    #[test]
    #[ignore = "diagnostic serialization benchmark; requires NIR_CHECKPOINT_BENCH_OUT"]
    fn checkpoint_budget_serialization_benchmark() {
        use std::{hint::black_box, time::Instant};
        let output =
            std::env::var_os("NIR_CHECKPOINT_BENCH_OUT").expect("NIR_CHECKPOINT_BENCH_OUT");
        let base = snapshot(0, "x".repeat(256 * 1024));
        let mut timings = vec![];
        for repetition in 0..3 {
            let started = Instant::now();
            let mut old: Vec<Snapshot> = vec![];
            let mut old_serializations = 0;
            let mut old_json_bytes = 0;
            for index in 0..128 {
                let mut next = base.clone();
                next.tick_us = nir_format::Micros(index);
                old.push(next);
                while old.len() > MAX_COUNT
                    || old
                        .iter()
                        .map(|s| {
                            old_serializations += 1;
                            let size = serde_json::to_vec(s).unwrap().len();
                            old_json_bytes += size;
                            size
                        })
                        .sum::<usize>()
                        > MAX_BYTES
                {
                    old.remove(0);
                }
            }
            black_box(&old);
            let old_seconds = started.elapsed().as_secs_f64();
            let started = Instant::now();
            let mut cached = Checkpoints::default();
            for index in 0..128 {
                let mut next = base.clone();
                next.tick_us = nir_format::Micros(index);
                cached.push(next);
            }
            black_box(&cached);
            let cached_seconds = started.elapsed().as_secs_f64();
            assert_eq!(cached.len(), old.len());
            for (index, expected) in old.iter().enumerate() {
                assert_eq!(
                    serde_json::to_value(&cached[index]).unwrap(),
                    serde_json::to_value(expected).unwrap()
                );
            }
            assert_eq!(old_serializations, 3600);
            timings.push(
                serde_json::json!({"repetition":repetition,"old_seconds":old_seconds,
                "cached_seconds":cached_seconds,"old_serializations":old_serializations,
                "cached_serializations":128,"old_temporary_json_length_sum":old_json_bytes,
                "retained_count":cached.len(),"retained_serialized_bytes":cached.bytes}),
            );
        }
        let report = serde_json::json!({"format":1,"status":"passed-checkpoint-budget-algorithm-scope",
            "inputs":128,"payload_bytes":256*1024,"max_count":MAX_COUNT,"max_bytes":MAX_BYTES,
            "samples":timings,"limitations":["Synthetic snapshots and debug test binary; no input-to-pixel or device budget claim.",
                "Other live jobs may share the machine; timings are diagnostic.",
                "Counts concern budget serialization; snapshot cloning and other player work remain."]});
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
