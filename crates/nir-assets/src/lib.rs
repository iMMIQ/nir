//! Joint admission and leases. Physical allocation stays in the device owner.
#![forbid(unsafe_code)]
use nir_format::{Diagnostic, Result};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation {
    pub session: u32,
    pub device: u32,
    pub surface: u32,
    pub typography: u32,
    pub language: u32,
}
#[derive(Debug, Default)]
struct Entry {
    bytes: u64,
    pins: usize,
}
#[derive(Debug)]
struct Ledger {
    limit: u64,
    used: u64,
    items: BTreeMap<String, Entry>,
}
#[derive(Debug, Clone)]
pub struct BudgetLedger(Rc<RefCell<Ledger>>);
impl BudgetLedger {
    pub fn new(limit: u64) -> Self {
        Self(Rc::new(RefCell::new(Ledger {
            limit,
            used: 0,
            items: BTreeMap::new(),
        })))
    }
    pub fn reserve(&self, assets: &BTreeMap<String, u64>) -> Result<Reservation> {
        let mut ledger = self.0.borrow_mut();
        let mut extra = 0u64;
        for (id, cost) in assets {
            if !ledger.items.contains_key(id) {
                extra = extra
                    .checked_add(*cost)
                    .ok_or_else(|| Diagnostic::new("E_BUDGET", "reserve", "cost overflow"))?;
            }
        }
        if extra > ledger.limit.saturating_sub(ledger.used) {
            return Err(Diagnostic::new(
                "E_BUDGET",
                "reserve",
                format!(
                    "requires {} bytes; {} available",
                    extra,
                    ledger.limit.saturating_sub(ledger.used)
                ),
            ));
        }
        for (id, cost) in assets {
            let e = ledger.items.entry(id.clone()).or_insert(Entry {
                bytes: *cost,
                pins: 0,
            });
            e.pins += 1;
        }
        ledger.used += extra;
        Ok(Reservation {
            ledger: self.clone(),
            ids: assets.keys().cloned().collect(),
        })
    }
    pub fn used(&self) -> u64 {
        self.0.borrow().used
    }
    pub fn pins(&self) -> usize {
        self.0.borrow().items.values().map(|e| e.pins).sum()
    }
}
#[derive(Debug)]
pub struct Reservation {
    ledger: BudgetLedger,
    ids: BTreeSet<String>,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut l = self.ledger.0.borrow_mut();
        for id in &self.ids {
            if let Some(e) = l.items.get_mut(id) {
                e.pins -= 1;
                if e.pins == 0 {
                    let bytes = e.bytes;
                    l.items.remove(id);
                    l.used -= bytes;
                }
            }
        }
    }
}
#[derive(Debug)]
pub struct ReadyLease {
    pub activation: u32,
    pub generation: Generation,
    reservation: Reservation,
}
impl ReadyLease {
    pub fn assets(&self) -> &BTreeSet<String> {
        &self.reservation.ids
    }
    pub fn valid(&self, activation: u32, generation: Generation) -> bool {
        self.activation == activation && self.generation == generation
    }
}
#[derive(Debug)]
pub struct PrepareJob {
    pub activation: u32,
    pub generation: Generation,
    pub missing: BTreeSet<String>,
    reservation: Reservation,
}
impl PrepareJob {
    pub fn new(
        activation: u32,
        generation: Generation,
        assets: BTreeMap<String, u64>,
        ledger: &BudgetLedger,
    ) -> Result<Self> {
        let reservation = ledger.reserve(&assets)?;
        Ok(Self {
            activation,
            generation,
            missing: assets.into_keys().collect(),
            reservation,
        })
    }
    pub fn ready(&mut self, id: &str, generation: Generation) -> bool {
        generation == self.generation && self.missing.remove(id)
    }
    pub fn finish(self) -> Result<ReadyLease> {
        if !self.missing.is_empty() {
            return Err(Diagnostic::new(
                "E_NOT_READY",
                "lease",
                "incomplete resource group",
            ));
        }
        Ok(ReadyLease {
            activation: self.activation,
            generation: self.generation,
            reservation: self.reservation,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joint_admission_and_drop() {
        let l = BudgetLedger::new(100);
        let a = l.reserve(&BTreeMap::from([("a".into(), 60)])).unwrap();
        assert!(l.reserve(&BTreeMap::from([("b".into(), 60)])).is_err());
        assert_eq!(l.used(), 60);
        let b = l.reserve(&BTreeMap::from([("a".into(), 60)])).unwrap();
        drop(a);
        assert_eq!(l.used(), 60);
        drop(b);
        assert_eq!(l.used(), 0);
    }
    #[test]
    fn stale_generation_not_ready() {
        let g = Generation {
            session: 1,
            device: 1,
            surface: 1,
            typography: 1,
            language: 1,
        };
        let mut j = PrepareJob::new(
            1,
            g,
            BTreeMap::from([("a".into(), 1)]),
            &BudgetLedger::new(100),
        )
        .unwrap();
        let mut old = g;
        old.device = 0;
        assert!(!j.ready("a", old));
        assert!(j.finish().is_err());
    }
}
