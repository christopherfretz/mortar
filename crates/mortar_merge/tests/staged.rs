//! Phase 60 over inputs the staged store loaded (`spec/input.md` §11.1,
//! §11.3; `spec/synth.md` §1): every byte the phase reads is resident
//! under the store's rules, so `phase::read` borrows and never copies, and
//! the phase's output is the same as over the files' own bytes. A failed
//! read is `input-changed`, `cannot-open-file` or `internal-error`, never
//! `malformed-input`.
mod support;

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_core::{FileId, Inputs, SectionId};
use mortar_diag::{Collector, FileRef, Policy};
use mortar_elf::{Class, Object, Section, SectionIndex, SymbolIndex};
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::{Store, StorePolicy};
use mortar_merge::eh::Target;
use mortar_merge::phase::{self, Link, LinkSection};
use mortar_merge::{Problem, What};
use mortar_testutil::tools_required;
use support::{Objects, lib_dirs, relocs};

/// [`Objects`], with the bytes of each section read the way the pipeline
/// reads them: `phase::read` over the loaded `Inputs`.
struct Over<'a> {
    objs: &'a Objects,
    ids: &'a [FileId],
    inputs: &'a Inputs<'static>,
    copied: AtomicUsize,
}

impl Link<'static> for Over<'_> {
    fn files(&self) -> u32 {
        self.objs.files()
    }
    fn object(&self, file: FileId) -> Option<Object<'static>> {
        Link::object(self.objs, file)
    }
    fn section_id(&self, file: FileId, index: SectionIndex) -> Option<SectionId> {
        self.objs.section_id(file, index)
    }
    fn section(&self, file: FileId, index: SectionIndex) -> Option<LinkSection> {
        self.objs.section(file, index)
    }
    fn global(&self, file: FileId, index: SymbolIndex, first: u32) -> Result<Target, What> {
        self.objs.global(file, index, first)
    }
    fn bytes(&self, file: FileId, s: &Section<'static>) -> Result<Cow<'static, [u8]>, What> {
        let b = phase::read(self.inputs, self.ids[file.0 as usize], s)?;
        if matches!(b, Cow::Owned(_)) {
            self.copied.fetch_add(1, Ordering::Relaxed);
        }
        Ok(b)
    }
    fn class(&self) -> Class {
        self.objs.class()
    }
    fn form(&self) -> mortar_core::inputs::RelocForm {
        self.objs.form()
    }
}

#[test]
fn s11_1_phase_60_reads_only_resident_bytes_under_the_staged_store() {
    // The native target's libraries (the link's own target is the host's).
    let Some((machine, gcc, libc)) =
        lib_dirs().into_iter().find(|(m, ..)| *m == mortar_elf::Machine::AArch64)
    else {
        assert!(!tools_required(), "the AArch64 libraries are required");
        return;
    };
    let Some(cc) = mortar_testutil::tool("gcc") else {
        assert!(!tools_required(), "gcc is required");
        return;
    };
    // One object whose `.text`, string section, `.eh_frame` and
    // `.rela.eh_frame` are each above 64 KiB, so the store splits it into
    // parts and leaves `.text` and the DWARF bodies out.
    let dir = mortar_testutil::Scratch::new("merge-staged");
    let mut src = String::new();
    for i in 0..4000 {
        writeln!(
            src,
            "const char *s{i}(int x) {{ return x ? \"string {i}, long enough to fill\" : 0; }}"
        )
        .unwrap();
    }
    std::fs::write(dir.join("big.c"), src).unwrap();
    let out = mortar_testutil::run_with(
        std::process::Command::new(&cc)
            .args(["-c", "-g", "-O1", "-o", "big.o", "big.c"])
            .current_dir(dir.path()),
        b"",
        std::time::Duration::from_mins(2),
    );
    assert!(out.success(), "gcc: {}", out.stderr_text());
    let mut args: Vec<PathBuf> =
        vec!["-maarch64linux".into(), dir.join("big.o"), gcc.join("crtbegin.o")];
    args.push("--whole-archive".into());
    for a in [gcc.join("libgcc_eh.a"), gcc.join("libsupc++.a"), libc.join("libc_nonshared.a")] {
        if a.exists() {
            args.push(a);
        }
    }
    let plan = mortar_cli::parse(&args.iter().map(Into::into).collect::<Vec<_>>()).plan;
    let pool = Pool::new(4);
    let store: &'static Store = Box::leak(Box::new(Store::new(StorePolicy::Staged)));
    let diag = Collector::new(Policy::default());
    let env = LoadEnv { cwd: std::env::current_dir().unwrap(), pool };
    let inputs = load(&plan, &env, store, &diag).expect("the inputs load");
    let inputs: &'static Inputs<'static> = Box::leak(Box::new(inputs));

    // The objects, by their bytes as `copy_to` gives them.
    let mut files = Vec::new();
    let mut ids = Vec::new();
    for (id, f) in inputs.files.iter() {
        let (Reader::Object(_), Some(b)) = (f.reader, f.bytes) else { continue };
        let mut buf = vec![0u8; b.len as usize];
        inputs.copy_to(id, 0..b.len, &mut buf).unwrap();
        files.push((format!("{:?}", f.display()), buf));
        ids.push(id);
    }
    assert!(files.len() > 50, "{} objects", files.len());
    let objs = Objects::new(files);
    assert_eq!(objs.machine, machine);
    let r = relocs(machine);
    let own = phase::run(&pool, &objs, r.as_ref()).expect("phase 60 over the own bytes");
    let over = Over { objs: &objs, ids: &ids, inputs, copied: AtomicUsize::new(0) };
    let staged = phase::run(&pool, &over, r.as_ref()).expect("phase 60 over the staged store");
    assert!(!own.split.merge_of.is_empty() && !own.eh.sections.is_empty());
    let stats = store.stats();
    assert!(stats.resident < stats.bytes, "nothing was left out: {stats:?}");
    assert_eq!(format!("{own:?}"), format!("{staged:?}"), "the phase depends on the store");
    assert_eq!(over.copied.load(Ordering::Relaxed), 0, "phase 60 copied a section");
    eprintln!(
        "{} objects, {} merge sections, {} placed whole, {} .eh_frame; store {:?}",
        objs.len(),
        own.split.merge_of.len(),
        own.whole.len(),
        own.eh.sections.len(),
        store.stats()
    );
}

#[test]
fn s11_5_read_failures_are_not_malformed() {
    let p = |what: What| Problem { file: FileId(1), section: 3, piece: 0, offset: 0, what };
    let id = |what: What| p(what).diagnostic(FileRef::new("a.o"), ".rodata").id();
    assert_eq!(id(What::InputChanged), "input-changed");
    assert_eq!(id(What::ReadFailed(5)), "cannot-open-file");
    assert_eq!(id(What::ReadFailed(0)), "cannot-open-file");
    assert_eq!(id(What::NotResident("x".into())), "internal-error");
    assert_eq!(id(What::Malformed("x".into())), "malformed-input");
}
