//! AArch64's dynamic shapes against GNU ld and lld (WP-047's oracle;
//! a64.md §8.1, §8.2, §8.4, §10.2, §10.3; `spec/dyn.md` D-30, D-46, D-48,
//! D-49).
//!
//! Hand-written assembly calling four functions of a shared object and
//! using its TLS (and a TLS variable of its own) is linked by each
//! reference linker as `pie` and `-no-pie`, lazy and `-z now`, with and
//! without the BTI property. From each image:
//!
//! - **PLT**: PLT0 and every entry are compared with `write_plt` for the
//!   image's own `.plt` address and `JUMP_SLOT` places, the variant being
//!   what `plt_variant` gives the output's `FEATURE_1_AND`; `.plt`'s size
//!   is the shape's.
//! - **lazy word**: every `JUMP_SLOT` place holds `lazy_target`'s address
//!   (D-46), and a `.got.plt` begins with three zero words.
//! - **tags**: `DT_AARCH64_BTI_PLT` exactly with the BTI property,
//!   `DT_AARCH64_VARIANT_PCS` exactly with a `JUMP_SLOT` against an
//!   `STO_AARCH64_VARIANT_PCS` symbol; never `DT_AARCH64_PAC_PLT`.
//! - **TLS**: each sequence (TLSDESC → IE and → LE, IE kept and → LE, the
//!   traditional GD → IE and → LE) is applied by mortar to the object's
//!   bytes with the image's slots and TLS block, and compared byte for
//!   byte. GNU ld keeps IE against the executable's own TLS (D-48: mortar
//!   relaxes it, as lld does), and lld does not take the traditional GD
//!   dialect: those two are compared with the other linker only.

#![allow(clippy::too_many_lines, clippy::too_many_arguments)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mortar_a64::synth;
use mortar_arch::{
    Arch, LazyTarget, OutputCx, OutputKind, PltPart, Reloc, SlotKind, SynthCx, TargetKind,
};
use mortar_testutil::{LLVM_MAJORS, Scratch, run, tool};

mod support;
use support::{Case, M2, Tables, key};

mod elf;
use elf::{Image, Shdr, u32_at, u64_at};

const BTI_NOTE: &str = r#"
    .section .note.gnu.property, "a"
    .p2align 3
    .word 4, 16, 5
    .asciz "GNU"
    .word 0xc0000000, 4, 1, 0
"#;

const LIB: &str = r#"
    .text
    .globl f0, f1, f2, f3, __tls_get_addr
    .type f0, %function
    .type f1, %function
    .type f2, %function
    .type f3, %function
    .type __tls_get_addr, %function
f0: ret
f1: ret
f2: ret
f3: ret
__tls_get_addr: ret
    .section .tbss, "awT", @nobits
    .globl tv
    .type tv, %object
    .size tv, 8
    .p2align 3
    .zero 16
tv: .zero 8
"#;

/// The calls and the TLSDESC and IE sequences (every linker).
const MAIN: &str = r#"
    .text
    .globl _start
    .type _start, %function
_start:
    bl f0
    bl f1
    bl f2
    b f3
desc_tv:
    adrp x0, :tlsdesc:tv
    ldr x1, [x0, :tlsdesc_lo12:tv]
    add x0, x0, :tlsdesc_lo12:tv
    .tlsdesccall tv
    blr x1
desc_lv:
    adrp x0, :tlsdesc:lv
    ldr x1, [x0, :tlsdesc_lo12:lv]
    add x0, x0, :tlsdesc_lo12:lv
    .tlsdesccall lv
    blr x1
ie_tv:
    adrp x3, :gottprel:tv
    ldr x3, [x3, :gottprel_lo12:tv]
ie_lv:
    adrp x5, :gottprel:lv
    ldr x5, [x5, :gottprel_lo12:lv]
    ret
    .section .tbss, "awT", @nobits
    .p2align 4
    .zero 32
lv: .zero 8
"#;

/// The traditional GD dialect (a64.md §8.4's pattern) and the large
/// TLSDESC form against the executable's own TLS (R1l): GNU ld only.
const GD: &str = r#"
    .text
    .globl gd_tv
gd_tv:
    adrp x0, :tlsgd:tv
    add x0, x0, :tlsgd_lo12:tv
    bl __tls_get_addr
    nop
gd_lv:
    adrp x0, :tlsgd:lv2
    add x0, x0, :tlsgd_lo12:lv2
    bl __tls_get_addr
    nop
large_lv:
    movz x0, #:tlsdesc_off_g1:lv2
    movk x0, #:tlsdesc_off_g0_nc:lv2
    .tlsdescldr lv2
    ldr x1, [x2, x0]
    .tlsdescadd lv2
    add x0, x2, x0
    .tlsdesccall lv2
    blr x1
    ret
    .section .tbss, "awT", @nobits
    .p2align 3
    .zero 8
lv2: .zero 8
"#;

/// A call to a variant-PCS function (`DT_AARCH64_VARIANT_PCS`).
const VPCS: &str = r"
    .text
    .variant_pcs f3
    .globl vp
vp:
    bl f3
    ret
";

struct Tools {
    gcc: PathBuf,
    lld_dir: PathBuf,
}

fn tools() -> Option<Tools> {
    let lld_dir = LLVM_MAJORS
        .iter()
        .map(|m| PathBuf::from(format!("/usr/lib/llvm-{m}/bin")))
        .find(|d| d.join("ld.lld").is_file());
    let Some(lld_dir) = lld_dir else {
        tool("ld.lld")?;
        return None;
    };
    Some(Tools { gcc: tool("gcc")?, lld_dir })
}

fn must(what: &str, o: &mortar_testutil::Outcome) {
    assert!(o.success(), "{what} failed: {}", o.stderr_text());
}

fn assemble(t: &Tools, dir: &Scratch, name: &str, src: &str) -> PathBuf {
    let s = dir.write(&format!("{name}.s"), src);
    let o = dir.join(format!("{name}.o"));
    must(
        &format!("as {name}"),
        &run(&t.gcc, ["-c".into(), s.display().to_string(), "-o".into(), o.display().to_string()]),
    );
    o
}

fn ld_args(t: &Tools, linker: &str) -> Vec<String> {
    let mut a = vec!["-nostdlib".to_string(), "-Wl,--build-id=none".into()];
    if linker == "lld" {
        a.extend(["-fuse-ld=lld".into(), format!("-B{}/", t.lld_dir.display())]);
    } else {
        a.push("-fuse-ld=bfd".into());
    }
    a
}

// ------------------------------------------------------------ the image

/// A dynamic relocation: place, type, symbol name.
type DynRel = (u64, u32, String);

fn section<'a>(img: &'a Image, name: &str) -> Option<&'a Shdr> {
    img.sections.iter().find(|s| s.name == name)
}

fn cstr(b: &[u8], o: usize) -> String {
    let s = &b[o..];
    String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap()]).into_owned()
}

/// The records of a RELA section, with `.dynsym` names; and each
/// `.dynsym` name's `st_other`.
fn dyn_relocs(img: &Image, name: &str) -> Vec<DynRel> {
    let Some(rs) = section(img, name) else { return Vec::new() };
    let dynsym = section(img, ".dynsym").unwrap();
    let dynstr = img.data(&img.sections[dynsym.link as usize]).to_vec();
    let syms = img.data(dynsym).to_vec();
    img.data(rs)
        .chunks_exact(24)
        .map(|e| {
            let info = u64_at(e, 8);
            let i = (info >> 32) as usize;
            let n =
                if i == 0 { String::new() } else { cstr(&dynstr, u32_at(&syms, 24 * i) as usize) };
            (u64_at(e, 0), (info & 0xFFFF_FFFF) as u32, n)
        })
        .collect()
}

fn dyn_tags(img: &Image) -> Vec<u64> {
    section(img, ".dynamic")
        .map(|d| img.data(d).chunks_exact(16).map(|e| u64_at(e, 0)).collect())
        .unwrap_or_default()
}

fn word_at(img: &Image, addr: u64) -> u64 {
    let s = img
        .sections
        .iter()
        .find(|s| s.flags & 2 != 0 && s.addr <= addr && addr + 8 <= s.addr + s.size as u64)
        .unwrap_or_else(|| panic!("no section at {addr:#x}"));
    u64_at(img.data(s), (addr - s.addr) as usize)
}

fn bytes_at(img: &Image, addr: u64, n: usize) -> &[u8] {
    let s = section(img, ".text").unwrap();
    let o = (addr - s.addr) as usize;
    &img.data(s)[o..o + n]
}

fn sym(img: &Image, name: &str) -> elf::Sym {
    img.symbols().into_iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no {name}"))
}

#[derive(Default)]
struct Tally {
    headers: u32,
    entries: u32,
    /// lld's BTI entries without the landing pad (see `check_plt`).
    lld_unpadded: u32,
    lazy_words: u32,
    tag_checks: u32,
    tls: BTreeMap<&'static str, u32>,
    mismatches: Vec<String>,
}

/// PLT0, the entries, the lazy words and the tags of one image.
fn check_plt(label: &str, img: &Image, bti: bool, vpcs: bool, tally: &mut Tally) {
    let v = M2
        .plt_variant(&OutputCx {
            output: OutputKind::Exec,
            feature_1_and: u32::from(bti),
            pac_plt: false,
            force_bti: false,
            bind_now: false,
        })
        .unwrap();
    let shape = M2.plt(v);
    let (hdr, ent) = (u64::from(shape.chunks[0].header), u64::from(shape.chunks[0].entry));
    let plt = section(img, ".plt").unwrap_or_else(|| panic!("{label}: no .plt"));
    let pbytes = img.data(plt);
    let slots: Vec<DynRel> = dyn_relocs(img, ".rela.plt");
    assert!(slots.iter().all(|r| r.1 == 1026), "{label}: {slots:?}");
    // f0..f3, and GNU ld's entry for `__tls_get_addr` (whose calls R4
    // consumed) where the GD object is linked.
    assert!(slots.len() >= 4, "{label}: {slots:?}");
    assert_eq!(plt.size as u64, hdr + ent * slots.len() as u64, "{label}: .plt size");
    let g = slots[0].0 - 24;
    for (i, s) in slots.iter().enumerate() {
        assert_eq!(s.0, g + 24 + 8 * i as u64, "{label}: JUMP_SLOT places");
    }
    let mut buf = vec![0u8; hdr as usize];
    let cx = SynthCx { addr: plt.addr, header: plt.addr, got_plt: g + 16, got_base: 0, index: 0 };
    M2.write_plt(v, PltPart::Header(0), &cx, &mut buf).unwrap();
    if buf[..] == pbytes[..hdr as usize] {
        tally.headers += 1;
    } else {
        tally.mismatches.push(format!(
            "{label}: PLT0 {:02x?} != {:02x?}",
            buf,
            &pbytes[..hdr as usize]
        ));
    }
    for (n, s) in slots.iter().enumerate() {
        let addr = plt.addr + hdr + ent * n as u64;
        let cx = SynthCx { addr, header: plt.addr, got_plt: s.0, got_base: 0, index: n as u32 };
        let mut buf = vec![0u8; ent as usize];
        M2.write_plt(v, PltPart::Entry { chunk: 0, n: n as u32 }, &cx, &mut buf).unwrap();
        let o = (hdr + ent * n as u64) as usize;
        let theirs = &pbytes[o..o + ent as usize];
        // lld (19) leaves out the `bti c` of an entry whose address is not
        // taken (only direct branches reach it), ending the entry with a
        // second `nop`. mortar keeps the landing pad: its thunks reach a
        // PLT entry with `br x16` (§9, §10.3), and GNU ld writes it.
        let unpadded = |b: &[u8]| {
            let mut w = b[4..].to_vec();
            w.extend_from_slice(&synth::NOP.to_le_bytes());
            w
        };
        if buf[..] == theirs[..] {
            tally.entries += 1;
        } else if label.starts_with("lld") && bti && unpadded(&buf) == theirs {
            tally.lld_unpadded += 1;
        } else {
            tally.mismatches.push(format!(
                "{label}: entry {n} ({}) {:08x?} != {:08x?}",
                s.2,
                words_of(&buf),
                words_of(&pbytes[o..o + ent as usize])
            ));
        }
        // D-46: the place holds the lazy target.
        let lazy = match M2.lazy_target(v) {
            Some(LazyTarget::Plt0) => plt.addr,
            other => panic!("{other:?}"),
        };
        if word_at(img, s.0) == lazy {
            tally.lazy_words += 1;
        } else {
            tally.mismatches.push(format!(
                "{label}: lazy word of {} is {:#x}, not {lazy:#x}",
                s.2,
                word_at(img, s.0)
            ));
        }
    }
    if let Some(gp) = section(img, ".got.plt") {
        assert_eq!(gp.addr, g, "{label}: .got.plt header");
        assert_eq!(&img.data(gp)[..24], &[0; 24], "{label}: .got.plt header words");
    }
    // D-30 rows 24-25.
    let tags = dyn_tags(img);
    let have = |t: u64| tags.contains(&t);
    // The variant-PCS mark is on the executable's reference only: GNU ld
    // sets the tag from it, lld only from the definition's `st_other`
    // (both set it when the shared object's `f3` is marked).
    let vpcs_tag = vpcs && label.starts_with("bfd");
    for (t, want) in [(0x7000_0001, bti), (0x7000_0005, vpcs_tag), (0x7000_0003, false)] {
        if have(t) == want {
            tally.tag_checks += 1;
        } else {
            tally.mismatches.push(format!("{label}: tag {t:#x} present={}", have(t)));
        }
    }
}

/// Applies mortar to one sequence of `obj`'s `.text` and compares it with
/// the image's bytes.
fn check_seq(
    label: &str,
    what: &'static str,
    obj: &Image,
    img: &Image,
    out: OutputKind,
    label_sym: &str,
    relocs: &[(u64, u32, u32)],
    t: &Tables,
    tally: &mut Tally,
) {
    let o = sym(obj, label_sym).value as usize;
    // R4's pattern includes the `nop` after the call.
    let tail = if relocs.iter().any(|r| r.2 == 283) { 8 } else { 4 };
    let len = relocs.iter().map(|r| r.0 as usize + tail).max().unwrap();
    let text = section(obj, ".text").unwrap();
    let input = img_slice(obj, text, o, len);
    let at = sym(img, label_sym).value;
    let mut c = Case::new(t, &input, at);
    c.arch = M2;
    c.output = out;
    c.tls = img.tls;
    let rs: Vec<Reloc> = relocs
        .iter()
        .map(|&(off, k, ty)| Reloc { r_offset: off, key: key(k), r_type: ty, addend: 0 })
        .collect();
    let mine = c.apply(&rs).unwrap_or_else(|e| panic!("{label} {what}: {e:?}"));
    let theirs = bytes_at(img, at, len);
    if mine[..] == theirs[..] {
        *tally.tls.entry(what).or_default() += 1;
    } else if what.starts_with("large") && x0_of(&mine).is_some() && x0_of(&mine) == x0_of(theirs) {
        // R1l: GNU ld writes `T` as three words (`movz` `lsl #48`, two
        // `movk`) where a64.md §8.1 writes two; x0 ends equal.
        *tally.tls.entry("large TLSDESC->LE (same x0, GNU ld's 3 words)").or_default() += 1;
    } else {
        tally
            .mismatches
            .push(format!("{label} {what}: mortar {mine:02x?} reference {theirs:02x?}"));
    }
}

/// The value a run of `movz`/`movk x0` and `nop`s leaves in x0, if the
/// words are only those.
fn x0_of(b: &[u8]) -> Option<u64> {
    let mut x = 0u64;
    for w in words_of(b) {
        let (imm, sh) = (u64::from((w >> 5) & 0xFFFF), 16 * ((w >> 21) & 3));
        match w & 0xFF80_001F {
            0xD280_0000 => x = imm << sh,
            0xF280_0000 => x = (x & !(0xFFFF << sh)) | imm << sh,
            _ if w == synth::NOP => {}
            _ => return None,
        }
    }
    Some(x)
}

fn words_of(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

fn img_slice(obj: &Image, text: &Shdr, o: usize, len: usize) -> Vec<u8> {
    obj.data(text)[o..o + len].to_vec()
}

/// The TLS tables of one image: `tv` (key 1) in the shared object, `lv`
/// and `lv2` (keys 2, 4) the executable's own, `__tls_get_addr` (key 3).
fn tls_tables(img: &Image) -> Tables {
    let mut t = Tables::default();
    let block = img.tls.expect("PT_TLS");
    t.tls_sym(key(1), 0, TargetKind::Shlib { func: false });
    for (k, n) in [(2, "lv"), (4, "lv2")] {
        if let Some(s) = img.symbols().into_iter().find(|s| s.name == n) {
            t.tls_sym(key(k), block.start + s.value, TargetKind::Local);
        }
    }
    t.sym(key(3), 0, TargetKind::Shlib { func: true });
    t.names.insert(key(3), b"__tls_get_addr".to_vec());
    for (place, ty, name) in dyn_relocs(img, ".rela.dyn") {
        // `TLS_TPREL64` against `tv`: its GTPREL slot.
        if ty == 1030 && name == "tv" {
            t.slots.insert((key(1), SlotKind::TpOff { negated: false }), place);
        }
    }
    t
}

/// The image's bytes are those of a link with `--build-id=none`; the
/// links go to fresh paths (no linker writes over an existing output).
fn link(
    t: &Tools,
    dir: &Scratch,
    linker: &str,
    pie: bool,
    now: bool,
    objs: &[&Path],
    lib: &Path,
    name: &str,
) -> Option<Image> {
    let out = dir.join(name);
    assert!(!out.exists());
    let mut a = ld_args(t, linker);
    a.push(if pie { "-pie".into() } else { "-no-pie".into() });
    if now {
        a.push("-Wl,-z,now".into());
    }
    a.extend(objs.iter().map(|p| p.display().to_string()));
    a.extend([lib.display().to_string(), "-o".into(), out.display().to_string()]);
    let o = run(&t.gcc, &a);
    if !o.success() {
        eprintln!(
            "note: {linker} refuses {name}: {}",
            o.stderr_text().lines().next().unwrap_or("")
        );
        return None;
    }
    Some(Image::read(&out))
}

#[test]
fn plt_and_tls_against_gnu_ld_and_lld() {
    let Some(t) = tools() else { return };
    let dir = Scratch::new("a64-dynref");
    let mut tally = Tally::default();
    for bti in [false, true] {
        let note = if bti { BTI_NOTE } else { "" };
        let tag = if bti { "bti" } else { "plain" };
        let lib_o = assemble(&t, &dir, &format!("lib-{tag}"), &format!("{LIB}{note}"));
        let lib = dir.join(format!("lib{tag}.so"));
        let mut a = ld_args(&t, "bfd");
        a.extend([
            "-shared".into(),
            lib_o.display().to_string(),
            "-o".into(),
            lib.display().to_string(),
        ]);
        must("ld -shared", &run(&t.gcc, &a));
        let main = assemble(&t, &dir, &format!("main-{tag}"), &format!("{MAIN}{note}"));
        let gd = assemble(&t, &dir, &format!("gd-{tag}"), &format!("{GD}{note}"));
        let vp = assemble(&t, &dir, &format!("vp-{tag}"), &format!("{VPCS}{note}"));
        let main_img = Image::read(&main);
        let gd_img = Image::read(&gd);
        for linker in ["bfd", "lld"] {
            for pie in [false, true] {
                for now in [false, true] {
                    for vpcs in [false, true] {
                        let label = format!(
                            "{linker} {tag} {} {}{}",
                            if pie { "pie" } else { "no-pie" },
                            if now { "now" } else { "lazy" },
                            if vpcs { " vpcs" } else { "" }
                        );
                        let mut objs: Vec<&Path> = vec![&main];
                        if linker == "bfd" {
                            objs.push(&gd);
                        }
                        if vpcs {
                            objs.push(&vp);
                        }
                        let name = label.replace(' ', "-");
                        let Some(img) = link(&t, &dir, linker, pie, now, &objs, &lib, &name) else {
                            tally.mismatches.push(format!("{label}: not linked"));
                            continue;
                        };
                        check_plt(&label, &img, bti, vpcs, &mut tally);
                        if vpcs {
                            continue;
                        }
                        let out = if pie { OutputKind::Pie } else { OutputKind::Exec };
                        let tt = tls_tables(&img);
                        let desc = |k| [(0, k, 562), (4, k, 563), (8, k, 564), (12, k, 569)];
                        let ie = |k| [(0, k, 541), (4, k, 542)];
                        let s = |w, n, r: &[(u64, u32, u32)], tally: &mut Tally| {
                            check_seq(&label, w, &main_img, &img, out, n, r, &tt, tally);
                        };
                        s("TLSDESC->IE", "desc_tv", &desc(1), &mut tally);
                        s("TLSDESC->LE", "desc_lv", &desc(2), &mut tally);
                        s("IE kept", "ie_tv", &ie(1), &mut tally);
                        if linker == "lld" {
                            s("IE->LE", "ie_lv", &ie(2), &mut tally);
                        }
                        if linker == "bfd" {
                            let gd_seq = |k| [(0, k, 513), (4, k, 514), (8, 3, 283)];
                            check_seq(
                                &label,
                                "GD->IE",
                                &gd_img,
                                &img,
                                out,
                                "gd_tv",
                                &gd_seq(1),
                                &tt,
                                &mut tally,
                            );
                            check_seq(
                                &label,
                                "GD->LE",
                                &gd_img,
                                &img,
                                out,
                                "gd_lv",
                                &gd_seq(4),
                                &tt,
                                &mut tally,
                            );
                            let large =
                                [(0, 4, 565), (4, 4, 566), (8, 4, 567), (12, 4, 568), (16, 4, 569)];
                            check_seq(
                                &label,
                                "large TLSDESC->LE",
                                &gd_img,
                                &img,
                                out,
                                "large_lv",
                                &large,
                                &tt,
                                &mut tally,
                            );
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "PLT0 {} / entries {} (+{} lld BTI entries without the pad) / lazy words {} / tags {} / TLS {:?}",
        tally.headers,
        tally.entries,
        tally.lld_unpadded,
        tally.lazy_words,
        tally.tag_checks,
        tally.tls
    );
    for m in &tally.mismatches {
        eprintln!("MISMATCH {m}");
    }
    assert!(tally.mismatches.is_empty(), "{} mismatches", tally.mismatches.len());
    // 2 linkers × 2 BTI × 2 PIE × 2 binding × 2 VPCS links.
    assert_eq!(tally.headers, 32);
    assert_eq!(tally.entries + tally.lld_unpadded, 144);
    assert_eq!(tally.lazy_words, 144);
    assert_eq!(tally.tls.values().sum::<u32>(), 80);
}
