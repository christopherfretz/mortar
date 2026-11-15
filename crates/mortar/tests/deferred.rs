//! `spec/image.md` I-14 (WP-032c): the relocations of non-alloc debug
//! sections are checked by fill, not the scan. Their faults are the
//! scan's diagnostics, keyed at phase 100, under both stores; a link that
//! fails before fill still reports them; a reference to a dropped COMDAT
//! copy is the tombstone; an `Undefined` from a debug section only stays
//! the scan's (I-110).

mod support;

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use mortar::front::Front;
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy, Style};
use mortar_image::fill::Failed;
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::{Scratch, tool, tools_required};

const START: &str = "
    .text
    .globl _start
_start:
    mov x8, #93
    svc #0
    .data
    .globl val
val:
    .xword 1
";

/// Field `[4, 12)` of an 8-byte section: outside it (§4.3).
const OUTSIDE: &str = "
    .section .debug_info,\"\",@progbits
    .xword 0
    .reloc 4, R_AARCH64_ABS64, val
";

/// An instruction type at a non-alloc place.
const NOT_DATA: &str = "
    .section .debug_line,\"\",@progbits
    .word 0
    .reloc 0, R_AARCH64_CALL26, val
";

const UNDEFINED: &str = "
    .text
    .globl f
f:
    bl missing_fn
    ret
";

/// Two copies of COMDAT group `f`, each `.debug_info` naming its own
/// copy's section symbol (the second copy is dropped: the tombstone) and
/// a section of its own that `--gc-sections` collects.
fn comdat(n: u32) -> String {
    format!(
        "
    .section .text.f,\"axG\",@progbits,f,comdat
    .globl f
.Lf{n}:
f:
    ret
    .section .text.dead{n},\"ax\",@progbits
.Ldead{n}:
    ret
    .section .debug_info,\"\",@progbits
    .xword .Lf{n}
    .xword .Ldead{n}
"
    )
}

/// Assembles `(object, source)` in `dir`; `None` without the tools.
fn assemble(dir: &Path, units: &[(&str, &str)]) -> Option<()> {
    let Some(cc) = tool("gcc") else {
        assert!(!tools_required(), "gcc is required");
        return None;
    };
    if std::env::consts::ARCH != "aarch64" {
        return None;
    }
    for (out, src) in units {
        let s = format!("{out}.s");
        std::fs::write(dir.join(&s), src).unwrap();
        let o = mortar_testutil::run_with(
            std::process::Command::new(&cc).args(["-c", "-o", out, &s]).current_dir(dir),
            b"",
            Duration::from_mins(1),
        );
        assert!(o.success(), "gcc: {}", o.stderr_text());
    }
    Some(())
}

/// The front, or its diagnostics rendered.
fn front(
    args: &[&str],
    cwd: &Path,
    policy: StorePolicy,
) -> Result<(Front<'static>, &'static Store), String> {
    let store: &'static Store = Box::leak(Box::new(Store::new(policy)));
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    match support::front_of(&args, cwd, store, Pool::new(2)) {
        Ok(f) => Ok((f, store)),
        Err(r) => Err(r.render(&Style::plain("mortar"))),
    }
}

/// The image's diagnostics as the driver renders them, and their keys.
fn rendered(e: &Failed) -> (String, Vec<mortar_diag::Key>) {
    let c = Collector::new(Policy::default());
    for (k, d) in e.diagnostics.iter().cloned() {
        c.emit(k, d);
    }
    let mut keys: Vec<_> = e.diagnostics.iter().map(|d| d.0).collect();
    keys.sort();
    (c.finish().render(&Style::plain("mortar")), keys)
}

const ARGS: &[&str] = &["-maarch64linux", "-static", "-e", "_start", "-o", "out", "a.o"];

/// I-14: the faults of debug sections pass the scan and fail the image
/// with the scan's text at phase 100, the same in both stores, leaving
/// no output.
#[test]
fn i14_debug_faults_are_reported_by_fill_in_both_stores() {
    let dir = Scratch::new("deferred-faults");
    if assemble(dir.path(), &[("a.o", START), ("d.o", OUTSIDE), ("e.o", NOT_DATA)]).is_none() {
        return;
    }
    let mut args = ARGS.to_vec();
    args.extend(["d.o", "e.o"]);
    let out = dir.join("out");
    let mut seen = None;
    for policy in [StorePolicy::Resident, StorePolicy::Staged] {
        let (f, store) = front(&args, dir.path(), policy).unwrap_or_else(|t| panic!("front: {t}"));
        assert!(f.scanned.diagnostics.is_empty(), "{:?}", f.scanned.diagnostics);
        assert_eq!(f.scanned.order.deferred().len(), 2, "both debug sections are deferred");
        let e = mortar::image::write(&f, store, &out, &|_| {}, &mut |_, _| {})
            .expect_err("the faults fail the link");
        let (text, keys) = rendered(&e);
        assert_eq!(
            text,
            "mortar: error: d.o:(.debug_info+0x4): relocation R_AARCH64_ABS64 at offset 0x4 lies outside its section\n\
             mortar: error: e.o:(.debug_line+0x0): relocation R_AARCH64_CALL26 cannot be used against symbol 'val'; the instruction does not match the relocation\n"
        );
        assert!(keys.iter().all(|k| k.phase == 100), "{keys:?}");
        assert!(!out.exists(), "no output file after a failed link");
        match &seen {
            None => seen = Some(text),
            Some(t) => assert_eq!(*t, text, "the stores' diagnostics differ"),
        }
    }
}

/// I-14: a link that fails at the scan (an undefined symbol) still
/// reports the faults of its deferred sections, after the scan's.
#[test]
fn i14_faults_are_reported_when_the_scan_fails() {
    let dir = Scratch::new("deferred-scan-fails");
    if assemble(dir.path(), &[("a.o", START), ("d.o", OUTSIDE), ("u.o", UNDEFINED)]).is_none() {
        return;
    }
    let mut args = ARGS.to_vec();
    args.extend(["d.o", "u.o"]);
    let Err(text) = front(&args, dir.path(), StorePolicy::Staged).map(|_| ()) else {
        panic!("the link is accepted");
    };
    assert_eq!(
        text,
        "mortar: error: undefined symbol: missing_fn (undefined reference to `missing_fn')\n\
         >>> referenced by u.o:(.text+0x0)\n\
         mortar: error: d.o:(.debug_info+0x4): relocation R_AARCH64_ABS64 at offset 0x4 lies outside its section\n"
    );
}

/// I-110 under I-14: a debug section of a file that mentions an
/// undefined key is the scan's, which reports `undefined-symbol` at its
/// own key.
#[test]
fn i14_i110_undefined_from_debug_stays_the_scans() {
    let dir = Scratch::new("deferred-i110");
    let src = "    .section .debug_info,\"\",@progbits\n    .xword nowhere_defined\n";
    if assemble(dir.path(), &[("a.o", START), ("d.o", src)]).is_none() {
        return;
    }
    let mut args = ARGS.to_vec();
    args.push("d.o");
    let Err(text) = front(&args, dir.path(), StorePolicy::Staged).map(|_| ()) else {
        panic!("the link is accepted");
    };
    assert_eq!(
        text,
        "mortar: error: undefined symbol: nowhere_defined (undefined reference to `nowhere_defined')\n\
         >>> referenced by d.o:(.debug_info+0x0)\n"
    );
}

/// `.debug_info` of the (ELF64 little-endian) output, from its section
/// headers.
fn debug_info(out: &Path) -> Vec<u8> {
    let b = std::fs::read(out).unwrap();
    let u16_at = |o: usize| usize::from(u16::from_le_bytes(b[o..o + 2].try_into().unwrap()));
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let u64_at =
        |o: usize| usize::try_from(u64::from_le_bytes(b[o..o + 8].try_into().unwrap())).unwrap();
    let (shoff, shentsize, shnum, shstrndx) =
        (u64_at(0x28), u16_at(0x3a), u16_at(0x3c), u16_at(0x3e));
    let sh = |i: usize| shoff + i * shentsize;
    let strtab = u64_at(sh(shstrndx) + 0x18);
    for i in 1..shnum {
        let name = &b[strtab + u32_at(sh(i))..];
        if name.starts_with(b".debug_info\0") {
            let (off, size) = (u64_at(sh(i) + 0x18), u64_at(sh(i) + 0x20));
            return b[off..off + size].to_vec();
        }
    }
    panic!("no .debug_info");
}

/// I-14, I-45, I-47: a deferred section's reference to a dropped COMDAT
/// copy, or (`--gc-sections`) to a collected section, is the tombstone;
/// the stores and thread counts agree.
#[test]
fn i14_dead_targets_are_the_tombstone_at_fill() {
    let dir = Scratch::new("deferred-tombstone");
    let (c1, c2) = (comdat(1), comdat(2));
    if assemble(dir.path(), &[("a.o", START), ("c1.o", &c1), ("c2.o", &c2)]).is_none() {
        return;
    }
    let ones = u64::MAX.to_le_bytes();
    for gc in [false, true] {
        let mut args = ARGS.to_vec();
        args.extend(["c1.o", "c2.o"]);
        if gc {
            args.push("--gc-sections");
        }
        let mut first: Option<Vec<u8>> = None;
        for (policy, threads) in
            [(StorePolicy::Resident, 1), (StorePolicy::Staged, 1), (StorePolicy::Staged, 4)]
        {
            let store: &'static Store = Box::leak(Box::new(Store::new(policy)));
            let a: Vec<OsString> = args.iter().map(OsString::from).collect();
            let f = match support::front_of(&a, dir.path(), store, Pool::new(threads)) {
                Ok(f) => f,
                // `--gc-sections` before WP-031: refused by name.
                Err(r) if gc => {
                    let text = r.render(&Style::plain("mortar"));
                    assert!(text.contains("gc-sections"), "{text}");
                    break;
                }
                Err(r) => panic!("front: {}", r.render(&Style::plain("mortar"))),
            };
            assert_eq!(f.scanned.order.deferred().len(), 2);
            let out = dir.join("out");
            mortar::image::write(&f, store, &out, &|_| {}, &mut |_, _| {})
                .unwrap_or_else(|e| panic!("image: {:?}", e.diagnostics));
            let info = debug_info(&out);
            assert_eq!(info.len(), 32);
            let word = |i: usize| -> [u8; 8] { info[8 * i..8 * i + 8].try_into().unwrap() };
            assert_ne!(word(0), ones, "the kept copy's address");
            assert_eq!(word(2), ones, "the dropped copy's reference is the tombstone");
            if gc {
                assert_eq!(word(1), ones, "a collected section's reference is the tombstone");
                assert_eq!(word(3), ones);
            } else {
                assert_ne!(word(1), ones);
                assert_ne!(word(3), ones);
            }
            let bytes = std::fs::read(&out).unwrap();
            match &first {
                None => {
                    // The verifier on the output the others must equal
                    // (WP-035): the tombstones are its §5.8.
                    let map = mortar::image::map_of(&f, store).expect("the map");
                    let path = out.with_extension("map");
                    std::fs::write(&path, mortar_image::map::render(&map, &f.pool)).unwrap();
                    support::plumb::clean(&out, &path, dir.path());
                    first = Some(bytes);
                }
                Some(b) => assert!(*b == bytes, "{policy:?} at {threads} threads differs"),
            }
        }
    }
}

// ---- `Arch::nonalloc_demand_types` ------------------------------------------

struct NoRecords;

impl mortar_arch::Records for NoRecords {
    fn len(&self) -> u32 {
        1
    }

    fn get(&self, _: u32) -> Option<mortar_arch::Reloc> {
        None
    }
}

struct Facts(mortar_arch::SymFacts);

impl mortar_arch::SymNames for Facts {
    fn name(&self, _: mortar_core::SymKey) -> &[u8] {
        b"t"
    }
}

impl mortar_arch::ScanTables for Facts {
    fn sym(&self, _: mortar_core::SymKey) -> mortar_arch::SymFacts {
        self.0
    }
}

/// Whether `arch.scan` of one record of type `r_type` at a non-alloc
/// debug place raises any demand, for any target kind, TLS or not, and
/// output kind.
fn raises(arch: &dyn mortar_arch::Arch, r_type: u32) -> bool {
    use mortar_arch::{DemandOut, DemandSink, OutputKind, ScanCx, Site, TargetKind};
    struct Raised(bool);
    impl DemandOut for Raised {
        fn raise(&mut self, _: mortar_core::SymKey, f: mortar_arch::DemandFlags) {
            self.0 |= !f.is_empty();
        }
        fn raise_global(&mut self, f: mortar_arch::GlobalFlags) {
            self.0 |= !f.is_empty();
        }
        fn count(&mut self, _: mortar_arch::DynRole, n: u32) {
            self.0 |= n != 0;
        }
        fn error(&mut self, _: u32, _: mortar_arch::ScanError) {}
        fn reference(&mut self, _: u32, _: mortar_arch::EventClass, _: mortar_core::SymKey) {}
    }
    let bytes = [0u8; 64];
    let site = Site { bytes: &bytes, offset: 0 };
    let Ok(h) = arch.classify(r_type, site) else { return false };
    let key = mortar_core::SymKey::local(mortar_core::SymEntry {
        file: mortar_core::FileId(1),
        index: mortar_elf::SymbolIndex(1),
    });
    let kinds = [
        TargetKind::Local,
        TargetKind::Abs,
        TargetKind::Preemptible,
        TargetKind::Shlib { func: false },
        TargetKind::Ifunc,
        TargetKind::Weak0,
        TargetKind::WeakDyn,
    ];
    let outputs = [OutputKind::Static, OutputKind::Exec, OutputKind::Pie, OutputKind::Shared];
    let mut any = false;
    for kind in kinds {
        for tls in [false, true] {
            for output in outputs {
                let facts = Facts(mortar_arch::SymFacts {
                    kind,
                    sym_type: if tls { 6 } else { 1 },
                    st_other: 0,
                    tls,
                    dead: false,
                    comdat_only: false,
                    gnu_warning: false,
                });
                let at = mortar_arch::RelocAt {
                    file: mortar_core::FileId(1),
                    section: mortar_core::SectionId(1),
                    index: 0,
                    r_offset: 8,
                    key,
                    r_type,
                    addend: Some(0),
                    howto: &h,
                    site: Site { bytes: &bytes, offset: 8 },
                    records: &NoRecords,
                    proof: &[],
                    leads: true,
                };
                let place = mortar_arch::PlaceFacts { alloc: false, writable: false };
                let cx = ScanCx { tables: &facts, output, place, name: b".debug_info" };
                let mut out = Raised(false);
                arch.scan(&cx, &at, &mut DemandSink::new(&mut out));
                any |= out.0;
            }
        }
    }
    any
}

/// I-14: every type that raises a demand at a non-alloc place is in the
/// architecture's `nonalloc_demand_types`, whose records the scan still
/// visits (AArch64 M1 and M2, x86-64).
#[test]
fn i14_nonalloc_demand_types_are_complete() {
    let a64_m2 = mortar_a64::A64::new(mortar_arch::Status::M2, false);
    let arches: [(&str, &dyn mortar_arch::Arch); 3] =
        [("a64 M1", &mortar_a64::A64::M1), ("a64 M2", &a64_m2), ("x64", &mortar_x64::X64::M1)];
    for (name, arch) in arches {
        let listed = arch.nonalloc_demand_types();
        for r_type in 0..2048 {
            if raises(arch, r_type) {
                assert!(listed.contains(&r_type), "{name}: type {r_type} raises a demand");
            }
        }
    }
    assert!(mortar_arch::Arch::nonalloc_demand_types(&mortar_a64::A64::M1).is_empty());
}
