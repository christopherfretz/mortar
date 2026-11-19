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
//! - F-1: file 0's `SYMTAB` range holds the global area, then the
//!   internal file's block; an update's map states the stored blocks,
//!   the internal one included, and no other.
//!
//! - `META.boot_id` (IN-77c): the full link writes the boot it ran on;
//!   the update's boot check on the same boot trusts the output without
//!   re-hashing it (a byte changed behind its back is not seen), on
//!   another boot it re-hashes and refuses `output-changed`.
//!
//! - `DIAGS` (IN-75a): what an update loses today. The full link
//!   prints a reference-site warning; its state holds no `DIAGS` row
//!   (no producer: the capture cannot see the collector's warnings before
//!   `finish`). The test states what the updates after it print.
//!
//! (State version 10: `symtab_rows.rs`, `version_9_state_is_refused`.)
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;
#[path = "support/update.rs"]
#[allow(dead_code, reason = "a shared helper file; this test uses part of it")]
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

/// F-1 (WP-263): file 0's `SYMTAB` range holds the global area, then the
/// internal file's block; `symtab_block(FileId(0))` is the block. The
/// map of an update that places nothing (`update_map` with an empty delta
/// and plan, over the stored rows) states every stored block and no other,
/// the internal one as `symblock -` (a full link's map states none:
/// `decisions_of`, IN-49a), and the blocks tile `[1, tail_first)`.
#[test]
fn the_internal_block_is_stored_and_mapped() {
    use mortar_core::incr::{LayoutDelta, LayoutRows, SymtabPlan};
    use mortar_image::map::{render, update_map};
    for line in lines(&[]) {
        let args = line.args(&["wp255d_m2", "wp255d_m1"]);
        line.archive(&["m1.o", "m2.o"]);
        let old = state(&line, &args, "old");
        let rows = mortar_incr::state5::LayoutOf { view: &old };
        let area = old.symtab_of(mortar_core::FileId(0)).expect("SYMTAB").expect("the area");
        let internal = rows.symtab_block(mortar_core::FileId(0)).expect("SYMTAB");
        let internal = internal.unwrap_or_else(|| panic!("{}: no internal block", line.label));
        let (tables_area, tables) = rows.symtab_tables().expect("the tables");
        assert_eq!(tables_area, area, "{}: symtab_tables' area is file 0's first row", line.label);
        assert_eq!(area.first, tables.sh_info, "{}: the area starts at sh_info", line.label);
        assert_eq!(internal.first, 1, "{}: the internal block is the first", line.label);
        assert!(internal.count > 0, "{}: it has entries", line.label);
        // Every stored block, file 0's area excepted, as the map states it.
        let memory = mortar_incr::state5::read::decode(
            &std::fs::read(line.dir.join("old.state5")).expect("the state"),
        )
        .expect("the state decodes");
        assert_eq!(memory.symtab.first(), Some(&(mortar_core::FileId(0), area)), "{}", line.label);
        assert_eq!(
            memory.symtab.get(1),
            Some(&(mortar_core::FileId(0), internal)),
            "{}",
            line.label
        );
        let mut want: Vec<String> = memory.symtab[1..]
            .iter()
            .map(|(_, b)| {
                format!(
                    "{} {} {} {:x} {:x}",
                    b.first, b.count, b.capacity, b.strtab_offset, b.strtab_capacity
                )
            })
            .collect();
        want.sort();
        let store = support::store();
        let f = whole(&line, &args, &store);
        let stored = StoredLayout::new(&rows);
        let pool = pool();
        let map = mortar::image::with_link(&f, &store, &|_| {}, |l| {
            let m = update_map(l, &LayoutDelta::default(), &stored, &SymtabPlan::default())
                .expect("update_map");
            render(&m, &pool)
        })
        .expect("the link");
        let update: Vec<String> = String::from_utf8(map)
            .expect("a map is text")
            .lines()
            .filter(|l| l.starts_with("symblock "))
            .map(str::to_owned)
            .collect();
        let dash: Vec<&String> = update.iter().filter(|l| l.starts_with("symblock - ")).collect();
        eprintln!("{}: {} blocks; internal {internal:?}: {dash:?}", line.label, update.len());
        assert_eq!(dash.len(), 1, "{}: one internal block: {dash:?}", line.label);
        assert!(
            dash[0].starts_with(&format!("symblock - 1 {} {} ", internal.count, internal.capacity)),
            "{}: the stored internal block: {dash:?}",
            line.label
        );
        let mut got: Vec<String> =
            update.iter().map(|l| l.splitn(3, ' ').nth(2).unwrap_or("").to_owned()).collect();
        got.sort();
        assert_eq!(got, want, "{}: the update's blocks are the stored ones", line.label);
        // Tiled in `.symtab` order: each block ends where the next begins,
        // the last at `sh_info`.
        // `symblock <file> <first> <count> <capacity> <strtab> <held>`.
        let mut tiles: Vec<(u64, u64)> = update
            .iter()
            .map(|l| {
                let v: Vec<u64> =
                    l.split(' ').skip(2).take(3).map(|x| x.parse().expect("dec")).collect();
                (v[0], v[2])
            })
            .collect();
        tiles.sort_unstable();
        let mut at = 1u64;
        for (first, capacity) in tiles {
            assert_eq!(first, at, "{}: the blocks tile the locals", line.label);
            at = first + capacity;
        }
        // Then the tail reserve `[tail_first, sh_info)` (fillers).
        assert_eq!(at, u64::from(tables.tail_first), "{}: the last ends at the tail", line.label);
    }
}

/// `META.boot_id` (IN-77c, WP-264): the full link's state holds this
/// boot's id. With one output byte changed behind the state's back, the
/// boot check of an update on the same boot does not re-hash (`Ok`: the
/// change goes unseen, as IN-77c allows for a boot that did not crash),
/// while on any other boot, or an unreadable one, it re-hashes and
/// refuses `output-changed`.
#[test]
fn the_same_boot_does_not_rehash() {
    use mortar::update::write::{boot_check, boot_id};
    use mortar_core::incr::IncrReason;
    for line in lines(&[]) {
        let args = line.args(&["wp255d_m2", "wp255d_m1"]);
        line.archive(&["m1.o", "m2.o"]);
        let old = state(&line, &args, "old");
        let meta = old.meta().expect("META");
        let boot = boot_id();
        assert_eq!(meta.boot_id, boot.unwrap_or_default(), "{}: META.boot_id", line.label);
        assert!(!meta.output_synced, "{}: a full link's output is not synced", line.label);
        let path = line.dir.join("old");
        let good = std::fs::read(&path).expect("the output");
        let out = std::fs::File::open(&path).expect("open the output");
        assert_eq!(boot_check(&old, &out, boot).map_err(|e| e.report()), Ok(()), "{}", line.label);
        assert_eq!(
            boot_check(&old, &out, Some([0xa5; 16])).map_err(|e| e.report()),
            Ok(()),
            "{}: an untouched output passes the re-hash",
            line.label
        );
        // One byte changed (the ELF magic: never the sealed build id).
        let mut bad = good.clone();
        bad[1] ^= 0x20;
        std::fs::write(&path, &bad).expect("change the output");
        let out = std::fs::File::open(&path).expect("open the output");
        let same = boot_check(&old, &out, boot);
        let other = boot_check(&old, &out, Some([0xa5; 16]));
        let unread = boot_check(&old, &out, None);
        eprintln!("{}: boot {boot:?}: same {same:?}; other {other:?}", line.label);
        if boot.is_some() {
            assert!(same.is_ok(), "{}: the same boot does not re-hash: {same:?}", line.label);
        } else {
            assert!(matches!(same, Err(IncrReason::OutputChanged(_))), "{}", line.label);
        }
        for r in [other, unread] {
            assert!(
                matches!(&r, Err(IncrReason::OutputChanged(w)) if w.contains("window 0 differs")),
                "{}: another boot re-hashes: {r:?}",
                line.label
            );
        }
        std::fs::write(&path, &good).expect("restore the output");
    }
}

/// glibc's `link_warning` form (as `incr_diags.rs`): `.gnu.warning.<sym>`,
/// not allocated.
const WARNED: &str = "int wp264_warned(int x) { return x + 1; }\n\
    static const char w_wp264_warned[] __attribute__((used, \
    section(\".gnu.warning.wp264_warned\\n\\t#\"))) = \"wp264: wp264_warned is used\";\n";
const CALLER: &str =
    "int wp264_warned(int);\nint wp264_w2(int x) { return wp264_warned(x) + 2; }\n";

/// DIAGS (IN-75a, WP-263 F-2): what an update loses today. The full link
/// prints the reference-site warning, and its state holds no `DIAGS` row.
/// An update with nothing changed, and one with an unrelated file
/// (`m1.o`) changed, are then run: both refuse, the second
/// `update-not-implemented (diags)` (its front saw the warning). So no
/// warning is lost today; the update is: a link with any warning never
/// updates. Once the join replays `DIAGS` (U-9), an empty `DIAGS` would
/// drop the warning of an unchanged file. When a producer lands, the
/// first assertion flips.
#[test]
fn diags_hold_no_warning_today() {
    let m1v = format!("{}{TAG}", update::M1);
    for line in lines(&[("w1", "c", WARNED), ("w2", "c", CALLER), ("m1v", "c", &m1v)]) {
        let label = &line.label;
        let args = line.args(&["wp255d_m2", "wp255d_m1", "wp264_w2"]);
        line.archive(&["m1.o", "m2.o", "w1.o", "w2.o"]);
        let mut a = support::with_output(&args, &line.dir.join("out"));
        a.extend(["--incremental-state", "out.mortar-state"].iter().map(std::ffi::OsString::from));
        let warned = |o: &mortar::Outcome| match o {
            mortar::Outcome::Linked { report, .. } => {
                report.warnings().filter(|(_, d)| d.id() == "gnu-warning").count()
            }
            _ => usize::MAX,
        };
        let mut full = a.clone();
        full.push("--incremental=full".into());
        let inv = mortar::invoke_state5(
            "mortar",
            &full,
            &line.dir,
            mortar::incremental::State5 {
                path: line.dir.join("out.state5"),
                collide: false,
                rows: None,
            },
        );
        assert_eq!(warned(&inv.outcome), 1, "{label}: the full link warns: {:?}", inv.outcome);
        let m = mortar_incr::state5::read::decode(
            &std::fs::read(line.dir.join("out.state5")).expect("the state"),
        )
        .expect("the state decodes");
        assert!(m.diags.is_empty(), "{label}: DIAGS has a producer now: {:?}", m.diags);
        let mut upd = a.clone();
        upd.push("--incremental=update".into());
        let same = mortar::invoke_in("mortar", &upd, &line.dir);
        let same_w = warned(&same.outcome);
        let shown = |i: &mortar::Invocation| match &i.outcome {
            mortar::Outcome::Linked { .. } => i.incremental.trim().to_owned(),
            o => format!("{o:?} {}", i.incremental.trim()),
        };
        eprintln!("[wp264] {label}: nothing changed: {same_w} warning(s); {}", shown(&same));
        std::fs::copy(line.dir.join("m1v.o"), line.dir.join("m1.o")).expect("the variant");
        line.archive(&["m1.o", "m2.o", "w1.o", "w2.o"]);
        let one = mortar::invoke_in("mortar", &upd, &line.dir);
        let one_w = warned(&one.outcome);
        eprintln!("[wp264] {label}: m1.o changed: {one_w} warning(s); {}", shown(&one));
        // What a full link of the same inputs prints.
        let again = mortar::invoke_in("mortar", &full, &line.dir);
        assert_eq!(warned(&again.outcome), 1, "{label}: a full link warns");
        // Today neither update links: `--incremental=update` refuses. The
        // one whose front ran (`m1.o` changed) refuses `(diags)`: path (A)'s
        // `image_link` falls back on any front diagnostic, so no warning is
        // lost today; it is the update that is lost. (Nothing changed:
        // refused before the front, by the reason printed.)
        assert!(shown(&same).contains("update-not-implemented"), "{label}: {}", shown(&same));
        assert!(
            shown(&one).contains("update-not-implemented (diags)"),
            "{label}: the changed file's update: {}",
            shown(&one)
        );
        assert_eq!((same_w, one_w), (usize::MAX, usize::MAX), "{label}: neither update links");
    }
}
