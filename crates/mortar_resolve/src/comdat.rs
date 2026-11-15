//! COMDAT groups (`spec/resolve.md` §6): the keeper of each signature, the
//! lowest live `FileId` with a group of it (§6.1), and per file which of
//! its sections are dropped (§6.2) and in which group each is.

use mortar_core::par::Pool;
use mortar_core::{ComdatId, FileId, OptId};
use mortar_elf::SectionIndex;
use mortar_elf::consts::GRP_COMDAT;

use crate::problem::{Problem, What};
use crate::view::{Groups, View};

/// The live copies of every signature and the keepers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keepers {
    /// `comdat[G]` (§6.1).
    pub keeper: Vec<OptId<FileId>>,
    /// `(ComdatId, FileId)` of every live file with a group of the
    /// signature, sorted (§6.7, for `mortar_odr`).
    pub copies: Vec<(ComdatId, FileId)>,
}

/// Whether a group takes part in deduplication: `GRP_COMDAT` and no
/// unknown flag bit (§6.4: an unknown bit is refused, and the group is
/// then not merged).
pub(crate) const fn is_comdat(flags: u32) -> bool {
    flags == GRP_COMDAT
}

/// The keepers over the live files `live` (in `FileId` order).
pub(crate) fn keepers<V: View>(view: &V, live: &[FileId], pool: &Pool) -> Keepers {
    let found =
        pool.unit_cost(600).map_collect(live.len(), |u, out: &mut Vec<(ComdatId, FileId)>| {
            let file = live[u];
            let mut groups = Groups::default();
            view.groups(file, &mut groups);
            let start = out.len();
            for g in &groups.groups {
                if is_comdat(g.flags) {
                    out.push((g.signature, file));
                }
            }
            out[start..].sort_unstable();
            let mut w = start;
            for r in start..out.len() {
                if w == start || out[w - 1] != out[r] {
                    out[w] = out[r];
                    w += 1;
                }
            }
            out.truncate(w);
        });
    let mut copies: Vec<(ComdatId, FileId)> =
        (0..found.units()).flat_map(|u| found.unit(u).iter().copied()).collect();
    pool.unit_cost(17).sort_by_key(&mut copies, |c| *c);
    let mut keeper = vec![OptId::NONE; view.comdats() as usize];
    for &(sig, file) in copies.iter().rev() {
        if let Some(cell) = keeper.get_mut(sig.0 as usize) {
            *cell = OptId::some(file);
        }
    }
    Keepers { keeper, copies }
}

/// Per section of one file: its COMDAT group's signature and whether it
/// is dropped (§6.2). Reused across files.
#[derive(Debug, Default)]
pub(crate) struct SectionMap {
    /// Whether the file has any group; when not, the vectors are not
    /// sized and every section is in no group.
    pub any: bool,
    pub group: Vec<OptId<ComdatId>>,
    pub dropped: Vec<bool>,
    /// Whether a section is a member of some group (COMDAT or not).
    member: Vec<bool>,
    groups: Groups,
    sigs: Vec<(ComdatId, u32)>,
}

impl SectionMap {
    /// The group of section `index`, if it is a member of a COMDAT group.
    #[inline]
    pub fn group_of(&self, index: u32) -> OptId<ComdatId> {
        if self.any {
            self.group.get(index as usize).copied().unwrap_or(OptId::NONE)
        } else {
            OptId::NONE
        }
    }

    /// Whether section `index` is dropped for its group.
    #[inline]
    pub fn is_dropped(&self, index: u32) -> bool {
        self.any && self.dropped.get(index as usize).copied().unwrap_or(false)
    }

    /// Fills the map for `file` (with `sections` sections) from its
    /// groups and the keepers; reports refused flags and sections in two
    /// groups into `problems` when `report`.
    pub fn fill<V: View>(
        &mut self,
        view: &V,
        file: FileId,
        sections: u32,
        keeper: &[OptId<FileId>],
        report: bool,
        problems: &mut Vec<Problem>,
    ) {
        self.groups.clear();
        view.groups(file, &mut self.groups);
        self.any = !self.groups.groups.is_empty();
        if !self.any {
            return;
        }
        let n = sections as usize;
        self.group.clear();
        self.group.resize(n, OptId::NONE);
        self.dropped.clear();
        self.dropped.resize(n, false);
        self.member.clear();
        self.member.resize(n, false);
        // A second group of one signature in the file is dropped like
        // another file's copy (§6.1).
        self.sigs.clear();
        for (i, g) in self.groups.groups.iter().enumerate() {
            if is_comdat(g.flags) {
                self.sigs.push((g.signature, i as u32));
            }
        }
        self.sigs.sort_unstable();
        for i in 0..self.groups.groups.len() {
            let g = self.groups.groups[i];
            let comdat = is_comdat(g.flags);
            if !comdat && g.flags & !GRP_COMDAT != 0 && report {
                problems.push(Problem {
                    key: crate::problem::DiagKey::of(file, u64::from(g.section.0)),
                    what: What::GroupFlags { file, section: g.section, flags: g.flags },
                });
            }
            let kept = comdat && {
                let at = self.sigs.partition_point(|s| s.0 < g.signature);
                keeper.get(g.signature.0 as usize).and_then(|k| k.get()) == Some(file)
                    && self.sigs[at].1 == i as u32
            };
            let start = g.start as usize;
            for &member in &self.groups.members[start..start + g.len as usize] {
                let m = member as usize;
                if m >= n {
                    continue;
                }
                if self.member[m] {
                    if report {
                        problems.push(Problem {
                            key: crate::problem::DiagKey::of(file, u64::from(member)),
                            what: What::SectionInTwoGroups { file, section: SectionIndex(member) },
                        });
                    }
                    continue;
                }
                self.member[m] = true;
                if comdat {
                    self.group[m] = OptId::some(g.signature);
                    self.dropped[m] = !kept;
                }
            }
        }
    }
}
