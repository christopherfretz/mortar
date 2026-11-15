//! The plan's records over inputs the staged store loaded (`spec/input.md`
//! §11.1, §11.3; `spec/image.md` I-81, I-27): relocation sections the
//! store does not hold (a non-alloc section's) are read with `copy_to`,
//! and `layout_plan::records` gives what it gives over the resident
//! store, record for record. Fill's own reads go through `Source`, whose
//! non-resident path `tests/fill.rs` covers with `MemSource`.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::time::Duration;

use mortar_core::inputs::{Inputs, Reader};
use mortar_core::par::Pool;
use mortar_diag::{Collector, Policy};
use mortar_elf::consts::SHT_RELA;
use mortar_image::layout_plan::records;
use mortar_input::load::{LoadEnv, load};
use mortar_input::store::{Store, StorePolicy};
use mortar_testutil::{Scratch, tool, tools_required};

fn loaded(dir: &Scratch, policy: StorePolicy) -> Inputs<'static> {
    let args: Vec<OsString> = ["-maarch64linux", "big.o"].iter().map(OsString::from).collect();
    let plan = mortar_cli::parse(&args).plan;
    let store: &'static Store = Box::leak(Box::new(Store::new(policy)));
    let diag = Collector::new(Policy::default());
    let env = LoadEnv { cwd: dir.path().to_path_buf(), pool: Pool::new(4) };
    load(&plan, &env, store, &diag).expect("the inputs load")
}

#[test]
fn i81_plan_records_staged_equal_resident() {
    let Some(cc) = tool("gcc") else {
        assert!(!tools_required(), "gcc is required");
        return;
    };
    if std::env::consts::ARCH != "aarch64" {
        return;
    }
    let dir = Scratch::new("image-staged");
    let mut src = String::new();
    for i in 0..4000 {
        writeln!(src, "const char *s{i}(int x) {{ return x ? \"string {i}\" : 0; }}").unwrap();
    }
    std::fs::write(dir.join("big.c"), src).unwrap();
    let out = mortar_testutil::run_with(
        std::process::Command::new(&cc)
            .args(["-c", "-g", "-O1", "-o", "big.o", "big.c"])
            .current_dir(dir.path()),
        b"",
        Duration::from_mins(2),
    );
    assert!(out.success(), "gcc: {}", out.stderr_text());
    let resident = loaded(&dir, StorePolicy::Resident);
    let staged = loaded(&dir, StorePolicy::Staged);
    let mut copied = 0;
    let mut total = 0;
    for (file, f) in staged.files.iter() {
        let Reader::Object(o) = f.reader else { continue };
        for s in o.sections().filter(|s| s.sh_type() == SHT_RELA) {
            let h = s.header();
            if staged.resident(file, h.sh_offset..h.sh_offset + h.sh_size).is_none() {
                copied += 1;
            }
            total += 1;
            let ours = records(&staged, file, s.index()).expect("staged records");
            let theirs = records(&resident, file, s.index()).expect("resident records");
            assert_eq!(format!("{ours:?}"), format!("{theirs:?}"), "section {}", s.index().0);
        }
    }
    assert!(copied >= 1 && total > copied, "{copied} of {total} relocation sections copied");
}
