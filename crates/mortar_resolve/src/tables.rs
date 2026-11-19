//! Stage B of WP-022: resolution over the model's tables. [`InputsView`]
//! is the [`View`] of `mortar_core::Inputs`; [`options`] reads the `Plan`;
//! [`resolution`] writes the `Resolution` table (`spec/model.md` §3.3);
//! [`diagnostics`] turns the problems into `mortar_diag` diagnostics
//! (`spec/resolve.md` §14). [`resolve_inputs`] runs the four.
//!
//! `undefined-symbol` is not emitted here: the scan reports one per key at
//! phase 70 from the verdict this module writes into `SymProps`
//! (`spec/resolve.md` §5.1, §5.2).

use mortar_core::hash::hash_bytes;
use std::collections::HashMap;

use mortar_core::inputs::{
    DirectiveKind, EdgeOutcome, FileKind as InputKind, InputFlags, Inputs, OpenedKind, Origin,
    Reader,
};
use mortar_core::intern::{InternKey, SymName};
use mortar_core::par::Pool;
use mortar_core::plan::{OutputKind, Plan, Strip, SymbolRole, UnresolvedSymbols};
use mortar_core::resolution::{
    Assigned, Def as TableDef, InternalDef, RefFlags, Resolution, SymProps, Verdict as TableVerdict,
};
use mortar_core::{ComdatId, FileId, IdSet, OptId, SymEntry, SymbolId};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, RefusalClass, Relation};
use mortar_elf::consts::{STB_GLOBAL, STB_LOCAL, STB_WEAK, STT_SECTION};
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

use crate::index::HashIndex;
use crate::options::DefsymValue;
use crate::options::{
    Entry, MentionGroup, Named, Options, Output, SharedLink, SharedMention, SharedOutput,
    ShlibUndefined, Wrap, glob_match, is_glob,
};
use crate::problem::{DefPlace, DefsymWhy, MalformedSymbol, Problem, What};
use crate::resolve::{Def, Resolved, flags, resolve_steps};
use crate::sections::SectionRefusal;
use crate::steps::Steps;
use crate::verdict::{SymtabEntry, Unresolved, Verdict, symtab_entry, verdict};
use crate::view::{FileInfo, FileKind, Groups, Mention, SectionInfo, SharedInfo, View};
use mortar_core::plan::DefsymExpr;

/// One `SHT_GROUP` of a file, decoded once.
#[derive(Debug, Clone, Copy)]
struct GroupRow {
    section: SectionIndex,
    flags: u32,
    signature: ComdatId,
    /// Its members: `members[start..start + len]` of the view.
    start: u32,
    len: u32,
}

/// A fault the view found decoding a file the input phase accepted: the
/// mention or group is left out and the link fails (`malformed-input`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fault {
    pub file: FileId,
    pub what: String,
}

/// What one file contributes to the view, computed once.
#[derive(Default)]
struct PerFile<'s> {
    symtab: Option<SymbolTable<'s>>,
    groups: Vec<GroupRow>,
    members: Vec<u32>,
    /// `(st_index, the row foo@V)` for each definition `foo@@V`.
    also: Vec<(u32, SymbolId)>,
    faults: Vec<String>,
    /// A shared object's `.gnu.version` (`spec/resolve.md` §9.2).
    versym: Option<mortar_elf::Table<'s, mortar_elf::Versym>>,
}

/// The [`View`] of `Inputs` (`spec/model.md` §3.2).
#[derive(Debug)]
pub struct InputsView<'a, 's> {
    inputs: &'a Inputs<'s>,
    info: Vec<FileInfo>,
    symtab: Vec<Option<SymbolTable<'s>>>,
    groups: Vec<GroupRow>,
    group_members: Vec<u32>,
    /// Per file, its rows of `groups`: `groups[group_at[f]..group_at[f + 1]]`.
    group_at: Vec<u32>,
    also: Vec<(FileId, u32, SymbolId)>,
    symbols: HashIndex,
    faults: Vec<Fault>,
    /// Per file: a shared object's `.gnu.version`.
    versym: Vec<Option<mortar_elf::Table<'s, mortar_elf::Versym>>>,
    /// Per file: what a shared object is to the link.
    shared: Vec<Option<SharedInfo>>,
}

impl<'a, 's> InputsView<'a, 's> {
    /// The view of `inputs`: one parallel pass decodes every file's
    /// symbol table and groups.
    #[must_use]
    pub fn new(inputs: &'a Inputs<'s>, pool: &Pool) -> Self {
        Self::new_steps(inputs, pool, &mut Steps::default())
    }

    /// [`InputsView::new`], recording where the time went.
    pub fn new_steps(inputs: &'a Inputs<'s>, pool: &Pool, steps: &mut Steps) -> Self {
        steps.restart();
        // The keys' hashes are the input phase's (`SymbolKey::hash`).
        let hashes: Vec<u64> = inputs.symbols.as_slice().iter().map(|k| k.hash).collect();
        debug_assert!(
            inputs.symbols.as_slice().iter().all(|k| k.hash == sym_hash(k.name, k.version)),
            "a symbol key's hash is not its name's"
        );
        let symbols = HashIndex::new(&hashes);
        drop(hashes);
        steps.step("view.symbol_lookup");
        let names = inputs.comdats.as_slice();
        let comdat_hashes = pool.unit_cost(15).map(names.len(), |c| hash_bytes(names[c].name));
        let comdats = HashIndex::new(&comdat_hashes);
        drop(comdat_hashes);
        steps.step("view.comdat_lookup");
        let n = inputs.files.len() as usize;
        let per: Vec<PerFile<'s>> =
            pool.unit_cost(1400).map(n, |f| per_file(inputs, &symbols, &comdats, FileId(f as u32)));
        steps.step("view.per_file");
        let sources = sources(inputs);
        let mut info = Vec::with_capacity(n);
        let mut symtab = Vec::with_capacity(n);
        let mut groups = Vec::new();
        let mut group_members = Vec::new();
        let mut group_at = Vec::with_capacity(n + 1);
        let mut also = Vec::new();
        let mut faults = Vec::new();
        let mut versym = Vec::with_capacity(n);
        let mut shared = Vec::with_capacity(n);
        for (f, p) in per.into_iter().enumerate() {
            let id = FileId(f as u32);
            let file = &inputs.files[id];
            versym.push(p.versym);
            shared.push((file.kind() == InputKind::Shared).then(|| SharedInfo {
                closure: matches!(file.origin, Origin::Closure { .. }),
                as_needed: inputs.soname_unit(id).map_or_else(
                    || file.state.contains(InputFlags::AS_NEEDED),
                    |unit| unit.as_needed,
                ),
            }));
            info.push(FileInfo {
                kind: kind_of(file),
                archive: file.member.map_or(OptId::NONE, |m| OptId::some(m.archive)),
                group: inputs.group_of(id).map(|g| g.0),
                sections: file.sections,
                source: sources[f],
            });
            symtab.push(p.symtab);
            group_at.push(groups.len() as u32);
            let base = group_members.len() as u32;
            groups.extend(p.groups.iter().map(|g| GroupRow { start: g.start + base, ..*g }));
            group_members.extend_from_slice(&p.members);
            also.extend(p.also.iter().map(|&(index, key)| (id, index, key)));
            faults.extend(p.faults.into_iter().map(|what| Fault { file: id, what }));
        }
        group_at.push(groups.len() as u32);
        steps.step("view.assemble");
        Self {
            inputs,
            info,
            symtab,
            groups,
            group_members,
            group_at,
            also,
            symbols,
            faults,
            versym,
            shared,
        }
    }

    /// What the view could not decode, by file.
    #[must_use]
    pub fn faults(&self) -> &[Fault] {
        &self.faults
    }

    /// The row of `name` (`foo`, or `foo@V` for a non-default version).
    #[must_use]
    pub fn key_of(&self, name: &[u8]) -> Option<SymbolId> {
        let (base, version) = match name.iter().position(|&b| b == b'@') {
            Some(at) if !name[at..].starts_with(b"@@") => (&name[..at], Some(&name[at + 1..])),
            Some(at) => (&name[..at], None),
            None => (name, None),
        };
        self.lookup(base, version)
    }

    fn lookup(&self, name: &[u8], version: Option<&[u8]>) -> Option<SymbolId> {
        lookup(&self.inputs.symbols, &self.symbols, name, version)
    }
}

fn lookup(
    symbols: &mortar_core::Column<SymbolId, mortar_core::inputs::SymbolKey<'_>>,
    index: &HashIndex,
    name: &[u8],
    version: Option<&[u8]>,
) -> Option<SymbolId> {
    let rows = symbols.as_slice();
    index
        .find(sym_hash(name, version), |r| {
            let k = &rows[r as usize];
            k.name == name && k.version == version
        })
        .map(SymbolId)
}

/// The hash of a symbol key (`spec/model.md` §9): the interner's.
fn sym_hash(name: &[u8], version: Option<&[u8]>) -> u64 {
    SymName { name, version }.hash()
}

/// The row of the COMDAT signature `name`.
fn comdat_of(
    comdats: &mortar_core::Column<ComdatId, mortar_core::inputs::Comdat<'_>>,
    index: &HashIndex,
    name: &[u8],
) -> Option<ComdatId> {
    let rows = comdats.as_slice();
    index.find(hash_bytes(name), |r| rows[r as usize].name == name).map(ComdatId)
}

/// The view's file kind (§4.2).
fn kind_of(file: &mortar_core::inputs::File<'_>) -> FileKind {
    match file.kind() {
        InputKind::Internal => FileKind::Internal,
        InputKind::Shared => FileKind::Shared,
        InputKind::Object if file.member.is_some() => {
            if file.state.contains(InputFlags::WHOLE_ARCHIVE) {
                FileKind::WholeMember
            } else {
                FileKind::LazyMember
            }
        }
        InputKind::Object if file.state.contains(InputFlags::LAZY) => FileKind::LazyObject,
        InputKind::Object => FileKind::Object,
    }
}

/// `FileInfo::source`: the least `FileId` with the same found path and
/// member (name, ordinal), that is the file identity of `spec/model.md`
/// §7.1 without the mention ordinal.
fn sources(inputs: &Inputs<'_>) -> Vec<u32> {
    let files = inputs.files.as_slice();
    let key = |f: usize| (files[f].path, files[f].member.map(|m| (m.name, m.ordinal)));
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&a, &b| key(a).cmp(&key(b)).then(a.cmp(&b)));
    let mut out = vec![0u32; files.len()];
    let mut first = 0usize;
    for (i, &f) in order.iter().enumerate() {
        if i == 0 || key(order[i - 1]) != key(f) {
            first = f;
        }
        out[f] = first as u32;
    }
    out
}

fn per_file<'s>(
    inputs: &Inputs<'s>,
    symbols: &HashIndex,
    comdats: &HashIndex,
    id: FileId,
) -> PerFile<'s> {
    let mut out = PerFile::default();
    let file = &inputs.files[id];
    if let Reader::Shared(dso) = file.reader {
        shared_per_file(inputs, symbols, id, &dso, &mut out);
        return out;
    }
    let Reader::Object(obj) = file.reader else { return out };
    let symtab = match obj.symbols() {
        Ok(t) => t,
        Err(e) => {
            out.faults.push(format!("symbol table: {e}"));
            None
        }
    };
    out.symtab = symtab;
    let keys = inputs.sym_map_of(id);
    if let Some(t) = symtab {
        if t.globals().len() == keys.len() {
            let strings = t.strings().bytes();
            for ((index, sym), &key) in t.globals().zip(keys) {
                if let Err(e) = t.section_of(index, &sym) {
                    out.faults.push(format!("symbol {index}: {e}"));
                }
                // The key's name is the entry's name up to its first `@`
                // (the input phase read every name of this table): only an
                // entry whose next two bytes are `@@` needs its whole name.
                let base = inputs.symbols[key].name;
                let after = (sym.st_name as usize).saturating_add(base.len());
                if strings.get(after..after.saturating_add(2)) != Some(b"@@".as_slice()) {
                    continue;
                }
                let name = match t.name(&sym) {
                    Ok(name) => name,
                    Err(e) => {
                        out.faults.push(format!("symbol {index}: {e}"));
                        continue;
                    }
                };
                // `foo@@V`: the key is `foo`; `also` is `foo@V`, if a row.
                if name.len() > base.len() + 2 && name[base.len()..].starts_with(b"@@") {
                    let version = &name[base.len() + 2..];
                    if let Some(row) = lookup(&inputs.symbols, symbols, base, Some(version)) {
                        out.also.push((index.0, row));
                    }
                }
            }
        } else {
            out.faults.push("the symbol table is not the one the input phase read".to_owned());
            out.symtab = None;
        }
    }
    for i in 0..file.sections {
        let Ok(s) = obj.section(SectionIndex(i)) else { continue };
        if s.sh_type() != mortar_elf::consts::SHT_GROUP {
            continue;
        }
        let group = match s.group() {
            Ok(g) => g,
            Err(e) => {
                out.faults.push(format!("section {i}: {e}"));
                continue;
            }
        };
        let Some(t) = symtab else {
            out.faults.push(format!("section {i}: a group without a symbol table"));
            continue;
        };
        let name = signature_name(&obj, &t, group.signature());
        let Some(name) = name else {
            out.faults.push(format!("section {i}: the group's signature cannot be read"));
            continue;
        };
        let Some(signature) = comdat_of(&inputs.comdats, comdats, name) else {
            out.faults.push(format!("section {i}: the signature was not interned"));
            continue;
        };
        let start = out.members.len() as u32;
        for m in group.members() {
            match m {
                Ok(m) => out.members.push(m.0),
                Err(e) => out.faults.push(format!("section {i}: {e}")),
            }
        }
        let len = out.members.len() as u32 - start;
        out.groups.push(GroupRow {
            section: SectionIndex(i),
            flags: group.flags(),
            signature,
            start,
            len,
        });
    }
    out
}

/// A shared object's `.dynsym` (`spec/resolve.md` §9.1, §9.2): its table,
/// its `.gnu.version`, the row `foo@V` of each default definition
/// `foo@@V`, and the faults of §9.1-§9.2 the input phase does not report.
fn shared_per_file<'s>(
    inputs: &Inputs<'s>,
    symbols: &HashIndex,
    id: FileId,
    dso: &mortar_elf::SharedObject<'s>,
    out: &mut PerFile<'s>,
) {
    let t = match dso.symbols() {
        Ok(t) => t,
        Err(e) => {
            out.faults.push(format!("dynamic symbol table: {e}"));
            None
        }
    };
    let keys = inputs.sym_map_of(id);
    let Some(t) = t else { return };
    if t.globals().len() != keys.len() {
        out.faults.push("the dynamic symbol table is not the one the input phase read".to_owned());
        return;
    }
    out.symtab = Some(t);
    let versym = match dso.versym() {
        Ok(v) => v,
        Err(e) => {
            out.faults.push(format!(".gnu.version: {e}"));
            None
        }
    };
    out.versym = versym;
    // Version index -> (name, defined here); the base definition names no
    // version (§9.2's last row).
    let mut slots: Vec<Option<(&'s [u8], bool)>> = Vec::new();
    let mut put = |index: u16, slot: (&'s [u8], bool)| {
        let i = usize::from(index);
        if slots.len() <= i {
            slots.resize(i + 1, None);
        }
        slots[i] = Some(slot);
    };
    if let Ok(Some(defs)) = dso.version_definitions() {
        for d in defs.flatten() {
            if !d.is_base()
                && let Ok(name) = d.name()
            {
                put(d.index(), (name, true));
            }
        }
    }
    if let Ok(Some(needs)) = dso.version_needs() {
        for n in needs.flatten() {
            for e in n.entries().into_iter().flatten().flatten() {
                if let Ok(name) = e.name() {
                    put(e.index(), (name, false));
                }
            }
        }
    }
    for (index, sym) in t.globals() {
        let place = match t.section_of(index, &sym) {
            Ok(p) => p,
            Err(e) => {
                out.faults.push(format!("dynamic symbol {index}: {e}"));
                continue;
            }
        };
        if sym.binding() == STB_LOCAL {
            out.faults.push(format!("dynamic symbol {index}: STB_LOCAL past sh_info"));
            continue;
        }
        if place == SymbolSection::Common {
            out.faults.push(format!("dynamic symbol {index}: SHN_COMMON in a shared object"));
            continue;
        }
        if place == SymbolSection::Undefined {
            continue;
        }
        let Some(v) = versym.and_then(|v| v.get(index.0 as usize)) else { continue };
        match v.index() {
            0 => out.faults.push(format!(
                "dynamic symbol {index}: a definition with version index 0 (VER_NDX_LOCAL)"
            )),
            1 => {}
            i => match slots.get(usize::from(i)).copied().flatten() {
                Some((_, false)) => out.faults.push(format!(
                    "dynamic symbol {index}: a definition whose version index {i} names a needed version"
                )),
                Some((version, true)) if !v.hidden() => {
                    let base = inputs.symbols[keys[(index.0 - t.first_global().0) as usize]].name;
                    if let Some(row) = lookup(&inputs.symbols, symbols, base, Some(version)) {
                        out.also.push((index.0, row));
                    }
                }
                _ => {}
            },
        }
    }
}

/// The effective signature (`spec/model.md` §2.3): the symbol's name, or
/// its section's name for an `STT_SECTION` symbol.
fn signature_name<'s>(
    obj: &mortar_elf::Object<'s>,
    t: &SymbolTable<'s>,
    index: SymbolIndex,
) -> Option<&'s [u8]> {
    let sym = t.get(index).ok()?;
    if sym.sym_type() == STT_SECTION {
        match t.section_of(index, &sym).ok()? {
            SymbolSection::Section(s) => obj.section(s).ok()?.name().ok(),
            _ => None,
        }
    } else {
        t.name(&sym).ok()
    }
}

impl View for InputsView<'_, '_> {
    fn machine(&self) -> mortar_elf::Machine {
        self.inputs.target.machine
    }

    fn files(&self) -> u32 {
        self.inputs.files.len()
    }

    fn keys(&self) -> u32 {
        self.inputs.symbols.len()
    }

    fn comdats(&self) -> u32 {
        self.inputs.comdats.len()
    }

    fn file(&self, file: FileId) -> FileInfo {
        self.info[file.0 as usize]
    }

    fn mentions(&self, file: FileId, out: &mut Vec<Mention>) {
        let Some(t) = self.symtab[file.0 as usize] else { return };
        let shared = self.shared[file.0 as usize].is_some();
        let keys = self.inputs.sym_map_of(file);
        let from = self.also.partition_point(|a| a.0 < file);
        let mut also = self.also[from..].iter().take_while(|a| a.0 == file).peekable();
        for ((index, sym), &key) in t.globals().zip(keys) {
            let Ok(place) = t.section_of(index, &sym) else { continue };
            let (place, large) = crate::view::place_of(self.inputs.target.machine, place);
            if shared && !shared_mention(&sym, place) {
                continue;
            }
            let mut row = OptId::NONE;
            while let Some(a) = also.peek() {
                if a.1 < index.0 {
                    also.next();
                } else {
                    if a.1 == index.0 {
                        row = OptId::some(a.2);
                    }
                    break;
                }
            }
            out.push(Mention {
                index,
                key,
                also: row,
                binding: sym.binding(),
                sym_type: sym.sym_type(),
                visibility: sym.st_other & 3,
                other: sym.st_other & !3,
                place,
                value: sym.st_value,
                size: sym.st_size,
                large,
            });
        }
    }

    fn mention(&self, file: FileId, index: SymbolIndex) -> Option<Mention> {
        let t = self.symtab.get(file.0 as usize).copied().flatten()?;
        let first = t.first_global().0;
        if index.0 < first {
            return None;
        }
        let key = *self.inputs.sym_map_of(file).get((index.0 - first) as usize)?;
        let sym = t.get(index).ok()?;
        let place = t.section_of(index, &sym).ok()?;
        let (place, large) = crate::view::place_of(self.inputs.target.machine, place);
        let at = self.also.binary_search_by(|a| (a.0, a.1).cmp(&(file, index.0))).ok();
        Some(Mention {
            index,
            key,
            also: at.map_or(OptId::NONE, |at| OptId::some(self.also[at].2)),
            binding: sym.binding(),
            sym_type: sym.sym_type(),
            visibility: sym.st_other & 3,
            other: sym.st_other & !3,
            place,
            value: sym.st_value,
            size: sym.st_size,
            large,
        })
    }

    fn groups(&self, file: FileId, out: &mut Groups) {
        let f = file.0 as usize;
        let rows = &self.groups[self.group_at[f] as usize..self.group_at[f + 1] as usize];
        for g in rows {
            let members = &self.group_members[g.start as usize..(g.start + g.len) as usize];
            out.push(g.section, g.flags, g.signature, members);
        }
    }

    fn section(&self, file: FileId, index: SectionIndex) -> Option<SectionInfo<'_>> {
        let Reader::Object(obj) = self.inputs.files.get(file)?.reader else { return None };
        let s = obj.section(index).ok()?;
        let h = s.header();
        Some(SectionInfo {
            name: s.name().unwrap_or_default(),
            sh_type: h.sh_type,
            flags: h.sh_flags,
            entsize: h.sh_entsize,
            addralign: h.sh_addralign,
            size: h.sh_size,
            link: h.sh_link,
            info: h.sh_info,
        })
    }

    fn key_name(&self, key: SymbolId) -> (&[u8], Option<&[u8]>) {
        let k = &self.inputs.symbols[key];
        (k.name, k.version)
    }

    fn find_key(&self, name: &[u8]) -> Option<SymbolId> {
        self.lookup(name, None)
    }

    fn shared(&self, file: FileId) -> Option<SharedInfo> {
        self.shared.get(file.0 as usize).copied().flatten()
    }

    fn needed(&self, file: FileId, out: &mut Vec<Option<FileId>>) {
        let edges = &self.inputs.link.closure;
        let from = edges.partition_point(|e| e.needing < file);
        out.extend(edges[from..].iter().take_while(|e| e.needing == file).map(
            |e| match e.outcome {
                EdgeOutcome::Satisfied(f) => Some(f),
                EdgeOutcome::NotFound => None,
            },
        ));
    }

    fn default_version(&self, file: FileId, index: SymbolIndex) -> Option<&[u8]> {
        if self.shared(file).is_some() {
            return None;
        }
        let t = self.symtab.get(file.0 as usize).copied().flatten()?;
        let name = t.name(&t.get(index).ok()?).ok()?;
        let at = name.windows(2).position(|w| w == b"@@")?;
        Some(&name[at + 2..])
    }

    fn version(&self, file: FileId, index: SymbolIndex) -> u16 {
        self.versym
            .get(file.0 as usize)
            .copied()
            .flatten()
            .and_then(|v| v.get(index.0 as usize))
            .map_or(0, mortar_elf::Versym::index)
    }
}

/// Whether a `.dynsym` entry of a shared object is a mention resolution
/// reads (`spec/resolve.md` §9.1): a reference, or a definition of default
/// or protected visibility (hidden and internal ones are ignored; local
/// and common ones are faults of the view).
fn shared_mention(sym: &mortar_elf::Sym, place: SymbolSection) -> bool {
    use mortar_elf::consts::{STV_HIDDEN, STV_INTERNAL};
    if sym.binding() == STB_LOCAL || place == SymbolSection::Common {
        return false;
    }
    place == SymbolSection::Undefined || !matches!(sym.st_other & 3, STV_HIDDEN | STV_INTERNAL)
}

// ---- the Plan -------------------------------------------------------------

/// The keys the options name, **by name** (`mortar_core::incr::OptionKeys`;
/// `spec/resolve.md` §4.2, §7.1, §7.2): the entry (`-e`, else the last
/// script `ENTRY`, else `_start`; none for `-shared` and `-r`), `-u` then
/// each script `EXTERN` (`arg` [`crate::options::EXTERN_ARG`] plus its
/// ordinal), `--require-defined`, `--wrap` (`Plan::wraps`), and `--defsym`
/// (the last option of each symbol, in command-line order of those; a
/// value the command line refuses is left out). The ONE function that
/// reads these names from the plan and the scripts' directives: the full
/// link's [`options`] maps them to keys, and an incremental update's join
/// passes the same value to U-3 (WP-215).
#[must_use]
pub fn option_keys(
    plan: &Plan,
    directives: &[mortar_core::inputs::Directive<'_>],
) -> mortar_core::incr::OptionKeys {
    use mortar_core::incr::{DefsymOperand, OptionDefsym, OptionKey, OptionKeys};
    let key = |name: &[u8], arg: u32| OptionKey { name: name.into(), arg };
    let entry = match plan.kind {
        OutputKind::Shared | OutputKind::Relocatable => None,
        _ => Some(match (&plan.entry, script_entry(directives)) {
            // The `Plan` keeps `-e`'s value, not its argument index: seq 0.
            (Some(e), _) => key(e.as_encoded_bytes(), 0),
            (None, Some(d)) => key(d.value, 0),
            (None, None) => key(b"_start", 0),
        }),
    };
    let by_role = |role: SymbolRole| -> Vec<OptionKey> {
        plan.symbols
            .iter()
            .filter(|s| s.role == role)
            .map(|s| key(s.value.as_encoded_bytes(), s.arg))
            .collect()
    };
    let externs = directives
        .iter()
        .filter(|d| d.kind == DirectiveKind::Extern)
        .enumerate()
        .map(|(i, d)| key(d.value, crate::options::EXTERN_ARG + i as u32));
    let mut defsym: Vec<OptionDefsym> = Vec::new();
    let all = plan.symbols.iter().filter(|s| s.role == SymbolRole::Defsym);
    for (s, parsed) in all.zip(plan.defsyms()) {
        let Ok(d) = parsed else { continue };
        let value = match d.expr {
            DefsymExpr::Number(n) => DefsymOperand::Number(n),
            DefsymExpr::Symbol(b) => DefsymOperand::Alias(b.into()),
            DefsymExpr::Offset(b, n) => DefsymOperand::Offset(b.into(), n),
        };
        defsym.retain(|o| *o.name != *d.sym);
        defsym.push(OptionDefsym { name: d.sym.into(), value, arg: s.arg });
    }
    OptionKeys {
        entry,
        undefined: by_role(SymbolRole::Undefined).into_iter().chain(externs).collect(),
        require_defined: by_role(SymbolRole::RequireDefined),
        wrap: plan.wraps().into_iter().map(Into::into).collect(),
        defsym,
        gc_sections: plan.shaping.gc_sections,
        init: plan.dynamic.init.as_ref().map(|s| key(s.as_encoded_bytes(), 0)),
        fini: plan.dynamic.fini.as_ref().map(|s| key(s.as_encoded_bytes(), 0)),
        gc_keep_exported: plan.shaping.gc_keep_exported,
    }
}

/// The last script `ENTRY` (§4.2: a later one wins).
fn script_entry<'d, 's>(
    directives: &'d [mortar_core::inputs::Directive<'s>],
) -> Option<&'d mortar_core::inputs::Directive<'s>> {
    directives.iter().rev().find(|d| d.kind == DirectiveKind::Entry)
}

/// The options resolution reads, from the `Plan` and the input scripts'
/// `ENTRY` (`spec/resolve.md` §4.2, §5.2, §5.5): `-e` wins over `ENTRY`,
/// the last `ENTRY` over earlier ones; `--wrap` (§7.1) by the rows the
/// input phase gave its three names.
#[must_use]
pub fn options(plan: &Plan, inputs: &Inputs<'_>, view: &InputsView<'_, '_>) -> Options {
    let named = |name: &[u8], arg: u32| Named {
        key: view.key_of(name).map_or(OptId::NONE, OptId::some),
        name: name.into(),
        arg,
    };
    let keys = option_keys(plan, &inputs.link.directives);
    let entry = match &keys.entry {
        None => Entry::None,
        Some(_) if plan.entry.is_none() && script_entry(&inputs.link.directives).is_none() => {
            Entry::Default(view.find_key(b"_start").map_or(OptId::NONE, OptId::some))
        }
        Some(e) => Entry::Named(named(&e.name, e.arg)),
    };
    let by_role = |role: SymbolRole| -> Vec<Named> {
        plan.symbols
            .iter()
            .filter(|s| s.role == role)
            .map(|s| named(s.value.as_encoded_bytes(), s.arg))
            .collect()
    };
    let mut ignored: Vec<SymbolId> =
        by_role(SymbolRole::IgnoreUnresolved).iter().filter_map(|n| n.key.get()).collect();
    ignored.sort_unstable();
    ignored.dedup();
    let d = &plan.diagnostics;
    let unresolved = match d.unresolved_symbols {
        Some(UnresolvedSymbols::IgnoreAll | UnresolvedSymbols::IgnoreInObjectFiles) => {
            Unresolved::Ignore
        }
        _ if d.warn_unresolved_symbols == Some(true) => Unresolved::Warn,
        _ => Unresolved::Error,
    };
    Options {
        entry,
        undefined: keys.undefined.iter().map(|k| named(&k.name, k.arg)).collect(),
        require_defined: keys.require_defined.iter().map(|k| named(&k.name, k.arg)).collect(),
        wrap: keys
            .wrap
            .iter()
            .map(|sym| &**sym)
            .filter_map(|sym| {
                // The input phase gave each of the three a row (model.md §2.3).
                let row = |name: &[u8]| view.find_key(name);
                Some(Wrap {
                    sym: row(sym)?,
                    wrap: row(&[b"__wrap_", sym].concat())?,
                    real: row(&[b"__real_", sym].concat())?,
                })
            })
            .collect(),
        defsym: defsyms(plan, &keys, view),
        allow_multiple_definition: d.allow_multiple_definition,
        warn_common: d.warn_common,
        warn_backrefs: d.warn_backrefs,
        strip_debug: matches!(plan.shaping.strip, Some(Strip::Debug | Strip::All)),
        consume_gnu_pubnames: plan.shaping.gdb_index == Some(true)
            && !matches!(plan.shaping.strip, Some(Strip::Debug | Strip::All)),
        output: Output {
            pie: matches!(plan.kind, OutputKind::PieExecutable | OutputKind::StaticPie),
            // WP-117: a shared object is a dynamic link (`_DYNAMIC`; no
            // static IPLT bounds, resolve §8.1).
            dynamic: matches!(
                plan.kind,
                OutputKind::Executable | OutputKind::PieExecutable | OutputKind::Shared
            ),
        },
        unresolved: SharedOutput::unresolved(SharedOutput::of(plan), unresolved),
        ignore_unresolved: ignored,
        shared: shared_link(plan, inputs, view, &by_role(SymbolRole::ExportDynamicSymbol)),
        shared_output: SharedOutput::of(plan),
        versions: crate::version::VersionLine {
            script: None,
            export_dynamic: plan.dynamic.export_dynamic,
            no_undefined_version: plan.dynamic.undefined_version == Some(false),
        },
        exclude_libs: exclude_libs(inputs),
        dynamic_list: None,
    }
}

/// [`Options::defsym`]: the last `--defsym` of each symbol, in the order
/// of those (§7.2: a later option of a symbol replaces an earlier one, in
/// all three references). A value the command line refuses never gets
/// here (`spec/cli.md` §4.2); the input phase gave each name a row
/// (model.md §2.3).
fn defsyms(
    plan: &Plan,
    keys: &mortar_core::incr::OptionKeys,
    view: &InputsView<'_, '_>,
) -> Vec<crate::options::Defsym> {
    use mortar_core::incr::DefsymOperand;
    let mut out: Vec<crate::options::Defsym> = Vec::new();
    for d in &keys.defsym {
        let Some(sym) = view.find_key(&d.name) else { continue };
        let value = match &d.value {
            DefsymOperand::Number(n) => DefsymValue::Number(*n),
            DefsymOperand::Alias(b) => match view.find_key(b) {
                Some(b) => DefsymValue::Alias(b),
                None => continue,
            },
            DefsymOperand::Offset(b, n) => match view.find_key(b) {
                Some(b) => DefsymValue::Offset(b, *n),
                None => continue,
            },
        };
        // The option's text, `SYM=EXPR`, by its argument index.
        let text = plan
            .symbols
            .iter()
            .find(|s| s.role == SymbolRole::Defsym && s.arg == d.arg)
            .map_or(Box::default(), |s| s.value.as_encoded_bytes().into());
        out.retain(|o| o.sym != sym);
        out.push(crate::options::Defsym { sym, value, arg: d.arg, text });
    }
    out
}

/// [`Options::exclude_libs`]: the walk's flag of each file, or nothing
/// when no file has it.
fn exclude_libs(inputs: &Inputs<'_>) -> Vec<bool> {
    let flag = |f: &mortar_core::inputs::File<'_>| f.state.contains(InputFlags::EXCLUDE_LIBS);
    if !inputs.files.iter().any(|(_, f)| flag(f)) {
        return Vec::new();
    }
    inputs.files.iter().map(|(_, f)| flag(f)).collect()
}

/// [`SharedLink`]: the mentions of shared objects and the positions of the
/// relocatable objects, from `opened` (`spec/input.md` §10.4, load order
/// with scripts expanded inline), and the switches of D-13 and D-21.
#[allow(clippy::too_many_lines, reason = "one walk over `opened` and the plan")]
fn shared_link(
    plan: &Plan,
    inputs: &Inputs<'_>,
    view: &InputsView<'_, '_>,
    export: &[Named],
) -> SharedLink {
    let files = inputs.files.as_slice();
    if !files.iter().any(|f| f.kind() == InputKind::Shared) {
        // WP-148: the switches of D-21 hold without a shared input too (a
        // dynamic executable of objects alone exports by §9.6).
        return SharedLink {
            export_dynamic: plan.dynamic.export_dynamic,
            export_symbols: export_symbols(view, export),
            ..SharedLink::default()
        };
    }
    let link = &inputs.link;
    let mut position = vec![u32::MAX; files.len()];
    let mut archive_at: HashMap<u32, u32> = HashMap::new();
    let mut mentions = Vec::new();
    // The command-line group ordinal -> `FileInfo::group`'s numbering.
    let mut plan_group: HashMap<u32, u32> = HashMap::new();
    for (f, file) in files.iter().enumerate() {
        if let Origin::Input { input, .. } = file.origin
            && let Some(g) = plan.inputs.get(input as usize).and_then(|i| i.state.group)
            && let Some(o) = view.file(FileId(f as u32)).group
        {
            plan_group.entry(g).or_insert(o);
        }
    }
    let script_of = |entry: u32| link.script_entries.get(entry as usize);
    for (i, o) in link.opened.iter().enumerate() {
        let i = i as u32;
        if matches!(o.origin, Origin::Closure { .. }) {
            continue;
        }
        match o.kind {
            OpenedKind::Object(f) => position[f.0 as usize] = i,
            OpenedKind::Archive(a) => {
                archive_at.insert(a.0, i);
            }
            OpenedKind::Shared(f) => mentions.push(SharedMention {
                file: f,
                position: i,
                as_needed: files[f.0 as usize].state.contains(InputFlags::AS_NEEDED),
                group: view.file(f).group.map_or(MentionGroup::None, MentionGroup::In),
            }),
            OpenedKind::SharedNotLoaded(f) => {
                // A script entry carries its mention's state (D-19a); a
                // command-line mention has its `Plan` input's.
                let Origin::Input { input, entry, .. } = o.origin else { continue };
                let (as_needed, group) = if let Some(e) = entry {
                    let Some(s) = script_of(e) else { continue };
                    (s.as_needed, s.group.map(|g| g.0))
                } else {
                    let Some(state) = plan.inputs.get(input as usize).map(|i| &i.state) else {
                        continue;
                    };
                    (state.as_needed, state.group.and_then(|g| plan_group.get(&g).copied()))
                };
                let group = group.map_or(MentionGroup::None, MentionGroup::In);
                mentions.push(SharedMention { file: f, position: i, as_needed, group });
            }
            _ => {}
        }
    }
    for (f, file) in files.iter().enumerate() {
        if let Some(m) = file.member
            && let Some(&at) = archive_at.get(&m.archive.0)
        {
            position[f] = at;
        }
    }
    let d = &plan.diagnostics;
    // An executable checks by default (D-13); a shared object only under
    // `--no-allow-shlib-undefined` (D-118, WP-117); other kinds never.
    let allow = match plan.kind {
        OutputKind::Executable | OutputKind::PieExecutable => {
            plan.dynamic.allow_shlib_undefined == Some(true)
        }
        OutputKind::Shared => plan.dynamic.allow_shlib_undefined != Some(false),
        _ => true,
    } || matches!(
        d.unresolved_symbols,
        Some(UnresolvedSymbols::IgnoreAll | UnresolvedSymbols::IgnoreInSharedLibs)
    );
    let shlib_undefined = if allow {
        ShlibUndefined::Allow
    } else if d.warn_unresolved_symbols == Some(true) {
        ShlibUndefined::Warn
    } else {
        ShlibUndefined::Error
    };
    SharedLink {
        mentions,
        position,
        shlib_undefined,
        export_dynamic: plan.dynamic.export_dynamic,
        export_symbols: export_symbols(view, export),
    }
}

/// The `--export-dynamic-symbol` keys, sorted (resolve.md §4.2): each key
/// matching the glob (a value without a wildcard names its key; a glob is
/// matched against the unversioned keys' names).
fn export_symbols(view: &InputsView<'_, '_>, export: &[Named]) -> Vec<SymbolId> {
    let mut keys: Vec<SymbolId> = export.iter().filter_map(|n| n.key.get()).collect();
    let globs: Vec<&[u8]> = export.iter().map(|n| &*n.name).filter(|p| is_glob(p)).collect();
    if !globs.is_empty() {
        keys.extend((0..view.keys()).map(SymbolId).filter(|&k| {
            let (name, version) = view.key_name(k);
            version.is_none() && globs.iter().any(|p| glob_match(p, name))
        }));
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

// ---- the Resolution table ---------------------------------------------------

/// `Resolution` (`spec/model.md` §3.3) from a result of [`resolve`].
///
/// # Errors
///
/// A column longer than an id can number (`spec/model.md` §2).
pub fn resolution<V: View>(
    view: &V,
    r: &Resolved,
    opts: &Options,
) -> Result<Resolution, mortar_core::IdOverflow> {
    // N, U and the rest of the loaded shared objects are the shared
    // stage's (`spec/dyn.md` D-11, D-12); a static link has none.
    let file_live = r.file_live.clone();
    // §7.2: a `--defsym` number or `B±N` is a row of `assigned`, in key
    // order.
    let mut assigned = Vec::new();
    let def = r
        .def
        .iter()
        .map(|&d| match d {
            Def::Defsym(DefsymValue::Number(n)) => {
                assigned.push(Assigned::Number(n));
                TableDef::Internal(InternalDef::Assign(assigned.len() as u32 - 1))
            }
            Def::Defsym(DefsymValue::Offset(to, addend)) => {
                assigned.push(Assigned::Offset { to, addend });
                TableDef::Internal(InternalDef::Assign(assigned.len() as u32 - 1))
            }
            d => table_def(d),
        })
        .collect();
    let dynamic = r.dynamic.as_ref();
    let props = r
        .def
        .iter()
        .zip(&r.props)
        .enumerate()
        .map(|(k, (&d, &p))| {
            let mut props = sym_props(SymbolId(k as u32), d, p, opts);
            // `spec/dyn.md` D-114 and D-110 item 1 (WP-150): a definition a
            // version script or `--exclude-libs` makes local is
            // `STB_LOCAL` in `.symtab` (synth.md S-77). Without a script
            // and without `--exclude-libs` both columns are empty.
            if props.binding != STB_LOCAL
                && (dynamic.is_some_and(|dy| dy.script_local.get(k) == Some(&true))
                    || excluded(opts, d))
            {
                props.binding = STB_LOCAL;
            }
            if let Some(dy) = dynamic {
                dynamic_props(view, k, d, dy, &mut props);
            }
            props
        })
        .collect();
    let set = |col: Option<&Vec<bool>>| -> IdSet<SymbolId> {
        let mut out = IdSet::new();
        if let Some(col) = col {
            out = IdSet::with_len(col.len() as u32);
            for (k, &on) in col.iter().enumerate() {
                if on {
                    out.insert(SymbolId(k as u32));
                }
            }
        }
        out
    };
    Ok(Resolution {
        file_live: mortar_core::Column::from_vec(file_live)?,
        file_round: r.file_round.clone().into_boxed_slice(),
        def: mortar_core::Column::from_vec(def)?,
        props: mortar_core::Column::from_vec(props)?,
        common: r.common.clone().into_boxed_slice(),
        clash: mortar_core::Column::from_vec(r.clash.clone())?,
        redirect: mortar_core::Column::from_vec(r.redirect.clone())?,
        comdat: mortar_core::Column::from_vec(r.keepers.keeper.clone())?,
        comdat_copies: r.keepers.copies.clone().into_boxed_slice(),
        section_base: mortar_core::Column::from_vec(r.section_base.clone())?,
        sections: mortar_core::Column::from_vec(r.sections.clone())?,
        meta: r.meta.clone().into_boxed_slice(),
        assigned: assigned.into_boxed_slice(),
        // The dynamic columns (`spec/dyn.md` D-11, D-12, D-21): empty in a
        // static link.
        exported: set(dynamic.map(|d| &d.exported)),
        imported: set(dynamic.map(|d| &d.imported)),
        ref_dynamic: set(dynamic.map(|d| &d.ref_dynamic)),
        defined_dynamic: set(dynamic.map(|d| &d.defined_dynamic)),
        // `spec/model.md` §3.3 (R-M10): empty outside `Shared` (WP-117,
        // `spec/dyn.md` D-112).
        preemptible: set(dynamic.map(|d| &d.preemptible)),
    })
}

/// The `RefFlags` bits that repeat the dynamic columns (kept equal to them,
/// `docs/status/m2.md`), `PREEMPTIBLE` for a shared definition, and the
/// version of `bind(k)`'s definition (`spec/dyn.md` D-27).
fn dynamic_props<V: View>(
    view: &V,
    k: usize,
    d: Def,
    dy: &crate::shared::Dynamic,
    props: &mut SymProps,
) {
    if dy.ref_dynamic[k] {
        props.refs |= RefFlags::FROM_SHARED;
    }
    if dy.defined_dynamic[k] {
        props.refs |= RefFlags::DEFINED_SHARED;
    }
    if dy.exported[k] {
        props.refs |= RefFlags::EXPORTED;
    }
    if let Def::Shared(e) = d {
        props.refs |= RefFlags::PREEMPTIBLE;
        props.version = view.version(e.file, e.index);
    }
    // WP-125: an export of a shared object, its output version (D-115).
    else if dy.exported[k] {
        props.version = dy.versions.get(k).copied().unwrap_or(0);
    }
}

/// The model's `def` (§3.3): strong and weak definitions are both
/// `Defined`; the binding is in `SymProps`.
#[must_use]
pub fn table_def(d: Def) -> TableDef {
    match d {
        Def::Undefined { weak } => TableDef::Undefined { weak },
        Def::Strong(e) | Def::Weak(e) => TableDef::Defined(e),
        Def::Common(e) => TableDef::Common(e),
        Def::Provide { conditional, .. } => {
            TableDef::Internal(InternalDef::Provide { conditional })
        }
        Def::Shared(e) => TableDef::Shared(e),
        Def::Defsym(DefsymValue::Alias(to)) => TableDef::Internal(InternalDef::Alias(to)),
        // A number and `B±N` index `Resolution::assigned`, which only the
        // table builder fills; alone, row 0.
        Def::Defsym(_) => TableDef::Internal(InternalDef::Assign(0)),
    }
}

/// A definition in a member of an archive `--exclude-libs` names
/// (`spec/dyn.md` D-110 item 1).
fn excluded(opts: &Options, d: Def) -> bool {
    match d {
        Def::Strong(e) | Def::Weak(e) | Def::Common(e) if !opts.exclude_libs.is_empty() => {
            opts.exclude_libs.get(e.file.0 as usize).copied().unwrap_or(false)
        }
        _ => false,
    }
}

fn sym_props(key: SymbolId, d: Def, p: crate::Props, opts: &Options) -> SymProps {
    let binding = match (symtab_entry(d, p, false), d) {
        (SymtabEntry::Entry { binding, .. }, _) => binding,
        (_, Def::Undefined { weak: true }) => STB_WEAK,
        _ => STB_GLOBAL,
    };
    let mut refs = RefFlags::empty();
    // A root of §4.2 is a reference from file 0, not an object's: a key
    // only roots name that stays undefined is written nowhere, as a root
    // no file mentions (WP-152: `-u K` of a library left out of N).
    let undefined = matches!(d, Def::Undefined { .. });
    if p.flags & flags::OBJ_REF != 0 || (p.flags & flags::REF_REGULAR != 0 && !undefined) {
        refs |= RefFlags::FROM_OBJECT;
    }
    if p.flags & flags::REF_STRONG != 0 {
        refs |= RefFlags::STRONG;
    }
    if p.flags & flags::VARIANT_PCS != 0 {
        refs |= RefFlags::VARIANT_PCS;
    }
    let v = match verdict(key, d, p, opts.unresolved, &opts.ignore_unresolved) {
        Verdict::Zero if matches!(d, Def::Undefined { weak: false }) => TableVerdict::Ignored,
        Verdict::Defined | Verdict::Zero => TableVerdict::Bound,
        Verdict::Warning => TableVerdict::Warning,
        Verdict::Error => TableVerdict::Error,
    };
    SymProps { visibility: p.visibility, binding, refs, verdict: v, version: 0 }
}

// ---- diagnostics ------------------------------------------------------------

/// The diagnostics of a resolution (`spec/resolve.md` §14), keyed
/// (`spec/model.md` §8), in key order.
#[must_use]
pub fn diagnostics(
    inputs: &Inputs<'_>,
    view: &InputsView<'_, '_>,
    opts: &Options,
    r: &Resolved,
) -> Vec<(Key, Diagnostic)> {
    let ctx = Ctx { inputs, view, opts };
    let mut out: Vec<(Key, Diagnostic)> = view
        .faults()
        .iter()
        .map(|f| {
            let d =
                Diagnostic::error(Kind::Malformed { what: f.what.clone() }).at(ctx.file(f.file));
            (Key::new(u16::from(crate::problem::PHASE), f.file.0, 0), d)
        })
        .collect();
    out.extend(r.problems.iter().map(|p| {
        let key = Key::new(u16::from(p.key.phase), p.key.unit, p.key.seq);
        (key, ctx.diagnostic(p))
    }));
    // The version stage's (WP-125, `crate::version::apply`).
    if let Some(dy) = &r.dynamic {
        out.extend(dy.version_problems.iter().map(|(k, d, file)| {
            let d = match file {
                Some(f) => d.clone().at(ctx.file(*f)),
                None => d.clone(),
            };
            (Key::new(u16::from(k.phase), k.unit, k.seq), d)
        }));
    }
    out.sort();
    out
}

struct Ctx<'c, 'a, 's> {
    inputs: &'c Inputs<'s>,
    view: &'c InputsView<'a, 's>,
    opts: &'c Options,
}

impl Ctx<'_, '_, '_> {
    fn file_ref(&self, file: FileId) -> FileRef {
        self.inputs.files[file].display()
    }

    fn file(&self, file: FileId) -> Place {
        Place::File(self.file_ref(file))
    }

    fn name(&self, key: SymbolId) -> String {
        let (name, version) = self.view.key_name(key);
        let name = String::from_utf8_lossy(name);
        match version {
            Some(v) => format!("{name}@{}", String::from_utf8_lossy(v)),
            None => name.into_owned(),
        }
    }

    fn section_name(&self, file: FileId, index: SectionIndex) -> String {
        self.view.section(file, index).map_or_else(
            || format!("section {}", index.0),
            |s| String::from_utf8_lossy(s.name).into_owned(),
        )
    }

    /// A symbol-table entry as a place: the symbol of its file, or for a
    /// root (file 0) the option that names it.
    fn entry(&self, e: SymEntry) -> Place {
        if e.file == FileId::INTERNAL {
            return Place::argument(self.root_text(e.index.0));
        }
        let name = self
            .view
            .mention(e.file, e.index)
            .map_or_else(|| format!("symbol {}", e.index.0), |m| self.name(m.key));
        Place::symbol(self.file_ref(e.file), name)
    }

    fn root_text(&self, arg: u32) -> String {
        let o = self.opts;
        let text = |n: &Named| String::from_utf8_lossy(&n.name).into_owned();
        match &o.entry {
            Entry::Named(n) if n.arg == arg => return format!("--entry={}", text(n)),
            Entry::Default(_) if arg == 0 => return "_start (the default entry)".to_owned(),
            _ => {}
        }
        if let Some(n) = o.undefined.iter().find(|n| n.arg == arg) {
            if arg >= crate::options::EXTERN_ARG {
                return format!("EXTERN({})", text(n));
            }
            return format!("--undefined={}", text(n));
        }
        if let Some(n) = o.require_defined.iter().find(|n| n.arg == arg) {
            return format!("--require-defined={}", text(n));
        }
        if let Some(d) = o.defsym.iter().find(|d| d.arg == arg) {
            return format!("--defsym={}", String::from_utf8_lossy(&d.text));
        }
        format!("argument {arg}")
    }

    fn def_place(&self, d: &DefPlace) -> Place {
        match d.section {
            Some(s) => Place::section(
                self.file_ref(d.entry.file),
                self.section_name(d.entry.file, s),
                d.value,
            ),
            None => self.entry(d.entry),
        }
    }

    #[allow(clippy::too_many_lines, reason = "one arm per diagnostic of §14")]
    fn diagnostic(&self, p: &Problem) -> Diagnostic {
        use RefusalClass as C;
        let warn = |kind| Diagnostic::warning(kind);
        match &p.what {
            What::ShlibUndefined { file, key, member, .. } => {
                let kind = Kind::UndefinedSymbol { name: self.name(*key) };
                let d = if p.severity() == crate::Severity::Warning {
                    Diagnostic::warning(kind)
                } else {
                    Diagnostic::error(kind)
                };
                let d = d
                    .at(self.file(*file))
                    .with(Relation::ReferencedBy, self.file(*file))
                    .note("a shared object's undefined symbol; --allow-shlib-undefined accepts it");
                // `spec/dyn.md` D-12 (WP-167): only a needed object's
                // references extract.
                match member {
                    Some((member, false)) => d.with(Relation::DefinedIn, self.file(*member)).note(
                        "the library is not needed (--as-needed): link it with \
                             --no-as-needed, or name the member's definition earlier (an \
                             object or -u)",
                    ),
                    Some((member, true)) => d.with(Relation::DefinedIn, self.file(*member)).note(
                        "the library is not on the command line, so its references extract \
                         nothing: name it on the line with --no-as-needed, or name the \
                         member's definition earlier (an object or -u)",
                    ),
                    None => d,
                }
            }
            What::NonExported { key, def, entry } => {
                Diagnostic::error(Kind::NonExportedSymbol { name: self.name(*key) })
                    .at(self.entry(*def))
                    .with(Relation::DefinedIn, self.entry(*def))
                    .with(Relation::ReferencedBy, self.file(entry.file))
            }
            What::RootAsNeeded { key, arg, lib, member } => Diagnostic::refused(
                C::Feature,
                format!(
                    "root of an unneeded --as-needed shared object and an archive member: {}",
                    self.name(*key)
                ),
            )
            .at(Place::argument(self.root_text(*arg)))
            .with(Relation::DefinedIn, self.file(*lib))
            .with(Relation::DefinedIn, self.file(*member))
            .note(
                "the references differ: GNU ld extracts the member, lld leaves the root \
                 unfulfilled; name the archive before the library, or the library under \
                 --no-as-needed",
            ),
            What::WrapComdat { key, dropped, kept } => Diagnostic::refused(
                C::Feature,
                format!(
                    "--wrap of a symbol with a definition in a dropped COMDAT copy: {}",
                    self.name(*key)
                ),
            )
            .at(Place::argument(format!("--wrap={}", self.name(*key))))
            .with(Relation::DefinedIn, self.file(*kept))
            .with(Relation::DefinedIn, self.entry(*dropped))
            .note(
                "the references differ on the calls in a file whose copy is dropped (GNU ld \
                 wraps them, lld wraps every call, mold none): a wrapped function must have one \
                 definition; make it non-inline, or wrap its callers' own symbol",
            ),
            What::Defsym { arg, why, .. } => {
                let (what, note) = match why {
                    DefsymWhy::Undefined(b) => (
                        format!("--defsym of an undefined symbol: {}", self.name(*b)),
                        "the symbol the expression names must be defined in the output \
                         (GNU ld, lld and mold refuse it too)"
                            .to_owned(),
                    ),
                    DefsymWhy::Shared(b) => (
                        format!("--defsym of a symbol a shared object defines: {}", self.name(*b)),
                        "the symbol the expression names must be defined in the output, not by \
                         a shared object (GNU ld and lld refuse it, mold crashes)"
                            .to_owned(),
                    ),
                    DefsymWhy::Chain(b) => (
                        format!("--defsym of a symbol another --defsym defines: {}", self.name(*b)),
                        "name the final symbol in each --defsym (a chain links in GNU ld and \
                         lld, crashes mold; a cycle fails in all three)"
                            .to_owned(),
                    ),
                    DefsymWhy::TooWide(n) => (
                        format!("--defsym value wider than the target's address: {n:#x}"),
                        "the target's addresses are 32 bits; GNU ld, lld and mold truncate the \
                         value"
                            .to_owned(),
                    ),
                };
                Diagnostic::refused(C::Feature, what)
                    .at(Place::argument(self.root_text(*arg)))
                    .note(note)
            }
            What::LtoSlim { file } => {
                Diagnostic::refused(C::FileFormat, "GCC LTO object").at(self.file(*file))
            }
            What::SymbolBinding { entry, binding } => {
                Diagnostic::refused(C::SymbolBinding, format!("{binding:#x}"))
                    .at(self.entry(*entry))
            }
            What::WeakCommon { entry } => {
                Diagnostic::refused(C::SymbolBinding, "STB_WEAK common").at(self.entry(*entry))
            }
            What::SymbolType { entry, sym_type } => {
                Diagnostic::refused(C::SymbolType, format!("{sym_type:#x}")).at(self.entry(*entry))
            }
            What::ReservedSection { entry, shndx } => {
                // §2: a reserved index this spec does not name (x86-64's
                // large common is a common there, §3.3, `view::place_of`).
                Diagnostic::refused(C::Feature, format!("SHN {shndx:#x}")).at(self.entry(*entry))
            }
            What::Malformed { entry, why } => {
                let what = match why {
                    MalformedSymbol::NonLocalSectionOrFile => {
                        "a non-local STT_SECTION or STT_FILE symbol".to_owned()
                    }
                    MalformedSymbol::UndefinedUnique => {
                        "an undefined STB_GNU_UNIQUE symbol".to_owned()
                    }
                    MalformedSymbol::CommonAlignment(a) => {
                        format!("common symbol alignment is not a power of two: {a}")
                    }
                };
                Diagnostic::error(Kind::Malformed { what }).at(self.entry(*entry))
            }
            What::Section { file, section, why } => {
                let (class, name) = match why {
                    SectionRefusal::LlvmType(t)
                    | SectionRefusal::DynamicType(t)
                    | SectionRefusal::Type(t) => (
                        C::SectionType,
                        // A known type by its name (`SHT_LLVM_ODRTAB`), else
                        // the number.
                        self.view
                            .machine()
                            .section_type_name(*t)
                            .map_or_else(|| format!("{t:#x}"), str::to_owned),
                    ),
                    SectionRefusal::Flags(f) => (C::SectionFlag, format!("{f:#x}")),
                    SectionRefusal::RelForm(t) => {
                        let name =
                            if *t == mortar_elf::consts::SHT_REL { "SHT_REL" } else { "SHT_RELA" };
                        (C::SectionType, name.to_owned())
                    }
                    SectionRefusal::Linkonce => (C::Feature, ".gnu.linkonce sections".to_owned()),
                    SectionRefusal::Zdebug => {
                        (C::Feature, ".zdebug compressed debug sections".to_owned())
                    }
                    SectionRefusal::Compressed => {
                        (C::Feature, "SHF_COMPRESSED input sections".to_owned())
                    }
                    SectionRefusal::SecondReloc(target) => (
                        C::Feature,
                        format!(
                            "two relocation sections for one section ({})",
                            self.section_name(*file, SectionIndex(*target))
                        ),
                    ),
                };
                let at =
                    Place::section(self.file_ref(*file), self.section_name(*file, *section), 0);
                Diagnostic::refused(class, name).at(at)
            }
            What::GroupFlags { file, section, flags } => {
                let at =
                    Place::section(self.file_ref(*file), self.section_name(*file, *section), 0);
                Diagnostic::refused(C::Feature, format!("group flags {flags:#x}")).at(at)
            }
            What::SectionInTwoGroups { file, section } => {
                let at =
                    Place::section(self.file_ref(*file), self.section_name(*file, *section), 0);
                Diagnostic::error(Kind::Malformed { what: "a section in two groups".to_owned() })
                    .at(at)
            }
            What::Duplicate { key, defs } => {
                let mut d = Diagnostic::error(Kind::DuplicateSymbol { name: self.name(*key) });
                if let Some(second) = defs.get(1) {
                    d = d.at(self.def_place(second));
                }
                for def in defs.iter().take(3) {
                    d = d.with(Relation::DefinedIn, self.def_place(def));
                }
                d
            }
            What::TlsMismatch { key, first, first_tls, other } => {
                let (tls, non) = if *first_tls { (first, other) } else { (other, first) };
                Diagnostic::error(Kind::TlsMismatch { name: self.name(*key) })
                    .at(self.entry(*first))
                    .with(Relation::DefinedIn, self.entry(*first))
                    .with(Relation::ReferencedBy, self.entry(*other))
                    .note(format!(
                        "TLS: {}; not TLS: {}",
                        self.file_ref(tls.file).path(),
                        self.file_ref(non.file).path()
                    ))
            }
            What::LinkerSymbolRedefined { key, entry } => {
                Diagnostic::error(Kind::LinkerSymbolRedefined { name: self.name(*key) })
                    .at(self.entry(*entry))
            }
            What::RequiredSymbol { name, arg, shared, .. } => {
                let name = String::from_utf8_lossy(name).into_owned();
                let d = Diagnostic::error(Kind::RequiredSymbol { name })
                    .at(Place::argument(self.root_text(*arg)));
                match shared {
                    Some(e) => d.with(Relation::DefinedIn, self.entry(*e)).note(
                        "only a shared object defines it; --require-defined needs a definition in the output (GNU ld)",
                    ),
                    None => d,
                }
            }
            What::EntryNotFound { name, .. } => {
                warn(Kind::EntryNotFound { name: String::from_utf8_lossy(name).into_owned() })
            }
            What::CommonLarger { key, def, def_size, common, common_size } => {
                warn(Kind::CommonLargerThanDefinition { name: self.name(*key) })
                    .at(self.entry(*def))
                    .with(Relation::DefinedIn, self.entry(*def))
                    .with(Relation::DefinedIn, self.entry(*common))
                    .note(format!(
                        "the definition has size {def_size}; the common symbol {common_size}"
                    ))
            }
            What::CommonAlignment { key, def, def_align, common, common_align } => warn(
                Kind::CommonAlignment { name: self.name(*key) },
            )
            .at(self.entry(*def))
            .with(Relation::DefinedIn, self.entry(*def))
            .with(Relation::DefinedIn, self.entry(*common))
            .note(format!(
                "the definition is aligned to {def_align}; the common symbol to {common_align}"
            )),
            What::MultipleCommon { key, first, second } => {
                warn(Kind::MultipleCommon { name: self.name(*key) })
                    .at(self.entry(*second))
                    .with(Relation::DefinedIn, self.entry(*first))
            }
            What::CommonOverridden { key, common, def } => {
                warn(Kind::CommonOverridden { name: self.name(*key) })
                    .at(self.entry(*def))
                    .with(Relation::DefinedIn, self.entry(*common))
            }
            What::ArchiveBackref { key, member, referrer } => {
                warn(Kind::ArchiveBackref { name: self.name(*key) })
                    .at(self.entry(*referrer))
                    .with(Relation::DefinedIn, self.file(*member))
            }
            What::ArchiveChoice { key, mortar, gnu, referrer } => {
                warn(Kind::ArchiveChoice { name: self.name(*key) })
                    .at(self.entry(*referrer))
                    .note(format!("mortar uses {}", place_text(&self.file_ref(mortar.file))))
                    .note(format!("GNU ld uses {}", place_text(&self.file_ref(gnu.file))))
            }
            What::GnuWarning { file, section } => {
                let text = self.gnu_warning_text(*file, *section);
                let at =
                    Place::section(self.file_ref(*file), self.section_name(*file, *section), 0);
                warn(Kind::GnuWarning { text }).at(at)
            }
            What::MetaNotContainer { file, section, relocated } => {
                let at =
                    Place::section(self.file_ref(*file), self.section_name(*file, *section), 0);
                let reason = if *relocated {
                    "the section is not a container: a relocation section applies to it \
                     (meta.md MT-2, MT-6)"
                } else {
                    "the section is not a container: its type or flags are not SHT_PROGBITS \
                     with exactly SHF_EXCLUDE (meta.md MT-1, MT-6)"
                };
                warn(Kind::MetaMalformed { reason: reason.to_owned() }).at(at)
            }
        }
    }

    /// The text of a `.gnu.warning` section: its bytes up to the first NUL.
    fn gnu_warning_text(&self, file: FileId, section: SectionIndex) -> String {
        let Some(Reader::Object(obj)) = self.inputs.files.get(file).map(|f| f.reader) else {
            return String::new();
        };
        let data = obj.section(section).and_then(|s| s.data()).unwrap_or_default();
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        String::from_utf8_lossy(&data[..end]).into_owned()
    }
}

fn place_text(f: &FileRef) -> String {
    match f.member() {
        Some(m) => format!("{}({m})", f.path()),
        None => f.path().to_owned(),
    }
}

// ---- the whole phase -------------------------------------------------------

/// What resolution hands on beside the table (`spec/resolve.md` §4.7, §5.5,
/// §6.3, §10).
#[derive(Debug, Clone)]
pub struct Outcome {
    pub resolution: Resolution,
    /// Every diagnostic of the phase, in key order; an `internal-error`
    /// if the table broke its own rules.
    pub diagnostics: Vec<(Key, Diagnostic)>,
    /// The keys only dropped COMDAT copies define (§6.3 (b)), for the scan.
    pub comdat_mismatch: Vec<crate::problem::ComdatMismatch>,
    /// `.gnu.warning.<sym>` sections of live files (§10), for the scan.
    pub gnu_warning_syms: Vec<(SymbolId, FileId, SectionIndex)>,
    /// `SHF_GNU_RETAIN` sections (§4.7).
    pub retained: Vec<(FileId, SectionIndex)>,
    pub entry: crate::EntryPoint,
    /// The keys the options root (§4.2: the entry, `-u`,
    /// `--require-defined`), in option order, then `-init`'s and
    /// `-fini`'s: their definitions are roots of `--gc-sections` (§4.7).
    pub roots: Vec<SymbolId>,
    /// The needed shared objects in `DT_NEEDED` order (`spec/dyn.md`
    /// D-19a): `Slots.needed`. Empty in a static link.
    pub needed: Vec<FileId>,
    /// Keys a relocatable object refers to strongly that stay undefined
    /// while a closure file defines them, with that file (`spec/dyn.md`
    /// D-12): the scan's `undefined-symbol` gets the note `defined in
    /// <file>, which is not on the command line`. Sorted by key.
    pub closure_only: Vec<(SymbolId, FileId)>,
    /// Where the time went (`MORTAR_STEPS`; decides nothing).
    pub steps: Steps,
}

/// Resolution of a link (`spec/resolve.md`): the view, the options, the
/// algorithms, the table, its checks (`Resolution::check`,
/// `check_against`) and the diagnostics.
#[must_use]
pub fn resolve_inputs(inputs: &Inputs<'_>, plan: &Plan, pool: &Pool) -> Outcome {
    resolve_inputs_with(inputs, plan, pool, None, None)
}

/// [`resolve_inputs`] with the line's version script, compiled
/// (`crate::version::VersionScript::compile`; `spec/resolve.md` §9.7):
/// applied in a `Shared` output; and its `--dynamic-list` files, compiled
/// (`crate::version::DynamicList::load`; `spec/dyn.md` D-112, WP-128).
#[must_use]
pub fn resolve_inputs_with(
    inputs: &Inputs<'_>,
    plan: &Plan,
    pool: &Pool,
    script: Option<std::sync::Arc<crate::version::VersionScript>>,
    dynamic_list: Option<std::sync::Arc<crate::version::DynamicList>>,
) -> Outcome {
    let mut steps = Steps::default();
    let view = InputsView::new_steps(inputs, pool, &mut steps);
    let mut opts = options(plan, inputs, &view);
    opts.versions.script = script;
    opts.dynamic_list = dynamic_list;
    steps.step("options");
    let r = resolve_steps(&view, &opts, pool, &mut steps);
    let mut diagnostics = diagnostics(inputs, &view, &opts, &r);
    steps.step("diagnostics");
    let internal = |what: String| {
        (
            Key::new(u16::from(crate::problem::PHASE), u32::MAX, 0),
            Diagnostic::error(Kind::InternalError { what }),
        )
    };
    let table = resolution(&view, &r, &opts);
    steps.step("table");
    let resolution = match table {
        Ok(t) => {
            if let Err(e) = t
                .check()
                .and_then(|()| t.check_against(inputs))
                .and_then(|()| t.check_preemptible(plan.kind == OutputKind::Shared))
            {
                diagnostics.push(internal(e.to_string()));
            }
            t
        }
        Err(e) => {
            diagnostics.push(internal(format!("{e:?}")));
            Resolution::empty(inputs.files.len(), inputs.symbols.len(), inputs.comdats.len())
        }
    };
    steps.step("check");
    steps.count("files", u64::from(inputs.files.len()));
    steps.count("live", r.file_live.iter().filter(|l| l.is_live()).count() as u64);
    steps.count("keys", u64::from(inputs.symbols.len()));
    steps.count("mentions", inputs.sym_map.len() as u64);
    steps.count("comdats", u64::from(inputs.comdats.len()));
    steps.count("sections", r.sections.len() as u64);
    steps.count("rounds", u64::from(r.rounds));
    // `spec/dyn.md` D-12 (WP-167): 1 unless a needed --as-needed shared
    // object's reference extracts a member.
    steps.count("stages", u64::from(r.stages));
    // §4.2, §4.7 (WP-128): `-init`/`-fini`'s symbols are roots of
    // `--gc-sections` (GNU ld and lld keep the function), never of
    // extraction (none of the three extracts a member for them).
    let mut roots: Vec<SymbolId> = opts.roots().iter().map(|root| root.key).collect();
    for name in [&plan.dynamic.init, &plan.dynamic.fini].into_iter().flatten() {
        if let Some(key) = view.find_key(name.as_encoded_bytes()) {
            roots.push(key);
        }
    }
    drop(view);
    let outcome = Outcome {
        resolution,
        diagnostics,
        comdat_mismatch: r.comdat_mismatch,
        gnu_warning_syms: r.gnu_warning_syms,
        retained: r.retained,
        entry: r.entry,
        roots,
        needed: r.dynamic.as_ref().map(|d| d.needed.clone()).unwrap_or_default(),
        closure_only: r.dynamic.as_ref().map(|d| d.closure_only.clone()).unwrap_or_default(),
        steps: Steps::default(),
    };
    steps.step("drop");
    Outcome { steps, ..outcome }
}
