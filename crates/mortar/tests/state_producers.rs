//! WP-264: the stored columns WP-263's sweep found with no producer
//! (`spec/landed/incr/WP-263.md` F-1, F-2), each with its producer and a
//! test that reads it back as the update does.
//!
//! - `REGIONS` (51): the full link's regions (IN-30, IN-72 item 2),
//!   from its placement. An unchanged file given to `delta::layout` as a
//!   changed one keeps every section where the full link put it and
//!   opens no region; with the same state's `REGIONS` emptied (what an
//!   update read before this package), the same call refuses
//!   `reserve-exhausted` (AArch64: the tail of `.text` is the patch
//!   island) or moves the file's sections to overflow regions.
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;
#[path = "support/update.rs"]
mod update;

use mortar::update::file_front::FrontCache;
use mortar::update::layout_in::layout_inputs;
use mortar_core::id::{SymKey, SymKind, SymbolId};
use mortar_core::incr::{FileDemands, LayoutInput, PlaceNow, StoredLayout};
use mortar_elf::Machine;
use mortar_merge::phase::ArchRelocs;
use update::{lines, member, pool, same_file, state, whole};

/// What `delta::layout` did with an unchanged file given as changed: the
/// sections it placed, those at their stored place, the regions it opened.
#[derive(Debug, PartialEq, Eq)]
struct Kept {
    placed: usize,
    at_stored_place: usize,
    moved: Vec<String>,
    opened: usize,
}

/// The tag that makes a file's bytes differ and nothing it places: one
/// absolute local symbol (a new `.symtab` entry, the same sections).
const TAG: &str = "__asm__(\".set wp264_empty_change, 1\");\n";

#[test]
fn regions_keep_an_unchanged_file_in_place() {
    let (m1v, m2v) = (format!("{}{TAG}", update::M1), format!("{}{TAG}", update::M2));
    for line in lines(&[("m1v", "c", &m1v), ("m2v", "c", &m2v)]) {
        let machine =
            if line.label.starts_with("aarch64") { Machine::AArch64 } else { Machine::X86_64 };
        let args = line.args(&["wp255d_m2", "wp255d_m1"]);
        let target = mortar_layout::target::Target::of(machine);
        for (name, variant) in [("m1.o", "m1v.o"), ("m2.o", "m2v.o")] {
            let what = |s: &str| format!("{} {name}: {s}", line.label);
            let keep = line.dir.join("orig.keep");
            std::fs::copy(line.dir.join(name), &keep).expect("keep the file");
            line.archive(&["m1.o", "m2.o"]);
            let old = state(&line, &args, "old");
            // An EMPTY change: other bytes, the same sections.
            std::fs::copy(line.dir.join(variant), line.dir.join(name)).expect("the variant");
            line.archive(&["m1.o", "m2.o"]);
            let _new = state(&line, &args, "new");
            let capture = mortar_incr::state5::read::decode(
                &std::fs::read(line.dir.join("new.state5")).expect("the new state"),
            )
            .expect("the new state decodes");
            let bytes = std::fs::read(line.dir.join("old.state5")).expect("the old state");
            let mut no_regions =
                mortar_incr::state5::read::decode(&bytes).expect("the old state decodes");
            assert!(!no_regions.regions.is_empty(), "{}", what("REGIONS has rows"));
            no_regions.regions.clear();
            let bare = no_regions.view();
            let store = support::store();
            let f = whole(&line, &args, &store);
            let file = member(&old, name);
            let nf = same_file(&f, &old, file);
            let (cf, delta) =
                mortar::update::changed::u5(&old, &[file], &[], &f, &capture, &mut |_| {})
                    .unwrap_or_else(|e| panic!("{}", what(&format!("U4-U5: {}", e.report()))))
                    .expect("the file is given as changed");
            let rows = mortar_incr::state5::LayoutOf { view: &old };
            let stored = StoredLayout::new(&rows);
            let regions = stored.regions_of(file).expect("REGIONS");
            assert!(!regions.is_empty(), "{}", what("the file has regions"));
            let fresh = support::store();
            let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
            let relocs = ArchRelocs(f.arch.get());
            let inputs =
                layout_inputs(&mut cache, &old, &stored, &f.plan, &cf, &delta, &relocs, &f.pool)
                    .unwrap_or_else(|e| {
                        panic!("{}", what(&format!("layout_inputs: {}", e.report())))
                    });
            // The whole front's demands of the file (TEST-ONLY, as
            // `layout_in.rs`), keys moved into the state's ids by name.
            let mut demands: Vec<FileDemands> = Vec::new();
            if let Some(i5) = &f.incr5 {
                for d in i5.demands.iter().filter(|d| d.file == nf) {
                    let mut v = Vec::new();
                    for s in &d.demands {
                        let key = match s.key.kind() {
                            SymKind::Global(g) => {
                                let k = &f.inputs.symbols[g];
                                let id: SymbolId = old
                                    .find_key(k.name, k.version.unwrap_or_default())
                                    .expect("SYMBOLS")
                                    .expect("an unchanged link's key is stored");
                                SymKey::global(id)
                            }
                            SymKind::Local(_) => s.key,
                        };
                        v.push(mortar_core::incr::SlotDemand { key, flags: s.flags });
                    }
                    v.sort();
                    demands.push(FileDemands { file, demands: v });
                }
            }
            let input = LayoutInput {
                changes: &inputs.changes,
                eh: &inputs.eh,
                pieces: &inputs.pieces,
                demands: &demands,
                debug: false,
            };
            let judge = |rows: &dyn mortar_core::incr::LayoutRows| -> Result<Kept, String> {
                let stored = StoredLayout::new(rows);
                let d = mortar_layout::delta::layout(&stored, &input, &target)
                    .map_err(|e| e.report())?;
                let mut k = Kept { placed: 0, at_stored_place: 0, moved: Vec::new(), opened: 0 };
                k.opened = d.regions_opened.len();
                for c in &inputs.changes {
                    let now = match d.place_of(&stored, c.id).expect("place_of") {
                        PlaceNow::Here(p) => p.get(),
                        PlaceNow::NotPlaced => None,
                        PlaceNow::Stored(_) => panic!("{}", what("a changed file's section")),
                    };
                    let was = c
                        .old
                        .and_then(|o| old.section(o).ok())
                        .and_then(|r| (r.place_chunk != u32::MAX).then_some(r))
                        .map(|r| (r.place_chunk, r.place_offset));
                    let Some((ch, off)) = now else {
                        assert!(was.is_none(), "{}", what("a placed section unplaced"));
                        continue;
                    };
                    k.placed += 1;
                    if was == Some((ch.0, off)) {
                        k.at_stored_place += 1;
                    } else {
                        k.moved.push(format!(
                            "{}: {was:?} -> ({}, {off:#x})",
                            String::from_utf8_lossy(c.key.name),
                            ch.0
                        ));
                    }
                }
                Ok(k)
            };
            let with =
                judge(&rows).unwrap_or_else(|e| panic!("{}", what(&format!("delta::layout: {e}"))));
            eprintln!("{}", what(&format!("{} regions; with REGIONS {with:?}", regions.len())));
            assert!(with.placed > 0, "{}", what("sections placed"));
            assert_eq!(with.at_stored_place, with.placed, "{}", what("every section kept"));
            assert_eq!(with.opened, 0, "{}", what("no region opened"));
            let bare_rows = mortar_incr::state5::LayoutOf { view: &bare };
            let without = judge(&bare_rows);
            eprintln!("{}", what(&format!("without REGIONS {without:?}")));
            // Refused (AArch64: `.text`'s tail is the patch island), or
            // placed in new overflow regions.
            assert!(
                without.as_ref().is_err_and(|e| e.contains("reserve-exhausted"))
                    || without.as_ref().is_ok_and(|k| k.opened > 0 && !k.moved.is_empty()),
                "{}",
                what("without REGIONS the update refuses or moves the file")
            );
            std::fs::copy(&keep, line.dir.join(name)).expect("restore the file");
        }
    }
}
