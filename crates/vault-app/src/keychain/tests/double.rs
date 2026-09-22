//! A scripted stand-in for the credential store and the vault's files, so
//! every failure and every crash point of the key lifecycle can be driven
//! (ADR-SEC-029 "Tests, written first").
//!
//! One shared [`World`]; a [`Double`] is one process looking at it. A
//! crash is modelled by a budget of mutating steps: once spent, that
//! process can do nothing more — every call fails, reads included — exactly
//! as if it had died there. A fresh `Double` over the same world is the
//! next process.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeroize::Zeroizing;

use crate::keychain::files::{CleanReport, Marker, VaultFiles};
use crate::keychain::lifecycle::{Ctx, Policy};
use crate::keychain::store::{KeyStore, Slot, StoreError, Stored};

/// A key the tests use.
pub(super) const K: [u8; 32] = [7u8; 32];

/// The names a vault folder holds that count as data sealed under a key.
pub(super) const KEYED_NAMES: &[&str] = &[
    "vault.db",
    "vault.db-wal",
    "vault.db-shm",
    "lance",
    "graph.duckdb",
    "graph.sealed",
    "reports",
];

/// One stored credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Cred {
    pub(super) bytes: Vec<u8>,
    pub(super) local: bool,
}

/// One kind of step, for failure injection and the step log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Op {
    Read(SlotName),
    IsLocal(SlotName),
    Write(SlotName),
    Delete(SlotName),
    MarkerRead,
    WriteMarker,
    RemoveMarker,
    KeyedCheck,
    Clean,
}

/// `Slot` with an ordering, for [`Op`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SlotName {
    Main,
    Spare,
}

impl From<Slot> for SlotName {
    fn from(s: Slot) -> Self {
        match s {
            Slot::Main => SlotName::Main,
            Slot::Spare => SlotName::Spare,
        }
    }
}

/// The marker as the world holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MarkerFile {
    Valid(PathBuf),
    Invalid,
}

/// Everything the processes share.
#[derive(Debug, Default)]
pub(super) struct World {
    pub(super) main: Option<Cred>,
    pub(super) spare: Option<Cred>,
    pub(super) marker: Option<MarkerFile>,
    /// Folder → the entry names in it.
    pub(super) folders: BTreeMap<PathBuf, BTreeSet<String>>,
    /// Entries that cannot be removed (an open file, say).
    pub(super) locked: BTreeSet<PathBuf>,
    /// This process's own vault folder.
    pub(super) own: PathBuf,
    /// The next N reads of main answer "no entry" although it exists.
    pub(super) phantom_absent_reads: u32,
    /// Each listed step fails the next time it runs (then is removed).
    pub(super) fail_once: Vec<Op>,
    /// Each listed step fails every time.
    pub(super) fail_always: Vec<Op>,
    /// Writes to these slots store different bytes than asked.
    pub(super) corrupt_writes: Vec<SlotName>,
    /// Writes do not take Local persistence.
    pub(super) writes_stay_enterprise: bool,
    /// Deletes of these slots report success but leave the credential.
    pub(super) sticky_deletes: Vec<SlotName>,
    /// Every step, in order.
    pub(super) log: Vec<Op>,
}

impl World {
    /// A fresh computer: no key, an empty vault folder at `/vault`.
    pub(super) fn fresh() -> Self {
        let own = own_folder();
        let mut folders = BTreeMap::new();
        folders.insert(own.clone(), BTreeSet::new());
        Self {
            own,
            folders,
            ..Self::default()
        }
    }

    /// Main holds `K`, Enterprise (as every key written before ADR-SEC-029).
    pub(super) fn with_enterprise_key(mut self) -> Self {
        self.main = Some(Cred {
            bytes: K.to_vec(),
            local: false,
        });
        self
    }

    /// Main holds `K`, Local.
    pub(super) fn with_local_key(mut self) -> Self {
        self.main = Some(Cred {
            bytes: K.to_vec(),
            local: true,
        });
        self
    }

    /// The own folder holds a vault's data and its operational files.
    pub(super) fn with_vault_data(mut self) -> Self {
        let own = self.own.clone();
        let names = self.folders.entry(own).or_default();
        for n in [
            "vault.db",
            "vault.db-wal",
            "lance",
            "graph.sealed",
            "reports",
        ] {
            names.insert(n.to_owned());
        }
        for n in [".acl-v1", ".vault-host.json", "maintenance.json"] {
            names.insert(n.to_owned());
        }
        self
    }

    /// The own folder holds only files written before any key exists.
    pub(super) fn with_operational_files_only(mut self) -> Self {
        let own = self.own.clone();
        let names = self.folders.entry(own).or_default();
        for n in [".acl-v1", ".vault-host.json", "maintenance.json", ".keeper"] {
            names.insert(n.to_owned());
        }
        self
    }

    /// Whether the own folder still holds data sealed under a key.
    pub(super) fn own_keyed(&self) -> bool {
        self.folders
            .get(&self.own)
            .is_some_and(|names| names.iter().any(|n| KEYED_NAMES.contains(&n.as_str())))
    }

    /// How many writes of any credential happened.
    pub(super) fn writes(&self) -> usize {
        self.log
            .iter()
            .filter(|op| matches!(op, Op::Write(_)))
            .count()
    }
}

/// One process over a shared world.
pub(super) struct Double {
    world: Arc<Mutex<World>>,
    /// Mutating steps this process may take before it "dies"; `None` = no
    /// crash.
    budget: Mutex<Option<usize>>,
    dead: Mutex<bool>,
}

impl Double {
    pub(super) fn new(world: &Arc<Mutex<World>>) -> Self {
        Self {
            world: Arc::clone(world),
            budget: Mutex::new(None),
            dead: Mutex::new(false),
        }
    }

    /// A process that dies after `steps` mutating steps.
    pub(super) fn crashing_after(world: &Arc<Mutex<World>>, steps: usize) -> Self {
        let d = Self::new(world);
        *d.budget.lock().unwrap() = Some(steps);
        d
    }

    /// Whether this process died.
    pub(super) fn died(&self) -> bool {
        *self.dead.lock().unwrap()
    }

    /// A lifecycle context over this process, with no pauses.
    pub(super) fn ctx(&self) -> Ctx<'_> {
        Ctx {
            store: self,
            files: self,
            policy: Policy {
                reads: 5,
                delay: Duration::ZERO,
            },
        }
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    /// Enter a step: dead processes do nothing; a mutating step spends the
    /// budget; scripted failures fire.
    fn step(&self, op: Op, mutating: bool) -> Result<(), String> {
        if *self.dead.lock().unwrap() {
            return Err("the process has died".into());
        }
        if mutating {
            let mut budget = self.budget.lock().unwrap();
            if let Some(left) = budget.as_mut() {
                if *left == 0 {
                    *self.dead.lock().unwrap() = true;
                    return Err("the process died here".into());
                }
                *left -= 1;
            }
        }
        let mut w = self.world();
        w.log.push(op);
        if w.fail_always.contains(&op) {
            return Err(format!("scripted failure: {op:?}"));
        }
        if let Some(i) = w.fail_once.iter().position(|o| *o == op) {
            w.fail_once.remove(i);
            return Err(format!("scripted failure: {op:?}"));
        }
        Ok(())
    }
}

fn slot_mut(w: &mut World, slot: Slot) -> &mut Option<Cred> {
    match slot {
        Slot::Main => &mut w.main,
        Slot::Spare => &mut w.spare,
    }
}

impl KeyStore for Double {
    fn read(&self, slot: Slot) -> Result<Stored, StoreError> {
        self.step(Op::Read(slot.into()), false)
            .map_err(StoreError)?;
        let mut w = self.world();
        if slot == Slot::Main && w.phantom_absent_reads > 0 {
            w.phantom_absent_reads -= 1;
            return Ok(Stored::Absent);
        }
        Ok(match slot_mut(&mut w, slot) {
            Some(c) => Stored::Present(Zeroizing::new(c.bytes.clone())),
            None => Stored::Absent,
        })
    }

    fn is_local(&self, slot: Slot) -> Result<Option<bool>, StoreError> {
        self.step(Op::IsLocal(slot.into()), false)
            .map_err(StoreError)?;
        let mut w = self.world();
        Ok(slot_mut(&mut w, slot).as_ref().map(|c| c.local))
    }

    fn write_local(&self, slot: Slot, key: &[u8; 32]) -> Result<(), StoreError> {
        self.step(Op::Write(slot.into()), true)
            .map_err(StoreError)?;
        let mut w = self.world();
        let mut bytes = key.to_vec();
        if w.corrupt_writes.contains(&slot.into()) {
            bytes[0] ^= 0xFF;
        }
        let local = !w.writes_stay_enterprise;
        *slot_mut(&mut w, slot) = Some(Cred { bytes, local });
        Ok(())
    }

    fn delete(&self, slot: Slot) -> Result<bool, StoreError> {
        self.step(Op::Delete(slot.into()), true)
            .map_err(StoreError)?;
        let mut w = self.world();
        let sticky = w.sticky_deletes.contains(&slot.into());
        let cred = slot_mut(&mut w, slot);
        let existed = cred.is_some();
        if !sticky {
            *cred = None;
        }
        Ok(existed)
    }
}

impl VaultFiles for Double {
    fn keyed_data_present(&self) -> io::Result<bool> {
        self.step(Op::KeyedCheck, false).map_err(io::Error::other)?;
        Ok(self.world().own_keyed())
    }

    fn marker(&self) -> io::Result<Marker> {
        self.step(Op::MarkerRead, false).map_err(io::Error::other)?;
        Ok(match &self.world().marker {
            None => Marker::None,
            Some(MarkerFile::Valid(p)) => Marker::Valid(p.clone()),
            Some(MarkerFile::Invalid) => Marker::Invalid,
        })
    }

    fn write_marker(&self, folder: &Path) -> io::Result<()> {
        self.step(Op::WriteMarker, true).map_err(io::Error::other)?;
        self.world().marker = Some(MarkerFile::Valid(folder.to_path_buf()));
        Ok(())
    }

    fn remove_marker(&self) -> io::Result<()> {
        self.step(Op::RemoveMarker, true)
            .map_err(io::Error::other)?;
        self.world().marker = None;
        Ok(())
    }

    fn clean(&self, folder: &Path) -> CleanReport {
        let names: Vec<String> = self
            .world()
            .folders
            .get(folder)
            .map(|n| n.iter().cloned().collect())
            .unwrap_or_default();
        let mut report = CleanReport::default();
        for name in names {
            let path = folder.join(&name);
            // Each removal is a mutating step; a failed step leaves it.
            if self.step(Op::Clean, true).is_err() || self.world().locked.contains(&path) {
                report.left.push(path);
                continue;
            }
            if let Some(names) = self.world().folders.get_mut(folder) {
                names.remove(&name);
            }
            report.removed += 1;
        }
        report
    }

    fn is_own_folder(&self, folder: &Path) -> bool {
        folder == self.world().own
    }
}

/// The double's own vault folder.
pub(super) fn own_folder() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\vault" } else { "/vault" })
}

/// A shared world.
pub(super) fn shared(world: World) -> Arc<Mutex<World>> {
    Arc::new(Mutex::new(world))
}
