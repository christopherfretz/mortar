//! The weak classes (`spec/dyn.md` D-25): which undefined weak keys of
//! an `Exec` or `Pie` are `WeakDyn` and which stay `Weak0`, decided per
//! **key** (arch.md §2.1: `target_kind` is a property of the key, never
//! of the relocation class) from every live relocation that names it,
//! before the scan's demands, which read the decision.
//!
//! [`classes`] walks the relocations of the live allocated sections of
//! the files that mention an undefined weak key (none in most links: the
//! walk costs nothing then), classes each one that names such a key with
//! `Arch::weak_use`, and decides each key from the union of its classes
//! ([`WeakUses::verdict`]). Two refusals are made here, once per key, at
//! the key's first relocation of the refused class in input order:
//! `weak-undefined-pc-in-pie` (`Pie` only) and a thread-local undefined
//! weak key in a dynamic executable (D-25, supervisor's ruling after
//! WP-040r). A static link, a static PIE included, does no walk: every
//! undefined weak key is `Weak0` there.

use mortar_arch::{Arch, DynRefusal, OutputKind, Refusal, ScanError, WeakClass};
use mortar_core::par::Pool;
use mortar_core::resolution::Def;
use mortar_core::{FileId, IdSet, SectionId, SymKind, SymbolId};
use mortar_diag::{Diagnostic, Key, RefusalClass};
use mortar_elf::consts::SHF_ALLOC;
use mortar_elf::SectionIndex;

use crate::driver::{Fault, Sec, key_at};
use crate::view::{Bufs, View};
use crate::{ScanInputs, ScanOptions};

/// What the weak classes need of the section mapping (D-25: "a writable
/// place is a place whose output section has `SHF_WRITE` after layout.md's
/// section rules, known from the section mapping, before addresses";
/// RELRO counts as writable).
pub trait SectionMap: Sync {
    /// Whether the output section that live input section `section` maps
    /// to is writable.
    fn writable(&self, section: SectionId) -> bool;
}

/// The uses of one undefined weak key, over its live relocations
/// (`spec/dyn.md` D-25's table, by [`WeakClass`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeakUses {
    pub got: bool,
    pub abs_word: bool,
    pub branch: bool,
    /// The first unfillable use in input order, as the scan key of its
    /// relocation: where `weak-undefined-pc-in-pie` is reported.
    pub unfillable: Option<Key>,
}

/// What [`WeakUses`] make of a key (D-25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakVerdict {
    /// Undefined weak, value 0.
    Weak0,
    /// In `.dynsym`, preemptible: GOT slots get `GLOB_DAT`, absolute
    /// words a symbolic record, branches a PLT entry.
    WeakDyn,
    /// `weak-undefined-pc-in-pie`, at the given key (`Pie` only).
    Refused(Key),
}

impl WeakUses {
    /// Adds one relocation's use (`None`: the relocation does not use the
    /// key); `at` is its scan key, the order of input.
    pub fn add(&mut self, class: Option<WeakClass>, at: Key) {
        match class {
            Some(WeakClass::Got) => self.got = true,
            Some(WeakClass::AbsWord) => self.abs_word = true,
            Some(WeakClass::Branch) => self.branch = true,
            Some(WeakClass::Unfillable) if self.unfillable.is_none_or(|k| at < k) => {
                self.unfillable = Some(at);
            }
            Some(WeakClass::Unfillable) | None => {}
        }
    }

    /// D-25: `WeakDyn` in `Exec` and `Pie` with a GOT use or an absolute
    /// word in a writable place and no unfillable use; in `Pie` an
    /// unfillable use is refused, once per key; `Weak0` otherwise (every
    /// output without `.dynamic`, static PIE included, and an `Exec` key
    /// with an unfillable use). `dynamic`: the output has `.dynamic`
    /// ([`ScanOptions::dynamic`]).
    #[must_use]
    pub fn verdict(&self, output: OutputKind, dynamic: bool) -> WeakVerdict {
        let dynamic = dynamic && matches!(output, OutputKind::Exec | OutputKind::Pie);
        match self.unfillable {
            Some(at) if dynamic && output == OutputKind::Pie => WeakVerdict::Refused(at),
            None if dynamic && (self.got || self.abs_word) => WeakVerdict::WeakDyn,
            _ => WeakVerdict::Weak0,
        }
    }
}

/// The decision for the whole link.
#[derive(Debug, Clone, Default)]
pub struct WeakKeys {
    /// The `WeakDyn` keys: `Demands.weak_dyn` (`spec/model.md` §3.7).
    /// With room for every key in an `Exec` or `Pie` with `.dynamic`,
    /// empty (no room) otherwise.
    pub weak_dyn: IdSet<SymbolId>,
    /// One diagnostic per refused key, at its first relocation of the
    /// refused class: `weak-undefined-pc-in-pie`, or the thread-local
    /// refusal ([`TLS_WEAK`]).
    pub refused: Vec<(Key, Diagnostic)>,
    /// The refused keys. The walk drops what the architecture's scan
    /// says of their relocations (the `Weak0` cell of `Pc` in a `Pie`,
    /// arch.md §4.3: the refusal precedes it).
    pub refused_keys: IdSet<SymbolId>,
}

/// D-25: a thread-local undefined weak key in a dynamic executable,
/// refused once per key (supervisor's ruling after WP-040r).
pub const TLS_WEAK: &str = "thread-local undefined weak symbol in a dynamic executable";

/// `spec/dyn.md` D-25: the weak classes of every undefined weak key of
/// the link, from each live relocation that names it through
/// `arch.weak_use` and `sections`. Runs after liveness and before the
/// scan's walk (the demands read [`WeakKeys::weak_dyn`]); [`crate::scan`]
/// runs it on its own view. A static link (and a static PIE) never has a
/// weak-dynamic key and does no walk.
#[must_use]
pub fn weak_classes(
    arch: &dyn Arch,
    t: &ScanInputs<'_, '_>,
    opts: &ScanOptions<'_>,
    sections: Option<&dyn SectionMap>,
) -> WeakKeys {
    let opts = ScanOptions { sections, ..*opts };
    let view = View::new(arch, t, opts);
    classes(&view, &Pool::sequential())
}

/// One use of an undefined weak key: the relocation's scan key, its
/// class, whether its class is a TLS one, and where it is (to report it).
#[derive(Debug, Clone, Copy)]
struct Use {
    id: SymbolId,
    at: Key,
    class: Option<WeakClass>,
    tls: bool,
    site: Site,
}

/// A relocation record: enough to open its section again and report it.
#[derive(Debug, Clone, Copy)]
struct Site {
    file: FileId,
    sid: SectionId,
    index: u32,
    rel: u32,
    record: u32,
}

/// What [`classes`] gathers per key.
#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    uses: WeakUses,
    unfillable: Option<Site>,
    tls: Option<(Key, Site)>,
}

/// [`weak_classes`] on the scan's own view, the files walked on `pool`
/// (one unit per file that mentions an undefined weak key).
pub(crate) fn classes(view: &View<'_, '_>, pool: &Pool) -> WeakKeys {
    let output = view.opts.output;
    if !(view.opts.dynamic && matches!(output, OutputKind::Exec | OutputKind::Pie)) {
        return WeakKeys::default();
    }
    let n = view.inputs.symbols.len();
    let mut weak = IdSet::with_len(n);
    let mut any = false;
    for i in 0..n {
        let id = SymbolId(i);
        if matches!(view.def(id), Def::Undefined { weak: true }) {
            weak.insert(id);
            any = true;
        }
    }
    let mut keys = WeakKeys {
        weak_dyn: IdSet::with_len(n),
        refused: Vec::new(),
        refused_keys: IdSet::with_len(n),
    };
    if !any {
        return keys;
    }
    let r = view.resolution;
    let files: Vec<FileId> = r
        .section_base
        .iter()
        .map(|(f, _)| f)
        .filter(|&f| {
            r.file_live.get(f).is_some_and(|l| l.is_live())
                && view.prep(f).is_some_and(|p| p.obj.is_some())
                && view.inputs.sym_map_of(f).iter().any(|&id| {
                    weak.contains(id) || r.redirect.get(id).is_some_and(|&to| weak.contains(to))
                })
        })
        .collect();
    let uses: Vec<Vec<Use>> = pool.map(files.len(), |k| file_uses(view, files[k], &weak));
    let mut acc: std::collections::BTreeMap<SymbolId, Acc> = std::collections::BTreeMap::new();
    for u in uses.iter().flatten() {
        let a = acc.entry(u.id).or_default();
        let before = a.uses.unfillable;
        a.uses.add(u.class, u.at);
        if a.uses.unfillable != before {
            a.unfillable = Some(u.site);
        }
        if u.tls && a.tls.is_none_or(|(k, _)| u.at < k) {
            a.tls = Some((u.at, u.site));
        }
    }
    let mut bufs = Bufs::default();
    for (id, a) in acc {
        let refusal = if let Some((_, site)) = a.tls {
            Some((site, ScanError::Refused(Refusal::named(RefusalClass::Feature, TLS_WEAK))))
        } else {
            match a.uses.verdict(output, view.opts.dynamic) {
                WeakVerdict::WeakDyn => {
                    keys.weak_dyn.insert(id);
                    None
                }
                WeakVerdict::Weak0 => None,
                WeakVerdict::Refused(_) => a.unfillable.map(|site| {
                    (site, ScanError::Dynamic(DynRefusal::WeakUndefinedPcInPie))
                }),
            }
        };
        if let Some((site, e)) = refusal {
            keys.refused_keys.insert(id);
            if let Ok(sec) = Sec::open(view, site.file, site.sid, site.index, site.rel, &mut bufs) {
                keys.refused.push(sec.fault(site.record, Fault::Scan(e)));
            }
        }
    }
    keys
}

/// The uses of undefined weak keys (`weak`) in the live allocated
/// sections of `file`, in input order.
fn file_uses(view: &View<'_, '_>, file: FileId, weak: &IdSet<SymbolId>) -> Vec<Use> {
    let r = view.resolution;
    let mut out = Vec::new();
    let mut bufs = Bufs::default();
    let Some(range) = r.section_base.get(file).copied() else { return out };
    for (k, sid) in range.iter().enumerate() {
        let Some(row) = r.sections.get(sid) else { continue };
        if row.reloc.0 == 0 || !view.liveness.is_live(sid) {
            continue;
        }
        let index = k as u32;
        let alloc = view
            .prep(file)
            .and_then(|p| p.obj)
            .and_then(|o| o.section(SectionIndex(index)).ok())
            .is_some_and(|s| s.flags() & SHF_ALLOC != 0);
        if !alloc {
            continue;
        }
        // A section that cannot be read is the walk's diagnostic.
        let Ok(sec) = Sec::open(view, file, sid, index, row.reloc.0, &mut bufs) else { continue };
        let writable = view.opts.sections.map_or(sec.place.writable, |m| m.writable(sid));
        let n = u32::try_from(sec.recs.records.len()).unwrap_or(u32::MAX);
        for i in 0..n {
            let Some(raw) = sec.recs.records.raw(i) else { continue };
            let Some(key) = view.key_of(file, sec.recs.table, raw.r_sym) else { continue };
            let SymKind::Global(id) = key.kind() else { continue };
            if !weak.contains(id) {
                continue;
            }
            let site = mortar_arch::Site { bytes: sec.recs.bytes, offset: raw.r_offset };
            let tls = view.arch.classify(raw.r_type, site).is_ok_and(|h| h.class.is_tls());
            let class = view.arch.weak_use(raw.r_type, sec.recs.bytes, raw.r_offset, writable);
            out.push(Use {
                id,
                at: key_at(file, index, i),
                class,
                tls,
                site: Site { file, sid, index, rel: row.reloc.0, record: i },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{WeakUses, WeakVerdict};
    use mortar_arch::{OutputKind, WeakClass};
    use mortar_diag::Key;

    // spec: dyn.md D-25 (the per-key decision)
    #[test]
    fn d25_verdicts() {
        let (k1, k2) = (Key::new(70, 1, 5), Key::new(70, 2, 0));
        let mut got = WeakUses::default();
        got.add(Some(WeakClass::Got), k1);
        got.add(Some(WeakClass::Branch), k1);
        got.add(None, k1);
        assert_eq!(got.verdict(OutputKind::Exec, true), WeakVerdict::WeakDyn);
        assert_eq!(got.verdict(OutputKind::Pie, true), WeakVerdict::WeakDyn);
        assert_eq!(got.verdict(OutputKind::Static, false), WeakVerdict::Weak0);
        assert_eq!(got.verdict(OutputKind::Pie, false), WeakVerdict::Weak0, "static PIE");
        let mut branch = WeakUses::default();
        branch.add(Some(WeakClass::Branch), k1);
        assert_eq!(branch.verdict(OutputKind::Exec, true), WeakVerdict::Weak0, "no GOT, no word");
        let mut pc = got;
        pc.add(Some(WeakClass::Unfillable), k2);
        pc.add(Some(WeakClass::Unfillable), k1);
        assert_eq!(pc.verdict(OutputKind::Pie, true), WeakVerdict::Refused(k1), "first in input");
        assert_eq!(pc.verdict(OutputKind::Exec, true), WeakVerdict::Weak0);
        assert_eq!(pc.verdict(OutputKind::Pie, false), WeakVerdict::Weak0, "static PIE");
    }
}
