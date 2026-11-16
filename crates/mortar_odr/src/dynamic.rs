//! The dynamic side (`spec/odr.md` §10, O-24, O-50, O-51): the static
//! check of a dynamic output against the `.note.mason` tables of its
//! shared-object inputs, and the rows of the output's own table.
//!
//! - O-24 (a): the output's exported definitions against each in-scope
//!   library's rows of the name (unversioned or the library's default
//!   version).
//! - O-24 (b): two in-scope libraries' rows of a name the output does not
//!   export, paired as glibc's loader connects them (measured, WP-075).
//! - O-24 (c): a sized kind 5 declaration of a variable against the
//!   library definition it binds (`st_size`).
//!
//! The scope is the output's `DT_NEEDED` (resolution's `needed`, in its
//! order: the lookup order after the output). A library's table is read
//! from the bytes the link already holds (a shared object is read whole);
//! a library without a table takes part in (c) only.

use mortar_core::inputs::{File, Reader};
use mortar_core::resolution::Def;
use mortar_core::{FileId, Inputs, Resolution, SymbolId};
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, Relation};
use mortar_elf::consts::{STV_DEFAULT, STV_PROTECTED};
use mortar_elf::version::Version;
use mortar_elf::{SharedObject, SymbolIndex};
use mortar_meta::table::{self, NONPREEMPTIBLE, TableError};
use mortar_meta::{flags, fnv1a64};

use crate::foreign::PHASE;
use crate::records::{Dynamic, Keeper, seq, tag};

/// The dynamic output the checks run for.
#[derive(Debug, Clone, Copy)]
pub struct Output<'a> {
    /// The shared objects in `DT_NEEDED`, in that order (O-24's scope).
    pub needed: &'a [FileId],
    /// An executable (`Exec` or `Pie`): every row of its table is
    /// `NONPREEMPTIBLE` (an executable binds its own definitions).
    pub executable: bool,
}

/// What the dynamic checks switch on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Switches {
    /// O-24 (O-47 row "O-24 (shared-object inputs)").
    pub shared: bool,
    /// O-50's table (O-47 row "`.note.mason` table").
    pub table: bool,
    /// O-50's `=full`: the table's second note, of the hidden vague
    /// definitions and the types (WP-076). Only with [`Switches::table`].
    pub full: bool,
}

/// `=full`'s rows (O-50, WP-076): the vague definitions the output defines
/// and does not export, and the live units' types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullRows {
    pub definitions: Vec<table::FullRow>,
    pub types: Vec<table::FullRow>,
}

/// A row of the output's table before its `.dynsym` index is known: the
/// symbol, the hash's first 8 bytes, the flags (O-51).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableRow {
    pub sym: SymbolId,
    pub hash: [u8; 8],
    pub flags: u16,
}

impl TableRow {
    /// The O-51 row, given the symbol's name and `.dynsym` index.
    #[must_use]
    pub fn row(&self, name: &[u8], dynsym: u32) -> table::Row {
        table::Row { key: fnv1a64(name), hash: self.hash, dynsym, flags: self.flags }
    }
}

/// A library row's version (O-24's identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ver<'s> {
    /// Index 1, or no version table.
    Unversioned,
    /// A version the library defines: its index, whether hidden
    /// (`name@V`, not the default), its name.
    Node { index: u16, hidden: bool, name: &'s [u8] },
}

impl Ver<'_> {
    fn is_default(self) -> bool {
        matches!(self, Self::Node { hidden: false, .. })
    }

    fn suffix(self) -> String {
        match self {
            Self::Unversioned => String::new(),
            Self::Node { hidden, name, .. } => {
                format!("{}{}", if hidden { "@" } else { "@@" }, String::from_utf8_lossy(name))
            }
        }
    }
}

/// One row of a library's table, decoded.
#[derive(Debug, Clone, Copy)]
struct LibRow<'s> {
    /// The library's place in lookup order (0 is the first in
    /// `DT_NEEDED`).
    pos: usize,
    name: &'s [u8],
    hash: [u8; 8],
    flags: u16,
    ver: Ver<'s>,
}

impl LibRow<'_> {
    fn np(&self) -> bool {
        self.flags & NONPREEMPTIBLE != 0
    }
}

/// The dynamic checks' result.
#[derive(Debug, Default)]
pub struct Found {
    pub diagnostics: Vec<(Key, Diagnostic)>,
    /// The output's table rows, sorted by symbol (with the table on).
    pub table: Option<Vec<TableRow>>,
    /// `=full`'s rows (with `=full`).
    pub full: Option<FullRows>,
    /// Libraries whose table was read, and their rows.
    pub tables: u64,
    pub rows: u64,
}

/// `=full`'s rows (O-50, WP-076). The definitions: each keeper row with
/// `VAGUE` or `EXPLICIT_SPEC` whose symbol the output does not export (so
/// that no row is in both notes), keyed by the symbol's name (MT-13), its
/// O-15 flags without `NONPREEMPTIBLE` (no other module binds it). The
/// types: the first kind 3 row of each type of the live units
/// ([`Dynamic::types`]), its hash's first 8 bytes.
fn full_rows(
    inputs: &Inputs<'_>,
    dynamic: &Dynamic,
    exported: &dyn Fn(SymbolId) -> bool,
) -> FullRows {
    let definitions = dynamic
        .keepers
        .iter()
        .filter(|k| !exported(k.sym) && k.flags & (flags::VAGUE | flags::EXPLICIT_SPEC) != 0)
        .filter_map(|k| {
            let name = inputs.symbols.get(k.sym)?.name;
            let mut hash = [0; 8];
            hash.copy_from_slice(&k.hash[..8]);
            Some(table::FullRow {
                key: fnv1a64(name),
                hash,
                flags: k.flags & !NONPREEMPTIBLE,
                ..table::FullRow::default()
            })
        })
        .collect();
    let types = dynamic
        .types
        .iter()
        .map(|t| {
            let mut hash = [0; 8];
            hash.copy_from_slice(&t.hash[..8]);
            table::FullRow {
                key: t.key,
                hash,
                layout: t.layout,
                size: t.size,
                align: t.align,
                flags: t.flags,
            }
        })
        .collect();
    FullRows { definitions, types }
}

/// Runs O-24 and gathers O-50's rows.
#[must_use]
pub fn check(
    inputs: &Inputs<'_>,
    resolution: &Resolution,
    output: Output<'_>,
    switches: Switches,
    dynamic: &Dynamic,
) -> Found {
    let mut found = Found::default();
    let exported = |sym: SymbolId| {
        resolution.exported.contains(sym)
            && resolution
                .props
                .get(sym)
                .is_some_and(|p| matches!(p.visibility, STV_DEFAULT | STV_PROTECTED))
    };
    let keepers: Vec<&Keeper> = dynamic.keepers.iter().filter(|k| exported(k.sym)).collect();
    if switches.table {
        let rows = keepers
            .iter()
            .filter(|k| k.flags & (flags::VAGUE | flags::EXPLICIT_SPEC) != 0)
            .map(|k| {
                let protected =
                    resolution.props.get(k.sym).is_some_and(|p| p.visibility == STV_PROTECTED);
                let np = output.executable || protected;
                let mut hash = [0; 8];
                hash.copy_from_slice(&k.hash[..8]);
                TableRow {
                    sym: k.sym,
                    hash,
                    flags: (k.flags & !NONPREEMPTIBLE) | if np { NONPREEMPTIBLE } else { 0 },
                }
            })
            .collect();
        found.table = Some(rows);
        if switches.full {
            found.full = Some(full_rows(inputs, dynamic, &exported));
        }
    }
    if !switches.shared {
        return found;
    }
    let mut cx = Cx { inputs, output, out: Vec::new() };
    // The tables of the in-scope libraries, in lookup order.
    let mut rows: Vec<LibRow<'_>> = Vec::new();
    for (pos, &file) in output.needed.iter().enumerate() {
        let Some(Reader::Shared(so)) = inputs.files.get(file).map(|f| &f.reader) else { continue };
        match library_rows(so, pos) {
            Ok(None) => {}
            Ok(Some(r)) => {
                found.tables += 1;
                found.rows += r.len() as u64;
                rows.extend(r);
            }
            Err(e) => cx.malformed(file, &e),
        }
    }
    rows.sort_by(|a, b| (a.name, a.pos).cmp(&(b.name, b.pos)));
    // The names the output exports: (b) is not for them.
    let mut names: Vec<&[u8]> = resolution
        .exported
        .iter()
        .filter(|&s| exported(s))
        .filter_map(|s| inputs.symbols.get(s).map(|k| k.name))
        .collect();
    names.sort_unstable();
    names.dedup();
    // (a): per exported keeper, the libraries' rows of its name.
    let mut sorted_keepers: Vec<(&[u8], &Keeper)> = keepers
        .iter()
        .filter_map(|k| {
            let key = inputs.symbols.get(k.sym)?;
            key.version.is_none().then_some((key.name, *k))
        })
        .collect();
    sorted_keepers.sort_by(|a, b| (a.0, a.1.sym).cmp(&(b.0, b.1.sym)));
    for (name, k) in sorted_keepers {
        let at = rows.partition_point(|r| r.name < name);
        for r in rows[at..].iter().take_while(|r| r.name == name) {
            let bound = matches!(r.ver, Ver::Unversioned) || r.ver.is_default();
            if bound && r.hash != k.hash[..8] {
                cx.output_finding(k, r);
            }
        }
    }
    // (b): per name the output does not export, the pairs of libraries.
    let mut start = 0;
    while start < rows.len() {
        let name = rows[start].name;
        let end =
            rows[start..].iter().position(|r| r.name != name).map_or(rows.len(), |p| start + p);
        if names.binary_search(&name).is_err() {
            cx.pairs(&rows[start..end]);
        }
        start = end;
    }
    // (c): sized declarations against the library definition they bind.
    for &(sym, size, decl) in &dynamic.declared {
        let Some(Def::Shared(e)) = resolution.def.get(sym) else { continue };
        if !output.needed.contains(&e.file) {
            continue;
        }
        let Some(Reader::Shared(so)) = inputs.files.get(e.file).map(|f| &f.reader) else {
            continue;
        };
        let Ok(Some(symbols)) = so.symbols() else { continue };
        let Ok(s) = symbols.get(e.index) else { continue };
        if s.st_size != 0 && s.st_size != size {
            let name = inputs.symbols.get(sym).map_or(&b""[..], |k| k.name);
            cx.size_finding(name, size, decl, e.file, s.st_size);
        }
    }
    cx.out.sort_by_key(|(k, _)| *k);
    found.diagnostics = cx.out;
    found
}

/// A library's table rows, decoded; `None` without a table.
fn library_rows<'s>(
    so: &SharedObject<'s>,
    pos: usize,
) -> Result<Option<Vec<LibRow<'s>>>, TableError> {
    let Some(t) = table::find(so)? else { return Ok(None) };
    let symbols = so.symbols().ok().flatten();
    let count = symbols.as_ref().map_or(0, mortar_elf::SymbolTable::len);
    let versions = so.versions().ok();
    let mut out = Vec::with_capacity(t.len() as usize);
    for (i, r) in t.iter().enumerate() {
        let bad = || TableError::Dynsym { row: i as u32, index: r.dynsym };
        let Some(symbols) = &symbols else { return Err(bad()) };
        if r.dynsym == 0 || r.dynsym >= count {
            return Err(bad());
        }
        let index = SymbolIndex(r.dynsym);
        let Ok(sym) = symbols.get(index) else { return Err(bad()) };
        let Ok(name) = symbols.name(&sym) else { return Err(bad()) };
        let ver = match versions.as_ref().map(|v| v.of(index)) {
            None | Some(Ok(None)) => Ver::Unversioned,
            Some(Ok(Some(sv))) => match sv.version {
                Version::Global => Ver::Unversioned,
                Version::Defined(d) => {
                    let Ok(n) = d.name() else { continue };
                    Ver::Node { index: sv.versym.index(), hidden: sv.versym.hidden(), name: n }
                }
                // A local or a needed version is not a definition another
                // module binds: nothing to compare.
                Version::Local | Version::Needed { .. } => continue,
            },
            Some(Err(_)) => continue,
        };
        out.push(LibRow { pos, name, hash: r.hash, flags: r.flags, ver });
    }
    Ok(Some(out))
}

struct Cx<'a, 's> {
    inputs: &'a Inputs<'s>,
    output: Output<'a>,
    out: Vec<(Key, Diagnostic)>,
}

impl Cx<'_, '_> {
    fn file_ref(&self, file: FileId) -> FileRef {
        self.inputs.files.get(file).map_or_else(|| FileRef::new(""), File::display)
    }

    fn lib(&self, pos: usize) -> FileId {
        self.output.needed[pos]
    }

    fn lib_text(&self, pos: usize) -> String {
        self.file_ref(self.lib(pos)).path().to_owned()
    }

    fn key(&self, file: FileId, t: u64) -> Key {
        Key::new(PHASE, file.0, seq(t, self.out.len() as u64))
    }

    fn malformed(&mut self, file: FileId, e: &TableError) {
        let key = self.key(file, tag::DYN_META);
        let d = Diagnostic::error(Kind::MetaMalformed { reason: e.reason() }).at(Place::section(
            self.file_ref(file),
            ".note.mason",
            0,
        ));
        self.out.push((key, d));
    }

    fn place(&self, r: &LibRow<'_>) -> Place {
        let name = format!("{}{}", String::from_utf8_lossy(r.name), r.ver.suffix());
        Place::record(self.file_ref(self.lib(r.pos)), Some(name), false, None)
    }

    fn severity(a: u16, b: u16) -> (fn(Kind) -> Diagnostic, Option<String>) {
        if a & b & flags::SYSTEM != 0 {
            let note = "both definitions are in a system header: a warning (spec/odr.md O-32)";
            (Diagnostic::warning, Some(note.to_owned()))
        } else if (a | b) & flags::PARTIAL != 0 {
            let note = "a definition's hash is partial (PARTIAL, spec/odr.md O-15): a warning";
            (Diagnostic::warning, Some(note.to_owned()))
        } else {
            (Diagnostic::error, None)
        }
    }

    /// O-24 (a): the output's copy against a library's.
    fn output_finding(&mut self, k: &Keeper, r: &LibRow<'_>) {
        let lib = self.lib(r.pos);
        let name = String::from_utf8_lossy(r.name).into_owned();
        let (make, extra) = Self::severity(k.flags, r.flags);
        let lib_text = self.lib_text(r.pos);
        let runs = if r.np() {
            format!(
                "{lib_text} runs its own copy (its row is NONPREEMPTIBLE: protected, or bound by \
                 -Bsymbolic or -Bsymbolic-functions)"
            )
        } else {
            format!("{lib_text} runs the output's copy")
        };
        let mut d = make(Kind::OdrViolation { name: name.clone() })
            .with(Relation::DefinedIn, Place::record(self.file_ref(k.file), Some(name), true, None))
            .with(Relation::DefinedDifferentlyIn, self.place(r))
            .note(format!(
                "the output exports the definition and comes first in lookup order: it runs its \
                 own copy; {runs}"
            ));
        if let Some(n) = extra {
            d = d.note(n);
        }
        let key = self.key(lib, tag::DYNAMIC);
        self.out.push((key, d));
    }

    /// O-24 (b): the rows of one name, every pair of libraries.
    fn pairs(&mut self, rows: &[LibRow<'_>]) {
        let mut libs: Vec<usize> = rows.iter().map(|r| r.pos).collect();
        libs.dedup();
        for (i, &e) in libs.iter().enumerate() {
            for &l in &libs[i + 1..] {
                let of = |p: usize| rows.iter().filter(|r| r.pos == p).collect::<Vec<_>>();
                if let Some((a, b, how)) = pair(&of(e), &of(l)) {
                    self.library_finding(a, b, how);
                }
            }
        }
    }

    fn library_finding(&mut self, e: &LibRow<'_>, l: &LibRow<'_>, how: How) {
        let name = String::from_utf8_lossy(e.name).into_owned();
        let (make, extra) = Self::severity(e.flags, l.flags);
        let (et, lt) = (self.lib_text(e.pos), self.lib_text(l.pos));
        let bind = match how {
            How::Entity => format!("{et} comes before {lt} in lookup order (DT_NEEDED)"),
            How::UnversionedFirst => format!(
                "{et} comes before {lt} in lookup order (DT_NEEDED), and an unversioned \
                 definition earlier in lookup order binds every versioned reference to its name \
                 (glibc's loader, measured)"
            ),
            How::VersionedFirst => format!(
                "{et} comes before {lt} in lookup order (DT_NEEDED), and an unversioned reference \
                 binds the definition at version index 2 of an earlier library (glibc's loader, \
                 measured)"
            ),
        };
        let runs = if l.np() {
            format!("{et} runs its own copy; so does {lt} (its row is NONPREEMPTIBLE)")
        } else {
            format!("{et} runs its own copy and provides it to {lt}")
        };
        let mut d = make(Kind::OdrViolation { name })
            .with(Relation::DefinedIn, self.place(e))
            .with(Relation::DefinedDifferentlyIn, self.place(l))
            .note(format!("{bind}: {runs}"));
        if let Some(n) = extra {
            d = d.note(n);
        }
        let key = self.key(self.lib(l.pos), tag::DYNAMIC);
        self.out.push((key, d));
    }

    /// O-24 (c).
    fn size_finding(&mut self, name: &[u8], size: u64, decl: FileId, lib: FileId, lib_size: u64) {
        let name = String::from_utf8_lossy(name).into_owned();
        let lib_text = self.file_ref(lib).path().to_owned();
        let d = Diagnostic::error(Kind::OdrSizeMismatch { name: name.clone() })
            .with(
                Relation::ReferencedBy,
                Place::record(self.file_ref(decl), Some(name.clone()), false, None),
            )
            .with(Relation::DefinedIn, Place::symbol(self.file_ref(lib), name))
            .note(format!(
                "the declaration's size is {size} bytes and {lib_text} defines {lib_size}: a copy \
                 relocation made for one size runs against the other"
            ));
        let key = self.key(lib, tag::SIZE);
        self.out.push((key, d));
    }
}

/// How two libraries' rows are connected (O-24's bullets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    /// One entity: same node name, or both unversioned.
    Entity,
    /// The earlier library's unversioned row and the later one's default.
    UnversionedFirst,
    /// The earlier library's row at index 2 and the later one's
    /// unversioned row.
    VersionedFirst,
}

/// The first pair of differing rows that O-24 (b) compares, earlier
/// library first, if any.
fn pair<'r, 's>(
    e: &[&'r LibRow<'s>],
    l: &[&'r LibRow<'s>],
) -> Option<(&'r LibRow<'s>, &'r LibRow<'s>, How)>
where
    's: 'r,
{
    fn node<'n>(r: &LibRow<'n>) -> Option<&'n [u8]> {
        match r.ver {
            Ver::Node { name, .. } => Some(name),
            Ver::Unversioned => None,
        }
    }
    let unversioned = |r: &&&LibRow<'_>| matches!(r.ver, Ver::Unversioned);
    for &a in e {
        for &b in l {
            let entity = match (a.ver, b.ver) {
                (Ver::Unversioned, Ver::Unversioned) => true,
                (Ver::Node { .. }, Ver::Node { .. }) => node(a) == node(b),
                _ => false,
            };
            if entity && a.hash != b.hash {
                return Some((a, b, How::Entity));
            }
        }
    }
    let e_unv = e.iter().find(unversioned).copied();
    let l_unv = l.iter().find(unversioned).copied();
    match (e_unv, l_unv) {
        (Some(u), None) => {
            let d = l.iter().find(|r| r.ver.is_default()).copied()?;
            (u.hash != d.hash).then_some((u, d, How::UnversionedFirst))
        }
        (None, Some(u)) => {
            let two = e.iter().find(|r| matches!(r.ver, Ver::Node { index: 2, .. })).copied()?;
            (two.hash != u.hash).then_some((two, u, How::VersionedFirst))
        }
        _ => None,
    }
}
