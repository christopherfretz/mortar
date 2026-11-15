//! Fakes for the image's tests: an in-memory store (resident or copied,
//! with a failure at a chosen offset), tables, a memory sink, synthetic
//! chunks, a generator of random plans, and the simple sequential
//! reference fill that every parallel result is compared with.
#![allow(dead_code, clippy::missing_panics_doc, clippy::too_many_lines)]

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mortar_a64::A64;
use mortar_arch::{
    Arch, DemandFlags, Field, FillCx, FillTables, OutputKind, Proof, ProofRun, Reloc, RelocAt,
    SectionOut, Site, SlotKind, SymNames, SymValue, TargetKind, TlsBlock,
};
use mortar_core::inputs::{CopyError, Errno};
use mortar_core::par::Pool;
use mortar_core::records::RecIndex;
use mortar_core::{ChunkId, FileId, SectionId, SymKey, SymbolId};
use mortar_diag::FileRef;
use mortar_image::fill::{Image, Sink, Tables};
use mortar_image::plan::{
    Builder, InputUnit, Pattern, Plan, RecRows, RelocSection, SynthUnit, Unit,
};
use mortar_image::source::{NoKeys, Source};
use mortar_image::synth::{Division, SizeCx, SynthChunk, SynthCx, SynthError};
use mortar_image::target::Target;
use mortar_image::tombstone;

pub mod front;
pub mod reflink;

pub const ARCH: A64 = A64::M1;
/// M2: the TLS relaxations (`spec/a64.md` §8.4).
pub const ARCH_M2: A64 = A64::new(mortar_arch::Status::M2, true);
pub const ABS64: u32 = 257;
pub const ABS32: u32 = 258;
pub const PREL32: u32 = 261;
pub const ADR_PREL_PG_HI21: u32 = 275;
pub const ADD_ABS_LO12_NC: u32 = 277;
pub const CALL26: u32 = 283;
pub const TLSGD_ADR_PAGE21: u32 = 513;
pub const TLSGD_ADD_LO12_NC: u32 = 514;
pub const BL: u32 = 0x9400_0000;
pub const ADRP_X0: u32 = 0x9000_0000;
pub const ADD_X0_X0: u32 = 0x9100_0000;
pub const NOP: u32 = 0xd503_201f;
pub const TLS: TlsBlock = TlsBlock { start: 0x50_0000, size: 0x1000, align: 16 };

#[must_use]
pub fn key(n: u32) -> SymKey {
    SymKey::global(SymbolId(n))
}

/// The scan's record index for `sections` (section, records): the
/// permutation of each unsorted one (I-29); every section with records
/// leads (`RelocAt.leads = true` is always correct, arch.md §3.5).
#[must_use]
pub fn index_of<'r>(sections: impl IntoIterator<Item = (SectionId, &'r [Reloc])>) -> RecIndex {
    let mut permuted = Vec::new();
    let mut leads = Vec::new();
    for (s, r) in sections {
        if r.is_empty() {
            continue;
        }
        leads.push(s);
        if !r.windows(2).all(|w| w[0].r_offset <= w[1].r_offset) {
            let mut perm: Vec<u32> = (0..r.len() as u32).collect();
            perm.sort_by_key(|&i| r[i as usize].r_offset);
            permuted.push((s, perm.into_boxed_slice()));
        }
    }
    RecIndex::new(permuted, leads)
}

/// The record index of a plan whose records are all decoded.
#[must_use]
pub fn plan_index(plan: &Plan) -> RecIndex {
    index_of(plan.relocs.iter().filter_map(|r| match &r.rows {
        RecRows::Decoded(d) => Some((r.section, &d[..])),
        RecRows::Rela(_) => None,
    }))
}

/// The decoded records of a relocation section.
#[must_use]
pub fn decoded(rs: &RelocSection) -> &[Reloc] {
    match &rs.rows {
        RecRows::Decoded(d) => d,
        RecRows::Rela(_) => panic!("records in the store"),
    }
}

/// The answers a chunk's `write` reads, none known.
pub static OUT_REF: std::sync::LazyLock<FileRef> = std::sync::LazyLock::new(|| FileRef::new("out"));
pub static SEQ_POOL: std::sync::LazyLock<Pool> = std::sync::LazyLock::new(Pool::sequential);

#[must_use]
pub fn no_answers() -> SynthCx<'static> {
    mortar_image::synth::no_answers(&ARCH, &SEQ_POOL, &OUT_REF)
}

/// A target with AArch64's facts.
#[must_use]
pub fn a64() -> Target {
    Target::from_facts(Arch::facts(&A64::M1))
}

// ---- the store -------------------------------------------------------------

/// Files in memory. A non-resident file answers only `copy_to`, so both
/// paths of I-35 step 1 run. `fail` makes `copy_to` of a range holding
/// that byte fail with `EIO`.
#[derive(Default)]
pub struct MemSource {
    pub files: BTreeMap<FileId, (Vec<u8>, bool)>,
    pub fail: Option<(FileId, u64)>,
}

impl Source for MemSource {
    fn resident(&self, file: FileId, range: Range<u64>) -> Option<&[u8]> {
        let (bytes, resident) = self.files.get(&file)?;
        if !resident {
            return None;
        }
        bytes.get(range.start as usize..range.end as usize)
    }

    fn copy_to(&self, file: FileId, range: Range<u64>, out: &mut [u8]) -> Result<(), CopyError> {
        if let Some((f, at)) = self.fail
            && f == file
            && range.contains(&at)
        {
            return Err(CopyError::Read(Errno(5)));
        }
        let (bytes, _) = self.files.get(&file).ok_or(CopyError::OutOfRange)?;
        let src =
            bytes.get(range.start as usize..range.end as usize).ok_or(CopyError::OutOfRange)?;
        if src.len() != out.len() {
            return Err(CopyError::OutOfRange);
        }
        out.copy_from_slice(src);
        Ok(())
    }
}

// ---- the tables ------------------------------------------------------------

#[derive(Default, Clone)]
pub struct FakeTables {
    pub syms: HashMap<SymKey, SymValue>,
    pub alloc: HashMap<SymKey, bool>,
    pub names: HashMap<SymKey, Vec<u8>>,
    pub redirects: HashMap<(SectionId, u64), u64>,
}

impl FakeTables {
    pub fn define(&mut self, k: SymKey, s: u64, live: bool) -> &mut Self {
        self.syms.insert(k, SymValue { s, kind: TargetKind::Local, size: 0, live, tls: false });
        self.alloc.insert(k, true);
        self
    }
}

impl SymNames for FakeTables {
    fn name(&self, key: SymKey) -> &[u8] {
        self.names.get(&key).map_or(b"sym", |n| n)
    }
}

impl FillTables for FakeTables {
    fn sym(&self, key: SymKey) -> SymValue {
        self.syms[&key]
    }
    fn slot(&self, _key: SymKey, _kind: SlotKind) -> Option<u64> {
        None
    }
    fn plt(&self, _key: SymKey) -> Option<u64> {
        None
    }
    fn flags(&self, _key: SymKey) -> DemandFlags {
        DemandFlags::default()
    }
    fn redirect(&self, section: SectionId, offset: u64) -> Option<u64> {
        self.redirects.get(&(section, offset)).copied()
    }
}

impl Tables for FakeTables {
    fn target_alloc(&self, key: SymKey) -> bool {
        self.alloc.get(&key).copied().unwrap_or(true)
    }
}

// ---- the sink --------------------------------------------------------------

/// The file in memory; `fail_at` makes the write of the window holding
/// that offset fail with `ENOSPC`.
#[derive(Default)]
pub struct MemSink {
    pub bytes: Mutex<Vec<u8>>,
    pub fail_at: Option<u64>,
    pub writes: Mutex<Vec<(u64, usize)>>,
}

impl Sink for MemSink {
    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        if let Some(at) = self.fail_at
            && (offset..offset + bytes.len() as u64).contains(&at)
        {
            return Err(io::Error::from_raw_os_error(28));
        }
        self.writes.lock().unwrap().push((offset, bytes.len()));
        let mut b = self.bytes.lock().unwrap();
        let end = offset as usize + bytes.len();
        if b.len() < end {
            b.resize(end, 0xee);
        }
        b[offset as usize..end].copy_from_slice(bytes);
        Ok(())
    }
}

// ---- synthetic chunks -------------------------------------------------------

/// A divisible chunk: byte `i` is `seed + i * 31`.
pub struct Counting {
    pub size: u64,
    pub seed: u8,
    pub sealed: Option<Range<u64>>,
}

impl SynthChunk for Counting {
    fn size(&self, _cx: &SizeCx<'_>) -> u64 {
        self.size
    }
    fn division(&self) -> Division {
        Division::Divisible
    }
    fn write(
        &self,
        _cx: &SynthCx<'_>,
        range: Range<u64>,
        out: &mut [u8],
    ) -> Result<(), SynthError> {
        assert_eq!(out.len() as u64, range.end - range.start);
        for (k, b) in out.iter_mut().enumerate() {
            *b = self.seed.wrapping_add(((range.start + k as u64) * 31) as u8);
        }
        Ok(())
    }
    fn sealed(&self) -> Option<Range<u64>> {
        self.sealed.clone()
    }
}

/// A whole chunk: its bytes are a running sum, so a split call would give
/// other bytes; `write` refuses any range but the whole chunk.
pub struct Summing {
    pub size: u64,
}

impl SynthChunk for Summing {
    fn size(&self, _cx: &SizeCx<'_>) -> u64 {
        self.size
    }
    fn division(&self) -> Division {
        Division::Whole { max_size: self.size }
    }
    fn write(
        &self,
        _cx: &SynthCx<'_>,
        range: Range<u64>,
        out: &mut [u8],
    ) -> Result<(), SynthError> {
        if range != (0..self.size) {
            return Err(SynthError::from(mortar_diag::Diagnostic::error(
                mortar_diag::Kind::InternalError {
                    what: format!("whole chunk written in part: {range:?}"),
                },
            )));
        }
        let mut acc = 0u8;
        for (k, b) in out.iter_mut().enumerate() {
            acc = acc.wrapping_add(k as u8 ^ 0x5a);
            *b = acc;
        }
        Ok(())
    }
    fn sealed(&self) -> Option<Range<u64>> {
        None
    }
}

// ---- the reference ---------------------------------------------------------

/// The simple sequential reference (I-35 "as if the whole unit were filled
/// into one buffer"): every unit whole, every relocation unclipped on the
/// whole input section, in record order; then each row's bytes.
#[must_use]
pub fn reference(img: &Image<'_>) -> Vec<u8> {
    let plan = img.plan;
    let mut file = vec![0u8; plan.file_size as usize];
    let cx =
        FillCx { tables: img.tables, output: img.output, got_base: img.got_base, tls: img.tls };
    for row in &plan.rows {
        let dst = &mut file[row.offset as usize..row.end() as usize];
        match row.unit {
            Unit::Gap(p) => p.fill(row.offset, dst),
            Unit::Bytes(i) => dst.copy_from_slice(&plan.bytes[i as usize]),
            Unit::Synth(i) => {
                let su = &plan.synth[i as usize];
                let chunk = img.chunks[su.chunk as usize];
                chunk.write(&img.synth, 0..row.size, dst).unwrap();
                if let Some(z) = chunk.sealed() {
                    dst[z.start as usize..z.end as usize].fill(0);
                }
            }
            Unit::Merged(_) => panic!("the reference has no merged chunks"),
            Unit::Input(i) => {
                let u = &plan.inputs[i as usize];
                let mut sec = vec![0u8; u.section_size as usize];
                let base = u.src - u.start;
                img.source.copy_to(u.file, base..base + u.section_size, &mut sec).unwrap();
                let input = sec.clone();
                if let Some(r) = u.relocs {
                    reference_relocs(img, &cx, u, &plan.relocs[r as usize], &input, &mut sec);
                }
                dst.copy_from_slice(&sec[u.start as usize..(u.start + row.size) as usize]);
            }
        }
    }
    file
}

fn reference_relocs(
    img: &Image<'_>,
    cx: &FillCx<'_>,
    u: &InputUnit,
    rs: &RelocSection,
    input: &[u8],
    sec: &mut [u8],
) {
    let name = img.plan.name(u.name).to_vec();
    let records: &[Reloc] = decoded(rs);
    for (i, rec) in records.iter().enumerate() {
        let Ok(howto) = img.arch.classify(rec.r_type, Site { bytes: input, offset: rec.r_offset })
        else {
            continue;
        };
        if let Field::Data { bytes: w } = howto.field
            && !img.tables.sym(rec.key).live
            && let Some(v) = tombstone::field(img.arch, &name, u.alloc, w)
        {
            let v = v.to_le_bytes();
            let o = rec.r_offset as usize;
            let w = usize::from(w);
            sec[o..o + w].copy_from_slice(&v[..w]);
            if name == tombstone::ARANGES
                && rec.r_type == img.plan.target.abs_addr
                && img.tables.target_alloc(rec.key)
                && !records.iter().any(|x| {
                    (rec.r_offset + w as u64..rec.r_offset + 2 * w as u64).contains(&x.r_offset)
                })
                && o + 2 * w <= sec.len()
            {
                sec[o + w..o + 2 * w].fill(0);
            }
            continue;
        }
        let at = RelocAt {
            file: u.file,
            section: u.section,
            index: i as u32,
            r_offset: rec.r_offset,
            key: rec.key,
            r_type: rec.r_type,
            addend: rec.addend,
            howto: &howto,
            site: Site { bytes: input, offset: rec.r_offset },
            records: &records,
            proof: &rs.proof,
            leads: true,
        };
        let mut out = SectionOut::whole(sec, u.addr, u.alloc, &name);
        let _ = img.arch.apply(cx, &at, &mut out);
    }
}

// ---- a random plan ---------------------------------------------------------

/// A deterministic generator (an LCG; the tests need no quality).
pub struct Gen(pub u64);

impl Gen {
    pub fn next(&mut self) -> u64 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 17
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// One input section of a random plan, before the builder.
pub struct GenSection {
    pub bytes: Vec<u8>,
    pub records: Vec<Reloc>,
    pub name: &'static [u8],
    pub alloc: bool,
    pub exec: bool,
}

/// Everything a random test link owns.
pub struct World {
    pub plan: Plan,
    pub source: MemSource,
    pub tables: FakeTables,
    pub chunks: Vec<Box<dyn SynthChunk + Sync>>,
    pub records: RecIndex,
}

/// A chunk as fill takes it.
pub type Chunk<'a> = &'a (dyn SynthChunk + Sync);

impl World {
    /// The world with the record index of its plan.
    #[must_use]
    pub fn indexed(mut self) -> Self {
        self.records = plan_index(&self.plan);
        self
    }

    pub fn image(&self) -> (Vec<Chunk<'_>>, ()) {
        (self.chunks.iter().map(|c| &**c as Chunk<'_>).collect(), ())
    }
}

/// The image of `w` with `chunks`.
#[must_use]
pub fn image<'a>(w: &'a World, chunks: &'a [Chunk<'a>]) -> Image<'a> {
    Image {
        plan: &w.plan,
        source: &w.source,
        arch: &ARCH,
        tables: &w.tables,
        records: &w.records,
        keys: &NoKeys,
        output: OutputKind::Static,
        got_base: None,
        tls: Some(TLS),
        chunks,
        synth: no_answers(),
        merged: None,
        eh: None,
        build_id: None,
    }
}

const TEXT_ADDR: u64 = 0x40_0000;

/// A key whose target is live (`random_world` makes every seventh dead).
fn live_key(g: &mut Gen, syms: u32) -> SymKey {
    loop {
        let n = 1 + g.below(u64::from(syms)) as u32;
        if !n.is_multiple_of(7) {
            return key(n);
        }
    }
}

/// A code section of `words` words: random data words, and relocations of
/// the forms the M1 apply accepts, at random places.
fn code(g: &mut Gen, words: usize, syms: u32) -> GenSection {
    let mut ws: Vec<u32> = (0..words).map(|_| g.next() as u32 & 0x00ff_ffff).collect();
    let mut records = Vec::new();
    let mut k = 0usize;
    while k + 4 < words {
        match g.below(6) {
            0 => {
                ws[k] = BL;
                records.push(Reloc {
                    r_offset: 4 * k as u64,
                    key: live_key(g, syms),
                    r_type: CALL26,
                    addend: 0,
                });
            }
            1 => {
                ws[k] = ADRP_X0;
                ws[k + 1] = ADD_X0_X0;
                let s = live_key(g, syms);
                let a = g.below(64) as i64;
                records.push(Reloc {
                    r_offset: 4 * k as u64,
                    key: s,
                    r_type: ADR_PREL_PG_HI21,
                    addend: a,
                });
                records.push(Reloc {
                    r_offset: 4 * k as u64 + 4,
                    key: s,
                    r_type: ADD_ABS_LO12_NC,
                    addend: a,
                });
                k += 1;
            }
            _ => {}
        }
        k += 1 + g.below(3) as usize;
    }
    let bytes = ws.iter().flat_map(|w| w.to_le_bytes()).collect();
    GenSection { bytes, records, name: b".text", alloc: true, exec: true }
}

/// A data section: 8-byte words with `ABS64` or `PREL32`, some against dead
/// targets in a debug section.
fn data(g: &mut Gen, words: usize, syms: u32, name: &'static [u8], alloc: bool) -> GenSection {
    let bytes: Vec<u8> = (0..words * 8).map(|_| g.next() as u8).collect();
    let mut records = Vec::new();
    let mut k = 0usize;
    while k < words {
        // A dead target only where fill accepts one (I-45, I-46): the
        // scan refuses it from any other section.
        let s = if alloc && name != b".gcc_except_table" {
            live_key(g, syms)
        } else {
            key(1 + g.below(u64::from(syms)) as u32)
        };
        if g.below(3) == 0 && alloc {
            records.push(Reloc {
                r_offset: 8 * k as u64,
                key: s,
                r_type: PREL32,
                addend: g.below(16) as i64,
            });
        } else {
            records.push(Reloc {
                r_offset: 8 * k as u64,
                key: s,
                r_type: ABS64,
                addend: g.below(16) as i64,
            });
        }
        k += 1 + g.below(3) as usize;
    }
    GenSection { bytes, records, name, alloc, exec: false }
}

/// A random plan of about `budget` bytes: code, data, debug sections (some
/// with unsorted records), fixed bytes, divisible and whole synthetic
/// chunks, padding with the trap pattern between code.
#[must_use]
pub fn random_world(seed: u64, budget: u64, window: u64) -> World {
    let mut g = Gen(seed);
    let syms = 40u32;
    let mut tables = FakeTables::default();
    for n in 1..=syms {
        let live = n % 7 != 0;
        tables.define(key(n), TEXT_ADDR + 0x1000 * u64::from(n) + g.below(0x800) * 4, live);
    }
    let mut source = MemSource::default();
    let target = a64();
    let mut rows: Vec<(u64, GenSection)> = Vec::new();
    let mut fixed: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut synths: Vec<(u64, u64, bool)> = Vec::new();
    let mut pads: Vec<(Range<u64>, Pattern)> = Vec::new();
    let mut at = 64u64;
    fixed.push((0, (0..64).map(|i| i as u8).collect()));
    let mut prev_exec_end: Option<u64> = None;
    while at < budget {
        let kind = g.below(10);
        let pad = [0, 4, 8, 12, 60][g.below(5) as usize];
        // Sections are 8-aligned in the file and in memory.
        let start = if kind <= 6 { (at + pad).next_multiple_of(8) } else { at + pad };
        match kind {
            0..=3 => {
                let n = 1 + g.below(3 * window / 4 + 64) as usize;
                let s = code(&mut g, n, syms);
                if let Some(e) = prev_exec_end
                    && e < start
                {
                    pads.push((e..start, target.trap));
                }
                at = start + s.bytes.len() as u64;
                prev_exec_end = Some(at);
                rows.push((start, s));
                continue;
            }
            4 | 5 => {
                let name: &'static [u8] = [
                    &b".debug_info"[..],
                    b".debug_aranges",
                    b".debug_ranges",
                    b".gcc_except_table",
                ][g.below(4) as usize];
                let alloc = name == b".gcc_except_table";
                let n = 1 + g.below(window / 6 + 8) as usize;
                let s = data(&mut g, n, syms, name, alloc);
                at = start + s.bytes.len() as u64;
                rows.push((start, s));
            }
            6 => {
                let n = 1 + g.below(window / 10 + 8) as usize;
                let s = data(&mut g, n, syms, b".data", true);
                at = start + s.bytes.len() as u64;
                rows.push((start, s));
            }
            7 => {
                let len = 1 + g.below(300);
                fixed.push((start, (0..len).map(|_| g.next() as u8).collect()));
                at = start + len;
            }
            8 => {
                let size = 1 + g.below(2 * window);
                synths.push((start, size, false));
                at = start + size;
            }
            _ => {
                let size = 1 + g.below(3 * window);
                synths.push((start, size, true));
                at = start + size;
            }
        }
        prev_exec_end = None;
    }
    let file_size = at + g.below(100);
    let mut b = Builder::new(target, file_size);
    let mut chunks: Vec<Box<dyn SynthChunk + Sync>> = Vec::new();
    for (k, (offset, mut s)) in rows.into_iter().enumerate() {
        let file = FileId(1 + k as u32);
        let section = SectionId(100 + k as u32);
        b.path(file, FileRef::new(format!("in{k}.o")));
        // Every fourth section has its records out of order (I-29).
        if k % 4 == 3 {
            s.records.reverse();
        }
        let len = s.bytes.len() as u64;
        let proof = if s.exec {
            vec![ProofRun { start: 0, end: len, proof: Proof::Insn, unit: 0, unit_offset: 0 }]
        } else {
            Vec::new()
        };
        let relocs = (!s.records.is_empty()).then(|| {
            b.relocs(RelocSection {
                section,
                rows: RecRows::Decoded(s.records.into_boxed_slice()),
                proof: proof.into_boxed_slice(),
            })
        });
        // A leading junk region: the section is not at offset 0 of its file.
        let lead = 8 * (k as u64 % 5);
        let mut fbytes = vec![0xab; lead as usize];
        fbytes.extend_from_slice(&s.bytes);
        source.files.insert(file, (fbytes, k % 2 == 0));
        let name = b.name(s.name);
        let addr = if s.alloc { TEXT_ADDR + offset } else { 0 };
        let unit = InputUnit {
            file,
            index: 1 + k as u32,
            section,
            src: lead,
            start: 0,
            section_size: len,
            addr,
            alloc: s.alloc,
            name,
            relocs,
        };
        if s.exec {
            pads.push((offset..offset + len, target.trap));
        }
        b.input(offset, len, unit);
    }
    for (offset, bytes) in fixed {
        b.bytes(offset, bytes.into_boxed_slice());
    }
    for (k, (offset, size, whole)) in synths.into_iter().enumerate() {
        let name = b.name(b".synth");
        let chunk = chunks.len() as u32;
        if whole {
            chunks.push(Box::new(Summing { size }));
        } else {
            chunks.push(Box::new(Counting { size, seed: k as u8, sealed: None }));
        }
        b.synth(
            offset,
            size,
            SynthUnit { chunk, id: ChunkId(500 + k as u32), name, whole, eh: false },
        );
    }
    // Pads are disjoint by construction (code rows and the gaps between).
    pads.sort_by_key(|p| p.0.start);
    for (r, p) in pads {
        b.pad(r, p);
    }
    let plan = b.finish(window, &Pool::sequential()).expect("a valid plan");
    let records = plan_index(&plan);
    World { plan, source, tables, chunks, records }
}

/// Fills `img` into memory.
pub fn fill_mem(img: &Image<'_>, pool: &Pool) -> Result<Vec<u8>, mortar_image::fill::Failed> {
    let sink = MemSink::default();
    mortar_image::fill::write(img, &sink, "out", pool)?;
    Ok(sink.bytes.into_inner().unwrap())
}

// ---- a unique temporary directory ------------------------------------------

/// A directory under the system temp dir, removed on drop (the workspace
/// mount is slow and does not enforce `flock`).
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("mortar-image-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    pub fn dir(&self) -> &Path {
        &self.0
    }
    /// The names in the directory, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn arch() -> &'static dyn Arch {
    &ARCH
}
