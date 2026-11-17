//! Test inputs: an `.eh_frame` source (bytes parsed by `mortar_merge`), a symbol source, an
//! address oracle, and note bytes.
#![allow(dead_code)]

use std::collections::BTreeMap;

use mortar_core::par::Pool;
use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_diag::{Collector, Policy, Report, Style};
use mortar_merge::eh::{EhFrames, EhIn, EhReloc, RelocClass, Target};
use mortar_synth::eh::{EhInfo, EhSource};
use mortar_synth::symtab::{
    GlobalIn, InternalValue, LocalIn, SecKind, Sections, SymAddrs, SymSource,
};

pub fn collector() -> Collector {
    Collector::new(Policy { error_limit: 0, ..Policy::default() })
}

pub fn text(report: &Report) -> String {
    report.render(&Style::plain("ld"))
}

// ---- .eh_frame ------------------------------------------------------------

/// One input record.
#[derive(Debug, Clone)]
pub enum R {
    /// A CIE with this body after the CIE id.
    Cie(Vec<u8>),
    /// An FDE of CIE record `cie`, initial location `code` (section,
    /// offset; `None`: no relocation and a zero field, S-38), `extra`
    /// bytes after the augmentation data, range 0 when `empty`. Its body
    /// is the CIE pointer, the location and range (`sdata4`), a zero
    /// augmentation length and three `DW_CFA_nop`: 16 bytes before
    /// `extra`.
    Fde { cie: u32, code: Option<(u32, i64)>, extra: usize, empty: bool },
    /// A terminator (a zero length word), last in its section (S-39).
    Term,
}

pub fn fde(cie: u32, section: u32, offset: i64) -> R {
    R::Fde { cie, code: Some((section, offset)), extra: 0, empty: false }
}

/// One input section: id, file, bytes, relocations, record offsets.
type Input = (SectionId, FileId, Vec<u8>, Vec<EhReloc>, Vec<u32>);

/// `.eh_frame` sections as bytes and relocations, parsed by
/// `mortar_merge::eh::parse` after every change.
#[derive(Debug)]
pub struct Eh {
    ins: Vec<Input>,
    pub info: Vec<EhInfo>,
    pub frames: EhFrames,
}

impl Default for Eh {
    fn default() -> Self {
        Self {
            ins: Vec::new(),
            info: Vec::new(),
            frames: EhFrames { sections: Box::new([]), records: Box::new([]), classes: 0 },
        }
    }
}

fn pc32(offset: u64, target: Target) -> EhReloc {
    EhReloc {
        offset,
        r_type: 261,
        name: "R_AARCH64_PREL32",
        class: RelocClass::Pc,
        width: 4,
        addend: 0,
        target,
    }
}

impl Eh {
    /// Adds section `id` of `file` with `records` (sections must be added
    /// in ascending id order).
    pub fn add(&mut self, id: u32, file: u32, records: &[R]) -> &mut Self {
        let mut bytes = Vec::new();
        let mut relocs = Vec::new();
        let mut offs = Vec::new();
        for r in records {
            let off = bytes.len() as u32;
            offs.push(off);
            match r {
                R::Cie(body) => {
                    let len = 4 + body.len() as u32;
                    bytes.extend_from_slice(&len.to_le_bytes());
                    bytes.extend_from_slice(&0u32.to_le_bytes());
                    bytes.extend_from_slice(body);
                }
                R::Fde { cie, code, extra, empty } => {
                    let len = 16 + *extra as u32;
                    bytes.extend_from_slice(&len.to_le_bytes());
                    let ptr = off + 4 - offs[*cie as usize];
                    bytes.extend_from_slice(&ptr.to_le_bytes());
                    bytes.extend_from_slice(&0u32.to_le_bytes());
                    bytes.extend_from_slice(&u32::from(!*empty).to_le_bytes());
                    bytes.extend_from_slice(&[0; 4]);
                    bytes.extend(std::iter::repeat_n(0xee, *extra));
                    if let Some((s, o)) = code {
                        let target = Target::Section { section: SectionId(*s), value: *o as u64 };
                        relocs.push(pc32(u64::from(off) + 8, target));
                    }
                }
                R::Term => bytes.extend_from_slice(&[0; 4]),
            }
        }
        self.ins.push((SectionId(id), FileId(file), bytes, relocs, offs));
        self.info.push(EhInfo {
            file: FileId(file),
            align: 3,
            sh_type: mortar_elf::consts::SHT_PROGBITS,
        });
        self.reparse()
    }

    /// A relocation at offset `at` of record `record` of section row
    /// `section` against `target` (offset 0), as a CIE carries for its
    /// personality (class `Other`: no pointer field of a `zR` CIE).
    pub fn reloc(&mut self, section: usize, record: usize, at: u32, target: u32) -> &mut Self {
        let s = &mut self.ins[section];
        let offset = u64::from(s.4[record] + at);
        let t = Target::Section { section: SectionId(target), value: 0 };
        s.3.push(EhReloc { class: RelocClass::Other, ..pc32(offset, t) });
        s.3.sort_by_key(|r| r.offset);
        self.reparse()
    }

    fn reparse(&mut self) -> &mut Self {
        let ins: Vec<EhIn<'_>> = self
            .ins
            .iter()
            .enumerate()
            .map(|(i, (section, file, bytes, relocs, _))| EhIn {
                section: *section,
                file: *file,
                index: i as u32,
                bytes,
                relocs,
            })
            .collect();
        self.frames = mortar_merge::eh::parse(&Pool::sequential(), 8, &ins)
            .unwrap_or_else(|p| panic!("the test's .eh_frame does not parse: {p:?}"));
        self
    }
}

impl EhSource for Eh {
    fn frames(&self) -> &EhFrames {
        &self.frames
    }
    fn info(&self, s: usize) -> EhInfo {
        self.info[s]
    }
    fn bytes(&self, s: usize) -> &[u8] {
        &self.ins[s].2
    }
}

/// The output records: `(offset, length word, CIE pointer or id)`, and
/// whether exactly one terminator ends the bytes.
pub fn walk(out: &[u8]) -> (Vec<(usize, u32, u32)>, bool) {
    let mut at = 0;
    let mut v = Vec::new();
    while at + 4 <= out.len() {
        let len = u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        if len == 0 {
            return (v, at + 4 == out.len());
        }
        let id = u32::from_le_bytes(out[at + 4..at + 8].try_into().unwrap());
        v.push((at, len, id));
        at += 4 + len as usize;
    }
    (v, false)
}

// ---- symbols ----------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Syms {
    /// Per file, its locals (file 0 has none).
    pub files: Vec<Vec<LocalIn<'static>>>,
    pub globals: Vec<Option<GlobalIn<'static>>>,
}

impl SymSource for Syms {
    fn files(&self) -> u32 {
        self.files.len() as u32
    }
    fn locals(&self, file: FileId) -> u32 {
        self.files[file.0 as usize].len() as u32
    }
    fn local(&self, file: FileId, i: u32) -> LocalIn<'_> {
        self.files[file.0 as usize][i as usize]
    }
    fn globals(&self) -> u32 {
        self.globals.len() as u32
    }
    fn global(&self, id: SymbolId) -> Option<GlobalIn<'_>> {
        self.globals[id.0 as usize]
    }
    fn key_name(&self, key: SymKey) -> &[u8] {
        match key.kind() {
            mortar_core::SymKind::Global(id) => {
                self.globals[id.0 as usize].map_or(b"?", |g| g.name)
            }
            mortar_core::SymKind::Local(_) => b"local",
        }
    }
}

/// Every section live and plain except those listed.
#[derive(Debug, Clone, Default)]
pub struct Secs {
    pub dead: Vec<SectionId>,
    pub eh: Vec<SectionId>,
}

impl Sections for Secs {
    fn live(&self, section: SectionId) -> bool {
        !self.dead.contains(&section)
    }
    fn kind(&self, section: SectionId) -> SecKind {
        if self.eh.contains(&section) { SecKind::EhFrame } else { SecKind::Plain }
    }
    fn merge_live(&self, _: SectionId, _: u64) -> bool {
        true
    }
}

/// Input section `s` is at `0x10000 + 0x100 × s` in output section 1
/// (`0x10000..0x20000`); section 2 is `.tdata` at `0x30000..0x30100`
/// holding input section 900; `.eh_frame` is section 3 at 0x40000.
#[derive(Debug, Clone, Default)]
pub struct Addrs {
    pub internal: BTreeMap<u32, InternalValue>,
    /// Output sections beyond index 3 (for `SHN_XINDEX`).
    pub high: Option<u32>,
}

impl SymAddrs for Addrs {
    fn section(&self, section: SectionId, value: u64) -> Option<(u32, u64)> {
        match section.0 {
            900 => Some((2, 0x30000 + value)),
            800 => self.high.map(|h| (h, 0x50000 + value)),
            s => Some((1, 0x10000 + 0x100 * u64::from(s) + value)),
        }
    }
    fn eh_frame(&self) -> Option<(u32, u64)> {
        Some((3, 0x40000))
    }
    fn common(&self, id: SymbolId) -> Option<(u32, u64)> {
        Some((1, 0x1f000 + 16 * u64::from(id.0)))
    }
    fn internal(&self, id: SymbolId) -> Option<InternalValue> {
        self.internal.get(&id.0).copied()
    }
    fn patch(&self, index: usize) -> Option<(u32, u64)> {
        Some((1, 0x1e000 + 16 * index as u64))
    }
    fn tls_start(&self) -> Option<u64> {
        Some(0x30000)
    }
    fn section_range(&self, shndx: u32) -> Option<(u64, u64)> {
        match shndx {
            1 => Some((0x10000, 0x10000)),
            2 => Some((0x30000, 0x100)),
            3 => Some((0x40000, 0x1000)),
            h if Some(h) == self.high => Some((0x50000, 0x100)),
            _ => None,
        }
    }
}

// ---- notes ------------------------------------------------------------------

/// One `NT_GNU_PROPERTY_TYPE_0` note (ELF64 padding) of `props`.
pub fn prop_note(props: &[(u32, &[u8])]) -> Vec<u8> {
    let mut desc = Vec::new();
    for &(t, d) in props {
        desc.extend_from_slice(&t.to_le_bytes());
        desc.extend_from_slice(&(d.len() as u32).to_le_bytes());
        desc.extend_from_slice(d);
        while desc.len() % 8 != 0 {
            desc.push(0);
        }
    }
    let mut v = Vec::new();
    v.extend_from_slice(&4u32.to_le_bytes());
    v.extend_from_slice(&(desc.len() as u32).to_le_bytes());
    v.extend_from_slice(&5u32.to_le_bytes());
    v.extend_from_slice(b"GNU\0");
    v.extend_from_slice(&desc);
    v
}
