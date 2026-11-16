//! Fakes of the tables `scan` and `fill` read, for the behavioural tests.
#![allow(dead_code)]

use std::collections::HashMap;

use mortar_a64::A64;
use mortar_arch::{
    Arch, DemandFlags, DemandOut, DemandSink, DynRole, EventClass, FieldError, FillCx, FillTables,
    GlobalFlags, Howto, OutputKind, PlaceFacts, Proof, ProofRun, Reloc, RelocAt, ScanCx, ScanError,
    ScanTables, SectionOut, Site, SlotKind, SymFacts, SymNames, SymValue, TargetKind, TlsBlock,
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
    pub names: HashMap<SymKey, Vec<u8>>,
}

impl Tables {
    pub fn sym(&mut self, k: SymKey, s: u64, kind: TargetKind) -> &mut Self {
        self.syms.insert(k, SymValue { s, kind, size: 0, live: true, tls: false });
        self.facts.insert(
            k,
            SymFacts {
                kind,
                sym_type: 0,
                st_other: 0,
                tls: false,
                dead: false,
                comdat_only: false,
                gnu_warning: false,
                large: false,
            },
        );
        self
    }

    pub fn tls_sym(&mut self, k: SymKey, s: u64, kind: TargetKind) -> &mut Self {
        self.sym(k, s, kind);
        self.facts.get_mut(&k).unwrap().tls = true;
        self.syms.get_mut(&k).unwrap().tls = true;
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
}

impl SymNames for Tables {
    fn name(&self, key: SymKey) -> &[u8] {
        self.names.get(&key).map_or(b"sym", Vec::as_slice)
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
    /// The instruction proof of `input` (all code when empty: see `proof`).
    pub proof: Vec<ProofRun>,
}

/// One `$x` run over `len` bytes.
#[must_use]
pub fn all_code(len: usize) -> Vec<ProofRun> {
    vec![ProofRun { start: 0, end: len as u64, proof: Proof::Insn, unit: 0, unit_offset: 0 }]
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
            proof: all_code(input.len()),
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
        for i in 0..relocs.len() {
            self.apply_into(relocs, i, &mut bytes)?;
        }
        Ok(bytes)
    }

    /// Applies record `i` of `relocs` alone (the others are its records
    /// for §3.5) and returns the bytes.
    pub fn apply_at(&self, relocs: &[Reloc], i: usize) -> Result<Vec<u8>, FieldError> {
        let mut bytes = self.input.to_vec();
        self.apply_into(relocs, i, &mut bytes)?;
        Ok(bytes)
    }

    fn apply_into(&self, relocs: &[Reloc], i: usize, bytes: &mut [u8]) -> Result<(), FieldError> {
        let cx = FillCx {
            tables: self.tables,
            output: self.output,
            got_base: self.got_base,
            tls: self.tls,
        };
        {
            let r = &relocs[i];
            let h = self.howto(r.r_type, r.r_offset);
            let at = RelocAt {
                file: FileId(1),
                section: SectionId(3),
                index: u32::try_from(i).unwrap(),
                r_offset: r.r_offset,
                key: r.key,
                r_type: r.r_type,
                addend: Some(r.addend),
                howto: &h,
                site: Site { bytes: self.input, offset: r.r_offset },
                records: &relocs,
                proof: &self.proof,
                leads: true,
            };
            let mut out = SectionOut::whole(bytes, self.addr, self.alloc, self.name);
            self.arch.apply(&cx, &at, &mut out)?;
        }
        Ok(())
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
    /// The `Undefined` events, as `(key, record)`.
    pub undefined: Vec<(SymKey, u32)>,
    /// Every event, as `(record, class, key)`.
    pub events: Vec<(u32, EventClass, SymKey)>,
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
    fn reference(&mut self, record: u32, class: EventClass, key: SymKey) {
        if class == EventClass::Undefined {
            self.undefined.push((key, record));
        }
        self.events.push((record, class, key));
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
    let mut sink = Sink::default();
    scan_all(arch, tables, output, place, input, &[r], &mut sink)?;
    Ok(sink)
}

/// Scans every record of one section over `input`, in order; the
/// section is `.text` when `place` is alloc, else `.debug_info`.
pub fn scan_all(
    arch: A64,
    tables: &Tables,
    output: OutputKind,
    place: PlaceFacts,
    input: &[u8],
    relocs: &[Reloc],
    sink: &mut Sink,
) -> Result<(), mortar_arch::Refusal> {
    let name: &[u8] = if place.alloc { b".text" } else { b".debug_info" };
    scan_named(arch, tables, output, place, name, input, relocs, sink)
}

/// [`scan_all`] of a section called `name`.
#[allow(clippy::too_many_arguments)]
pub fn scan_named(
    arch: A64,
    tables: &Tables,
    output: OutputKind,
    place: PlaceFacts,
    name: &[u8],
    input: &[u8],
    relocs: &[Reloc],
    sink: &mut Sink,
) -> Result<(), mortar_arch::Refusal> {
    let proof = all_code(input.len());
    for (i, r) in relocs.iter().enumerate() {
        let h = arch.classify(r.r_type, Site { bytes: input, offset: r.r_offset })?;
        let at = RelocAt {
            file: FileId(1),
            section: SectionId(3),
            index: u32::try_from(i).unwrap(),
            r_offset: r.r_offset,
            key: r.key,
            r_type: r.r_type,
            addend: Some(r.addend),
            howto: &h,
            site: Site { bytes: input, offset: r.r_offset },
            records: &relocs,
            proof: &proof,
            leads: true,
        };
        let cx = ScanCx { tables, output, place, name };
        arch.scan(&cx, &at, &mut DemandSink::new(sink));
    }
    Ok(())
}

/// The flags raised for `k`, merged.
#[must_use]
pub fn raised(sink: &Sink, k: SymKey) -> DemandFlags {
    sink.raised.iter().filter(|(x, _)| *x == k).fold(DemandFlags::empty(), |a, (_, f)| a | *f)
}

#[must_use]
pub fn words(ws: &[u32]) -> Vec<u8> {
    ws.iter().flat_map(|w| w.to_le_bytes()).collect()
}
