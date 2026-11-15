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
    Def as TableDef, InternalDef, RefFlags, Resolution, SymProps, Verdict as TableVerdict,
};
use mortar_core::{ComdatId, FileId, IdSet, OptId, SymEntry, SymbolId};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, RefusalClass, Relation};
use mortar_elf::consts::{STB_GLOBAL, STB_WEAK, STT_SECTION};
use mortar_elf::{SectionIndex, SymbolIndex, SymbolSection, SymbolTable};

use crate::index::HashIndex;
use crate::options::{
    Entry, MentionGroup, Named, Options, Output, SharedLink, SharedMention, ShlibUndefined,
};
use crate::problem::{DefPlace, MalformedSymbol, Problem, What};
use crate::resolve::{Def, Resolved, flags, resolve_steps};
use crate::sections::SectionRefusal;
use crate::steps::Steps;
use crate::verdict::{SymtabEntry, Unresolved, Verdict, symtab_entry, verdict};
use crate::view::{FileInfo, FileKind, Groups, Mention, SectionInfo, SharedInfo, View};

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
        if sym.binding() == mortar_elf::consts::STB_LOCAL {
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
                place,
                value: sym.st_value,
                size: sym.st_size,
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
        let at = self.also.binary_search_by(|a| (a.0, a.1).cmp(&(file, index.0))).ok();
        Some(Mention {
            index,
            key,
            also: at.map_or(OptId::NONE, |at| OptId::some(self.also[at].2)),
            binding: sym.binding(),
            sym_type: sym.sym_type(),
            visibility: sym.st_other & 3,
            place,
            value: sym.st_value,
            size: sym.st_size,
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
    use mortar_elf::consts::{STB_LOCAL, STV_HIDDEN, STV_INTERNAL};
    if sym.binding() == STB_LOCAL || place == SymbolSection::Common {
        return false;
    }
    place == SymbolSection::Undefined || !matches!(sym.st_other & 3, STV_HIDDEN | STV_INTERNAL)
}

// ---- the Plan -------------------------------------------------------------

/// The options resolution reads, from the `Plan` and the input scripts'
/// `ENTRY` (`spec/resolve.md` §4.2, §5.2, §5.5): `-e` wins over `ENTRY`,
/// the last `ENTRY` over earlier ones. `--wrap` and `--defsym` (M4) are
/// refused by the command line (`spec/cli.md` §4.2) and not read.
#[must_use]
pub fn options(plan: &Plan, inputs: &Inputs<'_>, view: &InputsView<'_, '_>) -> Options {
    let named = |name: &[u8], arg: u32| Named {
        key: view.key_of(name).map_or(OptId::NONE, OptId::some),
        name: name.into(),
        arg,
    };
    let script_entry = inputs
        .link
        .directives
        .iter()
        .rev()
        .find(|d| d.kind == DirectiveKind::Entry)
        .map(|d| named(d.value, 0));
    let entry = match plan.kind {
        OutputKind::Shared | OutputKind::Relocatable => Entry::None,
        _ => match (&plan.entry, script_entry) {
            // The `Plan` keeps `-e`'s value, not its argument index: seq 0.
            (Some(e), _) => Entry::Named(named(e.as_encoded_bytes(), 0)),
            (None, Some(n)) => Entry::Named(n),
            (None, None) => {
                Entry::Default(view.find_key(b"_start").map_or(OptId::NONE, OptId::some))
            }
        },
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
        undefined: by_role(SymbolRole::Undefined),
        require_defined: by_role(SymbolRole::RequireDefined),
        wrap: Vec::new(),
        allow_multiple_definition: d.allow_multiple_definition,
        warn_common: d.warn_common,
        warn_backrefs: d.warn_backrefs,
        strip_debug: matches!(plan.shaping.strip, Some(Strip::Debug | Strip::All)),
        output: Output {
            pie: matches!(plan.kind, OutputKind::PieExecutable | OutputKind::StaticPie),
            dynamic: matches!(plan.kind, OutputKind::Executable | OutputKind::PieExecutable),
        },
        unresolved,
        ignore_unresolved: ignored,
        shared: shared_link(plan, inputs, view, &by_role(SymbolRole::ExportDynamicSymbol)),
    }
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
        return SharedLink::default();
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
                as_needed: Some(files[f.0 as usize].state.contains(InputFlags::AS_NEEDED)),
                group: view.file(f).group.map_or(MentionGroup::None, MentionGroup::In),
            }),
            OpenedKind::SharedNotLoaded(f) => {
                let Origin::Input { input, entry, .. } = o.origin else { continue };
                let Some(state) = plan.inputs.get(input as usize).map(|i| &i.state) else {
                    continue;
                };
                let as_needed = match entry {
                    None => Some(state.as_needed),
                    // `AS_NEEDED(…)` can only turn it on.
                    Some(_) if state.as_needed => Some(true),
                    Some(_) => None,
                };
                let group = match (state.group, entry) {
                    (Some(g), _) => {
                        plan_group.get(&g).map_or(MentionGroup::None, |&o| MentionGroup::In(o))
                    }
                    (None, None) => MentionGroup::None,
                    (None, Some(e)) => {
                        // The script's own GROUP, as its other entries got it.
                        let here = script_of(e).map(|s| (s.script, s.parent));
                        match here {
                            Some((script, None)) => {
                                let mut found = MentionGroup::None;
                                for (g, file) in files.iter().enumerate() {
                                    if let Origin::Input { entry: Some(e2), .. } = file.origin
                                        && script_of(e2).is_some_and(|s| s.script == script)
                                        && let Some(o) = view.file(FileId(g as u32)).group
                                    {
                                        found = MentionGroup::In(o);
                                        break;
                                    }
                                }
                                found
                            }
                            _ => MentionGroup::Unknown,
                        }
                    }
                };
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
    // A merged `as_needed` (on only if on at every mention) decides the
    // mentions the tables do not record when it can.
    let mut by_file: HashMap<FileId, Vec<usize>> = HashMap::new();
    for (i, m) in mentions.iter().enumerate() {
        by_file.entry(m.file).or_default().push(i);
    }
    for (file, at) in by_file {
        let merged = inputs.soname_unit(file).map(|u| u.as_needed);
        if merged == Some(true) {
            for &i in &at {
                mentions[i].as_needed = Some(true);
            }
        } else if merged == Some(false) {
            let unknown: Vec<usize> =
                at.iter().copied().filter(|&i| mentions[i].as_needed.is_none()).collect();
            let known_off = at.iter().any(|&i| mentions[i].as_needed == Some(false));
            if unknown.len() == 1 && !known_off {
                mentions[unknown[0]].as_needed = Some(false);
            }
        }
    }
    let d = &plan.diagnostics;
    let allow = plan.dynamic.allow_shlib_undefined == Some(true)
        || !matches!(plan.kind, OutputKind::Executable | OutputKind::PieExecutable)
        || matches!(
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
    let mut export_symbols: Vec<SymbolId> = export.iter().filter_map(|n| n.key.get()).collect();
    export_symbols.sort_unstable();
    export_symbols.dedup();
    SharedLink {
        mentions,
        position,
        shlib_undefined,
        export_dynamic: plan.dynamic.export_dynamic,
        export_symbols,
    }
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
    let def = r.def.iter().map(|&d| table_def(d)).collect();
    let dynamic = r.dynamic.as_ref();
    let props = r
        .def
        .iter()
        .zip(&r.props)
        .enumerate()
        .map(|(k, (&d, &p))| {
            let mut props = sym_props(SymbolId(k as u32), d, p, opts);
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
        // The dynamic columns (`spec/dyn.md` D-11, D-12, D-21): empty in a
        // static link.
        exported: set(dynamic.map(|d| &d.exported)),
        imported: set(dynamic.map(|d| &d.imported)),
        ref_dynamic: set(dynamic.map(|d| &d.ref_dynamic)),
        defined_dynamic: set(dynamic.map(|d| &d.defined_dynamic)),
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
    }
}

fn sym_props(key: SymbolId, d: Def, p: crate::Props, opts: &Options) -> SymProps {
    let binding = match (symtab_entry(d, p, false), d) {
        (SymtabEntry::Entry { binding, .. }, _) => binding,
        (_, Def::Undefined { weak: true }) => STB_WEAK,
        _ => STB_GLOBAL,
    };
    let mut refs = RefFlags::empty();
    if p.flags & flags::REF_REGULAR != 0 {
        refs |= RefFlags::FROM_OBJECT;
    }
    if p.flags & flags::REF_STRONG != 0 {
        refs |= RefFlags::STRONG;
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
            return format!("--undefined={}", text(n));
        }
        if let Some(n) = o.require_defined.iter().find(|n| n.arg == arg) {
            return format!("--require-defined={}", text(n));
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
            What::ShlibUndefined { file, key, .. } => {
                let kind = Kind::UndefinedSymbol { name: self.name(*key) };
                let d = if p.severity() == crate::Severity::Warning {
                    Diagnostic::warning(kind)
                } else {
                    Diagnostic::error(kind)
                };
                d.at(self.file(*file))
                    .with(Relation::ReferencedBy, self.file(*file))
                    .note("a shared object's undefined symbol; --allow-shlib-undefined accepts it")
            }
            What::NonExported { key, def, entry } => {
                Diagnostic::error(Kind::NonExportedSymbol { name: self.name(*key) })
                    .at(self.entry(*def))
                    .with(Relation::DefinedIn, self.entry(*def))
                    .with(Relation::ReferencedBy, self.file(entry.file))
            }
            What::AsNeededExtraction { key, entry, member } => Diagnostic::refused(
                C::Feature,
                format!(
                    "archive member needed by an --as-needed shared object: {}",
                    self.name(*key)
                ),
            )
            .at(self.file(entry.file))
            .with(Relation::DefinedIn, self.file(*member))
            .note(
                "link the library with --no-as-needed, or name the member's definition \
                 earlier (an object or -u)",
            ),
            What::NeededOrderUnknown { file } => Diagnostic::refused(
                C::Feature,
                "DT_NEEDED order of a shared object mentioned again inside a linker script",
            )
            .at(self.file(*file)),
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
                // §3.3: the index is x86-64's large common only on x86-64
                // (accepted there by `spec/x64.md` X-55, staged as this
                // refusal until `.lbss` exists); elsewhere it is any
                // reserved index (§2).
                let name = if *shndx == mortar_elf::consts::SHN_X86_64_LCOMMON
                    && self.inputs.target.machine == mortar_elf::Machine::X86_64
                {
                    "SHN_X86_64_LCOMMON".to_owned()
                } else {
                    format!("SHN {shndx:#x}")
                };
                Diagnostic::refused(C::Feature, name).at(self.entry(*entry))
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
            What::RequiredSymbol { name, arg, .. } => {
                let name = String::from_utf8_lossy(name).into_owned();
                Diagnostic::error(Kind::RequiredSymbol { name })
                    .at(Place::argument(self.root_text(*arg)))
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
            What::MetaNotContainer { file, section } => {
                let at =
                    Place::section(self.file_ref(*file), self.section_name(*file, *section), 0);
                warn(Kind::MetaMalformed {
                    reason: "the section is not a container: its type or flags are not \
                             SHT_PROGBITS with exactly SHF_EXCLUDE (meta.md MT-1, MT-6)"
                        .to_owned(),
                })
                .at(at)
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
    /// `--require-defined`), in option order: their definitions are
    /// roots of `--gc-sections` (§4.7).
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
    let mut steps = Steps::default();
    let view = InputsView::new_steps(inputs, pool, &mut steps);
    let opts = options(plan, inputs, &view);
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
            if let Err(e) = t.check().and_then(|()| t.check_against(inputs)) {
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
    drop(view);
    let outcome = Outcome {
        resolution,
        diagnostics,
        comdat_mismatch: r.comdat_mismatch,
        gnu_warning_syms: r.gnu_warning_syms,
        retained: r.retained,
        entry: r.entry,
        roots: opts.roots().iter().map(|root| root.key).collect(),
        needed: r.dynamic.as_ref().map(|d| d.needed.clone()).unwrap_or_default(),
        closure_only: r.dynamic.as_ref().map(|d| d.closure_only.clone()).unwrap_or_default(),
        steps: Steps::default(),
    };
    steps.step("drop");
    Outcome { steps, ..outcome }
}
