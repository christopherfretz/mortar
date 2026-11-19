//! The symbols as resolution leaves them, for `.symtab`
//! (`mortar_synth::SymSource` over the core tables, `spec/synth.md` §7.1).

use mortar_core::assignment::Assignment;
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::resolution::{Def, InternalDef, RefFlags, Resolution};
use mortar_core::{FileId, SectionId, SymEntry, SymKey, SymKind, SymbolId};
use mortar_elf::consts::{
    STB_GLOBAL, STB_GNU_UNIQUE, STB_LOCAL, STB_WEAK, STO_AARCH64_VARIANT_PCS, STT_NOTYPE,
    STT_OBJECT, STT_TLS, STV_DEFAULT, STV_HIDDEN, STV_INTERNAL, STV_PROTECTED,
};
use mortar_elf::{Machine, Sym, SymbolIndex, SymbolSection, SymbolTable};
use mortar_synth::SymSource;
use mortar_synth::symtab::{CoreSections, GlobalIn, LocalIn, SecKind, Sections, SymDef};

/// The more constraining of two visibilities (gABI "Symbol Visibility":
/// internal, then hidden, then protected, then default).
fn stricter(a: u8, b: u8) -> u8 {
    let rank = |v: u8| match v & 3 {
        STV_INTERNAL => 3,
        STV_HIDDEN => 2,
        STV_PROTECTED => 1,
        _ => 0,
    };
    if rank(a) >= rank(b) { a & 3 } else { b & 3 }
}

/// [`SymSource`] over `Inputs`, `Resolution` and `Assignment`.
pub struct CoreSyms<'a, 's> {
    inputs: &'a Inputs<'s>,
    resolution: &'a Resolution,
    assignment: &'a Assignment<'a>,
    /// Per file: the symbol table of a live object.
    tables: Vec<Option<SymbolTable<'s>>>,
}

impl std::fmt::Debug for CoreSyms<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreSyms").field("files", &self.tables.len()).finish_non_exhaustive()
    }
}

impl<'a, 's> CoreSyms<'a, 's> {
    #[must_use]
    pub fn new(
        inputs: &'a Inputs<'s>,
        resolution: &'a Resolution,
        assignment: &'a Assignment<'a>,
    ) -> Self {
        let tables = inputs
            .files
            .iter()
            .map(|(file, row)| {
                let live = resolution.file_live.get(file).is_some_and(|l| l.is_live());
                match row.reader {
                    Reader::Object(o) if live => o.symbols().ok().flatten(),
                    _ => None,
                }
            })
            .collect();
        Self { inputs, resolution, assignment, tables }
    }

    fn table(&self, file: FileId) -> Option<&SymbolTable<'s>> {
        self.tables.get(file.0 as usize)?.as_ref()
    }

    /// The `st_other` bits above the visibility of the relocatable
    /// entry that defines `id` (`STO_AARCH64_VARIANT_PCS`); 0 for any
    /// other definition.
    #[must_use]
    pub fn defined_other_flags(&self, id: SymbolId) -> u8 {
        let Some(Def::Defined(e)) = self.resolution.def.get(id).copied() else { return 0 };
        self.sym(e).map_or(0, |(s, _)| s.st_other & !3)
    }

    /// The version `V` of a key whose definition is written `foo@@V` in
    /// its relocatable object (a default-version definition, resolve
    /// §9.2: its key is `foo`, so the key's own name carries no version).
    #[must_use]
    pub fn default_version(&self, id: SymbolId) -> Option<&'s [u8]> {
        let Some(Def::Defined(e)) = self.resolution.def.get(id).copied() else { return None };
        let t = self.table(e.file)?;
        let name = t.name(&t.get(e.index).ok()?).ok()?;
        let at = name.windows(2).position(|w| w == b"@@")?;
        Some(&name[at + 2..])
    }

    fn sym(&self, e: SymEntry) -> Option<(Sym, &SymbolTable<'s>)> {
        let t = self.table(e.file)?;
        Some((t.get(e.index).ok()?, t))
    }

    /// Where entry `e` is defined, read from its file.
    fn def_of(&self, e: SymEntry, s: &Sym, t: &SymbolTable<'s>) -> SymDef {
        match t.section_of(e.index, s) {
            Ok(SymbolSection::Section(index)) => {
                self.resolution.section_id(e.file, index).map_or(SymDef::Undefined, |section| {
                    SymDef::Section { section, value: s.st_value }
                })
            }
            Ok(SymbolSection::Absolute) => SymDef::Absolute(s.st_value),
            Ok(SymbolSection::Common) => SymDef::Common,
            _ => SymDef::Undefined,
        }
    }
}

impl SymSource for CoreSyms<'_, '_> {
    fn files(&self) -> u32 {
        self.tables.len() as u32
    }

    fn locals(&self, file: FileId) -> u32 {
        self.table(file).map_or(0, |t| t.first_global().0.saturating_sub(1))
    }

    fn local(&self, file: FileId, i: u32) -> LocalIn<'_> {
        let e = SymEntry { file, index: SymbolIndex(i + 1) };
        let Some((s, t)) = self.sym(e) else {
            return LocalIn {
                name: b"",
                info: 0,
                other: 0,
                size: 0,
                def: SymDef::Undefined,
                referenced: false,
            };
        };
        LocalIn {
            name: t.name(&s).unwrap_or_default(),
            info: s.st_info,
            other: s.st_other,
            size: s.st_size,
            def: self.def_of(e, &s, t),
            // Follow-up (WP-027b): the scan does not record which locals a
            // live relocation names. `false` only lets S-50 rule 4 drop an
            // unreferenced local inside a removed `.eh_frame` record, which
            // no compiler emits.
            referenced: false,
        }
    }

    fn globals(&self) -> u32 {
        self.inputs.symbols.len()
    }

    fn global(&self, id: SymbolId) -> Option<GlobalIn<'_>> {
        if self.assignment.is_withdrawn(id) {
            return None;
        }
        let key = self.inputs.symbols.get(id)?;
        let def = self
            .assignment
            .def(self.resolution, id)
            .or_else(|| self.resolution.def.get(id).copied())?;
        let props = self.resolution.props.get(id).copied().unwrap_or_default();
        let referenced = props.refs.contains(RefFlags::FROM_OBJECT);
        let other = props.visibility & 3;
        // synth.md S-121: the bits above the visibility
        // (`STO_AARCH64_VARIANT_PCS`) are the defining entry's, or for an
        // undefined entry those of its references (resolve.md §3.7).
        let row = |typ, size, def, file, binding| GlobalIn {
            name: key.name,
            typ,
            binding,
            other,
            size,
            def,
            file,
            referenced,
        };
        Some(match def {
            Def::Defined(e) => {
                let (s, t) = self.sym(e)?;
                let g = row(STT_NOTYPE, 0, self.def_of(e, &s, t), e.file, props.binding);
                defined_global(g, &s, t)
            }
            Def::Common(e) => {
                let size = self.resolution.common_of(id).map_or(0, |c| c.size);
                let mut g = row(STT_OBJECT, size, SymDef::Common, e.file, props.binding);
                g.other |= self.sym(e).map_or(0, |(s, _)| s.st_other & !3);
                g
            }
            // resolve.md §8.1: the table's visibility (local hidden for
            // `__ehdr_start`, the array bounds, `__rela_iplt_*`, ...;
            // protected for `__start_`/`__stop_`), combined with the
            // references' (the most constraining, gABI "Symbol
            // Visibility"): a hidden one is demoted to a local (S-77).
            Def::Internal(InternalDef::Provide { .. }) => {
                let table = mortar_resolve::linker::TABLE
                    .iter()
                    .find(|r| r.name == key.name)
                    .map(|r| r.visibility)
                    .or_else(|| mortar_resolve::linker::start_stop(key.name).map(|p| p.binding().1))
                    .unwrap_or(STV_DEFAULT);
                // `_TLS_MODULE_BASE_` is the TLS block's own symbol
                // (x64.md X-47; GNU ld writes it `STT_TLS`): its value is
                // then a template offset (synth.md S-81), 0.
                let typ = if key.name == b"_TLS_MODULE_BASE_" { STT_TLS } else { STT_NOTYPE };
                // A version script's `local:` makes it `STB_LOCAL`
                // (`spec/dyn.md` D-114, WP-150; resolution's binding).
                let binding = if props.binding == STB_LOCAL { STB_LOCAL } else { STB_GLOBAL };
                let mut g = row(typ, 0, SymDef::Internal, FileId::INTERNAL, binding);
                g.other = (g.other & !3) | stricter(other, table);
                g
            }
            // resolve.md §7.2 (WP-153): `--defsym SYM=B` has `B`'s type,
            // size 0 (all three references) and the bits above the
            // visibility of `B`'s entry; a number and `B+N` are
            // `STT_NOTYPE`, size 0. A version script's `local:` makes it
            // `STB_LOCAL`.
            Def::Internal(InternalDef::Alias(to)) => {
                let (typ, high) = match self.resolution.def.get(to).copied() {
                    Some(Def::Defined(e)) => self
                        .sym(e)
                        .map_or((STT_NOTYPE, 0), |(s, _)| (s.sym_type(), s.st_other & !3)),
                    Some(Def::Common(e)) => {
                        (STT_OBJECT, self.sym(e).map_or(0, |(s, _)| s.st_other & !3))
                    }
                    _ => (STT_NOTYPE, 0),
                };
                let binding = if props.binding == STB_LOCAL { STB_LOCAL } else { STB_GLOBAL };
                let mut g = row(typ, 0, SymDef::Internal, FileId::INTERNAL, binding);
                g.other |= high;
                g
            }
            Def::Internal(InternalDef::Assign(_)) => {
                let binding = if props.binding == STB_LOCAL { STB_LOCAL } else { STB_GLOBAL };
                row(STT_NOTYPE, 0, SymDef::Internal, FileId::INTERNAL, binding)
            }
            // gABI "Symbol Visibility": a hidden or internal symbol must
            // be removed or made local in an executable; an undefined one
            // has no file block to be local in, so it is removed (GNU's
            // form; lld writes it `STB_LOCAL` undefined). synth.md S-75.
            Def::Undefined { .. } if matches!(other, STV_INTERNAL | STV_HIDDEN) => return None,
            Def::Undefined { weak } if referenced => {
                let binding = if weak { STB_WEAK } else { STB_GLOBAL };
                let mut g = row(STT_NOTYPE, 0, SymDef::Undefined, FileId::INTERNAL, binding);
                if self.inputs.target.machine == Machine::AArch64
                    && props.refs.contains(RefFlags::VARIANT_PCS)
                {
                    g.other |= STO_AARCH64_VARIANT_PCS;
                }
                g
            }
            // A key no live object references has no entry; a shared
            // definition is M2 (no shared object is loaded in M1).
            Def::Undefined { .. } | Def::Shared(_) => return None,
        })
    }

    fn key_name(&self, key: SymKey) -> &[u8] {
        match key.kind() {
            SymKind::Global(id) => self.inputs.symbols.get(id).map_or(&[][..], |k| k.name),
            SymKind::Local(e) => self.sym(e).and_then(|(s, t)| t.name(&s).ok()).unwrap_or_default(),
        }
    }
}

/// resolve.md §3.7, synth.md S-75, S-121: a DEFINED key's entry from its
/// chosen definition `s` of table `t`. `g` is the key as resolution leaves
/// it: its name (the key's, before any `@`), the resolution's binding, the
/// merged visibility in `other`, the definition, the defining file,
/// whether a live relocation refers to it; its type and size are `s`'s.
/// - A unique definition stays `STB_GNU_UNIQUE` in every output kind,
///   static included (dyn.md D-23; GNU ld and lld; WP-059 B).
/// - `st_other`'s bits above the visibility (`STO_AARCH64_VARIANT_PCS`)
///   are the defining entry's.
/// - A definition its object writes with a version (`foo@V1`, `foo@@V2`,
///   `.symver`) keeps that name, GNU ld's form (WP-150). The key's name is
///   the entry's before any `@` (resolve.md §2.3), so one byte of the
///   entry's string decides: no scan of the name for an unversioned
///   definition.
///
/// The full link's [`CoreSyms`] and an update's global changes (IN-49a)
/// call it.
#[must_use]
pub fn defined_global<'n>(g: GlobalIn<'n>, s: &Sym, t: &SymbolTable<'n>) -> GlobalIn<'n> {
    let binding = if g.binding == STB_GLOBAL && s.st_info >> 4 == STB_GNU_UNIQUE {
        STB_GNU_UNIQUE
    } else {
        g.binding
    };
    let mut g = GlobalIn { typ: s.sym_type(), size: s.st_size, binding, ..g };
    g.other |= s.st_other & !3;
    let strings = t.strings().bytes();
    let at = (s.st_name as usize).saturating_add(g.name.len());
    if strings.get(at) == Some(&b'@')
        && let Ok(full) = t.name(s)
        && full.starts_with(g.name)
    {
        g.name = full;
    }
    g
}

/// [`Sections`] of a link: `CoreSections`, where a section without a
/// place (`spec/resolve.md` §10.1 P-1: dropped, consumed by the link with
/// no output section, layout §8.1: `.note.gnu.property`,
/// `.note.GNU-stack`, `.gnu.warning*`, `.ARM.attributes`; or collected)
/// counts as not live, so its symbols have no entry (synth.md S-76).
#[derive(Debug, Clone, Copy)]
pub struct LinkSections<'a> {
    pub core: CoreSections<'a>,
    pub assignment: &'a Assignment<'a>,
}

impl Sections for LinkSections<'_> {
    fn live(&self, section: SectionId) -> bool {
        mortar_core::place::section_place(
            self.core.resolution,
            Some(self.assignment),
            self.core.liveness,
            section,
        )
        .is_ok()
    }

    fn kind(&self, section: SectionId) -> SecKind {
        self.core.kind(section)
    }

    fn merge_live(&self, section: SectionId, offset: u64) -> bool {
        self.core.merge_live(section, offset)
    }
}
