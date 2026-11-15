//! Test inputs: an in-memory `.eh_frame` source, a symbol source, an
//! address oracle, and note bytes.
#![allow(dead_code)]

use std::collections::BTreeMap;

use mortar_core::{FileId, SectionId, SymKey, SymbolId};
use mortar_diag::{Collector, Policy, Report, Style};
use mortar_synth::eh::{CieReloc, CodeRef, EhSection, EhSource, InRecord, RecordKind};
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
    /// offset), `extra` bytes after the address range, range 0 when
    /// `empty`.
    Fde { cie: u32, code: Option<(u32, u64)>, extra: usize, empty: bool },
}

pub fn fde(cie: u32, section: u32, offset: u64) -> R {
    R::Fde { cie, code: Some((section, offset)), extra: 0, empty: false }
}

#[derive(Debug, Default)]
pub struct Eh {
    pub secs: Vec<EhSection>,
    pub recs: Vec<Vec<InRecord>>,
    pub bytes: Vec<Vec<u8>>,
    pub relocs: BTreeMap<(usize, u32), Vec<CieReloc>>,
}

impl Eh {
    /// Adds section `id` of `file` with `records` (sections must be added
    /// in ascending id order).
    pub fn add(&mut self, id: u32, file: u32, records: &[R]) -> &mut Self {
        let mut bytes = Vec::new();
        let mut recs = Vec::new();
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
                    recs.push(InRecord { offset: off, len, kind: RecordKind::Cie });
                }
                R::Fde { cie, code, extra, empty } => {
                    let len = 12 + *extra as u32;
                    bytes.extend_from_slice(&len.to_le_bytes());
                    let ptr = off + 4 - offs[*cie as usize];
                    bytes.extend_from_slice(&ptr.to_le_bytes());
                    bytes.extend_from_slice(&0x1111_1111u32.to_le_bytes());
                    bytes.extend_from_slice(&u32::from(!*empty).to_le_bytes());
                    bytes.extend(std::iter::repeat_n(0xee, *extra));
                    recs.push(InRecord {
                        offset: off,
                        len,
                        kind: RecordKind::Fde {
                            cie: *cie,
                            code: code.map(|(s, o)| CodeRef { section: SectionId(s), offset: o }),
                            empty_range: *empty,
                        },
                    });
                }
            }
        }
        self.secs.push(EhSection {
            id: SectionId(id),
            file: FileId(file),
            size: bytes.len() as u32,
            align: 3,
            sh_type: mortar_elf::consts::SHT_PROGBITS,
        });
        self.recs.push(recs);
        self.bytes.push(bytes);
        self
    }

    pub fn reloc(&mut self, section: usize, record: u32, r: CieReloc) -> &mut Self {
        self.relocs.entry((section, record)).or_default().push(r);
        self
    }
}

impl EhSource for Eh {
    fn sections(&self) -> &[EhSection] {
        &self.secs
    }
    fn records(&self, section: usize) -> &[InRecord] {
        &self.recs[section]
    }
    fn bytes(&self, section: usize) -> &[u8] {
        &self.bytes[section]
    }
    fn cie_relocs(&self, section: usize, record: u32, out: &mut Vec<CieReloc>) {
        if let Some(v) = self.relocs.get(&(section, record)) {
            out.extend_from_slice(v);
        }
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
