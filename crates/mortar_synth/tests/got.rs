//! L1 tests of the GOT, `.got.plt`, the IPLT and `.rela.plt`
//! (`spec/synth.md` §6) on AArch64, through the image interface.

mod common;

use std::collections::BTreeMap;

use common::{collector, text};
use mortar_arch::{Arch, PltVariant, TlsBlock};
use mortar_core::demands::GlobalFlags;
use mortar_core::par::Pool;
use mortar_core::{Slots, SymKey, SymbolId};
use mortar_diag::FileRef;
use mortar_synth::got::{GotAddrs, GotPlan, Region, SlotValues};
use mortar_synth::hdr::HdrAddrs;
use mortar_synth::image::{Division, FillCx, GotChunk, GotPart, SizeCx, SynthChunk};

const A64: mortar_a64::A64 = mortar_a64::A64::M1;

fn k(n: u32) -> SymKey {
    SymKey::global(SymbolId(n))
}

struct Values(BTreeMap<SymKey, u64>);

impl SlotValues for Values {
    fn address(&self, key: SymKey) -> Option<u64> {
        self.0.get(&key).copied()
    }
    fn tls(&self) -> Option<TlsBlock> {
        Some(TlsBlock { start: 0x30000, size: 0x100, align: 16 })
    }
}

struct NoSyms;
impl mortar_synth::symtab::SymAddrs for NoSyms {
    fn section(&self, _: mortar_core::SectionId, _: u64) -> Option<(u32, u64)> {
        None
    }
    fn eh_frame(&self) -> Option<(u32, u64)> {
        None
    }
    fn common(&self, _: SymbolId) -> Option<(u32, u64)> {
        None
    }
    fn internal(&self, _: SymbolId) -> Option<mortar_synth::symtab::InternalValue> {
        None
    }
    fn patch(&self, _: usize) -> Option<(u32, u64)> {
        None
    }
    fn tls_start(&self) -> Option<u64> {
        None
    }
    fn section_range(&self, _: u32) -> Option<(u64, u64)> {
        None
    }
}

fn slots() -> Slots {
    Slots {
        got: vec![k(1), k(2), k(3)].into(),
        tls_ie: vec![k(4)].into(),
        tls_gd: vec![k(4)].into(),
        tls_ld: true,
        iplt: vec![k(3), k(5)].into(),
        ..Slots::default()
    }
}

fn plan(s: &Slots, flags: GlobalFlags) -> Result<GotPlan, String> {
    let diag = collector();
    let p = GotPlan::new(&A64, s, flags, PltVariant::Plain, &diag);
    let r = diag.finish();
    p.map_err(|_| text(&r))
}

const AT: GotAddrs = GotAddrs { got: 0x20000, got_plt: 0x21000, plt: 0x400 };

fn fill(p: &GotPlan, part: GotPart) -> Vec<u8> {
    let values = Values(BTreeMap::from([
        (k(1), 0x1111),
        (k(2), 0x2222),
        (k(3), 0x3333),
        (k(4), 0x30010),
        (k(5), 0x5555),
    ]));
    let output = FileRef::new("a.out");
    let code = |_: mortar_synth::eh::CodeRef| None;
    let cx = FillCx {
        arch: &A64,
        pool: &Pool::sequential(),
        output: &output,
        slots: &values,
        syms: &NoSyms,
        got: AT,
        hdr: HdrAddrs { hdr: 0, eh: 0 },
        code_addr: &code,
        section_offset: &|_| None,
    };
    let chunk = GotChunk { plan: p, part };
    let size = chunk.size(&SizeCx::new());
    let mut whole = vec![0xaa; size as usize];
    chunk.write(&cx, 0..size, &mut whole).unwrap();
    // I-103: a split by 4 gives the same bytes.
    let mut split = vec![0xaa; size as usize];
    for (n, part) in split.chunks_mut(4).enumerate() {
        let s = 4 * n as u64;
        chunk.write(&cx, s..s + part.len() as u64, part).unwrap();
    }
    assert_eq!(split, whole);
    assert_eq!(chunk.division(), Division::Divisible);
    whole
}

fn words(b: &[u8]) -> Vec<u64> {
    b.chunks(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

#[test]
fn s63_regions_in_order_with_their_slot_sizes() {
    let p = plan(&slots(), GlobalFlags::empty()).unwrap();
    // Addr 3, TpOff 1, TlsGd 1 × 2, TlsLd 2 words.
    assert_eq!(p.got_size(), 8 * (3 + 1 + 2 + 2));
    assert_eq!(p.got_offset(Region::TpOff, 0), 24);
    assert_eq!(p.got_offset(Region::TlsGd, 0), 32);
    assert_eq!(p.got_offset(Region::TlsLd, 0), 48);
}

#[test]
fn s65_header_word_only_with_got_base() {
    let without = plan(&slots(), GlobalFlags::empty()).unwrap();
    let with = plan(&slots(), GlobalFlags::GOT_BASE).unwrap();
    assert_eq!(with.got_size(), without.got_size() + 8);
    assert_eq!(words(&fill(&with, GotPart::Got))[0], 0, "S-65: 0 in a static output");
}

#[test]
fn s66_s72_slot_values() {
    let p = plan(&slots(), GlobalFlags::empty()).unwrap();
    let w = words(&fill(&p, GotPart::Got));
    assert_eq!(w[0..2], [0x1111, 0x2222]);
    assert_eq!(w[2], AT.plt, "an IFUNC's address slot holds its IPLT entry");
    // AArch64 TLS variant I: tcb 16, so tp offset 0x10 + 16.
    assert_eq!(w[3], 0x20);
    assert_eq!(w[4..6], [1, 0x10], "TLS GD: module 1, dtp offset");
    assert_eq!(w[6..8], [1, 0], "TLS LD: module 1, 0");
}

#[test]
fn s68_iplt_entries() {
    let p = plan(&slots(), GlobalFlags::empty()).unwrap();
    assert_eq!(p.plt_size(), 2 * p.iplt_entry(1));
    let b = fill(&p, GotPart::Plt);
    assert_eq!(b.len() as u64, p.plt_size());
    let mut expect = vec![0u8; p.iplt_entry(1) as usize];
    let cx = mortar_arch::SynthCx {
        addr: AT.plt + p.iplt_entry(1),
        header: AT.plt,
        got_plt: AT.got_plt + p.iplt_word(1),
        got_base: AT.got,
        index: 1,
    };
    A64.write_plt(PltVariant::Plain, mortar_arch::PltPart::Iplt(1), &cx, &mut expect).unwrap();
    assert_eq!(&b[p.iplt_entry(1) as usize..], &expect[..]);
}

#[test]
fn s68_plt_entries_in_a_static_output_are_refused() {
    let s = Slots { plt: vec![k(9)].into(), ..Slots::default() };
    assert_eq!(
        plan(&s, GlobalFlags::empty()).unwrap_err(),
        "ld: error: unsupported feature: PLT entries in a static output\n"
    );
}

#[test]
fn s67_aarch64_got_plt_has_no_reserved_words() {
    let p = plan(&slots(), GlobalFlags::GOT_BASE).unwrap();
    assert_eq!(p.got_plt_size(), 16, "one word per IPLT entry");
    assert_eq!(
        words(&fill(&p, GotPart::GotPlt)),
        [0x3333, 0x5555],
        "AArch64: the word is the resolver S+A (dyn Q7, Facts.iplt_word)"
    );
}

#[test]
fn s69_rela_plt_one_irelative_per_iplt_entry() {
    let p = plan(&slots(), GlobalFlags::empty()).unwrap();
    assert_eq!(p.rela_entsize(), 24);
    let w = words(&fill(&p, GotPart::RelaPlt));
    let irelative = A64.dyn_type(mortar_arch::DynRole::IRelative).unwrap();
    assert_eq!(
        w,
        [AT.got_plt, u64::from(irelative), 0x3333, AT.got_plt + 8, u64::from(irelative), 0x5555]
    );
}

#[test]
fn s3_no_slots_no_chunks() {
    let p = plan(&Slots::default(), GlobalFlags::empty()).unwrap();
    assert_eq!((p.got_size(), p.got_plt_size(), p.plt_size(), p.rela_plt_size()), (0, 0, 0, 0));
}
