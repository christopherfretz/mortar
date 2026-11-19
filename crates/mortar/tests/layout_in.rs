//! WP-225 M2b-2a: the delta layout's inputs of a changed file from the
//! lazy `Link`'s producers (`mortar::update::layout_in::layout_inputs`),
//! F-9 (a changed file's `.eh_frame` relocation against a global answers
//! from the update's resolution), and `delta::layout` over them judged
//! against the FULL link of the new inputs, section by section by
//! identity.
//!
//! `delta::layout` is called here ONLY: its `demands` are the WHOLE
//! front's `FileDemands` of the changed file (a TEST-ONLY input, keys
//! moved into the update's ids by name), since their producer (the
//! per-file scan, 255d5) is not this package's.
//!
//! The judgement, per section of the changed file (front and update
//! matched by `(file, index)`, the same bytes): placed in the update
//! exactly when the full link places it; in the output section of the
//! same NAME; at an address its alignment admits; overlapping no other
//! placed section of the chunk (the stored ones of the other files, the
//! update's). The OFFSET is compared and printed but not required equal:
//! the update keeps a section whose size did not grow at its old place and
//! first-fits the others in the file's region (IN-72 item 2), where the
//! full link packs the file anew. Each merged piece's fragment is in
//! the stored `FRAGS` or in the update's new ones.
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;
#[path = "support/update.rs"]
mod update;

use mortar::update::file_front::FrontCache;
use mortar::update::layout_in::layout_inputs;
use mortar_core::id::{FileId, SectionId, SymKey, SymKind, SymbolId};
use mortar_core::incr::{FileDemands, LayoutInput, PlaceNow, StoredLayout};
use mortar_elf::{Machine, SectionIndex};
use mortar_merge::eh::Target;
use mortar_merge::phase::ArchRelocs;
use update::{first_free, lines, member, pool, same_file, state, whole};

/// `m1.o` before: C++, a `try` (`.eh_frame` with a personality: the CIE
/// relocates against the global `DW.ref.__gxx_personality_v0`, defined in
/// a COMDAT group the file keeps).
const EH_A: &str = "extern \"C\" { int wp255d_m1_data = 1; int wp255d_throw(int); }\n\
    extern \"C\" int wp255d_m1(int x) { try { return wp255d_throw(x) + wp255d_m1_data; } \
    catch (...) { return -1; } }\n\
    extern \"C\" int wp255d_throw(int x) { if (x < 0) throw x; return x; }\n";
/// `m1.o` after: the function grown, strings added (a merge section), a
/// COMDAT group added (`wp255d_inl`, kept by the earlier `m8.o`, and
/// `wp255d_inl3`, only here), more `.eh_frame` records.
const EH_B: &str = "extern \"C\" { int wp255d_m1_data = 1; int wp255d_throw(int); }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl3(int x) { return x * 7 + wp255d_m1_data; }\n\
    extern \"C\" const char *wp255d_str(int i) { static const char *const t[] = \
    { \"wp255d2a-alpha\", \"wp255d2a-beta\" }; return t[i % 2]; }\n\
    extern \"C\" int wp255d_m1(int x) { try { int s = 0; for (int i = 0; i < x; i++) s += i * x; \
    return s + wp255d_throw(x) + wp255d_inl(x) + wp255d_inl3(x) + *wp255d_str(x); } \
    catch (int e) { return e; } catch (...) { return -1; } }\n\
    extern \"C\" int wp255d_throw(int x) { if (x < 0) throw x; return x; }\n";
const M8: &str = "extern \"C\" { extern int wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m8(int x) { return wp255d_inl(x) + 8; }\n";
/// A C edit: the function grown only.
const C_GROWN: &str = "int wp255d_m1_data = 1;\nint wp255d_m1(int x) { int s = 0; \
    for (int i = 0; i < x; i++) s += i * x + wp255d_m1_data; return s; }\n";

#[test]
fn layout_inputs_of_changed_files_against_the_full_link() {
    let extra =
        [("eha", "c++", EH_A), ("ehb", "c++", EH_B), ("m8", "c++", M8), ("cg", "c", C_GROWN)];
    let mut f9_seen = 0;
    for line in lines(&extra) {
        let machine =
            if line.label.starts_with("aarch64") { Machine::AArch64 } else { Machine::X86_64 };
        for (case, before, after) in [("c grown", "m1.keep", "cg.o"), ("c++ eh", "eha.o", "ehb.o")]
        {
            let what = |s: &str| format!("{} {case}: {s}", line.label);
            let args = line.args(&["wp255d_m8", "wp255d_m2", "wp255d_m1"]);
            let keep = line.dir.join("m1.keep");
            std::fs::copy(line.dir.join("m1.o"), &keep).expect("keep m1.o");
            std::fs::copy(line.dir.join(before), line.dir.join("m1.o")).expect("before");
            line.archive(&["m8.o", "m1.o", "m2.o"]);
            let old = state(&line, &args, "old");
            std::fs::copy(line.dir.join(after), line.dir.join("m1.o")).expect("the edit");
            line.archive(&["m8.o", "m1.o", "m2.o"]);
            let _new = state(&line, &args, "new");
            let capture = mortar_incr::state5::read::decode(
                &std::fs::read(line.dir.join("new.state5")).expect("the new state"),
            )
            .expect("the new state decodes");
            let store = support::store();
            let f = whole(&line, &args, &store);
            let file = member(&old, "m1.o");
            let nf = same_file(&f, &old, file);
            assert_eq!(nf, file, "{}", what("one FILES row in both links"));
            let (cf, delta) =
                mortar::update::changed::u5(&old, &[file], &[], &f, &capture, &mut |_| {})
                    .unwrap_or_else(|e| panic!("{}", what(&format!("U4-U5: {}", e.report()))))
                    .expect("m1.o changed");
            let first = first_free(&old);
            assert_eq!(cf.files[0].first_section, SectionId(first), "{}", what("ids"));
            let rows = mortar_incr::state5::LayoutOf { view: &old };
            let stored = StoredLayout::new(&rows);
            let fresh = support::store();
            let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
            let relocs = ArchRelocs(f.arch.get());
            let inputs =
                layout_inputs(&mut cache, &old, &stored, &f.plan, &cf, &delta, &relocs, &f.pool)
                    .unwrap_or_else(|e| {
                        panic!("{}", what(&format!("layout_inputs: {}", e.report())))
                    });

            // F-9: the changed file's globals answer from the update's
            // resolution: a key the file defines resolves to its NEW
            // section (fresh id), its stored row to the OLD one, and the
            // split's `.eh_frame` relocations name the new one.
            let symbols = old.count(mortar_incr::format::kind::SYMBOLS);
            let mut moved: Vec<(SymbolId, u32, u32)> = Vec::new();
            for (id, r) in &delta.resolved {
                if u64::from(id.0) < symbols && r.section != u32::MAX && r.section >= first {
                    let was = old.resolved(*id).expect("RESOLVED");
                    if was.section != u32::MAX && was.section < first {
                        moved.push((*id, was.section, r.section));
                    }
                }
            }
            let ff = cache.get(file).expect("the front of m1.o");
            let facts =
                mortar::update::file_split::changed_facts(ff, &old, &f.plan, &delta.keepers)
                    .expect("facts");
            let null = mortar::update::file_split::SectionFact {
                kind: mortar_core::resolution::SectionKind::Null,
                kept: false,
                out: None,
                reloc: SectionIndex(0),
            };
            let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
            let split = mortar::update::file_split::split_of(
                ff,
                &old,
                &by_index,
                &relocs,
                &f.pool,
                Some(&delta),
            )
            .expect("the split");
            let eh_targets: Vec<SectionId> = split
                .eh_relocs
                .iter()
                .flatten()
                .filter_map(|r| match r.target {
                    Target::Section { section, .. } => Some(section),
                    _ => None,
                })
                .collect();
            for t in &eh_targets {
                assert!(
                    t.0 >= first || old.section(*t).is_ok_and(|s| s.file != file.0),
                    "{}",
                    what(&format!("an .eh_frame target {} is an OLD section of m1.o", t.0))
                );
            }
            let f9: Vec<_> =
                moved.iter().filter(|(_, _, new)| eh_targets.contains(&SectionId(*new))).collect();
            f9_seen += usize::from(case == "c++ eh" && !f9.is_empty());

            // The layout, with the whole front's demands (TEST-ONLY).
            let mut demands: Vec<FileDemands> = Vec::new();
            if let Some(i5) = &f.incr5 {
                for d in i5.demands.iter().filter(|d| d.file == nf) {
                    let mut v = Vec::new();
                    for s in &d.demands {
                        let key = match s.key.kind() {
                            SymKind::Global(g) => {
                                let k = &f.inputs.symbols[g];
                                let ver = k.version.unwrap_or_default();
                                let id =
                                    old.find_key(k.name, ver).expect("SYMBOLS").or_else(|| {
                                        let at = delta
                                            .new_keys
                                            .iter()
                                            .position(|n| n.name == k.name && n.version == ver)?;
                                        Some(SymbolId(u32::try_from(symbols).ok()? + at as u32))
                                    });
                                SymKey::global(id.expect("the key in the update"))
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
            let target = mortar_layout::target::Target::of(machine);
            let d = match mortar_layout::delta::layout(&stored, &input, &target) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{}", what(&format!("delta::layout falls back: {}", e.report())));
                    continue;
                }
            };
            // Section by section, by identity.
            let outsecs = old.outsecs().expect("OUTSECS");
            let out_name =
                |o: mortar_core::OutSecId| old.string(outsecs[o.0 as usize].name).expect("n");
            let (mut placed, mut same_offset, mut differ) = (0, 0, Vec::new());
            let mut mine: Vec<(u32, u64, u64)> = Vec::new();
            for c in &inputs.changes {
                let fid = f.resolution.section_id(nf, SectionIndex(c.index)).expect("front id");
                let full = f.layout.placement.place[fid].get();
                let now = match d.place_of(&stored, c.id).expect("place_of") {
                    PlaceNow::Here(p) => p.get(),
                    PlaceNow::NotPlaced => None,
                    PlaceNow::Stored(_) => panic!("{}", what("a changed file's section is Stored")),
                };
                let name = String::from_utf8_lossy(c.key.name).into_owned();
                assert_eq!(
                    now.is_some(),
                    full.is_some(),
                    "{}",
                    what(&format!(
                        "section {} {name}: placed (update {now:?}, full {full:?})",
                        c.index
                    ))
                );
                let (Some((uc, uo)), Some((fc, fo))) = (now, full) else { continue };
                placed += 1;
                let uchunk = &d.placement.chunks[uc];
                let fchunk = &f.layout.placement.chunks[fc];
                let uout = uchunk.kind.out().map(out_name);
                let fout = fchunk.kind.out().map(|o| f.assignment.outsecs[o].name.to_vec());
                assert_eq!(
                    uout,
                    fout,
                    "{}",
                    what(&format!("section {} {name}: output section", c.index))
                );
                let addr = uchunk.addr.unwrap_or(0) + uo;
                assert!(
                    addr.is_multiple_of(c.align.max(1)),
                    "{}",
                    what(&format!("{name}: aligned"))
                );
                mine.push((uc.0, uo, uo + c.size));
                if uo == fo {
                    same_offset += 1;
                } else {
                    differ.push(format!("{name}: update {uo:#x}, full {fo:#x}"));
                }
            }
            // No overlap with any other placed section of the same chunk.
            let files = u32::try_from(old.count(mortar_incr::format::kind::FILES)).expect("FILES");
            let mut others: Vec<(u32, u64, u64)> = Vec::new();
            for g in (1..files).map(FileId).filter(|&g| g != file) {
                for s in stored.sections_of(g).expect("SECTIONS") {
                    if let PlaceNow::Stored(p) = d.place_of(&stored, s.id).expect("place_of")
                        && let Some((ch, off)) = p.get()
                    {
                        others.push((ch.0, off, off + s.size));
                    }
                }
            }
            for (k, a) in mine.iter().enumerate() {
                for b in others.iter().chain(&mine[k + 1..]) {
                    let overlap = a.0 == b.0 && a.1 < b.2 && b.1 < a.2 && a.1 < a.2 && b.1 < b.2;
                    assert!(!overlap, "{}", what(&format!("{a:?} overlaps {b:?}")));
                }
            }
            // Every piece's fragment is stored or new.
            let mut pieces = 0;
            for p in inputs.pieces.iter().flat_map(|p| &p.pieces) {
                let key = (p.out, p.class.word(), p.hash);
                let found = stored.frags_of(key.0, key.1, key.2).expect("FRAGS").is_some()
                    || d.frags.iter().any(|g| (g.out, g.class, g.hash) == key);
                assert!(found, "{}", what(&format!("piece {p:?}: no fragment")));
                pieces += 1;
            }
            eprintln!(
                "{}",
                what(&format!(
                    "{} changes, {placed} placed ({same_offset} at the full link's offset; \
                     differ: {differ:?}), {pieces} pieces, {} eh records, F-9 keys {f9:?}",
                    inputs.changes.len(),
                    inputs.eh.iter().map(|e| e.records.len()).sum::<usize>()
                ))
            );
            std::fs::copy(&keep, line.dir.join("m1.o")).expect("restore m1.o");
        }
    }
    if mortar_testutil::tools_required() {
        assert!(f9_seen > 0, "F-9: no changed file's .eh_frame named a moved global's section");
    }
}
