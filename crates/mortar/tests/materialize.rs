//! WP-225 M2b-3 Part 2: the materializer (`mortar_incr::update::materialize`).
//!
//! - `chunk_map` (a copy of `capture.rs` `map`'s chunk block, ruling R-4)
//!   gives `map`'s answer on the same link.
//! - The identity oracle (D-8 1): a first link's stored layout with an
//!   EMPTY delta (`unchanged_delta`) materialized against a second front
//!   over the same inputs equals that front's own `Layout`, table by table.
//! - Mutations (D-8 2): hand-made deltas (a changed file whose sections keep
//!   their places; one moved; one chunk grown; one fragment moved): each
//!   difference is exactly the delta's; each refusal of the note is reached
//!   by name.
//!
//! Static C, C++ (no exceptions of its own) and C++ with exceptions, on
//! AArch64 and x86-64.

#![allow(dead_code, reason = "support/update.rs's helpers that other tests use")]

mod support;
#[path = "support/update.rs"]
mod update;

use mortar::front::Front;
use mortar_core::id::{ChunkId, FileId, SectionId, SymbolId};
use mortar_core::identity::{Anchor, SectionKey};
use mortar_core::incr::{IncrReason, LayoutDelta, MergeClass, SectionChange, StoredLayout};
use mortar_core::layout::{ChunkKind, DefValue, Hole, HoleKind, Layout, Place, Redirect, Sized};
use mortar_incr::capture::{EhFile, Keys, Link};
use mortar_incr::update::materialize::{
    Refusal, capture_chunk_map, chunk_map, first_difference, identity_hop, materialize,
    unchanged_delta,
};
use mortar_incr::update::view::StateView;
use mortar_testutil::target::Arch;
use mortar_testutil::{Scratch, tool, tools_required};
use support::Source;
use update::Line;

/// A C++ program that throws nothing itself.
const CXX_NOEH: &[Source] = &[Source {
    name: "n1.cpp",
    text: r#"#include <cstdio>
template <class T> T twice(T x) { return x + x; }
struct S { virtual int f(int x) { return twice(x); } virtual ~S() {} };
static const char *names[] = { "alpha", "beta" };
int main(int argc, char **) { S s; S *p = &s; std::printf("%d %s\n", p->f(argc), names[argc & 1]); return 0; }
"#,
}];

/// Every (target, program) line: static C, C++, C++ with EH.
fn programs() -> Vec<Line> {
    let mut v = Vec::new();
    for (arch, a) in [(Arch::Aarch64, "aarch64"), (Arch::X86_64, "x86-64")] {
        for (lang, cc, flags, src) in [
            ("c", "gcc", &["-O1"][..], support::C_PROGRAM),
            ("c++", "g++", &["-O1", "-fno-exceptions"][..], CXX_NOEH),
            ("c++ eh", "g++", &["-O1"][..], support::CXX_PROGRAM),
        ] {
            let cc = arch.gnu(cc);
            let s = Scratch::new(&format!("wp225m2b3-{a}-{}", lang.replace(' ', "-")));
            let Some(b) = support::build_by(&s, &cc, flags, src) else {
                assert!(!tools_required(), "{cc} is required");
                continue;
            };
            let cc = tool(&cc).expect("the compiler");
            v.push(Line {
                _scratch: s,
                label: format!("{a} {lang}"),
                dir: b.dir,
                base: b.args,
                cc,
            });
        }
    }
    v
}

/// The line's incremental arguments (the update's plan).
fn incr_args(line: &Line) -> Vec<String> {
    let mut a = line.base.clone();
    a.extend([
        "--incremental=full".into(),
        "--incremental-state".into(),
        "out.mortar-state".into(),
    ]);
    a
}

/// What `materialize` takes from a front.
struct Side<'f, 's> {
    f: &'f Front<'s>,
    link: Link<'f, 's>,
    keys: Keys<'s>,
    classes: Vec<MergeClass>,
    eh: Vec<EhFile>,
}

fn side<'f, 's>(f: &'f Front<'s>) -> Side<'f, 's> {
    let link = Link {
        inputs: &f.inputs,
        resolution: &f.resolution,
        assignment: &f.assignment,
        split: &f.merge.split,
    };
    let keys = Keys::new(&link).expect("keys");
    let classes = f
        .merge
        .meta
        .iter()
        .map(|m| MergeClass { strings: m.strings, entsize: m.entsize, align: m.align })
        .collect();
    let runs = f.incr5.as_ref().expect("incr5").eh.as_ref().expect("eh runs");
    let eh = runs
        .iter()
        .map(|r| EhFile {
            file: r.file,
            offset: r.offset,
            capacity: r.capacity,
            records: r.records,
            fdes: r.fdes,
            movable: r.movable,
        })
        .collect();
    Side { f, link, keys, classes, eh }
}

impl Side<'_, '_> {
    fn run(
        &self,
        view: &StateView<'_>,
        d: &LayoutDelta,
        changes: &[SectionChange<'_>],
        computed: &Layout,
    ) -> Result<Layout, Refusal> {
        self.run2(view, d, changes, computed).map(|x| x.0)
    }

    fn run2(
        &self,
        view: &StateView<'_>,
        d: &LayoutDelta,
        changes: &[SectionChange<'_>],
        computed: &Layout,
    ) -> Result<(Layout, LayoutDelta), Refusal> {
        let rows = mortar_incr::state5::LayoutOf { view };
        let stored = StoredLayout::new(&rows);
        materialize(
            view,
            &stored,
            d,
            changes,
            &self.link,
            &self.keys,
            computed,
            &self.classes,
            &self.eh,
        )
    }
}

/// The first link's state (and its bytes), and the front of the same line.
fn with_link(line: &Line, go: impl FnOnce(&StateView<'_>, &[u8], &Side<'_, '_>)) {
    let view = update::state(line, &incr_args(line), "out");
    let bytes = std::fs::read(line.dir.join("out.state5")).expect("the state's bytes");
    let store = support::store();
    let f = update::whole(line, &incr_args(line), &store);
    let s = side(&f);
    go(&view, &bytes, &s);
}

#[test]
fn chunk_map_is_the_capture_map_block() {
    for line in programs() {
        with_link(&line, |view, _, s| {
            let mut strings = mortar_incr::format::Strings::default();
            let mut s4 = mortar_incr::capture::capture(
                &s.link,
                &s.keys,
                &s.f.layout,
                &mut strings,
                &|_| Some([0; 16]),
                s.f.arch.get(),
            )
            .expect("a format-4 capture");
            s4.strings = strings.into_bytes();
            let want = capture_chunk_map(&s.link, &s.keys, &s4, &s.f.layout.placement)
                .expect("map's chunks");
            let outsecs: Vec<_> = s4
                .outsecs
                .iter()
                .map(|o| {
                    mortar_core::identity::outsec_id(
                        &s.keys.outsecs,
                        &mortar_core::identity::OutSecIdentity {
                            name: s4.str(o.name),
                            ordinal: o.ordinal,
                        },
                    )
                })
                .collect();
            let same = chunk_map(
                &s.link,
                &s4.chunks,
                &|x| Ok(s4.str(x).to_vec()),
                &outsecs,
                &s.f.layout.placement,
            )
            .expect("chunk_map over the same rows");
            assert_eq!(same, want, "{}: chunk_map over format 4's rows", line.label);
            // The stored rows (format 5, the first link's) name the same chunks.
            let rows = view.chunks().expect("CHUNKS");
            let o5: Vec<_> = view
                .outsecs()
                .expect("OUTSECS")
                .iter()
                .map(|o| {
                    let name = view.string(o.name).expect("a name");
                    mortar_core::identity::outsec_id(
                        &s.keys.outsecs,
                        &mortar_core::identity::OutSecIdentity { name: &name, ordinal: o.ordinal },
                    )
                })
                .collect();
            let got = chunk_map(
                &s.link,
                &rows,
                &|x| view.string(x).map_err(|e| e.report()),
                &o5,
                &s.f.layout.placement,
            )
            .expect("chunk_map over the stored rows");
            assert_eq!(got, want, "{}: chunk_map over the stored rows", line.label);
        });
    }
}

#[test]
fn materialize_equals_the_front_in_the_identity_case() {
    let mut ran = 0;
    for line in programs() {
        with_link(&line, |view, _, s| {
            let r = identity_hop(view, &s.link, &s.keys, &s.f.layout, &s.classes, &s.eh);
            assert!(r.is_ok(), "{}: {}", line.label, r.unwrap_err());
            ran += 1;
        });
    }
    if tools_required() {
        assert_eq!(ran, 6, "every target and program");
    }
}

// ---- mutations --------------------------------------------------------------------

/// The first object of the line's own (scratch) directory with a placed
/// executable section: its file and that section's index.
fn own_file(view: &StateView<'_>, s: &Side<'_, '_>, dir: &std::path::Path) -> (FileId, u32) {
    let rows = mortar_incr::state5::LayoutOf { view };
    let stored = StoredLayout::new(&rows);
    let dir = dir.as_os_str().as_encoded_bytes();
    for (file, x) in s.f.inputs.files.iter() {
        if !x.path.starts_with(dir) || x.member.is_some() {
            continue;
        }
        for r in stored.sections_of(file).expect("SECTIONS") {
            if r.sh_flags & 4 != 0 && r.size > 0 && r.place != Place::NONE {
                return (file, r.index);
            }
        }
    }
    panic!("no object of the line's own with code");
}

/// A delta that changes `file` and keeps every section where it is: its
/// sections at fresh ids `first .. first + n`, each placed at its stored
/// place.
fn keep_file<'v>(
    view: &'v StateView<'_>,
    stored: &StoredLayout<'_>,
    file: FileId,
    names: &'v [Vec<u8>],
) -> (LayoutDelta, Vec<SectionChange<'v>>) {
    let _ = view;
    let mut d = unchanged_delta(stored).expect("the empty delta");
    let first = update::first_free(view);
    let rows = stored.sections_of(file).expect("SECTIONS");
    let mut place = vec![Place::NONE; (first as usize) + rows.len()];
    let mut changes = Vec::new();
    for (k, r) in rows.iter().enumerate() {
        let id = SectionId(first + k as u32);
        place[id.0 as usize] = r.place;
        changes.push(SectionChange {
            id,
            file,
            index: r.index,
            key: SectionKey { anchor: Anchor::None, name: &names[k] },
            ordinal: r.ordinal,
            old: Some(r.id),
            sh_type: r.sh_type,
            sh_flags: r.sh_flags,
            size: r.size,
            align: r.align,
            live: true,
            kept: true,
        });
    }
    d.placement.place = mortar_core::column::Column::from_vec(place).expect("place");
    d.files = vec![file];
    (d, changes)
}

/// `computed` without its island rows: F-1's bound refuses a moved code
/// section on AArch64 (every link has empty islands), and the D-4 test is
/// of `check_moved_code`, not of that bound (tested in the mutations).
fn no_islands(computed: &Layout) -> Layout {
    let mut c = computed.clone();
    c.placement.sized = Box::default();
    c
}

/// The moved mutation's answer: only that section's place differs, 16
/// bytes on.
fn moved_checks(
    label: &str,
    r: Result<Layout, Refusal>,
    computed: &Layout,
    fid: SectionId,
    pc: ChunkId,
    po: u64,
) {
    let l = r.unwrap_or_else(|e| panic!("{label}: moved: {e:?}"));
    assert_eq!(
        place_diff(&l, computed),
        vec![fid.0],
        "{label}: moved: the place of that section only"
    );
    assert_eq!(l.placement.place[fid].get(), Some((pc, po + 16)), "{label}: moved: its new offset");
    let mut back = l.clone();
    back.placement.place.as_mut_slice()[fid.0 as usize] = computed.placement.place[fid];
    assert_eq!(first_difference(&back, computed), None, "{label}: moved: nothing else");
}

/// The rows where `a` and `b` differ in `place`.
fn place_diff(a: &Layout, b: &Layout) -> Vec<u32> {
    let (p, q) = (a.placement.place.as_slice(), b.placement.place.as_slice());
    (0..p.len()).filter(|&i| p[i] != q[i]).map(|i| i as u32).collect()
}

fn refused(label: &str, case: &str, r: Result<Layout, Refusal>, needle: &str) -> String {
    match r {
        Ok(_) => panic!("{label}: {case}: accepted, want a refusal naming {needle:?}"),
        Err((reason, name)) => {
            assert!(
                name.contains(needle),
                "{label}: {case}: refused {} {name:?}, want {needle:?}",
                reason.report()
            );
            name
        }
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "one mutation after the other"
)]
fn materialize_mutations_differ_by_the_delta_or_refuse_by_name() {
    let mut reached: Vec<&str> = Vec::new();
    for line in programs() {
        let label = line.label.clone();
        with_link(&line, |view, bytes, s| {
            let rows = mortar_incr::state5::LayoutOf { view };
            let stored = StoredLayout::new(&rows);
            let computed = &s.f.layout;
            let (file, code) = own_file(view, s, &line.dir);
            let names: Vec<Vec<u8>> =
                stored.sections_of(file).expect("SECTIONS").into_iter().map(|r| r.name).collect();
            let (d, changes) = keep_file(view, &stored, file, &names);
            // A changed file whose sections stay where they are: the front's.
            let l = s
                .run(view, &d, &changes, computed)
                .unwrap_or_else(|e| panic!("{label}: kept: {e:?}"));
            assert_eq!(
                first_difference(&l, computed),
                None,
                "{label}: a changed file kept in place"
            );
            // F-1's bound (ruling Part 2 item 2), the taken branch: every
            // AArch64 link has its empty islands (`placement.sized`), and a
            // delta that moves no code keeps them.
            let aarch64 = label.starts_with("aarch64");
            assert_eq!(!computed.placement.sized.is_empty(), aarch64, "{label}: island rows");
            assert_eq!(l.placement.sized, computed.placement.sized, "{label}: islands kept");
            let fid =
                s.f.resolution.section_id(file, mortar_elf::SectionIndex(code)).expect("its id");
            let uid = changes.iter().find(|c| c.index == code).expect("its change").id;
            let (pc, po) = computed.placement.place[fid].get().expect("placed");

            // Moved: one changed section 16 bytes on in its chunk.
            let mut m = d.clone();
            m.placement.place.as_mut_slice()[uid.0 as usize] = {
                let (sc, so) = stored
                    .section(changes.iter().find(|c| c.id == uid).unwrap().old.unwrap())
                    .expect("row")
                    .expect("a row")
                    .place
                    .get()
                    .expect("placed");
                Place::at(sc, so + 16)
            };
            m.moved = vec![uid];
            let r = s.run(view, &m, &changes, computed);
            if aarch64 {
                // F-1's bound, the refused branch: an island row, and the
                // delta moves a code section.
                let name = refused(&label, "F-1: code moved", r, "F-1's bound");
                assert!(name.contains("d.moved"), "{label}: F-1 names the move: {name}");
                reached.push("F-1 bound");
                // The same move with the island rows dropped from the
                // front (x86-64's shape): taken (the checks below).
                let mut c = computed.clone();
                c.placement.sized = Box::default();
                let r = s.run(view, &m, &changes, &c).map(|mut l| {
                    l.placement.sized = computed.placement.sized.clone();
                    l
                });
                moved_checks(&label, r, computed, fid, pc, po);
            } else {
                moved_checks(&label, r, computed, fid, pc, po);
            }

            // Grown: one body chunk that no DEFINED row names, 16 bytes more.
            let def_chunks: Vec<ChunkId> = d
                .placement
                .defined
                .iter()
                .filter_map(|x| match x.value {
                    DefValue::At { chunk, .. } => Some(chunk),
                    DefValue::Absolute(_) => None,
                })
                .collect();
            let row = (0..d.placement.chunks.len())
                .map(ChunkId)
                .find(|&c| {
                    matches!(d.placement.chunks[c].kind, ChunkKind::Body(_))
                        && !def_chunks.contains(&c)
                })
                .expect("a body chunk");
            let mut g = mortar_incr::state5::LayoutOf { view };
            let _ = &mut g;
            let mut m = unchanged_delta(&stored).expect("empty");
            m.placement.chunks.as_mut_slice()[row.0 as usize].size += 16;
            let l =
                s.run(view, &m, &[], computed).unwrap_or_else(|e| panic!("{label}: grown: {e:?}"));
            let diff: Vec<usize> = (0..l.placement.chunks.len() as usize)
                .filter(|&i| {
                    l.placement.chunks.as_slice()[i] != computed.placement.chunks.as_slice()[i]
                })
                .collect();
            assert_eq!(diff.len(), 1, "{label}: grown: one chunk");
            let (a, b) = (
                &l.placement.chunks.as_slice()[diff[0]],
                &computed.placement.chunks.as_slice()[diff[0]],
            );
            assert_eq!(a.size, b.size + 16, "{label}: grown: by 16");
            let mut back = l.clone();
            back.placement.chunks.as_mut_slice()[diff[0]].size = b.size;
            assert_eq!(first_difference(&back, computed), None, "{label}: grown: nothing else");

            // A fragment of d.frags: a stored key at a new offset.
            let mem = mortar_incr::state5::read::decode(bytes).expect("decode");
            if let Some(fr) = mem.frags.iter().find(|f| f.refs > 0) {
                let mut m = unchanged_delta(&stored).expect("empty");
                m.frags = vec![mortar_core::incr::StoredFrag {
                    out: mortar_core::id::OutSecId(fr.out),
                    class: fr.class,
                    hash: fr.hash,
                    offset: fr.offset + 1,
                    size: fr.size,
                    refs: fr.refs,
                }];
                let l = s
                    .run(view, &m, &[], computed)
                    .unwrap_or_else(|e| panic!("{label}: frag: {e:?}"));
                let (p, q) = (l.merge.frag_off.as_slice(), computed.merge.frag_off.as_slice());
                let diff: Vec<usize> = (0..p.len()).filter(|&i| p[i] != q[i]).collect();
                assert!(!diff.is_empty(), "{label}: frag: a difference");
                for &i in &diff {
                    assert_eq!((q[i], p[i]), (fr.offset, fr.offset + 1), "{label}: frag {i}");
                }
                let mut back = l.clone();
                back.merge = computed.merge.clone();
                assert_eq!(first_difference(&back, computed), None, "{label}: frag: nothing else");
                // refs 0 for a key the front has live: a FINDING.
                let mut m0 = m.clone();
                m0.frags[0].offset = fr.offset;
                m0.frags[0].refs = 0;
                refused(
                    &label,
                    "frag refs 0",
                    s.run(view, &m0, &[], computed),
                    "FINDING frag_off: fragment",
                );
                reached.push("frag refs 0");
                // A key the front has dead, given refs: a FINDING.
                m0.frags[0].hash = [0xa5; 16];
                m0.frags[0].refs = 1;
                refused(
                    &label,
                    "frag dead",
                    s.run(view, &m0, &[], computed),
                    "FINDING frag_off: d.frags gives",
                );
                reached.push("frag reverse");
                // A live fragment with no row.
                let mut mem2 = mem.clone();
                let at = mem2.frags.iter().position(|f| f.refs > 0).expect("a row");
                mem2.frags.remove(at);
                let v2 = mem2.view();
                let e = unchanged_delta(&StoredLayout::new(&mortar_incr::state5::LayoutOf {
                    view: &v2,
                }))
                .expect("d");
                refused(&label, "no FRAGS row", s.run(&v2, &e, &[], computed), "has no row");
                reached.push("frag no row");
            }

            // ---- refusals of the delta's side.
            let empty = unchanged_delta(&stored).expect("empty");
            let mut x = empty.clone();
            let other = stored.sections_of(FileId(1)).ok().and_then(|r| r.first().map(|r| r.id));
            x.moved = vec![other.unwrap_or(SectionId(0))];
            refused(&label, "place (i)", s.run(view, &x, &[], computed), "place (i)");
            reached.push("place (i)");
            // (ii): the changed code section placed in another kind of chunk.
            let mut x = d.clone();
            let merged = (0..x.placement.chunks.len())
                .map(ChunkId)
                .find(|&c| !matches!(x.placement.chunks[c].kind, ChunkKind::Body(_)))
                .expect("a chunk that is not a body");
            x.placement.place.as_mut_slice()[uid.0 as usize] = Place::at(merged, 0);
            refused(
                &label,
                "place (ii)",
                s.run(view, &x, &changes, computed),
                "FINDING place (ii)",
            );
            reached.push("place (ii)");
            // Not placed where the front places it.
            let mut x = d.clone();
            x.placement.place.as_mut_slice()[uid.0 as usize] = Place::NONE;
            refused(
                &label,
                "place (ii) none",
                s.run(view, &x, &changes, computed),
                "FINDING place (ii)",
            );
            // A section of d.files with no change; one named twice.
            refused(&label, "no change", s.run(view, &d, &changes[1..], computed), "has no change");
            reached.push("no change");
            let mut twice = changes.clone();
            let mut c = twice[1];
            c.id = SectionId(c.id.0 + 1000);
            twice.push(c);
            refused(&label, "named twice", s.run(view, &d, &twice, computed), "named twice");
            reached.push("named twice");
            // CHUNKS count.
            let mut x = empty.clone();
            let mut v = x.placement.chunks.into_vec();
            v.pop();
            x.placement.chunks = mortar_core::column::Column::from_vec(v).expect("chunks");
            refused(
                &label,
                "chunk count",
                s.run(view, &x, &[], computed),
                "d.placement.chunks has",
            );
            reached.push("chunk count");
            // DEFINED: a value the delta did not follow.
            let mut x = empty.clone();
            if let Some(row) = x
                .placement
                .defined
                .iter_mut()
                .find(|r| matches!(r.value, DefValue::At { offset, .. } if offset > 0))
            {
                if let DefValue::At { offset, .. } = &mut row.value {
                    *offset += 4;
                }
                refused(&label, "defined", s.run(view, &x, &[], computed), "defined:");
                reached.push("defined");
            }
            // An update-only key.
            let mut x = empty.clone();
            let mut v = x.placement.defined.to_vec();
            let n = view.count(mortar_incr::format::kind::SYMBOLS) as u32;
            v.push(mortar_core::layout::Defined {
                symbol: SymbolId(n),
                value: DefValue::Absolute(0),
            });
            x.placement.defined = v.into();
            refused(&label, "update-only key", s.run(view, &x, &[], computed), "past SYMBOLS");
            reached.push("update-only key");
            // A DeadSlot hole.
            let mut x = empty.clone();
            x.hole_chunks = vec![row];
            x.placement.holes =
                vec![Hole { chunk: row, offset: 0, size: 8, kind: HoleKind::DeadSlot }].into();
            refused(&label, "dead slot hole", s.run(view, &x, &[], computed), "DeadSlot");
            reached.push("dead slot hole");
            // Slots: appended; retired.
            let slots = stored.slots().expect("SLOTS");
            if let Some(&first) = slots.first() {
                let mut x = empty.clone();
                x.slots_appended = vec![mortar_core::incr::StoredSlot { index: 9999, ..first }];
                refused(&label, "appended slot", s.run(view, &x, &[], computed), "appended");
                reached.push("appended slot");
                let mut x = empty.clone();
                x.slots_dead = vec![mortar_core::incr::StoredSlot { refs: 0, ..first }];
                refused(&label, "retired slot", s.run(view, &x, &[], computed), "retired");
                reached.push("retired slot");
            }

            // ---- refusals of the front's side.
            let mut c = computed.clone();
            c.slots.tls_ld = !c.slots.tls_ld;
            refused(&label, "tls_ld", s.run(view, &empty, &[], &c), "tls_ld");
            reached.push("tls_ld");
            let mut c = computed.clone();
            c.slots.needed = vec![FileId(1)].into();
            refused(&label, "dynamic", s.run(view, &empty, &[], &c), "dynamic table");
            reached.push("dynamic");
            let mut c = computed.clone();
            c.folds.folded = Some(mortar_core::column::Column::new());
            refused(&label, "folds", s.run(view, &empty, &[], &c), "folds");
            reached.push("folds");
            let mut c = computed.clone();
            // A front row whose size the delta's chunk does not have.
            let size = c.placement.chunks.get(ChunkId(0)).expect("a chunk").size + 1;
            c.placement.sized = vec![Sized { chunk: ChunkId(0), size }].into();
            refused(&label, "sized", s.run(view, &empty, &[], &c), "sized");
            reached.push("sized");
            // WP-225 M2b-5 item 2: the spans are the front's plan's (front
            // 2's S-88 takes no other), not the stored blocks.
            let mut c = computed.clone();
            if let Some(sp) = c.slots.symtab_locals.as_mut_slice().get_mut(file.0 as usize) {
                sp.len += 1;
                let l = s.run(view, &empty, &[], &c).unwrap_or_else(|e| {
                    panic!("{label}: symtab_locals: {e:?}");
                });
                assert_eq!(l.slots.symtab_locals, c.slots.symtab_locals, "{label}: the front's spans");
                reached.push("symtab_locals taken");
            }
            let name = |c: ChunkId| match computed.placement.chunks[c].kind {
                ChunkKind::Synthetic(s) => s.name.to_vec(),
                _ => Vec::new(),
            };
            let table = |c: ChunkId| matches!(&name(c)[..], b".symtab" | b".strtab");
            if let Some(&(ch, size)) = computed.slots.sizes.iter().find(|x| !table(x.0)) {
                let mut c = computed.clone();
                c.slots.sizes = vec![(ch, size.saturating_sub(1))].into();
                refused(&label, "sizes", s.run(view, &empty, &[], &c), "sizes:");
                reached.push("sizes");
                let mut c = computed.clone();
                c.slots.sizes = vec![(ch, size + (1 << 40))].into();
                let r = s.run(view, &empty, &[], &c);
                assert!(
                    matches!(r, Err((IncrReason::ReserveExhausted(_), _))),
                    "{label}: sizes past the capacity: {:?}",
                    r.err()
                );
                reached.push("sizes reserve");
            }
            // M2b-5 item 2: `.symtab`'s size from the front's plan within the
            // chunk's capacity (the chunk takes it); past it, by name.
            if let Some(&(ch, size)) = computed.slots.sizes.iter().find(|x| table(x.0)) {
                let mut c = computed.clone();
                let mut v = c.slots.sizes.to_vec();
                v.iter_mut().find(|x| x.0 == ch).expect("the row").1 = size - 24;
                c.slots.sizes = v.into();
                let l = s.run(view, &empty, &[], &c).unwrap_or_else(|e| {
                    panic!("{label}: a smaller .symtab: {e:?}");
                });
                assert_eq!(l.placement.chunks[ch].size, size - 24, "{label}: the chunk's size");
                assert!(l.slots.sizes.contains(&(ch, size - 24)), "{label}: the slot's size");
                reached.push("symtab size taken");
                let cap = computed.placement.chunks[ch].capacity;
                let mut c = computed.clone();
                let mut v = c.slots.sizes.to_vec();
                v.iter_mut().find(|x| x.0 == ch).expect("the row").1 = cap + 24;
                c.slots.sizes = v.into();
                let r = s.run(view, &empty, &[], &c);
                assert!(
                    matches!(&r, Err((IncrReason::ReserveExhausted(n), _)) if n == ".symtab" || n == ".strtab"),
                    "{label}: .symtab past its capacity: {:?}",
                    r.err()
                );
                reached.push("symtab size reserve");
            }
            if !computed.slots.eh_records.is_empty() {
                let mut c = computed.clone();
                let mut v = c.slots.eh_records.to_vec();
                v.pop();
                c.slots.eh_records = v.into();
                let r = s.run(view, &empty, &[], &c);
                assert!(
                    matches!(&r, Err((IncrReason::StateCorrupt(_), n)) if n.contains("eh_records")),
                    "{label}: eh_records: {:?}",
                    r.err()
                );
                reached.push("eh_records");
            }
            // Part 2 item 5: d.eh's range for a file with records, with one
            // record more than the front gives it: `(eh)` by name.
            if let Some(run) = s.eh.iter().find(|r| r.records > 0) {
                let mut x = empty.clone();
                x.eh = Some(mortar_core::incr::EhLayout {
                    chunk: ChunkId(0),
                    whole: false,
                    ranges: vec![mortar_core::incr::EhRange {
                        file: run.file,
                        offset: run.offset,
                        capacity: run.capacity,
                        records: vec![run.offset; run.records as usize + 1],
                    }],
                });
                let r = s.run(view, &x, &[], computed);
                assert!(
                    matches!(&r, Err((IncrReason::UpdateNotImplemented(Some(mortar_core::incr::Unimplemented::Eh)), n)) if n.contains("d.eh's range")),
                    "{label}: the record count: {:?}",
                    r.err()
                );
                reached.push("eh record count");
            }
            let mut c = computed.clone();
            let mut v = c.placement.redirects.to_vec();
            v.push(Redirect { section: fid, offset: 0, patch: 0 });
            c.placement.redirects = v.into();
            refused(&label, "redirect in d.files", s.run(view, &d, &changes, &c), "patches (ii)");
            reached.push("patches (ii)");

            // ---- refusals of the stored side (rows mutated).
            let mut mm = mem.clone();
            mm.files[1].mention += 1;
            let v2 = mm.view();
            refused(&label, "files", s.run(&v2, &empty, &[], computed), "files:");
            reached.push("files");
            let mut mm = mem.clone();
            let at =
                mm.sections.iter().position(|r| r.file == file.0 && r.size > 0).expect("a row");
            mm.sections[at].size += 1;
            let v2 = mm.view();
            refused(
                &label,
                "sections",
                s.run(&v2, &empty, &[], computed),
                "sections: SECTIONS row",
            );
            reached.push("sections");
            let mut mm = mem.clone();
            mm.chunks[0].ordinal += 7;
            let v2 = mm.view();
            refused(&label, "chunks", s.run(&v2, &empty, &[], computed), "no such chunk");
            reached.push("chunks");
            if let Some(dr) = mem.defined.first() {
                let mut mm = mem.clone();
                mm.symbols[dr.symbol as usize].name = mortar_incr::format::Str { off: 0, len: 0 };
                let v2 = mm.view();
                let e = unchanged_delta(&StoredLayout::new(&mortar_incr::state5::LayoutOf {
                    view: &v2,
                }))
                .expect("d");
                refused(&label, "globals", s.run(&v2, &e, &[], computed), "no such key");
                reached.push("globals");
            }
        });
    }
    reached.sort_unstable();
    reached.dedup();
    eprintln!("refusals reached: {reached:?}");
}

// ---- D-4 ----------------------------------------------------------------------------

/// `refill` on the first link's output with the delta `d`: its answer,
/// `Ok` or the refusal (`check_moved_code`'s is `(eh)`).
fn refill_answer(
    line: &Line,
    s: &Side<'_, '_>,
    view: &StateView<'_>,
    d: &LayoutDelta,
) -> Result<(), String> {
    use mortar_core::incr::{RefillSet, SymtabPlan, Unimplemented};
    let rows = mortar_incr::state5::LayoutOf { view };
    let stored = StoredLayout::new(&rows);
    let out = std::fs::File::open(line.dir.join("out")).expect("the first link's output");
    let leaves = view.leaves().expect("LEAVES");
    let store = support::store();
    let set = RefillSet::default();
    let symtab = SymtabPlan::default();
    let r = mortar::image::with_link(s.f, &store, &|_| {}, |l| {
        mortar_image::update::refill(l, d, &symtab, &set, &stored, &out, &leaves).map(|_| ())
    })
    .expect("the link's view");
    match r {
        Ok(()) => Ok(()),
        Err(mortar_image::update::RefillError::Fallback(IncrReason::UpdateNotImplemented(
            Some(Unimplemented::Eh),
        ))) => Err("(eh)".to_owned()),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// D-4 (ruling: ACCEPTED, the latent id-space bug): `check_moved_code`
/// reads `LayoutDelta::moved` through the LINK's `sh_index`. A changed
/// file's code section moved (its FDEs not rewritten: no `d.eh`) under its
/// UPDATE id (`first_section + k`, past every front section here) is
/// judged as no section at all and passes; the same delta in the front's
/// ids (`materialize`'s second output) names the code section and is
/// refused `(eh)`.
#[test]
fn check_moved_code_judges_the_front_section_with_the_front_delta() {
    let mut ran = 0;
    for line in programs() {
        let label = line.label.clone();
        with_link(&line, |view, _, s| {
            let rows = mortar_incr::state5::LayoutOf { view };
            let stored = StoredLayout::new(&rows);
            let (file, code) = own_file(view, s, &line.dir);
            let names: Vec<Vec<u8>> =
                stored.sections_of(file).expect("SECTIONS").into_iter().map(|r| r.name).collect();
            let (mut d, changes) = keep_file(view, &stored, file, &names);
            let c = changes.iter().find(|c| c.index == code).expect("its change");
            let (sc, so) = stored
                .section(c.old.expect("old"))
                .expect("row")
                .expect("a row")
                .place
                .get()
                .expect("placed");
            d.placement.place.as_mut_slice()[c.id.0 as usize] = Place::at(sc, so + 16);
            d.moved = vec![c.id];
            let fid =
                s.f.resolution.section_id(file, mortar_elf::SectionIndex(code)).expect("its id");
            assert!(
                s.f.resolution
                    .sh_index(c.id)
                    .is_none_or(|x| x != (file, mortar_elf::SectionIndex(code))),
                "{label}: the update id {} must not be the code section's front id {}",
                c.id.0,
                fid.0
            );
            // Today (update ids): the moved code section is not found.
            let today = refill_answer(&line, s, view, &d);
            assert!(today.is_ok(), "{label}: update ids: refill answered {today:?}");
            // The front's ids: found, refused by name.
            let (_, d_f) = s
                .run2(view, &d, &changes, &no_islands(&s.f.layout))
                .unwrap_or_else(|e| panic!("{label}: materialize: {e:?}"));
            assert_eq!(d_f.moved, vec![fid], "{label}: d_F.moved is the front's section");
            let fixed = refill_answer(&line, s, view, &d_f);
            assert_eq!(fixed, Err("(eh)".to_owned()), "{label}: front ids");
            println!("{label}: D-4: update id {} -> Ok; front id {} -> (eh)", c.id.0, fid.0);
            ran += 1;
        });
    }
    if tools_required() {
        assert_eq!(ran, 6, "every target and program");
    }
}
