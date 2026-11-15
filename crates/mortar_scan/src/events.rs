//! Events and their deterministic merge (`spec/image.md` §4.4, §4.5).
//!
//! The scan reports one [`Event`] per surviving reference of the four
//! grouped kinds. Each scan unit folds its events into one [`Acc`] per
//! [`GroupKey`] ([`UnitEvents`], I-20); [`merge`] concatenates the units'
//! sorted vectors in unit order, sorts them stably by key and reduces each
//! run (I-21). The result is the sequential meaning of §4.5 whatever the
//! schedule: per group, the three least `RefId`s and the count.

use std::collections::HashMap;

use mortar_core::par::Pool;
use mortar_core::{FileId, SymKey};

/// The four kinds of grouped reference (§4.4); ordered as declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventClass {
    Undefined,
    ComdatMismatch,
    DiscardedSection,
    GnuWarning,
}

/// A relocation record: `(FileId, section index, record index)`, ordered
/// lexicographically (§2). `section` is the index of the relocated
/// section in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefId {
    pub file: FileId,
    pub section: u32,
    pub record: u32,
}

impl Default for RefId {
    fn default() -> Self {
        Self { file: FileId(0), section: 0, record: 0 }
    }
}

/// One surviving reference (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Event {
    pub class: EventClass,
    /// [`sym_group`] of the key; for `DiscardedSection`, `(referring
    /// section index << 32) | symbol index`.
    pub group: u64,
    pub at: RefId,
}

/// What one diagnostic gathers (§4.4): `file` is `at.file` for
/// `DiscardedSection` (its group is file-relative) and 0 otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupKey {
    pub class: EventClass,
    pub file: FileId,
    pub group: u64,
}

impl Event {
    /// The group the event belongs to.
    #[must_use]
    pub fn key(&self) -> GroupKey {
        let file =
            if self.class == EventClass::DiscardedSection { self.at.file } else { FileId(0) };
        GroupKey { class: self.class, file, group: self.group }
    }
}

/// A key as a group number: `SymKey`'s own packing (`spec/model.md` §2),
/// a global its id, a local `(file + 1) << 32 | index`.
#[must_use]
pub fn sym_group(key: SymKey) -> u64 {
    match key.kind() {
        mortar_core::id::SymKind::Global(id) => u64::from(id.0),
        mortar_core::id::SymKind::Local(e) => {
            (u64::from(e.file.0) + 1) << 32 | u64::from(e.index.0)
        }
    }
}

/// The key a [`sym_group`] number names.
#[must_use]
pub fn group_sym(group: u64) -> SymKey {
    let high = (group >> 32) as u32;
    if high == 0 {
        SymKey::global(mortar_core::SymbolId(group as u32))
    } else {
        SymKey::local(mortar_core::SymEntry {
            file: FileId(high - 1),
            index: mortar_elf::SymbolIndex(group as u32),
        })
    }
}

/// The accumulator of one group (I-20): the three least references seen,
/// in `RefId` order, and the number of references, those included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acc {
    pub first: [RefId; 3],
    pub len: u8,
    pub count: u64,
}

impl Acc {
    /// The accumulator of one reference.
    #[must_use]
    pub fn of(at: RefId) -> Self {
        Self { first: [at, RefId::default(), RefId::default()], len: 1, count: 1 }
    }

    /// The references kept, in `RefId` order.
    #[must_use]
    pub fn firsts(&self) -> &[RefId] {
        &self.first[..self.len as usize]
    }

    /// Adds one reference.
    pub fn add(&mut self, at: RefId) {
        self.count += 1;
        self.keep(at);
    }

    /// Folds another accumulator of the same group into this one: the
    /// three least of both, the counts summed (I-21).
    pub fn absorb(&mut self, other: &Self) {
        self.count += other.count;
        for &at in other.firsts() {
            self.keep(at);
        }
    }

    /// Keeps `at` if it is among the three least. In a unit the events
    /// arrive in increasing order and this is an append; it is general so
    /// that the result never depends on arrival order.
    fn keep(&mut self, at: RefId) {
        let len = self.len as usize;
        let pos = self.first[..len].partition_point(|&r| r <= at);
        if pos >= 3 {
            return;
        }
        let end = len.min(2);
        self.first.copy_within(pos..end, pos + 1);
        self.first[pos] = at;
        self.len = (len + 1).min(3) as u8;
    }
}

/// The events of one scan unit (I-20): a map used for lookup only.
#[derive(Debug, Default)]
pub struct UnitEvents {
    map: HashMap<GroupKey, Acc>,
    events: u64,
}

impl UnitEvents {
    pub fn push(&mut self, e: Event) {
        self.events += 1;
        self.map.entry(e.key()).and_modify(|a| a.add(e.at)).or_insert_with(|| Acc::of(e.at));
    }

    /// The number of events pushed.
    #[must_use]
    pub fn events(&self) -> u64 {
        self.events
    }

    /// The accumulators sorted by `GroupKey` (rule 5: no hash order
    /// escapes).
    #[must_use]
    pub fn finish(self) -> Vec<(GroupKey, Acc)> {
        let mut v: Vec<(GroupKey, Acc)> = self.map.into_iter().collect();
        v.sort_unstable_by_key(|e| e.0);
        v
    }
}

/// I-21: the units' vectors, in unit order, as one accumulator per group
/// sorted by `GroupKey`. The concatenation is sorted stably, so the
/// accumulators of one group keep unit order; each run is reduced left to
/// right. `pool` sorts in parallel when there are many.
#[must_use]
pub fn merge(units: Vec<Vec<(GroupKey, Acc)>>, pool: &Pool) -> Vec<(GroupKey, Acc)> {
    let mut all: Vec<(GroupKey, Acc)> = units.into_iter().flatten().collect();
    pool.sort_by_key(&mut all, |e| e.0);
    let mut out: Vec<(GroupKey, Acc)> = Vec::new();
    for (k, a) in all {
        match out.last_mut() {
            Some((lk, la)) if *lk == k => la.absorb(&a),
            _ => out.push((k, a)),
        }
    }
    out
}
