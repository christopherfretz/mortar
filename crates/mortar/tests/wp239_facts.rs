//! WP-239 R-4a probe (design experiment, not a gate): the scan's facts
//! per key (`mortar_scan::Walker::facts`, i.e. `View::facts(key, None)`),
//! per (file, key) mention (`Walker::facts_at`, the value a record of that
//! file reads), and each live file's section states, for one link, so that
//! `tests/incr-design/wp239/facts.py` can compare hops.
//!
//! Ignored. Run by hand, one link per run:
//!
//! ```text
//! WP239_CWD=dir WP239_ARGS=args.txt WP239_FACTS_OUT=hN.facts \
//!   cargo test -p mortar --test wp239_facts -- --ignored
//! ```
//!
//! `args.txt`: the linker's words, one per line (the map's `arg`
//! records). Writes, tab-separated: `F fid label live sections-hash`,
//! `K name version facts`, `M fid name version facts`.

use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::path::Path;

use mortar_core::inputs::Reader;
use mortar_core::par::Pool;
use mortar_core::{FileId, SymEntry, SymKey, SymbolId};
use mortar_elf::SymbolIndex;
use mortar_input::store::{Store, StorePolicy};
use mortar_scan::{ScanInputs, ScanOptions, Walker};

fn label(f: &mortar_core::inputs::File<'_>) -> String {
    let p = String::from_utf8_lossy(f.path).into_owned();
    match &f.member {
        Some(m) => format!("{p}({}#{})", String::from_utf8_lossy(m.name), m.ordinal),
        None => p,
    }
}

#[test]
#[ignore = "WP-239 R-4a design probe: needs WP239_CWD, WP239_ARGS, WP239_FACTS_OUT"]
fn facts_per_key_and_mention() {
    let cwd = std::env::var("WP239_CWD").expect("WP239_CWD");
    let args_path = std::env::var("WP239_ARGS").expect("WP239_ARGS");
    let out_path = std::env::var("WP239_FACTS_OUT").expect("WP239_FACTS_OUT");
    let words: Vec<std::ffi::OsString> = std::fs::read_to_string(&args_path)
        .expect("args")
        .lines()
        .filter(|l| !l.is_empty())
        .map(Into::into)
        .collect();
    let store = Store::new(StorePolicy::from_env());
    let parsed = mortar_cli::parse(&words);
    let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
    let f = mortar::front::front(
        parsed.plan,
        mortar::front::Env {
            cwd: Path::new(&cwd),
            pool: Pool::new(4),
            replay: None,
            slack: true,
            state5: false,
        },
        &store,
        &diag,
        &mut mortar::timing::Timer::new(false),
    );
    let report = diag.finish().render(&mortar_diag::Style::plain("mortar"));
    let f = f.unwrap_or_else(|| panic!("front: {report}"));
    assert!(f.dyn_tables.is_none(), "static links only");
    let scan_inputs = ScanInputs {
        inputs: &f.inputs,
        resolution: &f.resolution,
        liveness: &f.liveness,
        assignment: Some(&f.assignment),
        comdat_mismatch: &f.comdat_mismatch,
        gnu_warnings: &f.gnu_warnings,
        closure_only: &f.closure_only,
        eh: None,
    };
    let opts = ScanOptions {
        output: mortar::front::output_kind(f.plan.kind),
        dynamic: false,
        sections: None,
        proof: f.arch.proof(),
    };
    let walker = Walker::new(f.arch.get(), &scan_inputs, opts);
    let name = |id: SymbolId| {
        let k = &f.inputs.symbols[id];
        let v = k.version.map(|v| String::from_utf8_lossy(v).into_owned()).unwrap_or_default();
        (String::from_utf8_lossy(k.name).into_owned(), v)
    };
    let mut out = String::new();
    for i in 0..f.inputs.files.len() {
        let fid = FileId(i);
        let file = &f.inputs.files[fid];
        let live = f.resolution.file_live[fid].is_live();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let range = f.resolution.section_base[fid];
        for s in range.iter() {
            format!("{:?}", f.resolution.sections[s].state).hash(&mut h);
        }
        writeln!(out, "F\t{i}\t{}\t{}\t{:016x}", label(file), u8::from(live), h.finish()).unwrap();
    }
    for i in 0..f.inputs.symbols.len() {
        let id = SymbolId(i);
        let (n, v) = name(id);
        writeln!(out, "K\t{n}\t{v}\t{:?}", walker.facts(SymKey::global(id))).unwrap();
    }
    for i in 0..f.inputs.files.len() {
        let fid = FileId(i);
        if !f.resolution.file_live[fid].is_live() {
            continue;
        }
        if !matches!(f.inputs.files[fid].reader, Reader::Object(_)) {
            continue;
        }
        let Some(table) = walker.symbols(fid) else { continue };
        let first = table.first_global().0;
        for (k, &id) in f.inputs.sym_map_of(fid).iter().enumerate() {
            let mention = SymEntry { file: fid, index: SymbolIndex(first + k as u32) };
            let (n, v) = name(id);
            let facts = walker.facts_at(SymKey::global(id), Some(mention));
            writeln!(out, "M\t{i}\t{n}\t{v}\t{facts:?}").unwrap();
        }
    }
    std::fs::write(&out_path, out).expect("write the facts");
}
