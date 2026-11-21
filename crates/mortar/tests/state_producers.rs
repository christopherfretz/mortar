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
//! - WP-225 M2b-5b: after a taken update whose `.symtab` spans change,
//!   the update's map states the output's blocks (the full link's `SYMTAB`
//!   blocks of the same inputs), not the stored ones.
//!
//! (State version 11: `symtab_rows.rs`, `version_10_state_is_refused`.)
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
            // Refused (AArch64: `.text`'s tail is the patch island; or
            // M2b-2c's "a stored place in no region" refusal, L-159's
            // sibling in `delta.rs`), or placed in new overflow regions.
            assert!(
                without.as_ref().is_err_and(|e| {
                    e.contains("reserve-exhausted") || e == "update-not-implemented (layout)"
                }) || without.as_ref().is_ok_and(|k| k.opened > 0 && !k.moved.is_empty()),
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
/// prints the reference-site warning; since WP-265 its state holds the
/// warning's `DIAGS` row (`diags_producer.rs` judges the rows).
/// An update with nothing changed, and one with an unrelated file
/// (`m1.o`) changed, are then run: both refuse, the second
/// `update-not-implemented (diags)` (its front saw the warning). So no
/// warning is lost today; the update is: a link with any warning never
/// updates. Once the join replays `DIAGS` (U-9), an empty `DIAGS` would
/// drop the warning of an unchanged file. The producer has landed
/// (WP-265): the first assertion flipped.
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
        assert!(
            m.diags.iter().any(|d| d.code == "gnu-warning" && d.ref_count == 1),
            "{label}: DIAGS holds the warning (WP-265): {:?}",
            m.diags
        );
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

/// M2b-5b's program (WP-225, the map FINDING of M2b-5's hand-back): `a.o`
/// a weak `f`, `x.o` a local `f` (made global by an edit: the
/// `w10_local_to_global` shape), `y.o` one local (grown to five by an
/// edit, four local objects: a block past its capacity moves the blocks
/// after it).
const M5B_MAIN: &str = "#include <stdio.h>\nint f(void);\nint x_value(void);\nint y_value(int);\n\
    int main(void) { printf(\"%d %d %d\\n\", f(), x_value(), y_value(3)); return 0; }\n";
const M5B_A: &str = "__attribute__((weak)) int f(void) { return 1; }\n";
const M5B_X_LOCAL: &str = "static __attribute__((noinline, used)) int f(void) { return 2; }\n\
    int x_value(void) { return f() + 1; }\n";
const M5B_X_GLOBAL: &str = "__attribute__((noinline)) int f(void) { return 2; }\n\
    int x_value(void) { return f() + 1; }\n";
const M5B_Y_ONE: &str = "static volatile int d1 = 1;\nint y_value(int x) { return x + d1; }\n";
/// Four more local objects (no code of their own: no new FDE).
const M5B_Y_FIVE: &str = "static volatile int d1 = 1;\nstatic volatile int d2 = 2;\n\
    static volatile int d3 = 3;\nstatic volatile int d4 = 4;\nstatic volatile int d5 = 5;\n\
    int y_value(int x) { return x + d1 + d2 + d3 + d4 + d5; }\n";

/// A `symblock` record's fields after `<file>`, its file as
/// `(path, member)` (`-` the internal file).
type Block = ((String, String), [u64; 5]);

/// The program of M2b-5b on one target, by one compiler for `x` and `y`.
struct M5b {
    _scratch: mortar_testutil::Scratch,
    label: String,
    dir: std::path::PathBuf,
    args: Vec<String>,
    /// gcc, or kiln/hod (`MASON_KILN`, `MASON_HOD`) for `x` and `y`.
    kiln: Option<(std::ffi::OsString, std::ffi::OsString)>,
    gcc: std::path::PathBuf,
    triple: &'static str,
}

impl M5b {
    fn of(arch: mortar_testutil::target::Arch, kiln: bool) -> Option<Self> {
        let k = if kiln {
            Some((std::env::var_os("MASON_KILN")?, std::env::var_os("MASON_HOD")?))
        } else {
            None
        };
        let label = format!("{} {}", arch.triple(), if kiln { "kiln/hod" } else { "gcc" });
        let s = mortar_testutil::Scratch::new(&format!("wp225m5b-{}", arch.triple()));
        let sources = [
            support::Source { name: "main.c", text: M5B_MAIN },
            support::Source { name: "a.c", text: M5B_A },
            support::Source { name: "x.c", text: M5B_X_LOCAL },
            support::Source { name: "y.c", text: M5B_Y_ONE },
        ];
        let cc = arch.gnu("gcc");
        let Some(b) = support::build_by(&s, &cc, &["-O1"], &sources) else {
            assert!(!mortar_testutil::tools_required(), "{cc} is required");
            return None;
        };
        let gcc = mortar_testutil::tool(&cc).expect("the compiler");
        let p = Self {
            _scratch: s,
            label,
            dir: b.dir,
            args: b.args,
            kiln: k,
            gcc,
            triple: arch.triple(),
        };
        p.compile("x", M5B_X_LOCAL);
        p.compile("y", M5B_Y_ONE);
        Some(p)
    }

    /// `<name>.c.o` (the link line's object) from `text`: gcc -O1 as C, or
    /// kiln/hod as C++ in `extern "C"`.
    fn compile(&self, name: &str, text: &str) {
        let obj = self.dir.join(format!("{name}.c.o"));
        let o = if let Some((kiln, hod)) = &self.kiln {
            let src = self.dir.join(format!("{name}.cc"));
            std::fs::write(&src, format!("extern \"C\" {{\n{text}}}\n")).expect("source");
            let mut c = std::process::Command::new(kiln);
            c.env("KILN_HOD", hod).arg("-fkiln-backend=hod");
            c.arg(format!("--target={}", self.triple)).arg("-c").arg(&src).arg("-o").arg(&obj);
            mortar_testutil::run_with(&mut c, b"", std::time::Duration::from_mins(2))
        } else {
            let src = self.dir.join(format!("{name}.c"));
            std::fs::write(&src, text).expect("source");
            let args: Vec<std::ffi::OsString> =
                vec!["-O1".into(), "-c".into(), src.into(), "-o".into(), obj.into()];
            mortar_testutil::run(&self.gcc, &args)
        };
        assert!(o.success(), "{}: {name}: {}", self.label, o.stderr_text());
    }

    /// mortar on the line with `-o <out>` and `extra`.
    fn link(&self, out: &str, extra: &[String]) -> mortar_testutil::Outcome {
        let mut a = support::with_output(&self.args, &self.dir.join(out));
        a.extend(extra.iter().map(std::ffi::OsString::from));
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_mortar"));
        c.args(&a).current_dir(&self.dir).env_remove("MORTAR_SCHED_SEED");
        mortar_testutil::run_with(&mut c, b"", std::time::Duration::from_mins(2))
    }
}

/// The `symblock` records of a map, each file by its `file` record.
fn map_blocks(map: &str) -> Vec<Block> {
    let mut files = std::collections::HashMap::new();
    let mut out = Vec::new();
    for l in map.lines() {
        let w: Vec<&str> = l.split(' ').collect();
        match w[0] {
            "file" => {
                let member = w.get(3).copied().unwrap_or("").to_owned();
                files.insert(w[1].to_owned(), (w[2].to_owned(), member));
            }
            "symblock" => {
                let file = if w[1] == "-" {
                    ("-".to_owned(), String::new())
                } else {
                    files.get(w[1]).cloned().unwrap_or_else(|| panic!("no file {}: {l}", w[1]))
                };
                let n = |i: usize, radix| u64::from_str_radix(w[i], radix).expect(l);
                out.push((file, [n(2, 10), n(3, 10), n(4, 10), n(5, 16), n(6, 16)]));
            }
            _ => {}
        }
    }
    out.sort();
    out
}

/// The `SYMTAB` blocks of a state (the internal file's, then each file's;
/// not the global area), as the map would state them.
fn state_blocks(path: &std::path::Path) -> Vec<Block> {
    use mortar_core::incr::LayoutRows;
    let view = mortar_incr::state5::read::open_bytes(std::fs::read(path).expect("the state"))
        .expect("the state opens");
    let rows = mortar_incr::state5::LayoutOf { view: &view };
    let files = u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("FILES");
    let mut out = Vec::new();
    for id in 0..files {
        let Some(b) = rows.symtab_block(mortar_core::FileId(id)).expect("SYMTAB") else {
            continue;
        };
        let file = if id == 0 {
            ("-".to_owned(), String::new())
        } else {
            let r = view.file(mortar_core::FileId(id)).expect("FILES");
            let s = |x| String::from_utf8_lossy(&view.string(x).expect("a string")).into_owned();
            (s(r.path), if r.member == 0 { String::new() } else { s(r.member_name) })
        };
        let v = [b.first, b.count, b.capacity].map(u64::from);
        out.push((file, [v[0], v[1], v[2], b.strtab_offset, b.strtab_capacity]));
    }
    out.sort();
    out
}

/// A `.symtab` entry: its name (from `.strtab`), `st_info`, `st_name`.
type Entry = (Vec<u8>, u8, u32);

/// An ELF64 output's `.symtab` entries.
fn symtab_entries(elf: &[u8]) -> Vec<Entry> {
    let le = |at: usize, n: usize| {
        elf[at..at + n].iter().rev().fold(0u64, |a, &x| a << 8 | u64::from(x)) as usize
    };
    let (shoff, ent, num, strx) = (le(0x28, 8), le(0x3a, 2), le(0x3c, 2), le(0x3e, 2));
    let names = le(shoff + strx * ent + 24, 8);
    let range = |name: &[u8]| {
        (0..num)
            .find_map(|i| {
                let at = names + le(shoff + i * ent, 4);
                let end = at + elf[at..].iter().position(|&b| b == 0).expect("a name");
                let (o, n) = (le(shoff + i * ent + 24, 8), le(shoff + i * ent + 32, 8));
                (&elf[at..end] == name).then_some(o..o + n)
            })
            .expect("the section")
    };
    let (st, ss) = (range(b".symtab"), range(b".strtab"));
    let strtab = &elf[ss];
    elf[st]
        .chunks_exact(24)
        .map(|e| {
            let name = u32::from_le_bytes(e[0..4].try_into().expect("4"));
            let at = name as usize;
            let end = strtab[at..].iter().position(|&b| b == 0).map_or(strtab.len(), |n| at + n);
            (strtab[at..end].to_vec(), e[4], name)
        })
        .collect()
}

/// M2b-5b (WP-225; I-78a, IN-49a, IN-78): after a taken update whose
/// `.symtab` spans change, the update's map states the OUTPUT's blocks:
/// its `symblock` records are the `SYMTAB` blocks of the incremental-mode
/// full link of the same inputs (its capture; a full link's map writes no
/// `symblock`, `decisions_of`), and each block names the output's locals
/// (every entry of `[first, first + count)` is `STB_LOCAL`, its name in the
/// block's `.strtab` region). The hops: `x.o`'s local `f` made global
/// (`w10_local_to_global`: before M2b-5b the map kept `x.o`'s stored block
/// at the entry that is now a filler), back to local (the block returns),
/// `y.o`'s locals grown from one to five (past the block's capacity: the
/// blocks after it move) and back. A hop the update refuses is linked
/// whole (`--incremental=full`) and not compared; its reason is named.
/// gcc on AArch64 and x86-64, and kiln/hod objects for `x` and `y` when
/// `MASON_KILN` and `MASON_HOD` are set (skipped by name otherwise).
#[test]
fn the_update_map_states_the_output_symtab_blocks() {
    use mortar_testutil::target::Arch;
    let mut taken = Vec::new();
    let mut refused = Vec::new();
    for arch in [Arch::Aarch64, Arch::X86_64] {
        for kiln in [false, true] {
            let Some(p) = M5b::of(arch, kiln) else {
                if kiln {
                    eprintln!("{}: kiln/hod skipped: MASON_KILN or MASON_HOD unset", arch.triple());
                }
                continue;
            };
            let map = p.dir.join("u.map");
            let full = |out: &str| {
                let o = p.link(out, &["--incremental=full".into()]);
                assert!(o.success(), "{}: the full link {out}: {}", p.label, o.stderr_text());
            };
            full("u");
            let hops = [
                ("x", M5B_X_GLOBAL, "x local f made global"),
                ("x", M5B_X_LOCAL, "x global f made local"),
                ("y", M5B_Y_FIVE, "y grows to five locals"),
                ("y", M5B_Y_ONE, "y back to one local"),
            ];
            for (file, text, what) in hops {
                p.compile(file, text);
                let _ = std::fs::remove_file(&map);
                let mortar_map = format!("--mortar-map={}", map.display());
                let o = p.link("u", &["--incremental=update".into(), mortar_map]);
                if !o.success() {
                    let err = o.stderr_text();
                    assert!(
                        err.contains("--incremental=update: cannot update: "),
                        "{}: {what}: not a named refusal: {err}",
                        p.label
                    );
                    refused.push(format!("{}: {what}: {}", p.label, err.trim()));
                    full("u");
                    continue;
                }
                full("g");
                let map_text = std::fs::read_to_string(&map).expect("the update's map");
                let got = map_blocks(&map_text);
                let want = state_blocks(&p.dir.join("g.mortar-state"));
                assert!(!want.is_empty(), "{}: {what}: the full link stores no block", p.label);
                assert_eq!(got, want, "{}: {what}: the map's blocks are the full link's", p.label);
                let elf = std::fs::read(p.dir.join("u")).expect("the update's output");
                let entries = symtab_entries(&elf);
                let full_elf = std::fs::read(p.dir.join("g")).expect("the full link's output");
                let full_entries = symtab_entries(&full_elf);
                let mut outside = Vec::new();
                for ((path, member), [first, count, _, str_at, str_cap]) in &got {
                    for i in *first..first + count {
                        let (name, info, at) = &entries[i as usize];
                        let shown = String::from_utf8_lossy(name);
                        let at_full = full_entries.get(i as usize).map(|e| (&e.0, e.1));
                        assert_eq!(
                            Some((name, *info)),
                            at_full,
                            "{}: {what}: {path}{member} entry {i}: the full link's entry",
                            p.label
                        );
                        if path == "-" {
                            continue;
                        }
                        assert_eq!(info >> 4, 0, "{}: {what}: {path}{member} entry {i}", p.label);
                        // A name outside the block's `.strtab` region: recorded
                        // (an AArch64 mapping symbol's shared name).
                        if !(*str_at..str_at + str_cap).contains(&u64::from(*at)) {
                            outside.push(format!("{i} {shown} at {at:#x}"));
                        }
                    }
                }
                // The shape: x.o's block holds `f` exactly when `f` is local.
                let x_obj = p.dir.join("x.c.o").to_string_lossy().into_owned();
                let names_of = |path: &str| -> Vec<Vec<u8>> {
                    got.iter()
                        .filter(|((p, _), _)| p == path)
                        .flat_map(|(_, [first, count, ..])| {
                            (*first..first + count).map(|i| entries[i as usize].0.clone())
                        })
                        .collect()
                };
                let local_f = if p.kiln.is_some() { &b"_ZL1fv"[..] } else { b"f" };
                let x_names = names_of(&x_obj);
                let local = text == M5B_X_LOCAL;
                if file == "x" {
                    assert_eq!(
                        x_names.iter().any(|n| n == local_f),
                        local,
                        "{}: {what}: x.o's block {x_names:?}",
                        p.label
                    );
                }
                let globals_f = entries.iter().filter(|(n, info, _)| n == b"f" && info >> 4 == 1);
                eprintln!(
                    "{}: {what}: taken, {} blocks, x.o's {:?}, global f {}, {} names \
                     outside their block's strings (first: {:?})",
                    p.label,
                    got.len(),
                    x_names.iter().map(|n| String::from_utf8_lossy(n)).collect::<Vec<_>>(),
                    globals_f.count(),
                    outside.len(),
                    outside.first()
                );
                taken.push(format!("{}: {what}", p.label));
            }
        }
    }
    eprintln!("taken {}: {taken:#?}\nrefused {}: {refused:#?}", taken.len(), refused.len());
    assert!(
        !mortar_testutil::tools_required() || !taken.is_empty(),
        "no hop was taken: the comparison never ran: {refused:#?}"
    );
}
