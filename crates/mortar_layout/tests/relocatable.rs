//! RL-3 (WP-183): layout at `-r` (`spec/layout.md` §24, `spec/model.md`
//! §4.6 MD-R1) on hand-made `Inputs`, `Resolution`, `Liveness` and `Split`
//! over objects this file writes, and `Layout::check`'s L-R10 readings,
//! each against a layout that violates it. One part is tool-backed: the
//! order of a gcc pair against ld.lld's `-r` (L-R8 takes lld's order).
#![allow(
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::many_single_char_names
)]

use std::collections::HashMap;
use std::ops::Range;
use std::process::Command;
use std::time::Duration;

use mortar_core::RelocatableLayout;
use mortar_core::assignment::{Assignment, OutSec};
use mortar_core::check::relocatable::{validate_groups, validate_relocs};
use mortar_core::check::{CheckContext, SectionShapes, Shape, Violation};
use mortar_core::column::Column;
use mortar_core::demands::Demands;
use mortar_core::id::{
    ChunkId, ComdatId, FileId, IdRange, OptId, OutSecId, SectionId, Span, SymEntry, SymbolId,
};
use mortar_core::inputs::{
    Blob, Comdat, ContentHash, CopyError, File, FileBytes, InputFlags, InputStore, Inputs,
    LinkInputs, LinkTarget, Origin, Reader, StoreRef, SymbolKey,
};
use mortar_core::layout::{
    AllocKey, ChunkKind, DefValue, Defined, Hole, HoleKind, Part, Place, Redirect, Segment,
    SegmentRules, Sized as SizedRow, SymPlace, Synth, Tls,
};
use mortar_core::liveness::Liveness;
use mortar_core::par::{Pool, check_schedules};
use mortar_core::plan::Plan;
use mortar_core::resolution::{
    Def, DropReason, FileLive, InternalDef, RefFlags, Resolution, SectionKind, SectionRow,
    SectionState,
};
use mortar_core::split::{EhPiece, Split, SplitRow};
use mortar_elf::{Class, Machine, Object, SectionIndex, SymbolIndex, SymbolSection};
use mortar_layout::headers::{HeaderShapes, gather};
use mortar_layout::relocatable::{
    EntryName, NoteSizes, STRTAB_FIXED, SectionName, eh_frame_size, eh_removed, group_size, layout,
    needs_xindex, relocs_size, shstrtab_size, strtab_size, symtab_entries, symtab_shndx_size,
    symtab_size,
};
use mortar_layout::{LayoutDiag, Target};

// ---------------------------------------------------------------------
// A relocatable-object writer, independent of mortar's own code.
// ---------------------------------------------------------------------

const PROGBITS: u32 = 1;
const SYMTAB_T: u32 = 2;
const STRTAB_T: u32 = 3;
const RELA: u32 = 4;
const NOTE: u32 = 7;
const NOBITS: u32 = 8;
const REL: u32 = 9;
const INIT_ARRAY: u32 = 14;
const GROUP_T: u32 = 17;
const ADDRSIG: u32 = 0x6fff_4c03;

const W: u64 = 1;
const A: u64 = 2;
const X: u64 = 4;
const MERGE: u64 = 0x10;
const STRINGS: u64 = 0x20;
const INFO_LINK: u64 = 0x40;
const LINK_ORDER: u64 = 0x80;
const GROUP: u64 = 0x200;
const TLS: u64 = 0x400;
const COMPRESSED: u64 = 0x800;
const RETAIN: u64 = 0x20_0000;
const EXCLUDE: u64 = 0x8000_0000;

const STT_NOTYPE: u8 = 0;
const STT_OBJECT: u8 = 1;
const STT_FUNC: u8 = 2;
const STT_FILE: u8 = 4;
const GLOBAL: u8 = 0x10;
const SHN_ABS: u16 = 0xfff1;
const SHN_COMMON: u16 = 0xfff2;

/// `sh_link` naming the writer's `.symtab`.
const TO_SYMTAB: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct Sec {
    name: String,
    ty: u32,
    flags: u64,
    align: u64,
    data: Vec<u8>,
    size: u64,
    link: u32,
    info: u32,
    entsize: u64,
    /// A group's signature, by symbol name (its `sh_info`).
    sig: Option<String>,
}

#[derive(Debug, Clone)]
struct Sym {
    name: String,
    info: u8,
    shndx: u16,
    value: u64,
    size: u64,
}

/// An object: user sections 1..=n, then `.symtab`, `.strtab`, `.shstrtab`.
#[derive(Debug, Clone)]
struct Obj {
    c64: bool,
    machine: u16,
    secs: Vec<Sec>,
    locals: Vec<Sym>,
    globals: Vec<Sym>,
}

fn word(c64: bool) -> u64 {
    if c64 { 8 } else { 4 }
}

fn pad(out: &mut Vec<u8>, align: u64) {
    while !(out.len() as u64).is_multiple_of(align.max(1)) {
        out.push(0);
    }
}

fn put(out: &mut Vec<u8>, v: u64, n: usize) {
    // Values are at most 8 bytes; a wider field is zero-extended.
    let k = n.min(8);
    out.extend_from_slice(&v.to_le_bytes()[..k]);
    out.resize(out.len() + (n - k), 0);
}

impl Obj {
    fn new(machine: Machine) -> Self {
        let (c64, em) = match machine {
            Machine::AArch64 => (true, 183),
            Machine::X86_64 => (true, 62),
            Machine::I386 => (false, 3),
        };
        Self { c64, machine: em, secs: Vec::new(), locals: Vec::new(), globals: Vec::new() }
    }

    fn sec(&mut self, name: &str, ty: u32, flags: u64, align: u64, data: Vec<u8>) -> u16 {
        let size = data.len() as u64;
        self.secs.push(Sec {
            name: name.to_owned(),
            ty,
            flags,
            align,
            data,
            size,
            link: 0,
            info: 0,
            entsize: 0,
            sig: None,
        });
        self.secs.len() as u16
    }

    fn bss(&mut self, name: &str, flags: u64, align: u64, size: u64) -> u16 {
        let i = self.sec(name, NOBITS, flags, align, Vec::new());
        self.at(i).size = size;
        i
    }

    fn at(&mut self, i: u16) -> &mut Sec {
        &mut self.secs[i as usize - 1]
    }

    /// The relocation section of `target`, one record at each offset.
    fn rela(&mut self, target: u16, offsets: &[u64]) -> u16 {
        let t = self.secs[target as usize - 1].clone();
        let (prefix, ty, ent) = if self.c64 { (".rela", RELA, 24) } else { (".rel", REL, 8) };
        let mut data = Vec::new();
        for &o in offsets {
            if self.c64 {
                put(&mut data, o, 8);
                put(&mut data, 0, 8);
                put(&mut data, 0, 8);
            } else {
                put(&mut data, o, 4);
                put(&mut data, 0, 4);
            }
        }
        let i = self.sec(
            &format!("{prefix}{}", t.name),
            ty,
            INFO_LINK | (t.flags & GROUP),
            word(self.c64),
            data,
        );
        let s = self.at(i);
        s.link = TO_SYMTAB;
        s.info = u32::from(target);
        s.entsize = ent;
        i
    }

    /// An `SHT_GROUP` section; its members are set by [`Obj::members`].
    fn group(&mut self, comdat: bool, sig: &str) -> u16 {
        let i = self.sec(".group", GROUP_T, 0, 4, Vec::new());
        let s = self.at(i);
        s.link = TO_SYMTAB;
        s.entsize = 4;
        s.sig = Some(sig.to_owned());
        s.data = u32::from(comdat).to_le_bytes().to_vec();
        s.size = 4;
        i
    }

    fn members(&mut self, group: u16, members: &[u16]) {
        let s = self.at(group);
        for &m in members {
            s.data.extend_from_slice(&u32::from(m).to_le_bytes());
        }
        s.size = s.data.len() as u64;
    }

    fn file(&mut self, name: &str) {
        self.locals.push(Sym {
            name: name.into(),
            info: STT_FILE,
            shndx: SHN_ABS,
            value: 0,
            size: 0,
        });
    }

    fn local(&mut self, name: &str, ty: u8, shndx: u16, value: u64) {
        self.locals.push(Sym { name: name.into(), info: ty, shndx, value, size: 0 });
    }

    fn global(&mut self, name: &str, ty: u8, shndx: u16, value: u64) {
        self.globals.push(Sym { name: name.into(), info: GLOBAL | ty, shndx, value, size: 0 });
    }

    fn sym_index(&self, name: &str) -> u32 {
        if let Some(i) = self.locals.iter().position(|s| s.name == name) {
            return i as u32 + 1;
        }
        let g = self.globals.iter().position(|s| s.name == name).expect("signature symbol");
        (1 + self.locals.len() + g) as u32
    }

    fn bytes(&self) -> Vec<u8> {
        let c64 = self.c64;
        let n = self.secs.len() as u32;
        let (symtab_i, strtab_i, shstr_i) = (n + 1, n + 2, n + 3);
        let ehsize = if c64 { 64 } else { 52 };
        let mut out = vec![0u8; ehsize];
        let mut offs = Vec::new();
        for s in &self.secs {
            if s.ty == NOBITS {
                offs.push(out.len() as u64);
            } else {
                pad(&mut out, s.align);
                offs.push(out.len() as u64);
                out.extend_from_slice(&s.data);
            }
        }
        let mut strtab = vec![0u8];
        let mut syms = Vec::new();
        let all: Vec<&Sym> = self.locals.iter().chain(&self.globals).collect();
        put(&mut syms, 0, if c64 { 24 } else { 16 });
        for s in all {
            let name = strtab.len() as u64;
            strtab.extend_from_slice(s.name.as_bytes());
            strtab.push(0);
            if c64 {
                put(&mut syms, name, 4);
                put(&mut syms, u64::from(s.info), 1);
                put(&mut syms, 0, 1);
                put(&mut syms, u64::from(s.shndx), 2);
                put(&mut syms, s.value, 8);
                put(&mut syms, s.size, 8);
            } else {
                put(&mut syms, name, 4);
                put(&mut syms, s.value, 4);
                put(&mut syms, s.size, 4);
                put(&mut syms, u64::from(s.info), 1);
                put(&mut syms, 0, 1);
                put(&mut syms, u64::from(s.shndx), 2);
            }
        }
        pad(&mut out, word(c64));
        let sym_off = out.len() as u64;
        out.extend_from_slice(&syms);
        let str_off = out.len() as u64;
        out.extend_from_slice(&strtab);
        let mut shstr = vec![0u8];
        let mut name_of = |n: &str| {
            let at = shstr.len() as u64;
            shstr.extend_from_slice(n.as_bytes());
            shstr.push(0);
            at
        };
        let names: Vec<u64> = self.secs.iter().map(|s| name_of(&s.name)).collect();
        let (n_sym, n_str, n_shstr) =
            (name_of(".symtab"), name_of(".strtab"), name_of(".shstrtab"));
        let shstr_off = out.len() as u64;
        out.extend_from_slice(&shstr);
        pad(&mut out, 8);
        let shoff = out.len() as u64;
        let sh = |out: &mut Vec<u8>, h: [u64; 10]| {
            if c64 {
                for (k, v) in h.iter().enumerate() {
                    put(out, *v, if matches!(k, 0 | 1 | 6 | 7) { 4 } else { 8 });
                }
            } else {
                for v in h {
                    put(out, v, 4);
                }
            }
        };
        sh(&mut out, [0; 10]);
        let fix = |l: u32| if l == TO_SYMTAB { symtab_i } else { l };
        for (k, s) in self.secs.iter().enumerate() {
            let info = s.sig.as_ref().map_or(s.info, |g| self.sym_index(g));
            sh(
                &mut out,
                [
                    names[k],
                    u64::from(s.ty),
                    s.flags,
                    0,
                    offs[k],
                    s.size,
                    u64::from(fix(s.link)),
                    u64::from(info),
                    s.align,
                    s.entsize,
                ],
            );
        }
        let symsize = if c64 { 24 } else { 16 };
        sh(
            &mut out,
            [
                n_sym,
                u64::from(SYMTAB_T),
                0,
                0,
                sym_off,
                syms.len() as u64,
                u64::from(strtab_i),
                1 + self.locals.len() as u64,
                word(c64),
                symsize,
            ],
        );
        sh(&mut out, [n_str, u64::from(STRTAB_T), 0, 0, str_off, strtab.len() as u64, 0, 0, 1, 0]);
        sh(
            &mut out,
            [n_shstr, u64::from(STRTAB_T), 0, 0, shstr_off, shstr.len() as u64, 0, 0, 1, 0],
        );
        let shnum = u64::from(n) + 4;
        let mut head = Vec::new();
        head.extend_from_slice(b"\x7fELF");
        head.extend_from_slice(&[if c64 { 2 } else { 1 }, 1, 1, 0]);
        head.extend_from_slice(&[0; 8]);
        put(&mut head, 1, 2);
        put(&mut head, u64::from(self.machine), 2);
        put(&mut head, 1, 4);
        let w = word(c64) as usize;
        put(&mut head, 0, w);
        put(&mut head, 0, w);
        put(&mut head, shoff, w);
        put(&mut head, 0, 4);
        put(&mut head, ehsize as u64, 2);
        put(&mut head, 0, 2);
        put(&mut head, 0, 2);
        put(&mut head, if c64 { 64 } else { 40 }, 2);
        put(&mut head, shnum, 2);
        put(&mut head, u64::from(shstr_i), 2);
        out[..ehsize].copy_from_slice(&head);
        out
    }
}

/// `.eh_frame` bytes: one record per length word (4 + length bytes), and
/// a zero terminator when asked.
fn eh_bytes(lengths: &[u32], terminator: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for &l in lengths {
        out.extend_from_slice(&l.to_le_bytes());
        out.resize(out.len() + l as usize, 0);
    }
    if terminator {
        out.extend_from_slice(&[0; 4]);
    }
    out
}

// ---------------------------------------------------------------------
// The tables: `Inputs` (a stand-in for `mortar_input`), a hand-made
// `Resolution` with `-r`'s classification (RL-2's, `rl-sizes` S13).
// ---------------------------------------------------------------------

struct MemStore(Vec<&'static [u8]>);

impl InputStore for MemStore {
    fn resident(&self, blob: Blob, range: Range<u64>) -> Option<&[u8]> {
        self.0.get(blob.0 as usize)?.get(range.start as usize..range.end as usize)
    }

    fn copy_to(&self, blob: Blob, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError> {
        let b = self.resident(blob, range).ok_or(CopyError::OutOfRange)?;
        if b.len() != out.len() {
            return Err(CopyError::OutOfRange);
        }
        out.copy_from_slice(b);
        Ok(())
    }
}

fn leak<T: ?Sized>(b: Box<T>) -> &'static T {
    Box::leak(b)
}

/// The key of a mention (`spec/model.md` §2.3): `foo` for `foo` and
/// `foo@@V`, `(foo, V)` for `foo@V`.
fn split_name(name: &[u8]) -> (&[u8], Option<&[u8]>) {
    match name.iter().position(|&b| b == b'@') {
        Some(at) if name[at..].starts_with(b"@@") => (&name[..at], None),
        Some(at) => (&name[..at], Some(&name[at + 1..])),
        None => (name, None),
    }
}

/// A group's signature name.
fn signature(
    obj: &Object<'static>,
    index: SectionIndex,
) -> (bool, &'static [u8], Vec<SectionIndex>) {
    let g = obj.section(index).unwrap().group().unwrap();
    let t = obj.symbols().unwrap().unwrap();
    let sym = t.get(g.signature()).unwrap();
    let name = if sym.st_info & 0xf == 3 {
        match t.section_of(g.signature(), &sym).unwrap() {
            SymbolSection::Section(s) => obj.section(s).unwrap().name().unwrap(),
            _ => b"",
        }
    } else {
        t.name(&sym).unwrap()
    };
    let members = g.members().map(Result::unwrap).collect();
    (g.is_comdat(), name, members)
}

fn build_inputs(
    machine: Machine,
    objects: &[Vec<u8>],
    extra_keys: &[&'static str],
) -> Inputs<'static> {
    let blobs: Vec<&'static [u8]> =
        objects.iter().map(|o| leak(o.clone().into_boxed_slice())).collect();
    let store: &'static MemStore = leak(Box::new(MemStore(blobs.clone())));
    let mut files = vec![File {
        path: b"",
        member: None,
        origin: Origin::Internal,
        state: InputFlags::empty(),
        reader: Reader::Internal,
        bytes: None,
        sections: 0,
        syms: Span::default(),
        hash: ContentHash(0),
    }];
    let mut sym_map = Vec::new();
    let mut symbols: Vec<SymbolKey<'static>> = Vec::new();
    let mut keys: HashMap<(&[u8], Option<&[u8]>), SymbolId> = HashMap::new();
    let mut comdats: Vec<Comdat<'static>> = Vec::new();
    let mut sigs: HashMap<&[u8], ComdatId> = HashMap::new();
    for (k, &data) in blobs.iter().enumerate() {
        let obj = Object::parse(data).unwrap();
        let start = sym_map.len() as u32;
        if let Some(t) = obj.symbols().unwrap() {
            for (_, sym) in t.globals() {
                let (name, version) = split_name(t.name(&sym).unwrap());
                let next = SymbolId(symbols.len() as u32);
                let id = *keys.entry((name, version)).or_insert(next);
                if id == next {
                    symbols.push(SymbolKey { name, version, hash: 0 });
                }
                sym_map.push(id);
            }
        }
        for i in 0..obj.section_count() {
            if obj.section(SectionIndex(i)).unwrap().sh_type() != GROUP_T {
                continue;
            }
            let (comdat, name, _) = signature(&obj, SectionIndex(i));
            if comdat {
                let next = ComdatId(comdats.len() as u32);
                if *sigs.entry(name).or_insert(next) == next {
                    comdats.push(Comdat { name });
                }
            }
        }
        let path: &'static [u8] = leak(format!("o{k}.o").into_bytes().into_boxed_slice());
        files.push(File {
            path,
            member: None,
            origin: Origin::Input { input: k as u32, arg: k as u32 + 1, entry: None },
            state: InputFlags::empty(),
            reader: Reader::Object(obj),
            bytes: Some(FileBytes { blob: Blob(k as u32), base: 0, len: data.len() as u64 }),
            sections: obj.section_count(),
            syms: Span { start, len: sym_map.len() as u32 - start },
            hash: ContentHash(0),
        });
    }
    for name in extra_keys {
        symbols.push(SymbolKey { name: name.as_bytes(), version: None, hash: 0 });
    }
    let n = files.len() as u32;
    Inputs {
        target: LinkTarget::of(machine),
        files: Column::from_vec(files).unwrap(),
        sym_map: sym_map.into_boxed_slice(),
        symbols: Column::from_vec(symbols).unwrap(),
        comdats: Column::from_vec(comdats).unwrap(),
        archives: Column::new(),
        link: LinkInputs::of_files(n),
        store: StoreRef(store),
    }
}

/// `-r`'s classification (S13): `.note.GNU-stack` is placed (`Regular`),
/// `SHF_EXCLUDE` sections are copied, `.llvm_addrsig` is `Excluded`.
fn kind_of(index: u32, ty: u32, flags: u64, name: &[u8]) -> SectionKind {
    if index == 0 {
        return SectionKind::Null;
    }
    match ty {
        SYMTAB_T => SectionKind::Symtab,
        STRTAB_T => SectionKind::Strtab,
        RELA | REL => SectionKind::Reloc,
        GROUP_T => SectionKind::Group,
        ADDRSIG => SectionKind::Excluded,
        NOBITS => SectionKind::Nobits,
        NOTE => SectionKind::Note,
        _ if name == b".eh_frame" => SectionKind::EhFrame,
        _ if name == b".note.GNU-stack" => SectionKind::Regular,
        _ if flags & MERGE != 0 => SectionKind::Merge,
        _ if flags & A == 0 => SectionKind::Debug,
        _ => SectionKind::Regular,
    }
}

/// The resolution of a `-r` line: every file live; the first copy of a
/// COMDAT group keeps it (its other copies `Dropped(Comdat)`); each key
/// defined by its first definition, else common, else undefined.
fn build_resolution(inputs: &Inputs<'static>) -> Resolution {
    let nfiles = inputs.files.len();
    let mut r = Resolution::empty(nfiles, inputs.symbols.len(), inputs.comdats.len());
    r.file_live = Column::filled(nfiles, FileLive::Always);
    let comdat_of: HashMap<&[u8], ComdatId> =
        inputs.comdats.iter().map(|(id, c)| (c.name, id)).collect();
    let mut base = Vec::new();
    let mut rows: Vec<SectionRow> = Vec::new();
    for (f, file) in inputs.files.iter() {
        let start = rows.len() as u32;
        let Reader::Object(obj) = file.reader else {
            base.push(IdRange::empty(SectionId(start)));
            continue;
        };
        let n = obj.section_count();
        let mut mine: Vec<SectionRow> = (0..n)
            .map(|i| {
                let s = obj.section(SectionIndex(i)).unwrap();
                let kind = kind_of(i, s.sh_type(), s.flags(), s.name().unwrap_or(b""));
                let state = if kind == SectionKind::Excluded {
                    SectionState::Dropped(DropReason::Excluded)
                } else {
                    SectionState::Kept
                };
                SectionRow { kind, state, reloc: SectionIndex(0), comdat: OptId::NONE }
            })
            .collect();
        for i in 1..n {
            let s = obj.section(SectionIndex(i)).unwrap();
            if matches!(s.sh_type(), RELA | REL) {
                let target = s.header().sh_info as usize;
                if target < n as usize {
                    mine[target].reloc = SectionIndex(i);
                }
            }
        }
        for i in 1..n {
            if obj.section(SectionIndex(i)).unwrap().sh_type() != GROUP_T {
                continue;
            }
            let (comdat, name, members) = signature(&obj, SectionIndex(i));
            if !comdat {
                continue;
            }
            let c = comdat_of[name];
            let keeper = if let Some(k) = r.comdat[c].get() {
                k
            } else {
                r.comdat[c] = OptId::some(f);
                f
            };
            let lost = keeper != f;
            for m in members {
                mine[m.0 as usize].comdat = OptId::some(c);
                if lost {
                    mine[m.0 as usize].state = SectionState::Dropped(DropReason::Comdat);
                }
            }
            if lost {
                mine[i as usize].state = SectionState::Dropped(DropReason::Comdat);
            }
        }
        rows.extend(mine);
        base.push(IdRange::new(SectionId(start), SectionId(rows.len() as u32)));
        let keys = inputs.sym_map_of(f);
        if let Some(t) = obj.symbols().unwrap() {
            for (k, (index, sym)) in t.globals().enumerate() {
                let id = keys[k];
                let entry = SymEntry { file: f, index };
                match (sym.st_shndx, r.def[id]) {
                    (0, _) => {}
                    (SHN_COMMON, Def::Undefined { .. }) => r.def[id] = Def::Common(entry),
                    (SHN_COMMON, _) | (_, Def::Defined(_)) => {}
                    _ => r.def[id] = Def::Defined(entry),
                }
            }
        }
    }
    r.section_base = Column::from_vec(base).unwrap();
    r.sections = Column::from_vec(rows).unwrap();
    r
}

// ---------------------------------------------------------------------
// A case: the tables, the run and the checks.
// ---------------------------------------------------------------------

struct Case {
    inputs: Inputs<'static>,
    res: Resolution,
    live: Liveness,
    split: Split,
    target: Target,
    plan: Plan,
    notes: NoteSizes,
}

/// Shapes that cannot be read (rule 1's reading).
struct NoShapes;

impl SectionShapes for NoShapes {
    fn shape(&self, _: SectionId) -> Option<Shape> {
        None
    }
}

impl Case {
    fn new(machine: Machine, objs: &[Obj]) -> Self {
        let bytes: Vec<Vec<u8>> = objs.iter().map(Obj::bytes).collect();
        Self::of_bytes(machine, &bytes, &[])
    }

    fn of_bytes(machine: Machine, bytes: &[Vec<u8>], extra_keys: &[&'static str]) -> Self {
        let inputs = build_inputs(machine, bytes, extra_keys);
        let res = build_resolution(&inputs);
        let n = res.sections.len();
        Self {
            inputs,
            res,
            live: Liveness { live: Column::filled(n, true), frag_live: Column::new() },
            split: Split::default(),
            target: Target::of(machine),
            plan: Plan::default(),
            notes: NoteSizes::default(),
        }
    }

    /// Section `index` of object `obj` (0-based objects; file `obj + 1`).
    fn sid(&self, obj: u32, index: u16) -> SectionId {
        self.res.section_id(FileId(obj + 1), SectionIndex(u32::from(index))).unwrap()
    }

    fn row(&mut self, obj: u32, index: u16) -> &mut SectionRow {
        let s = self.sid(obj, index);
        &mut self.res.sections[s]
    }

    fn run_on(&self, pool: &Pool) -> Result<RelocatableLayout<'static>, Vec<LayoutDiag>> {
        layout(
            &self.inputs,
            &self.res,
            &self.live,
            &self.split,
            &self.target,
            &self.plan,
            self.notes,
            pool,
        )
    }

    fn run(&self) -> Result<RelocatableLayout<'static>, Vec<LayoutDiag>> {
        self.run_on(&Pool::new(4))
    }

    /// The layout, which every reading of L-R10 accepts.
    fn ok(&self) -> RelocatableLayout<'static> {
        let r = self.run().unwrap_or_else(|e| panic!("{:?}", messages(&e)));
        if let Err(v) = self.check(&r) {
            dump(&r);
            panic!("invalid layout: {v:?}");
        }
        r
    }

    /// The errors, which the run must give.
    fn err(&self) -> Vec<String> {
        match self.run() {
            Ok(r) => {
                dump(&r);
                panic!("the layout succeeded");
            }
            Err(e) => messages(&e),
        }
    }

    fn with_cx<R>(
        &self,
        a: &Assignment<'static>,
        unreadable: bool,
        f: impl FnOnce(&CheckContext<'_, 'static>) -> R,
    ) -> R {
        let pool = Pool::new(2);
        let headers = gather(&self.inputs, &self.res, &pool).unwrap();
        let demands = Demands::empty(self.inputs.symbols.len());
        let shapes = HeaderShapes(&headers);
        let cx = CheckContext {
            class: self.target.class,
            page_size: self.target.max_page,
            needs_entry: false,
            segment_rules: SegmentRules {
                class: self.target.class,
                max_page: self.target.max_page,
                stack: 6,
                pie: false,
            },
            resolution: &self.res,
            assignment: a,
            split: &self.split,
            liveness: &self.live,
            demands: &demands,
            shapes: if unreadable { &NoShapes } else { &shapes },
            shared: false,
            placeless: &[],
        };
        f(&cx)
    }

    /// `Layout::check` (dispatching to L-R10's readings), then the group
    /// and relocation rules with the facts of the inputs.
    fn check(&self, r: &RelocatableLayout<'static>) -> Result<(), Violation> {
        let kept = self.kept_groups();
        let kept: Vec<(SectionId, &[SectionId])> =
            kept.iter().map(|(g, m)| (*g, m.as_slice())).collect();
        let records = self.records(r);
        self.with_cx(&r.assignment, false, |cx| {
            r.layout.validate(cx)?;
            validate_groups(&r.layout, cx, &kept)?;
            validate_relocs(&r.layout, cx, &records)
        })
    }

    fn object(&self, file: FileId) -> Object<'static> {
        let Reader::Object(o) = self.inputs.files[file].reader else { panic!("not an object") };
        o
    }

    /// The kept groups with their placed members, read from the inputs.
    fn kept_groups(&self) -> Vec<(SectionId, Vec<SectionId>)> {
        let mut out = Vec::new();
        for (s, row) in self.res.sections.iter() {
            if row.kind != SectionKind::Group || !row.is_kept() {
                continue;
            }
            let (file, index) = self.res.sh_index(s).unwrap();
            let (_, _, members) = signature(&self.object(file), index);
            let placed = members
                .into_iter()
                .map(|m| self.res.section_id(file, m).unwrap())
                .filter(|&m| {
                    let r = self.res.sections[m];
                    r.is_kept()
                        && self.live.live[m]
                        && mortar_core::check::relocatable::placed_at_r(r.kind)
                })
                .collect();
            out.push((s, placed));
        }
        out
    }

    /// The output records per output section (I-R5), read from the inputs:
    /// every record of every piece's relocation section, less the records
    /// inside removed `.eh_frame` records.
    fn records(&self, r: &RelocatableLayout<'static>) -> Vec<u64> {
        let mut out = vec![0u64; r.assignment.outsecs.len() as usize];
        for (s, row) in self.res.sections.iter() {
            let Some(o) = r.assignment.out(s) else { continue };
            if row.reloc.0 == 0 || !row.is_kept() {
                continue;
            }
            let (file, _) = self.res.sh_index(s).unwrap();
            let obj = self.object(file);
            let rs = obj.section(row.reloc).unwrap();
            let ent = if self.target.class == Class::Elf64 { 24 } else { 8 };
            let data = rs.data().unwrap();
            let offsets: Vec<u64> = data
                .chunks(ent)
                .map(|c| {
                    let w = if ent == 24 { 8 } else { 4 };
                    c[..w].iter().rev().fold(0, |v, &b| (v << 8) | u64::from(b))
                })
                .collect();
            let (_, index) = self.res.sh_index(s).unwrap();
            let bytes = obj.section(index).unwrap().data().unwrap();
            let gone: Vec<(u64, u64)> = r
                .eh_removed
                .iter()
                .filter(|x| x.0 == s)
                .map(|&(_, off)| {
                    let at = off as usize;
                    let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                    (off, off + 4 + u64::from(len))
                })
                .collect();
            let kept = offsets.iter().filter(|&&x| !gone.iter().any(|g| g.0 <= x && x < g.1));
            out[o.0 as usize] += kept.count() as u64;
        }
        out
    }
}

fn messages(e: &[LayoutDiag]) -> Vec<String> {
    e.iter().map(|d| format!("{}: {}", d.id(), d.message())).collect()
}

/// A chunk's name as the output's section header names it.
fn name_of(r: &RelocatableLayout<'_>, c: ChunkId) -> String {
    let a = &r.assignment;
    let ch = &r.layout.placement.chunks[c];
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    match ch.kind {
        ChunkKind::FileHeader => "<fh>".into(),
        ChunkKind::SectionHeaders => "<sh>".into(),
        ChunkKind::Body(p) => text(a.outsecs[p.out].name),
        ChunkKind::Relocs(o) => {
            let prefix = if ch.sh_type == REL { ".rel" } else { ".rela" };
            format!("{prefix}{}", text(a.outsecs[o].name))
        }
        ChunkKind::Group(_) => ".group".into(),
        ChunkKind::Synthetic(s) => text(s.name),
        other => format!("{other:?}"),
    }
}

fn names(r: &RelocatableLayout<'_>) -> Vec<String> {
    r.layout.placement.order.iter().map(|&c| name_of(r, c)).collect()
}

/// The first chunk of that name, in order.
fn chunk(r: &RelocatableLayout<'_>, name: &str) -> ChunkId {
    *r.layout
        .placement
        .order
        .iter()
        .find(|&&c| name_of(r, c) == name)
        .unwrap_or_else(|| panic!("no chunk {name} in {:?}", names(r)))
}

fn size(r: &RelocatableLayout<'_>, name: &str) -> u64 {
    r.layout.placement.chunks[chunk(r, name)].size
}

fn outsecs_named(r: &RelocatableLayout<'_>, name: &str) -> Vec<OutSecId> {
    r.assignment
        .outsecs
        .iter()
        .filter(|(_, o)| o.name == name.as_bytes())
        .map(|(id, _)| id)
        .collect()
}

fn dump(r: &RelocatableLayout<'_>) {
    for &c in &r.layout.placement.order {
        let ch = &r.layout.placement.chunks[c];
        eprintln!(
            "{:>24} type={:#x} flags={:#x} off={:#x} size={:#x} align={} shndx={:?}",
            name_of(r, c),
            ch.sh_type,
            ch.flags,
            ch.offset,
            ch.size,
            ch.align,
            ch.shndx
        );
    }
}

/// The `.shstrtab` size of `names` written out by hand: a NUL, then each
/// distinct name and its NUL.
fn cstrings(names: &[&str]) -> u64 {
    let mut seen: Vec<&str> = Vec::new();
    let mut size = 1;
    for n in names {
        if !seen.contains(n) {
            seen.push(n);
            size += n.len() as u64 + 1;
        }
    }
    size
}

/// L-R8's offsets: each chunk at the next offset aligned to its
/// alignment after the previous one's file bytes; the table last,
/// word-aligned; section indexes dense from 1.
fn assert_offsets(r: &RelocatableLayout<'_>, class: Class) {
    let p = &r.layout.placement;
    let (fh, w) = match class {
        Class::Elf64 => (64, 8),
        Class::Elf32 => (52, 4),
    };
    let first = &p.chunks[p.order[0]];
    assert_eq!((first.kind, first.offset, first.size), (ChunkKind::FileHeader, 0, fh));
    let mut at = fh;
    for (k, &c) in p.order[1..].iter().enumerate() {
        let ch = &p.chunks[c];
        assert_eq!(ch.addr, None);
        if ch.kind == ChunkKind::SectionHeaders {
            assert_eq!(k + 2, p.order.len(), "the table is last");
            assert_eq!(ch.offset, at.next_multiple_of(w));
            assert_eq!(ch.size, (k as u64 + 1) * if w == 8 { 64 } else { 40 });
            continue;
        }
        assert_eq!(ch.shndx, Some(k as u32 + 1), "{}", name_of(r, c));
        let align = 1u64 << ch.align;
        assert_eq!(ch.offset, at.next_multiple_of(align), "{}", name_of(r, c));
        if ch.sh_type != NOBITS {
            at = ch.offset + ch.size;
        }
    }
}

// ---------------------------------------------------------------------
// The objects of the tests.
// ---------------------------------------------------------------------

/// `a.c` of the probe: `.text` (relocated), `.data`, `.bss`, `.comment`,
/// `.note.GNU-stack`, `.eh_frame` (relocated); `f` defined, `g`
/// undefined.
fn object_a(m: Machine) -> Obj {
    let mut o = Obj::new(m);
    o.file("a.c");
    let text = o.sec(".text", PROGBITS, A | X, 16, vec![0; 0x14]);
    o.rela(text, &[0, 8]);
    let data = o.sec(".data", PROGBITS, W | A, 8, vec![0; 8]);
    o.bss(".bss", W | A, 4, 4);
    let comment = o.sec(".comment", PROGBITS, MERGE | STRINGS, 1, vec![b'c'; 0x20]);
    o.at(comment).entsize = 1;
    o.sec(".note.GNU-stack", PROGBITS, 0, 1, Vec::new());
    let eh = o.sec(".eh_frame", PROGBITS, A, 8, eh_bytes(&[0x14, 0x14], false));
    o.rela(eh, &[0x20]);
    o.local("x", STT_OBJECT, data, 0);
    o.global("f", STT_FUNC, text, 0);
    o.global("g", STT_NOTYPE, 0, 0);
    o
}

/// `b.c` of the probe: `.text`, `.data`, `.rodata.str1.8`, `.comment`,
/// `.note.GNU-stack` (with a local in it), `.eh_frame`,
/// `.data.rel.local`; `g` defined, `f` undefined, `h` common.
fn object_b(m: Machine) -> Obj {
    let mut o = Obj::new(m);
    o.file("b.c");
    let text = o.sec(".text", PROGBITS, A | X, 4, vec![0; 0x30]);
    o.rela(text, &[0, 4, 8]);
    o.sec(".data", PROGBITS, W | A, 8, vec![0; 8]);
    let ro = o.sec(".rodata.str1.8", PROGBITS, A | MERGE | STRINGS, 8, vec![0; 0x10]);
    o.at(ro).entsize = 1;
    let comment = o.sec(".comment", PROGBITS, MERGE | STRINGS, 1, vec![b'c'; 0x20]);
    o.at(comment).entsize = 1;
    let stack = o.sec(".note.GNU-stack", PROGBITS, 0, 1, Vec::new());
    let eh = o.sec(".eh_frame", PROGBITS, A, 8, eh_bytes(&[0x14, 0x14], false));
    o.rela(eh, &[0x20]);
    let dr = o.sec(".data.rel.local", PROGBITS, W | A, 8, vec![0; 8]);
    o.rela(dr, &[0]);
    o.local("in_stack", STT_NOTYPE, stack, 0);
    o.global("g", STT_FUNC, text, 0);
    o.global("f", STT_NOTYPE, 0, 0);
    o.global("h", STT_OBJECT, SHN_COMMON, 8);
    o
}

// ---------------------------------------------------------------------
// L-R8, L-R4, L-R5: two C objects.
// ---------------------------------------------------------------------

#[test]
fn r_section_order() {
    let m = Machine::X86_64;
    let case = Case::new(m, &[object_a(m), object_b(m)]);
    let r = case.ok();
    assert_eq!(
        names(&r),
        [
            "<fh>",
            ".text",
            ".rela.text",
            ".data",
            ".bss",
            ".comment",
            ".note.GNU-stack",
            ".eh_frame",
            ".rela.eh_frame",
            ".rodata.str1.8",
            ".data.rel.local",
            ".rela.data.rel.local",
            ".symtab",
            ".strtab",
            ".shstrtab",
            "<sh>",
        ]
    );
    assert_offsets(&r, Class::Elf64);
    // L-R4: b's .text (alignment 4) right after a's 0x14 bytes.
    assert_eq!(size(&r, ".text"), 0x44);
    let text_b = case.sid(1, 1);
    assert_eq!(r.layout.placement.place[text_b].get().unwrap().1, 0x14);
    assert_eq!(size(&r, ".rela.text"), 5 * 24);
    assert_eq!(size(&r, ".data"), 0x10);
    assert_eq!(size(&r, ".bss"), 4);
    // L-R5: not merged, MERGE|STRINGS kept.
    assert_eq!(size(&r, ".comment"), 0x40);
    assert_eq!(r.layout.placement.chunks[chunk(&r, ".comment")].flags, MERGE | STRINGS);
    assert_eq!(size(&r, ".note.GNU-stack"), 0);
    assert_eq!(size(&r, ".eh_frame"), 0x60);
    assert_eq!(size(&r, ".rela.eh_frame"), 2 * 24);
    assert_eq!(size(&r, ".rodata.str1.8"), 0x10);
    // S4: null, 8 section symbols, a.c x b.c (in_stack dropped), f g h.
    assert_eq!(size(&r, ".symtab"), 15 * 24);
    // S2: the fixed 31 bytes, then the names.
    assert_eq!(size(&r, ".strtab"), 31 + 4 + 2 + 4 + 2 + 2 + 2);
    assert_eq!(
        size(&r, ".shstrtab"),
        cstrings(&[
            ".text",
            ".rela.text",
            ".data",
            ".bss",
            ".comment",
            ".note.GNU-stack",
            ".eh_frame",
            ".rela.eh_frame",
            ".rodata.str1.8",
            ".data.rel.local",
            ".rela.data.rel.local",
            ".symtab",
            ".strtab",
            ".shstrtab",
        ])
    );
    // MD-R1: a .eh_frame input has no place, its output section no body.
    let eh_a = case.sid(0, 7);
    assert!(r.layout.placement.place[eh_a].get().is_none());
    assert!(r.eh_removed.is_empty());
    // The chunk types and flags.
    let p = &r.layout.placement;
    let rt = &p.chunks[chunk(&r, ".rela.text")];
    assert_eq!((rt.sh_type, rt.flags, rt.align), (RELA, INFO_LINK, 3));
    let bss = &p.chunks[chunk(&r, ".bss")];
    assert_eq!((bss.sh_type, bss.flags), (NOBITS, W | A));
    assert_eq!(bss.offset, p.chunks[chunk(&r, ".comment")].offset);
}

#[test]
fn r_section_order_i386() {
    let m = Machine::I386;
    let case = Case::new(m, &[object_a(m), object_b(m)]);
    let r = case.ok();
    assert_offsets(&r, Class::Elf32);
    assert_eq!(names(&r)[2], ".rel.text");
    let rel = &r.layout.placement.chunks[chunk(&r, ".rel.text")];
    assert_eq!((rel.sh_type, rel.size, rel.align), (REL, 5 * 8, 2));
    assert_eq!(size(&r, ".symtab"), 15 * 16);
    assert!(names(&r).iter().any(|n| n == ".rel.eh_frame"));
    let sh = r.layout.placement.chunks[*r.layout.placement.order.last().unwrap()];
    assert_eq!(sh.size, 15 * 40);
    // SHT_RELA on i386 is refused by name.
    let mut a = Obj::new(m);
    let t = a.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    let r = a.rela(t, &[0]);
    a.at(r).ty = RELA;
    a.at(r).entsize = 12;
    let errs = Case::new(m, &[a]).err();
    assert!(errs.iter().any(|e| e.contains("SHT_RELA")), "{errs:?}");
}

#[test]
fn r_deterministic() {
    let m = Machine::AArch64;
    let case = Case::new(m, &[object_a(m), object_b(m), object_a(m)]);
    let _ = check_schedules(|pool| format!("{:?}", case.run_on(pool)));
}

// ---------------------------------------------------------------------
// L-R1, L-R1a: names, types, flags.
// ---------------------------------------------------------------------

#[test]
fn r_names_kept() {
    let m = Machine::X86_64;
    let mut a = Obj::new(m);
    a.sec(".data", PROGBITS, A, 4, vec![0; 4]);
    a.sec(".text.foo", PROGBITS, A | X, 4, vec![0; 4]);
    a.sec(".mixed", PROGBITS, W | A, 4, vec![0; 6]);
    a.sec(".init_array", INIT_ARRAY, W | A, 8, vec![0; 8]);
    a.sec(".init_array.101", INIT_ARRAY, W | A, 8, vec![0; 8]);
    let s1 = a.sec(".rodata.s", PROGBITS, A | MERGE | STRINGS, 1, vec![0; 3]);
    a.at(s1).entsize = 1;
    let s2 = a.sec(".rodata.k", PROGBITS, A | MERGE, 4, vec![0; 8]);
    a.at(s2).entsize = 4;
    let mut b = Obj::new(m);
    b.sec(".data", PROGBITS, W | A, 16, vec![0; 4]);
    b.sec(".text.foo", PROGBITS, A | X, 4, vec![0; 4]);
    b.bss(".mixed", W | A, 8, 8);
    b.sec(".init_array", PROGBITS, W | A, 8, vec![0; 8]);
    let s1 = b.sec(".rodata.s", PROGBITS, A | MERGE | STRINGS, 1, vec![0; 5]);
    b.at(s1).entsize = 1;
    let s2 = b.sec(".rodata.k", PROGBITS, A | MERGE, 8, vec![0; 8]);
    b.at(s2).entsize = 8;
    let case = Case::new(m, &[a, b]);
    let r = case.ok();
    let p = &r.layout.placement;
    let get = |n: &str| p.chunks[chunk(&r, n)];
    // "a" and "aw" give WA and the largest alignment.
    let data = get(".data");
    assert_eq!((data.flags, data.align, data.size), (W | A, 4, 0x14));
    // Same names merge; the prefix table does not apply.
    assert_eq!(outsecs_named(&r, ".text.foo").len(), 1);
    assert!(outsecs_named(&r, ".text").is_empty());
    // PROGBITS and NOBITS: PROGBITS (L-49), the NOBITS piece takes bytes.
    let mixed = get(".mixed");
    assert_eq!((mixed.sh_type, mixed.size), (PROGBITS, 0x10));
    // INIT_ARRAY with PROGBITS stays INIT_ARRAY; .init_array.N apart.
    assert_eq!(get(".init_array").sh_type, INIT_ARRAY);
    assert_eq!(get(".init_array.101").size, 8);
    // A uniform merge class is kept; a mixed one is dropped (L-50).
    let s = get(".rodata.s");
    assert_eq!((s.flags, s.size), (A | MERGE | STRINGS, 8));
    let o = outsecs_named(&r, ".rodata.s")[0];
    assert!(r.assignment.outsecs[o].merge.is_some());
    let k = get(".rodata.k");
    assert_eq!(k.flags, A);
    assert!(r.assignment.outsecs[outsecs_named(&r, ".rodata.k")[0]].merge.is_none());
    // Merge pieces have a place, whole (L-R5).
    let rs = case.sid(1, 5);
    assert_eq!(r.layout.placement.place[rs].get().unwrap().1, 3);
}

#[test]
fn r_retain_split() {
    let m = Machine::X86_64;
    let mut a = Obj::new(m);
    a.sec(".text.k", PROGBITS, A | X | RETAIN, 4, vec![0; 4]);
    let mut b = Obj::new(m);
    b.sec(".text.k", PROGBITS, A | X, 4, vec![0; 4]);
    b.sec(".text.k", PROGBITS, A | X | RETAIN, 4, vec![0; 4]);
    let r = Case::new(m, &[a, b]).ok();
    let outs = outsecs_named(&r, ".text.k");
    assert_eq!(outs.len(), 2);
    let flags: Vec<u64> = outs.iter().map(|&o| r.assignment.outsecs[o].flags).collect();
    assert_eq!(flags, [A | X | RETAIN, A | X]);
    let sizes: Vec<u64> = r
        .layout
        .placement
        .order
        .iter()
        .filter(|&&c| name_of(&r, c) == ".text.k")
        .map(|&c| r.layout.placement.chunks[c].size)
        .collect();
    assert_eq!(sizes, [8, 4]);
}

#[test]
fn r_section_conflicts() {
    let m = Machine::X86_64;
    // A LINK_ORDER mix cannot meet in one output section: the flag's
    // linked-to output section is part of L-R1's key.
    let pairs: [(u32, u64, u32, u64, &str); 5] = [
        (PROGBITS, A, PROGBITS, 0, "non-allocated"),
        (PROGBITS, W | A | TLS, PROGBITS, W | A, "non-TLS"),
        (PROGBITS, EXCLUDE, PROGBITS, 0, "SHF_EXCLUDE"),
        (NOTE, A, PROGBITS, A, "section-conflict"),
        (INIT_ARRAY, W | A, 15, W | A, "section-conflict"),
    ];
    for (t1, f1, t2, f2, want) in pairs {
        let mut a = Obj::new(m);
        a.sec(".x", PROGBITS, A | X, 4, vec![0; 4]);
        a.sec(".s", t1, f1, 4, vec![0; 4]);
        let mut b = Obj::new(m);
        b.sec(".s", t2, f2, 4, vec![0; 4]);
        let errs = Case::new(m, &[a, b]).err();
        assert!(
            errs.iter().any(|e| e.starts_with("section-conflict") && e.contains(want)),
            "{t1:#x}/{f1:#x} with {t2:#x}/{f2:#x}: {errs:?}"
        );
    }
}

#[test]
fn r_exclude_kept_and_addrsig_dropped() {
    let m = Machine::X86_64;
    let mut a = Obj::new(m);
    a.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    a.sec(".gnu.warning.f", PROGBITS, EXCLUDE, 1, vec![0; 3]);
    a.sec(".llvm_addrsig", ADDRSIG, EXCLUDE, 1, vec![1]);
    let mut b = Obj::new(m);
    b.sec(".gnu.warning.f", PROGBITS, EXCLUDE, 1, vec![0; 2]);
    let case = Case::new(m, &[a, b]);
    let r = case.ok();
    assert_eq!(names(&r)[1..3], [".text", ".gnu.warning.f"]);
    let w = r.layout.placement.chunks[chunk(&r, ".gnu.warning.f")];
    assert_eq!((w.flags, w.size), (EXCLUDE, 5));
    assert!(!names(&r).iter().any(|n| n == ".llvm_addrsig"));
    // S13: SHF_EXCLUDE classified Excluded is resolution's error.
    let mut d = Obj::new(m);
    d.sec(".x", PROGBITS, EXCLUDE, 1, vec![0]);
    let mut e = Case::new(m, &[d]);
    e.row(0, 1).kind = SectionKind::Excluded;
    e.row(0, 1).state = SectionState::Dropped(DropReason::Excluded);
    let errs = e.err();
    assert!(errs.iter().any(|x| x.starts_with("internal-error") && x.contains(".x")), "{errs:?}");
    // S13: a Stack section is resolution's error.
    let mut c = Case::new(m, &[object_a(m)]);
    c.row(0, 6).kind = SectionKind::Stack;
    let errs = c.err();
    assert!(
        errs.iter().any(|x| x.starts_with("internal-error") && x.contains("GNU-stack")),
        "{errs:?}"
    );
}

#[test]
fn r_stack_note() {
    // The notes merge into one empty output section; an input's
    // SHF_EXECINSTR is kept (L-R6a's X; the options are RL-7's); the
    // local defined in a note is not written (I-R4).
    let m = Machine::AArch64;
    let mut b = object_b(m);
    b.at(6).flags = X;
    let case = Case::new(m, &[object_a(m), b]);
    let r = case.ok();
    let stack = r.layout.placement.chunks[chunk(&r, ".note.GNU-stack")];
    assert_eq!((stack.sh_type, stack.flags, stack.size, stack.align), (PROGBITS, X, 0, 0));
    assert_eq!(outsecs_named(&r, ".note.GNU-stack").len(), 1);
    assert_eq!(size(&r, ".symtab"), 15 * 24);
}

#[test]
fn r_compressed_input() {
    // S15: a compressed input's decompressed size and alignment; the
    // output is not compressed.
    let m = Machine::X86_64;
    let mut a = Obj::new(m);
    let mut chdr = Vec::new();
    put(&mut chdr, 1, 4);
    put(&mut chdr, 0, 4);
    put(&mut chdr, 0x100, 8);
    put(&mut chdr, 4, 8);
    chdr.extend_from_slice(&[0x78, 0x9c, 3, 0, 0, 0, 0, 1]);
    a.sec(".debug_info", PROGBITS, COMPRESSED, 8, chdr);
    let mut b = Obj::new(m);
    b.sec(".debug_info", PROGBITS, 0, 1, vec![0; 3]);
    let case = Case::new(m, &[a, b]);
    let r = case.ok();
    let d = r.layout.placement.chunks[chunk(&r, ".debug_info")];
    assert_eq!((d.flags, d.size, d.align), (0, 0x103, 2));
}

// ---------------------------------------------------------------------
// L-R2, L-R3: groups.
// ---------------------------------------------------------------------

/// A plain `.text`, the COMDAT group `foo` (`.text.foo` and its
/// relocations, `foo` defined in it), `.data`.
fn object_group(m: Machine, text_first: bool) -> Obj {
    let mut o = Obj::new(m);
    if text_first {
        o.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    }
    let g = o.group(true, "foo");
    let tf = o.sec(".text.foo", PROGBITS, A | X | GROUP, 4, vec![0; 8]);
    let rtf = o.rela(tf, &[0]);
    o.members(g, &[tf, rtf]);
    if !text_first {
        let t = o.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
        o.rela(t, &[0]);
    }
    o.sec(".data", PROGBITS, W | A, 4, vec![0; 4]);
    o.global("foo", STT_FUNC, tf, 0);
    o
}

#[test]
fn r_group_comdat() {
    let m = Machine::X86_64;
    let a = object_group(m, true);
    let mut b = object_group(m, false);
    // b also has a non-COMDAT type-unit group whose signature is a local
    // defined in the group section itself (L-R3).
    let tg = b.group(false, "wt.1234");
    let dt = b.sec(".debug_types", PROGBITS, GROUP, 1, vec![0; 0x10]);
    b.members(tg, &[dt]);
    b.local("wt.1234", STT_NOTYPE, tg, 0);
    b.local("other", STT_NOTYPE, tg, 0);
    let case = Case::new(m, &[a, b]);
    let r = case.ok();
    assert_eq!(
        names(&r),
        [
            "<fh>",
            ".text",
            ".rela.text",
            ".group",
            ".text.foo",
            ".rela.text.foo",
            ".data",
            ".group",
            ".debug_types",
            ".symtab",
            ".strtab",
            ".shstrtab",
            "<sh>",
        ]
    );
    assert_offsets(&r, Class::Elf64);
    let p = &r.layout.placement;
    let groups: Vec<_> = p.order.iter().filter(|&&c| name_of(&r, c) == ".group").collect();
    // The keeper's group: the member and its relocation section.
    let g1 = p.chunks[*groups[0]];
    assert_eq!(g1.kind, ChunkKind::Group(case.sid(0, 2)));
    assert_eq!((g1.sh_type, g1.size, g1.align, g1.flags), (GROUP_T, group_size(1, 1), 2, 0));
    assert_eq!(g1.size, 12);
    let g2 = p.chunks[*groups[1]];
    assert_eq!((g2.kind, g2.size), (ChunkKind::Group(case.sid(1, 7)), 8));
    // SHF_GROUP on members and their relocation sections only.
    assert_eq!(p.chunks[chunk(&r, ".text.foo")].flags, A | X | GROUP);
    assert_eq!(p.chunks[chunk(&r, ".rela.text.foo")].flags, INFO_LINK | GROUP);
    assert_eq!(p.chunks[chunk(&r, ".debug_types")].flags, GROUP);
    assert_eq!(p.chunks[chunk(&r, ".rela.text")].flags, INFO_LINK);
    // The loser's copy has no output section; the keeper's is its own
    // output section, not merged with a same-named one.
    assert_eq!(r.assignment.out(case.sid(1, 2)), None);
    assert_eq!(outsecs_named(&r, ".text.foo").len(), 1);
    // S4: null; .text .text.foo .data .debug_types; wt.1234 (not
    // `other`); foo.
    assert_eq!(size(&r, ".symtab"), (1 + 4 + 1 + 1) * 24);
    assert_eq!(size(&r, ".strtab"), 31 + 8 + 4);
}

#[test]
fn r_group_member_outside_group_kept_apart() {
    // L-R2: a member is never merged with a same-named section outside
    // its group; two non-COMDAT copies stay apart.
    let m = Machine::X86_64;
    let mut a = Obj::new(m);
    a.sec(".text.foo", PROGBITS, A | X, 4, vec![0; 4]);
    let g = a.group(false, "s");
    let t = a.sec(".text.foo", PROGBITS, A | X | GROUP, 4, vec![0; 4]);
    a.members(g, &[t]);
    a.global("s", STT_NOTYPE, 0, 0);
    let r = Case::new(m, &[a.clone(), a]).ok();
    assert_eq!(outsecs_named(&r, ".text.foo").len(), 3);
    assert_eq!(names(&r).iter().filter(|n| *n == ".group").count(), 2);
}

#[test]
fn r_group_refusals() {
    let m = Machine::X86_64;
    // An .eh_frame member is RL-8's.
    let mut a = Obj::new(m);
    let g = a.group(true, "foo");
    let e = a.sec(".eh_frame", PROGBITS, A | GROUP, 8, eh_bytes(&[0x14], false));
    a.members(g, &[e]);
    a.global("foo", STT_NOTYPE, 0, 0);
    let errs = Case::new(m, &[a]).err();
    assert!(errs.iter().any(|x| x.contains(".eh_frame in a section group")), "{errs:?}");
    // --force-group-allocation is RL-8's.
    let mut c = Case::new(m, &[object_group(m, true)]);
    c.plan.force_group_allocation = true;
    let errs = c.err();
    assert!(errs.iter().any(|x| x.contains("--force-group-allocation")), "{errs:?}");
}

// ---------------------------------------------------------------------
// I-R4, S5: `.symtab_shndx`.
// ---------------------------------------------------------------------

#[test]
fn r_many_sections() {
    let m = Machine::X86_64;
    let per = 33_000u32;
    let objs: Vec<Obj> = ["a", "b"]
        .iter()
        .map(|p| {
            let mut o = Obj::new(m);
            for i in 0..per {
                o.sec(&format!(".t.{p}{i}"), PROGBITS, A | X, 1, vec![0]);
            }
            o
        })
        .collect();
    let case = Case::new(m, &objs);
    let r = case.ok();
    let n = names(&r);
    let tail: Vec<&str> = n[n.len() - 5..].iter().map(String::as_str).collect();
    assert_eq!(tail, [".symtab", ".symtab_shndx", ".strtab", ".shstrtab", "<sh>"]);
    let entries = 1 + 2 * u64::from(per);
    assert_eq!(size(&r, ".symtab"), entries * 24);
    assert_eq!(size(&r, ".symtab_shndx"), entries * 4);
    let sx = r.layout.placement.chunks[chunk(&r, ".symtab_shndx")];
    assert_eq!((sx.sh_type, sx.align), (18, 2));
    assert_offsets(&r, Class::Elf64);
    // Under 0xff00 sections, none.
    let small = Case::new(m, &[object_a(m)]).ok();
    assert!(!names(&small).iter().any(|x| x == ".symtab_shndx"));
}

// ---------------------------------------------------------------------
// L-R6, S1, S12: `.eh_frame`.
// ---------------------------------------------------------------------

/// a: the COMDAT group `foo` kept. b: its copy (loser), `.text`, and an
/// `.eh_frame` of a CIE, an FDE of `.text`, an FDE of `.text.foo` and a
/// terminator, with a local in each FDE.
fn eh_case(m: Machine) -> Case {
    eh_case_with(m, &[])
}

/// [`eh_case`] with more objects after the two.
fn eh_case_with(m: Machine, more: &[Obj]) -> Case {
    let a = object_group(m, true);
    let mut b = Obj::new(m);
    let g = b.group(true, "foo");
    let tf = b.sec(".text.foo", PROGBITS, A | X | GROUP, 4, vec![0; 8]);
    b.members(g, &[tf]);
    let text = b.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    let eh = b.sec(".eh_frame", PROGBITS, A, 8, eh_bytes(&[0x14, 0x1c, 0x1c], true));
    b.rela(eh, &[0x20, 0x40]);
    b.local("kept", STT_NOTYPE, eh, 0x18);
    b.local("gone", STT_NOTYPE, eh, 0x40);
    b.global("foo", STT_FUNC, tf, 0);
    let mut objs = vec![a, b];
    objs.extend_from_slice(more);
    let mut case = Case::new(m, &objs);
    let ehs = case.sid(1, eh);
    let fde = |code| EhPiece { cie: 0, code: OptId::some(code) };
    case.split = Split {
        eh_of: Box::new([SplitRow { section: ehs, pieces: Span { start: 0, len: 3 } }]),
        eh_off: Box::new([0, 0x18, 0x38]),
        eh_piece: Box::new([EhPiece::CIE, fde(case.sid(1, text)), fde(case.sid(1, tf))]),
        ..Split::default()
    };
    case
}

#[test]
fn r_eh_frame_dropped_fde() {
    let m = Machine::X86_64;
    let case = eh_case(m);
    let r = case.ok();
    let ehs = case.sid(1, 4);
    assert_eq!(&*r.eh_removed, &[(ehs, 0x38)]);
    // S12: 0x5c less the removed FDE's 4 + 0x1c.
    assert_eq!(size(&r, ".eh_frame"), 0x5c - 0x20);
    assert_eq!(size(&r, ".eh_frame"), eh_frame_size([0x5c], [0x20]));
    // S1: its relocation record goes with it.
    assert_eq!(size(&r, ".rela.eh_frame"), 24);
    let eh = r.layout.placement.chunks[chunk(&r, ".eh_frame")];
    assert_eq!(
        eh.kind,
        ChunkKind::Synthetic(Synth { name: b".eh_frame", address_dependent: false })
    );
    assert_eq!((eh.sh_type, eh.flags, eh.align), (PROGBITS, A, 3));
    // The local in the removed FDE is not written: null; .text .text.foo
    // .data .eh_frame; `kept`; foo.
    assert_eq!(size(&r, ".symtab"), (1 + 4 + 1 + 1) * 24);
    assert!(r.layout.placement.place[ehs].get().is_none());
    assert_eq!(r.assignment.out(ehs), Some(outsecs_named(&r, ".eh_frame")[0]));
}

#[test]
fn r_eh_frame_cies_kept() {
    // CIEs and FDEs of kept code stay; no relocation of a removed record
    // means all of them stay; the terminator stays.
    let m = Machine::X86_64;
    let mut case = eh_case(m);
    let tf = case.sid(1, 2);
    case.res.sections[tf].state = SectionState::Kept;
    let r = eh_removed(&case.split, &[case.sid(1, 4)], |_| false);
    assert!(r.is_empty());
    let r = eh_removed(&case.split, &[case.sid(1, 4)], |s| s == tf);
    assert_eq!(&*r, &[(case.sid(1, 4), 0x38)]);
    // A CIE is never removed, an FDE without code never.
    let mut split = case.split.clone();
    split.eh_piece = Box::new([EhPiece::CIE, EhPiece::CIE, EhPiece { cie: 0, code: OptId::NONE }]);
    assert!(eh_removed(&split, &[case.sid(1, 4)], |_| true).is_empty());
}

// ---------------------------------------------------------------------
// S9, S10: the notes.
// ---------------------------------------------------------------------

fn property_objects(m: Machine) -> [Obj; 2] {
    let mut a = Obj::new(m);
    a.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    a.sec(".note.gnu.property", NOTE, A, 8, vec![0; 0x20]);
    let mut b = Obj::new(m);
    b.sec(".note.gnu.property", NOTE, A, 8, vec![0; 0x20]);
    b.sec(".data", PROGBITS, W | A, 4, vec![0; 4]);
    [a, b]
}

#[test]
fn r_property_and_build_id_notes() {
    let m = Machine::AArch64;
    let mut case = Case::new(m, &property_objects(m));
    case.notes = NoteSizes { property: Some(0x20), build_id: Some(0x24) };
    let r = case.ok();
    assert_eq!(
        names(&r)[..5],
        ["<fh>", ".note.gnu.build-id", ".text", ".note.gnu.property", ".data"]
    );
    assert_offsets(&r, Class::Elf64);
    let pn = r.layout.placement.chunks[chunk(&r, ".note.gnu.property")];
    assert_eq!((pn.sh_type, pn.flags, pn.align, pn.size), (NOTE, A, 3, 0x20));
    let id = r.layout.placement.chunks[chunk(&r, ".note.gnu.build-id")];
    assert_eq!((id.sh_type, id.flags, id.align, id.size), (NOTE, A, 2, 0x24));
    for s in [case.sid(0, 2), case.sid(1, 1)] {
        assert_eq!(r.assignment.out_of[s], mortar_core::assignment::OutOf::Consumed);
        assert!(r.layout.placement.place[s].get().is_none());
    }
    // No property note planned: the inputs are still consumed.
    case.notes = NoteSizes::default();
    let r = case.ok();
    assert_eq!(names(&r)[1..3], [".text", ".data"]);
    // A planned note with no input note is the caller's error.
    let mut c = Case::new(m, &[object_a(m)]);
    c.notes.property = Some(0x20);
    assert!(c.err()[0].starts_with("internal-error"));
    // A property note with relocations is refused.
    let [mut a, b] = property_objects(m);
    a.rela(2, &[0]);
    let errs = Case::new(m, &[a, b]).err();
    assert!(errs.iter().any(|x| x.contains(".note.gnu.property with relocations")), "{errs:?}");
}

// ---------------------------------------------------------------------
// L-R1 SHF_LINK_ORDER.
// ---------------------------------------------------------------------

#[test]
fn r_link_order() {
    let m = Machine::X86_64;
    let pfe = "__patchable_function_entries";
    let mut a = Obj::new(m);
    a.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    let p = a.sec(pfe, PROGBITS, W | A | LINK_ORDER, 8, vec![0; 8]);
    a.at(p).link = 1;
    let mut b = Obj::new(m);
    b.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    b.sec(".text.g", PROGBITS, A | X, 4, vec![0; 4]);
    let p1 = b.sec(pfe, PROGBITS, W | A | LINK_ORDER, 8, vec![0; 8]);
    b.at(p1).link = 1;
    let p2 = b.sec(pfe, PROGBITS, W | A | LINK_ORDER, 8, vec![0; 8]);
    b.at(p2).link = 2;
    // A chain: .b is linked to .a, which is linked to .text.
    let ca = b.sec(".a", PROGBITS, A | LINK_ORDER, 1, vec![0]);
    b.at(ca).link = 1;
    let cb = b.sec(".b", PROGBITS, A | LINK_ORDER, 1, vec![0]);
    b.at(cb).link = u32::from(ca);
    let case = Case::new(m, &[a, b]);
    let r = case.ok();
    let outs = outsecs_named(&r, pfe);
    assert_eq!(outs.len(), 2);
    let o = |s| r.assignment.out(s).unwrap();
    assert_eq!(o(case.sid(0, 2)), o(case.sid(1, 3)));
    assert_ne!(o(case.sid(1, 3)), o(case.sid(1, 4)));
    assert_eq!(size(&r, pfe), 0x10);
    assert!(r.assignment.outsecs[o(case.sid(1, 6))].flags & LINK_ORDER != 0);
    // A linked-to section not in the output is refused by name.
    let mut x = Obj::new(m);
    x.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    let p = x.sec(pfe, PROGBITS, W | A | LINK_ORDER, 8, vec![0; 8]);
    x.at(p).link = 1;
    let mut c = Case::new(m, &[x]);
    c.row(0, 1).state = SectionState::Dropped(DropReason::Stripped);
    let errs = c.err();
    assert!(errs.iter().any(|x| x.contains("linked-to section")), "{errs:?}");
}

// ---------------------------------------------------------------------
// I-R4, S2, S14: the globals.
// ---------------------------------------------------------------------

#[test]
fn r_globals() {
    let m = Machine::X86_64;
    let mut a = Obj::new(m);
    let t = a.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
    a.global("u@V1", STT_NOTYPE, 0, 0);
    a.global("d@@V2", STT_FUNC, t, 0);
    a.global("c", STT_OBJECT, SHN_COMMON, 4);
    a.global("$x", STT_NOTYPE, 0, 0);
    let bytes = vec![a.bytes()];
    // `-u only` and `internal` are keys no object mentions.
    let mut case = Case::of_bytes(m, &bytes, &["only", "unwritten"]);
    let only = SymbolId(4);
    case.res.props[only].refs = RefFlags::FROM_OBJECT;
    let r = case.ok();
    // null; .text; u@V1 d@@V2 c $x only.
    assert_eq!(size(&r, ".symtab"), (1 + 1 + 5) * 24);
    // `u@V1` as the key spells it, `d@@V2` as its definition does, `$x`
    // in the fixed table.
    assert_eq!(size(&r, ".strtab"), 31 + 5 + 6 + 2 + 5);
    // A mentioned internal key is not written.
    case.res.def[SymbolId(2)] = Def::Internal(InternalDef::Assign(0));
    let r = case.ok();
    assert_eq!(size(&r, ".symtab"), (1 + 1 + 4) * 24);
    // A shared definition at -r is resolution's error.
    case.res.def[SymbolId(2)] = Def::Shared(SymEntry { file: FileId(1), index: SymbolIndex(3) });
    assert!(case.err()[0].starts_with("internal-error"));
}

// ---------------------------------------------------------------------
// The refusals.
// ---------------------------------------------------------------------

#[test]
fn r_refusals() {
    let m = Machine::AArch64;
    let one = |f: &dyn Fn(&mut Obj)| {
        let mut o = Obj::new(m);
        o.sec(".text", PROGBITS, A | X, 4, vec![0; 4]);
        f(&mut o);
        Case::new(m, &[o])
    };
    let expect = |case: &Case, want: &str| {
        let errs = case.err();
        assert!(errs.iter().any(|e| e.contains(want)), "want {want}: {errs:?}");
    };
    let mut c = one(&|_| {});
    c.plan.define_common = true;
    expect(&c, "-d with -r");
    expect(
        &one(&|o| {
            o.sec(".gnu.linkonce.t.f", PROGBITS, A | X, 4, vec![0; 4]);
        }),
        ".gnu.linkonce",
    );
    expect(
        &one(&|o| {
            o.sec(".zdebug_info", PROGBITS, 0, 1, vec![0; 4]);
        }),
        ".zdebug",
    );
    // SHF_AARCH64_PURECODE: a flag L-R1a cannot carry.
    expect(
        &one(&|o| {
            o.sec(".text.p", PROGBITS, A | X | 0x2000_0000, 4, vec![0; 4]);
        }),
        "section flags 0x20000000 of .text.p at -r",
    );
    // SHT_REL on a RELA target.
    expect(
        &one(&|o| {
            let r = o.rela(1, &[0]);
            o.at(r).ty = REL;
            o.at(r).entsize = 24;
        }),
        "SHT_REL",
    );
    let mut c = one(&|o| {
        o.sec(".mason", PROGBITS, 0, 1, vec![0; 4]);
    });
    c.row(0, 2).kind = SectionKind::Meta;
    c.row(0, 2).state = SectionState::Dropped(DropReason::Consumed);
    expect(&c, "Mason container");
    expect(
        &one(&|o| {
            o.sec(".eh_frame", PROGBITS, A | RETAIN, 8, eh_bytes(&[0x14], false));
        }),
        ".eh_frame with SHF_GNU_RETAIN",
    );
}

// ---------------------------------------------------------------------
// The pure sizes RL-7 tests against RL-4's.
// ---------------------------------------------------------------------

#[test]
fn r_pure_sizes() {
    assert_eq!(STRTAB_FIXED.len(), 31);
    let n = |name: &'static str, version: Option<&'static str>| EntryName {
        name: name.as_bytes(),
        version: version.map(str::as_bytes),
    };
    assert_eq!(strtab_size([]), 31);
    assert_eq!(strtab_size([n("", None), n("$x", None), n("$d", None)]), 31);
    assert_eq!(strtab_size([n("__erratum_843419_veneer", None)]), 31);
    assert_eq!(strtab_size([n("foo", None), n("foo", None)]), 31 + 8);
    assert_eq!(strtab_size([n("foo", Some("V1")), n("$x", Some("V"))]), 31 + 7 + 5);
    let p = SectionName::Plain;
    let rel = |rel, t: &'static str| SectionName::Relocs { rel, target: t.as_bytes() };
    assert_eq!(shstrtab_size([]), 1);
    assert_eq!(shstrtab_size([p(b".text"), rel(false, ".text"), p(b".text")]), 1 + 6 + 11);
    assert_eq!(shstrtab_size([rel(true, ".text"), p(b".rel.text"), p(b"")]), 1 + 10);
    assert_eq!(shstrtab_size([p(b".text"), p(b"text")]), 1 + 6 + 5);
    assert_eq!(symtab_entries(2, 3, 4), 10);
    assert_eq!(symtab_size(Class::Elf64, 10), 240);
    assert_eq!(symtab_size(Class::Elf32, 10), 160);
    assert_eq!(symtab_shndx_size(false, 10), None);
    assert_eq!(symtab_shndx_size(true, 10), Some(40));
    assert!(!needs_xindex(0xfeff) && needs_xindex(0xff00));
    assert_eq!(relocs_size(Class::Elf64, 0), None);
    assert_eq!(relocs_size(Class::Elf64, 3), Some(72));
    assert_eq!(relocs_size(Class::Elf32, 3), Some(24));
    assert_eq!(group_size(0, 0), 4);
    assert_eq!(group_size(2, 1), 16);
    assert_eq!(eh_frame_size([0x30, 0x5c], [0x20, 0x18]), 0x54);
}

// ---------------------------------------------------------------------
// L-R10: every reading, against a layout that violates it.
// ---------------------------------------------------------------------

/// A layout with everything L-R10 reads: groups (a kept and a lost copy),
/// relocation chunks, a removed FDE, a common, NOBITS.
fn rich() -> (Case, RelocatableLayout<'static>) {
    let m = Machine::X86_64;
    let mut c = Obj::new(m);
    let t = c.sec(".text", PROGBITS, A | X, 16, vec![0; 0x10]);
    c.rela(t, &[0, 8]);
    c.bss(".bss", W | A, 8, 0x10);
    c.sec(".data", PROGBITS, W | A, 8, vec![0; 8]);
    c.global("h", STT_OBJECT, SHN_COMMON, 8);
    let case = eh_case_with(m, &[c]);
    let r = case.ok();
    (case, r)
}

/// Mutates `rich()`'s layout (and assignment) and expects the reading
/// `want` to fail.
fn violates(
    want: &str,
    f: impl FnOnce(&Case, &mut RelocatableLayout<'static>),
    run: impl FnOnce(&Case, &RelocatableLayout<'static>) -> Result<(), Violation>,
) {
    let (case, mut r) = rich();
    f(&case, &mut r);
    match run(&case, &r) {
        Ok(()) => panic!("{want}: the layout passed"),
        Err(v) => assert!(v.what.contains(want), "want {want:?}, got {:?}", v.what),
    }
}

fn layout_check(case: &Case, r: &RelocatableLayout<'static>) -> Result<(), Violation> {
    case.with_cx(&r.assignment, false, |cx| r.layout.validate(cx))
}

/// The output section of chunk `name`'s body.
fn body_out(r: &RelocatableLayout<'_>, name: &str) -> OutSecId {
    match r.layout.placement.chunks[chunk(r, name)].kind {
        ChunkKind::Body(p) => p.out,
        k => panic!("{k:?}"),
    }
}

fn new_outsec(r: &mut RelocatableLayout<'static>, sh_type: u32, flags: u64) -> OutSecId {
    r.assignment
        .outsecs
        .push(OutSec {
            name: b".new",
            sh_type,
            flags,
            merge: None,
            keep: false,
            fill: None,
            constraint: None,
            script_position: None,
        })
        .unwrap()
}

#[test]
fn r_validate_accepts_rich() {
    let (case, r) = rich();
    assert!(case.check(&r).is_ok());
    assert!(names(&r).iter().any(|n| n == ".rela.eh_frame"));
    assert_eq!(r.eh_removed.len(), 1);
}

#[test]
fn r_validate_range() {
    let lc = layout_check;
    violates(
        "place is not per section",
        |_, r| {
            let n = r.layout.placement.place.len();
            r.layout.placement.place = Column::filled(n + 1, Place::NONE);
        },
        lc,
    );
    violates(
        "a chunk names a row that does not exist",
        |_, r| {
            let c = chunk(r, ".data");
            r.layout.placement.chunks[c].kind = ChunkKind::Body(Part::first(OutSecId(999)));
        },
        lc,
    );
    violates(
        "a place names a chunk that does not exist",
        |case, r| {
            r.layout.placement.place[case.sid(0, 1)] = Place::at(ChunkId(999), 0);
        },
        lc,
    );
    violates(
        "order names a chunk that does not exist",
        |_, r| {
            let mut o = r.layout.placement.order.to_vec();
            o.push(ChunkId(999));
            r.layout.placement.order = o.into_boxed_slice();
        },
        lc,
    );
}

#[test]
fn r_validate_places() {
    let lc = layout_check;
    violates(
        "rule 1: a live section has no place",
        |case, r| {
            r.layout.placement.place[case.sid(0, 1)] = Place::NONE;
        },
        lc,
    );
    violates(
        "rule 1: a section that needs no place has one",
        |case, r| {
            let c = chunk(r, ".text");
            r.layout.placement.place[case.sid(1, 2)] = Place::at(c, 0);
        },
        lc,
    );
    violates(
        "rule 1: the section is not in the body of its output section",
        |case, r| {
            let c = chunk(r, ".data");
            r.layout.placement.place[case.sid(0, 1)] = Place::at(c, 0);
        },
        lc,
    );
    violates(
        "rule 1: the section's size and alignment cannot be read",
        |_, _| {},
        |case, r| case.with_cx(&r.assignment, true, |cx| r.layout.validate(cx)),
    );
    violates(
        "rule 1: the section ends past its chunk's size",
        |_, r| {
            let c = chunk(r, ".text");
            r.layout.placement.chunks[c].size = 1;
        },
        lc,
    );
    violates(
        "rule 1: the section is misaligned",
        |case, r| {
            let c = chunk(r, ".text");
            r.layout.placement.place[case.sid(0, 1)] = Place::at(c, 2);
        },
        lc,
    );
    violates(
        "rule 5: an allocation that is not a common",
        |_, r| {
            let c = chunk(r, ".bss");
            r.layout.placement.sym_place = Box::new([SymPlace {
                key: AllocKey::Copy(0),
                chunk: c,
                offset: 0,
                size: 4,
                align: 2,
            }]);
        },
        lc,
    );
    violates(
        "rule 1: a common's allocation is not inside a body chunk",
        |case, r| {
            let h = case.inputs.symbols.iter().find(|(_, k)| k.name == b"h").unwrap().0;
            let c = chunk(r, ".symtab");
            r.layout.placement.sym_place = Box::new([SymPlace {
                key: AllocKey::Common(h),
                chunk: c,
                offset: 0,
                size: 4,
                align: 2,
            }]);
        },
        lc,
    );
    violates(
        "rule 1: two places overlap",
        |case, r| {
            let c = chunk(r, ".text");
            r.layout.placement.place[case.sid(1, 3)] = Place::at(c, 0);
        },
        lc,
    );
}

#[test]
fn r_validate_chunks() {
    let lc = layout_check;
    let reorder = |r: &mut RelocatableLayout<'static>, f: &dyn Fn(&mut Vec<ChunkId>)| {
        let mut o = r.layout.placement.order.to_vec();
        f(&mut o);
        r.layout.placement.order = o.into_boxed_slice();
    };
    violates("rule 2: a chunk twice in order", |_, r| reorder(r, &|o| o.push(o[1])), lc);
    violates(
        "rule 2: a chunk missing from order",
        |_, r| {
            reorder(r, &|o| {
                o.remove(1);
            });
        },
        lc,
    );
    violates(
        "rule 2: the file header is not first at offset 0",
        |_, r| {
            let c = r.layout.placement.order[0];
            r.layout.placement.chunks[c].offset = 8;
        },
        lc,
    );
    violates(
        "rule 2: the section header table is not last (L-R8)",
        |_, r| {
            reorder(r, &|o| {
                let sh = o.pop().unwrap();
                o.insert(1, sh);
            });
        },
        lc,
    );
    violates(
        "rule 2: a chunk has an address (L-R7)",
        |_, r| {
            let c = chunk(r, ".data");
            r.layout.placement.chunks[c].addr = Some(0);
        },
        lc,
    );
    violates(
        "rule 2: size exceeds capacity",
        |_, r| {
            let c = chunk(r, ".symtab");
            r.layout.placement.chunks[c].size += 1;
        },
        lc,
    );
    violates(
        "rule 2: the chunk's offset is misaligned",
        |_, r| {
            let c = chunk(r, ".symtab");
            r.layout.placement.chunks[c].offset += 1;
        },
        lc,
    );
    violates(
        "rule 2: a header with a section index",
        |_, r| {
            let c = r.layout.placement.order[0];
            r.layout.placement.chunks[c].shndx = Some(0);
        },
        lc,
    );
    violates(
        "MD-R1: a synthetic chunk a -r output does not have",
        |_, r| {
            let c = chunk(r, ".strtab");
            r.layout.placement.chunks[c].kind =
                ChunkKind::Synthetic(Synth { name: b".dynsym", address_dependent: false });
        },
        lc,
    );
    violates(
        "MD-R1: a synthetic chunk twice",
        |_, r| {
            let c = chunk(r, ".strtab");
            r.layout.placement.chunks[c].kind =
                ChunkKind::Synthetic(Synth { name: b".shstrtab", address_dependent: false });
        },
        lc,
    );
    violates(
        "MD-R1: no .symtab chunk",
        |_, r| {
            let c = chunk(r, ".symtab");
            r.layout.placement.chunks[c].kind =
                ChunkKind::Synthetic(Synth { name: b".symtab_shndx", address_dependent: false });
        },
        lc,
    );
    violates(
        "rule 2: chunks overlap in the file",
        |_, r| {
            let c = chunk(r, ".data");
            let t = chunk(r, ".text");
            r.layout.placement.chunks[c].offset = r.layout.placement.chunks[t].offset;
        },
        lc,
    );
    violates(
        "is not the next in order",
        |_, r| {
            let c = chunk(r, ".data");
            *r.layout.placement.chunks[c].shndx.as_mut().unwrap() += 1;
        },
        lc,
    );
}

#[test]
fn r_validate_empty() {
    let lc = layout_check;
    violates(
        "rule 3: a -r layout has segments",
        |_, r| {
            r.layout
                .placement
                .segments
                .push(Segment {
                    p_type: 1,
                    flags: 4,
                    offset: 0,
                    addr: 0,
                    filesz: 0,
                    memsz: 0,
                    align: 12,
                })
                .unwrap();
        },
        lc,
    );
    violates(
        "rule 3: a -r layout has a TLS template",
        |_, r| {
            r.layout.placement.tls = Some(Tls { start: 0, size: 0, align: 0 });
        },
        lc,
    );
    violates(
        "rule 4: a -r layout has patches",
        |case, r| {
            r.layout.placement.redirects =
                Box::new([Redirect { section: case.sid(0, 1), offset: 0, patch: 0 }]);
        },
        lc,
    );
    violates(
        "rule 5: a -r layout has defined rows",
        |_, r| {
            r.layout.placement.defined =
                Box::new([Defined { symbol: SymbolId(0), value: DefValue::Absolute(0) }]);
        },
        lc,
    );
    violates(
        "rule 5: a -r layout has sized rows",
        |_, r| {
            let c = chunk(r, ".data");
            r.layout.placement.sized = Box::new([SizedRow { chunk: c, size: 0 }]);
        },
        lc,
    );
    violates("rule 5: a -r layout has an entry", |_, r| r.layout.placement.entry = Some(0), lc);
    violates(
        "MD-R1: a -r layout has holes",
        |_, r| {
            let c = chunk(r, ".data");
            r.layout.placement.holes =
                Box::new([Hole { chunk: c, offset: 0, size: 1, kind: HoleKind::Fill }]);
        },
        lc,
    );
    violates(
        "MD-R1: a chunk a -r output does not have",
        |_, r| {
            let c = chunk(r, ".rela.text");
            let ChunkKind::Relocs(o) = r.layout.placement.chunks[c].kind else { unreachable!() };
            r.layout.placement.chunks[c].kind = ChunkKind::Merged(o);
        },
        lc,
    );
}

#[test]
fn r_validate_bodies() {
    let lc = layout_check;
    violates(
        "MD-R1: two bodies of one output section",
        |_, r| {
            let c = chunk(r, ".rela.text");
            let o = body_out(r, ".text");
            r.layout.placement.chunks[c].kind = ChunkKind::Body(Part::first(o));
        },
        lc,
    );
    violates(
        "MD-R1: a body chunk's type or flags are not its output section's",
        |_, r| {
            let c = chunk(r, ".data");
            r.layout.placement.chunks[c].flags |= X;
        },
        lc,
    );
    violates(
        "S1: an .eh_frame input's output section is not the .eh_frame chunk",
        |_, r| {
            let c = chunk(r, ".eh_frame");
            r.layout.placement.chunks[c].kind = ChunkKind::Synthetic(Synth {
                name: b".note.gnu.build-id",
                address_dependent: false,
            });
        },
        lc,
    );
    violates(
        "MD-R1: an output section with a live piece has no body",
        |case, r| {
            let o = new_outsec(r, PROGBITS, 0);
            // A relocation section (no place needed) assigned to it.
            let s = case.sid(0, 4);
            r.assignment.out_of[s] = mortar_core::assignment::OutOf::Out(o);
        },
        lc,
    );
    violates(
        "MD-R1: a body of an output section with no live piece",
        |_, r| {
            let o = new_outsec(r, RELA, INFO_LINK);
            let c = chunk(r, ".rela.text");
            r.layout.placement.chunks[c].kind = ChunkKind::Body(Part::first(o));
        },
        lc,
    );
}

#[test]
fn r_validate_groups_and_relocs() {
    let lc = layout_check;
    violates(
        "L-R3: a group chunk names a section that is not a group",
        |case, r| {
            let c = chunk(r, ".group");
            r.layout.placement.chunks[c].kind = ChunkKind::Group(case.sid(0, 1));
        },
        lc,
    );
    violates(
        "L-R10: a group has two group chunks",
        |case, r| {
            let c = chunk(r, ".rela.text");
            let ch = &mut r.layout.placement.chunks[c];
            ch.kind = ChunkKind::Group(case.sid(0, 2));
            (ch.sh_type, ch.align) = (GROUP_T, 2);
        },
        lc,
    );
    violates(
        "L-R3: a group chunk is not SHT_GROUP words",
        |_, r| {
            let c = chunk(r, ".group");
            r.layout.placement.chunks[c].size = 6;
        },
        lc,
    );
    violates(
        "L-R10: two relocation chunks of one output section",
        |_, r| {
            let c = chunk(r, ".rela.eh_frame");
            let o = body_out(r, ".text");
            r.layout.placement.chunks[c].kind = ChunkKind::Relocs(o);
        },
        lc,
    );
    violates(
        "L-R10: a relocation chunk targets no output section with content",
        |_, r| {
            let o = new_outsec(r, PROGBITS, 0);
            let c = chunk(r, ".rela.text");
            r.layout.placement.chunks[c].kind = ChunkKind::Relocs(o);
        },
        lc,
    );
    violates(
        "L-R10: a relocation chunk for an output section with no relocated piece",
        |_, r| {
            let c = chunk(r, ".rela.text");
            let o = body_out(r, ".bss");
            r.layout.placement.chunks[c].kind = ChunkKind::Relocs(o);
        },
        lc,
    );
    violates(
        "S6: a relocation chunk's type or size",
        |_, r| {
            let c = chunk(r, ".rela.text");
            r.layout.placement.chunks[c].size -= 1;
        },
        lc,
    );
    violates(
        "L-R8: a relocation chunk is not right after its target",
        |_, r| {
            let a = chunk(r, ".rela.text");
            let b = chunk(r, ".rela.text.foo");
            let p = &mut r.layout.placement;
            let (ka, kb) = (p.chunks[a].kind, p.chunks[b].kind);
            p.chunks[a].kind = kb;
            p.chunks[b].kind = ka;
        },
        lc,
    );
}

#[test]
fn r_validate_group_facts() {
    let groups = |case: &Case, r: &RelocatableLayout<'static>| {
        let kept = case.kept_groups();
        let kept: Vec<(SectionId, &[SectionId])> =
            kept.iter().map(|(g, m)| (*g, m.as_slice())).collect();
        case.with_cx(&r.assignment, false, |cx| validate_groups(&r.layout, cx, &kept))
    };
    violates(
        "a kept group has no SHT_GROUP chunk",
        |_, r| {
            let c = chunk(r, ".group");
            let o = body_out(r, ".text");
            r.layout.placement.chunks[c].kind = ChunkKind::Relocs(o);
        },
        groups,
    );
    violates(
        "an SHT_GROUP chunk names no kept group",
        |case, r| {
            let c = chunk(r, ".rela.text");
            r.layout.placement.chunks[c].kind = ChunkKind::Group(case.sid(1, 1));
        },
        groups,
    );
    violates(
        "a group member's output section has no SHF_GROUP",
        |_, r| {
            let o = body_out(r, ".text.foo");
            r.assignment.outsecs[o].flags &= !GROUP;
        },
        groups,
    );
    violates(
        "an output section outside every group has SHF_GROUP",
        |_, r| {
            let o = body_out(r, ".data");
            r.assignment.outsecs[o].flags |= GROUP;
        },
        groups,
    );
    violates(
        "a body chunk's SHF_GROUP differs",
        |_, r| {
            let c = chunk(r, ".text.foo");
            r.layout.placement.chunks[c].flags &= !GROUP;
        },
        groups,
    );
    violates(
        "a group member has no output section",
        |_, _| {},
        |case, r| {
            let lost = case.sid(1, 2);
            let g = case.sid(0, 2);
            let kept = [(g, &[lost][..])];
            case.with_cx(&r.assignment, false, |cx| validate_groups(&r.layout, cx, &kept))
        },
    );
}

/// `validate_relocs` with the inputs' record counts changed by `edit`.
fn relocs(
    edit: fn(&mut Vec<u64>, &RelocatableLayout<'static>),
) -> impl FnOnce(&Case, &RelocatableLayout<'static>) -> Result<(), Violation> {
    move |case, r| {
        let mut n = case.records(r);
        edit(&mut n, r);
        case.with_cx(&r.assignment, false, |cx| validate_relocs(&r.layout, cx, &n))
    }
}

#[test]
fn r_validate_record_facts() {
    violates(
        "the record counts are not per output section",
        |_, _| {},
        relocs(|n, _| {
            n.pop();
        }),
    );
    violates(
        "has a relocated piece and no relocation chunk",
        |_, _| {},
        relocs(|n, r| {
            n[body_out(r, ".bss").0 as usize] = 1;
        }),
    );
    violates(
        "a relocation chunk for an output section with no record",
        |_, _| {},
        relocs(|n, r| {
            n[body_out(r, ".text").0 as usize] = 0;
        }),
    );
    violates(
        "a relocation chunk's size is not its records'",
        |_, _| {},
        relocs(|n, r| {
            n[body_out(r, ".text").0 as usize] += 1;
        }),
    );
}

// ---------------------------------------------------------------------
// Tool-backed: L-R8 takes lld's order (`spec/layout.md` L-R8).
// ---------------------------------------------------------------------

#[path = "common/elf.rs"]
#[allow(unused_imports)]
mod elf_read;

#[test]
fn r_section_order_against_lld() {
    let (machine, extra): (Machine, &[&str]) = match std::env::consts::ARCH {
        "aarch64" => (Machine::AArch64, &["-mbranch-protection=none"]),
        "x86_64" => (Machine::X86_64, &["-fcf-protection=none"]),
        other => {
            mortar_testutil::named_skip(
                "relocatable::r_section_order_against_lld",
                &format!("no gcc target for host {other}"),
            );
            return;
        }
    };
    let Some(gcc) = mortar_testutil::tool("gcc") else { return };
    let Some(lld) = mortar_testutil::tool("ld.lld") else { return };
    let dir = mortar_testutil::Scratch::new("rl3-order");
    let a = dir.write(
        "a.c",
        "static int s = 3;\nint x[4] = {1};\nint y;\nextern int ext(int);\n\
         int f(int a) { return ext(a) + s + x[a]; }\n",
    );
    let b = dir.write(
        "b.c",
        "const char *msg = \"hello\";\nint g(int a) { return a * 2; }\n\
         int main(void) { return g(1); }\n",
    );
    let mut objs = Vec::new();
    for src in [&a, &b] {
        let o = src.with_extension("o");
        let mut c = Command::new(&gcc);
        c.args(["-c", "-O0", "-fno-common"]).args(extra).arg("-o").arg(&o).arg(src);
        let out = mortar_testutil::run_with(&mut c, b"", Duration::from_mins(1));
        assert!(out.success(), "{}", out.stderr_text());
        objs.push(o);
    }
    let r_out = dir.join("lld.o");
    let mut c = Command::new(&lld);
    c.arg("-r").arg("-o").arg(&r_out).args(&objs);
    let out = mortar_testutil::run_with(&mut c, b"", Duration::from_mins(1));
    assert!(out.success(), "{}", out.stderr_text());
    let theirs = elf_read::read_elf(&std::fs::read(&r_out).unwrap());
    let bytes: Vec<Vec<u8>> = objs.iter().map(|o| std::fs::read(o).unwrap()).collect();
    let case = Case::of_bytes(machine, &bytes, &[]);
    let r = case.ok();
    // The documented differences: lld writes `.note.GNU-stack` after the
    // last input section and `.shstrtab` before `.strtab` (L-R6a, L-R8).
    let skip = |n: &str| matches!(n, ".note.GNU-stack" | ".strtab" | ".shstrtab");
    let ours: Vec<String> =
        names(&r).into_iter().filter(|n| !n.starts_with('<') && !skip(n)).collect();
    let lld_names: Vec<String> =
        theirs.sections.iter().skip(1).map(|s| s.name.clone()).filter(|n| !skip(n)).collect();
    assert_eq!(ours, lld_names);
    // And the sizes of what both copy whole.
    for s in theirs.sections.iter().skip(1) {
        if matches!(s.name.as_str(), ".text" | ".data" | ".bss" | ".comment" | ".eh_frame")
            && s.ty != 0
        {
            let mine = size(&r, &s.name);
            if s.name == ".comment" {
                // lld merges strings at -r (L-R5); mortar does not.
                assert!(mine >= s.size, "{}", s.name);
            } else {
                assert_eq!(mine, s.size, "{}", s.name);
            }
        }
    }
}
