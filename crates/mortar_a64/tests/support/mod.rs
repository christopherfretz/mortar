//! Fakes of the tables `scan` and `fill` read, for the behavioural tests.
#![allow(dead_code)]

use std::collections::HashMap;

use mortar_a64::A64;
use mortar_arch::{
    Arch, DemandFlags, DemandOut, DemandSink, DynRole, FieldError, FillCx, FillTables, GlobalFlags,
    Howto, OutputKind, PlaceFacts, Reloc, RelocAt, ScanCx, ScanError, ScanTables, SectionOut, Site,
    SlotKind, SymFacts, SymValue, TargetKind, TlsBlock,
};
use mortar_core::{FileId, SectionId, SymKey, SymbolId};

pub const M2: A64 = A64::new(mortar_arch::Status::M2, true);

#[must_use]
pub fn key(n: u32) -> SymKey {
    SymKey::global(SymbolId(n))
}

#[derive(Default)]
pub struct Tables {
    pub syms: HashMap<SymKey, SymValue>,
    pub facts: HashMap<SymKey, SymFacts>,
    pub slots: HashMap<(SymKey, SlotKind), u64>,
    pub plts: HashMap<SymKey, u64>,
    pub flags: HashMap<SymKey, DemandFlags>,
    pub redirects: HashMap<(SectionId, u64), u64>,
}

impl Tables {
    pub fn sym(&mut self, k: SymKey, s: u64, kind: TargetKind) -> &mut Self {
        self.syms.insert(k, SymValue { s, kind, size: 0, live: true });
        self.facts.insert(k, SymFacts { kind, sym_type: 0, st_other: 0, tls: false });
        self
    }

    pub fn tls_sym(&mut self, k: SymKey, s: u64, kind: TargetKind) -> &mut Self {
        self.sym(k, s, kind);
        self.facts.get_mut(&k).unwrap().tls = true;
        self
    }
}

impl FillTables for Tables {
    fn sym(&self, key: SymKey) -> SymValue {
        self.syms[&key]
    }
    fn slot(&self, key: SymKey, kind: SlotKind) -> Option<u64> {
        self.slots.get(&(key, kind)).copied()
    }
    fn plt(&self, key: SymKey) -> Option<u64> {
        self.plts.get(&key).copied()
    }
    fn flags(&self, key: SymKey) -> DemandFlags {
        self.flags.get(&key).copied().unwrap_or_default()
    }
    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        self.redirects.get(&(section, offset)).copied()
    }
}

impl ScanTables for Tables {
    fn sym(&self, key: SymKey) -> SymFacts {
        self.facts[&key]
    }
    fn name(&self, _key: SymKey) -> &[u8] {
        b"sym"
    }
}

/// One relocation applied to a copy of `input` placed at `addr`.
pub struct Case<'a> {
    pub arch: A64,
    pub output: OutputKind,
    pub tables: &'a Tables,
    pub input: &'a [u8],
    pub addr: u64,
    pub alloc: bool,
    pub name: &'a [u8],
    pub got_base: Option<u64>,
    pub tls: Option<TlsBlock>,
}

impl<'a> Case<'a> {
    pub fn new(tables: &'a Tables, input: &'a [u8], addr: u64) -> Self {
        Self {
            arch: A64::M1,
            output: OutputKind::Static,
            tables,
            input,
            addr,
            alloc: true,
            name: b".text",
            got_base: None,
            tls: None,
        }
    }

    pub fn howto(&self, r_type: u32, offset: u64) -> Howto {
        self.arch
            .classify(r_type, Site { bytes: self.input, offset })
            .unwrap_or_else(|r| panic!("{r_type} refused: {r:?}"))
    }

    /// Applies `relocs` (in the given order) and returns the bytes.
    pub fn apply(&self, relocs: &[Reloc]) -> Result<Vec<u8>, FieldError> {
        let mut bytes = self.input.to_vec();
        let cx = FillCx {
            tables: self.tables,
            output: self.output,
            got_base: self.got_base,
            tls: self.tls,
        };
        for (i, r) in relocs.iter().enumerate() {
            let h = self.howto(r.r_type, r.r_offset);
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(3),
                index: u32::try_from(i).unwrap(),
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: r.addend,
                howto: &h,
                site: Site { bytes: self.input, offset: r.r_offset },
                records: &relocs,
            };
            let mut out = SectionOut {
                bytes: &mut bytes,
                addr: self.addr,
                alloc: self.alloc,
                name: self.name,
            };
            self.arch.apply(&cx, &at, &mut out)?;
        }
        Ok(bytes)
    }

    /// One relocation; the word at `r_offset` afterwards.
    pub fn word(
        &self,
        r_type: u32,
        r_offset: u64,
        k: SymKey,
        addend: i64,
    ) -> Result<u32, FieldError> {
        let out = self.apply(&[Reloc { r_offset, key: k, r_type, addend }])?;
        let o = usize::try_from(r_offset).unwrap();
        Ok(u32::from_le_bytes(out[o..o + 4].try_into().unwrap()))
    }
}

#[derive(Default, Debug)]
pub struct Sink {
    pub raised: Vec<(SymKey, DemandFlags)>,
    pub global: GlobalFlags,
    pub counts: Vec<(DynRole, u32)>,
    pub errors: Vec<(u32, ScanError)>,
}

impl DemandOut for Sink {
    fn raise(&mut self, key: SymKey, flags: DemandFlags) {
        self.raised.push((key, flags));
    }
    fn raise_global(&mut self, flags: GlobalFlags) {
        self.global |= flags;
    }
    fn count(&mut self, role: DynRole, n: u32) {
        self.counts.push((role, n));
    }
    fn error(&mut self, record: u32, error: ScanError) {
        self.errors.push((record, error));
    }
}

/// Scans one relocation over `input`.
pub fn scan_one(
    arch: A64,
    tables: &Tables,
    output: OutputKind,
    place: PlaceFacts,
    input: &[u8],
    r: Reloc,
) -> Result<Sink, mortar_arch::Refusal> {
    let h = arch.classify(r.r_type, Site { bytes: input, offset: r.r_offset })?;
    let records: &[Reloc] = &[r];
    let at = RelocAt {
        file: FileId(1),
        section: SectionId(3),
        index: 0,
        r_offset: r.r_offset,
        key: r.key,
        r_type: r.r_type,
        addend: r.addend,
        howto: &h,
        site: Site { bytes: input, offset: r.r_offset },
        records: &records,
    };
    let cx = ScanCx { tables, output, place };
    let mut sink = Sink::default();
    arch.scan(&cx, &at, &mut DemandSink::new(&mut sink));
    Ok(sink)
}

#[must_use]
pub fn words(ws: &[u32]) -> Vec<u8> {
    ws.iter().flat_map(|w| w.to_le_bytes()).collect()
}
