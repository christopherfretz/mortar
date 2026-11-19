//! U-3's tests (`spec/incr.md` IN-71, IN-71a..c, IN-74; WP-215). Each
//! entry point is tested alone against a `StateView` over hand-made rows
//! (`MemoryState`), the stored link built by a test-local copy of
//! resolve.md §4.1 and the update checked against the same rule on the
//! new inputs.

#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

include!("support/resolve_model.rs");

use mortar_core::incr::{LayoutDelta, MemoryLayout, StoredLayout, StoredSection, StoredSlot};
use mortar_core::layout::{Chunk, ChunkKind, DefValue, Defined, Place, SlotKind};
use mortar_core::{ChunkId, Column, SymEntry, SymKey, SymbolId};
use mortar_incr::format::index_kind;
use mortar_incr::format::{ChunkRow, DefinedRow, IndexRow};
use mortar_incr::update::{defined, live, refill};

// ---- extraction (IN-71b) ------------------------------------------------------

// spec: incr.md IN-71b
#[test]
fn nothing_changed_extracts_nothing() {
    let mut l = Link::new();
    l.add(R, false, vec![undef("a")]);
    l.add(2 * R, true, vec![def("a")]);
    let x = check(&l, &l);
    assert_eq!(x, extract::Extraction::default());
}

// spec: incr.md IN-71b
#[test]
fn newly_extracted_member() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![undef("f")];
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m), 1)]);
}

// spec: incr.md IN-71b
#[test]
fn newly_extracted_member_pulls_another() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![undef("f")];
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m1), 1), (FileId(m2), 2)]);
}

// spec: incr.md IN-71b
#[test]
fn no_longer_extracted_member() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m)]);
}

// spec: incr.md IN-71b (WP-211 §D)
#[test]
fn member_cycle_dropped_together() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g"), undef("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m1), FileId(m2)]);
}

// spec: incr.md IN-71b (WP-211 §D: `start_group_member_cycle`)
#[test]
fn start_group_member_cycle() {
    // Two members referencing each other, one also referenced by a named
    // file: dropping the cycle's other in-edge keeps both.
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let b = old.add(R + 1, false, vec![undef("g")]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g"), undef("f")]);
    let mut new = old.clone();
    new.files[b as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert!(x.unextracted.is_empty(), "{x:?}");
    // m2 was round 1 (from b), now only m1 (round 1) supports it.
    assert_eq!(x.rounds, vec![(FileId(m2), 2)]);
    let _ = (a, m1);
}

// spec: incr.md IN-71b
#[test]
fn extracted_member_keeps_in_edge() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let _b = old.add(R + 2, false, vec![undef("f")]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    let x = check(&old, &new);
    assert_eq!(x, extract::Extraction::default());
    let _ = m;
}

// spec: incr.md IN-71b
#[test]
fn member_round_shortcut() {
    // m3 reached at round 3 through m1 -> m2 -> m3; the edit adds a direct
    // reference from a named file: m3 stays live (any supporting round).
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    old.add(2 * R, true, vec![def("f"), undef("g")]);
    old.add(3 * R, true, vec![def("g"), undef("h")]);
    old.add(4 * R, true, vec![def("h")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![undef("f"), undef("h")];
    check(&old, &new);
}

// spec: incr.md IN-71b (WP-211 §D)
#[test]
fn member_flips_back_on_third_hop() {
    // a drops its reference to f; m1 (f) goes, but m3 (reached from m1
    // via m2) is referenced by an appeared file: m2 goes, m3 stays.
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let m1 = old.add(2 * R, true, vec![def("f"), undef("g")]);
    let m2 = old.add(3 * R, true, vec![def("g"), undef("h")]);
    let m3 = old.add(4 * R, true, vec![def("h")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![];
    new.add(R + 5, false, vec![undef("h")]);
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m1), FileId(m2)]);
    // m3 keeps its stored round: the appeared file (round 0) supports it.
    assert!(x.rounds.is_empty(), "{m3}: {x:?}");
}

// spec: incr.md IN-71b, resolve.md §4.1 (weak definition before a member)
#[test]
fn weak_definition_vs_later_member() {
    // A weak definition in a named file BEFORE the member keeps it out;
    // one AFTER it does not (first(K) counts any definition).
    let mut old = Link::new();
    let a = old.add(R, false, vec![undef("f")]);
    let w = old.add(R + 1, false, vec![]);
    let m = old.add(2 * R, true, vec![def("f")]);
    let mut new = old.clone();
    new.files[w as usize].as_mut().unwrap().ments = vec![weak_def("f")];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m)]);
    // Weak definition after the member: the member stays.
    let mut old2 = Link::new();
    old2.add(R, false, vec![undef("f")]);
    let m2 = old2.add(2 * R, true, vec![def("f")]);
    let w2 = old2.add(3 * R, false, vec![]);
    let mut new2 = old2.clone();
    new2.files[w2 as usize].as_mut().unwrap().ments = vec![weak_def("f")];
    let x2 = check(&old2, &new2);
    assert!(x2.unextracted.is_empty(), "{m2}: {x2:?}");
    let _ = a;
}

// spec: incr.md IN-71b, WP-215 rank correction: an appended file with a
// middle rank decides first(K).
#[test]
fn appended_middle_rank_member_wins_extraction() {
    // Two members define f, ranks 2R and 4R; a member appended at 3R
    // (row past both) is not first; one appended at R + 1 (row last) is.
    let mut old = Link::new();
    old.add(R, false, vec![]);
    let m1 = old.add(2 * R, true, vec![def("f")]);
    let m2 = old.add(4 * R, true, vec![def("f")]);
    let a = 1;
    let mut new = old.clone();
    new.files[a].as_mut().unwrap().ments = vec![undef("f")];
    let mid = new.add(3 * R, true, vec![def("f")]);
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m1), 1)]);
    let mut new2 = old.clone();
    new2.files[a].as_mut().unwrap().ments = vec![undef("f")];
    let early = new2.add(R + 1, true, vec![def("f")]);
    let x2 = check(&old, &new2);
    assert_eq!(x2.extracted, vec![(FileId(early), 1)]);
    let _ = (m2, mid);
}

// spec: incr.md IN-71b (common pulls cfirst)
#[test]
fn common_pulls_strong_member() {
    let mut old = Link::new();
    let a = old.add(R, false, vec![]);
    let m = old.add(2 * R, true, vec![def("c")]);
    let mut new = old.clone();
    new.files[a as usize].as_mut().unwrap().ments = vec![common("c")];
    let x = check(&old, &new);
    assert_eq!(x.extracted, vec![(FileId(m), 1)]);
}

// spec: incr.md IN-71b (roots, §4.2)
#[test]
fn root_option_targets_member() {
    let mut old = Link::new();
    old.add(R, false, vec![]);
    let m = old.add(2 * R, true, vec![def("r")]);
    old.roots.push("r".into());
    // A changed named file now defines r first: the member is dropped.
    let mut new = old.clone();
    new.files[1].as_mut().unwrap().ments = vec![def("r")];
    let x = check(&old, &new);
    assert_eq!(x.unextracted, vec![FileId(m)]);
}

// spec: incr.md IN-71b ("Iterations": the stored link iterated)
#[test]
fn stored_link_iterated_is_fallback() {
    let l = Link::new();
    let mut keys = Vec::new();
    let mut state = l.state(&mut keys);
    state.meta.iterations = 2;
    let view = state.view();
    let r = extract::extract(&view, &ChangedFiles::default(), &OptionKeys::default());
    assert!(matches!(r, Err(IncrReason::ClosureIterated(_))), "{r:?}");
}

// spec: incr.md IN-71 "Versions" (WP-215 D8)
#[test]
fn versioned_old_or_new_is_fallback() {
    let l = Link::new();
    let mut keys = Vec::new();
    let mut state = l.state(&mut keys);
    state.meta.versioned = 1;
    {
        let view = state.view();
        let r = extract::extract(&view, &ChangedFiles::default(), &OptionKeys::default());
        assert_eq!(r.err(), Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Versioned))));
    }
    state.meta.versioned = 0;
    let view = state.view();
    let mut c = changed_file(
        1,
        Change::Appeared,
        Some(&F { rank: R, member: false, ments: vec![] }),
        0,
        &mut Vec::new(),
        0,
    );
    c.versioned = 1;
    let changed = ChangedFiles {
        files: vec![c],
        appeared: vec![FileId(1)],
        positional: vec![],
        strings: vec![],
    };
    let r = extract::extract(&view, &changed, &OptionKeys::default());
    assert_eq!(r.err(), Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Versioned))));
}

fn random_ments(rng: &mut Rng, keys: u64) -> Vec<M> {
    let mut v: Vec<M> = Vec::new();
    for _ in 0..rng.below(4) {
        let k = format!("k{}", rng.below(keys));
        if v.iter().any(|m| m.key == k) {
            continue;
        }
        v.push(match rng.below(8) {
            0..=2 => undef(&k),
            3 => weak_undef(&k),
            4 => weak_def(&k),
            5 => common(&k),
            _ => def(&k),
        });
    }
    v
}

// spec: incr.md IN-71b: the model's random series (each update's live set
// equals §4.1 on the new inputs; rounds support).
#[test]
fn extraction_random_series() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut runs = 0;
    for _ in 0..3000 {
        let keys = 2 + rng.below(6);
        let mut old = Link::new();
        let n = 2 + rng.below(7);
        for i in 0..n {
            let member = rng.below(3) != 0;
            let ments = random_ments(&mut rng, keys);
            old.add((i + 1) * R, member, ments);
        }
        if rng.below(4) == 0 {
            old.roots.push(format!("k{}", rng.below(keys)));
        }
        let mut new = old.clone();
        for _ in 0..=rng.below(3) {
            let f = 1 + rng.below(n) as usize;
            match rng.below(5) {
                0 => new.files[f] = None,
                1 => {
                    let rank = (1 + rng.below(n + 1)) * R - 1 - rng.below(1000);
                    let member = rng.below(2) == 0;
                    let ments = random_ments(&mut rng, keys);
                    new.add(rank, member, ments);
                }
                _ => {
                    if let Some(x) = new.files[f].as_mut() {
                        x.ments = random_ments(&mut rng, keys);
                    }
                }
            }
        }
        check(&old, &new);
        runs += 1;
    }
    assert_eq!(runs, 3000);
}

// spec: incr.md IN-71a (not built: WP-238): without `--gc-sections` the
// empty delta, with it `update-not-implemented (live)` from the option
// alone (WP-215 seam 3, G3)
#[test]
fn live_is_empty_without_gc_and_falls_back_under_it() {
    let m = MemoryState::default();
    let view = m.view();
    let changed = ChangedFiles::default();
    let resolved = resolve::ResolveDelta::default();
    let mut options = OptionKeys::default();
    assert_eq!(live::live(&view, &changed, &resolved, &options), Ok(live::LiveDelta::default()));
    options.gc_sections = true;
    assert_eq!(
        live::live(&view, &changed, &resolved, &options).err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Live)))
    );
}

// spec: incr.md IN-70 (WP-215 seam 3, G2): one reader for a `Str` of the
// update's id space; equality by bytes
#[test]
fn changed_files_string_reads_state_and_buffer() {
    let mut m = MemoryState::default();
    let a = m.intern(b".text.f");
    let view = m.view();
    let changed = ChangedFiles { strings: b".text.f.data".to_vec(), ..ChangedFiles::default() };
    let n = a.len;
    assert_eq!(changed.string(&view, a).unwrap(), b".text.f");
    // The same bytes at the buffer's offset: another `Str`, equal bytes.
    let b = Str { off: n, len: n };
    assert_ne!(a, b);
    assert_eq!(changed.string(&view, b).unwrap(), changed.string(&view, a).unwrap());
    assert_eq!(changed.string(&view, Str { off: n + 8, len: 4 }).unwrap(), b"data");
    let corrupt = |at| matches!(changed.string(&view, at), Err(IncrReason::StateCorrupt(_)));
    assert!(corrupt(Str { off: n - 1, len: 2 }), "straddles the boundary");
    assert!(corrupt(Str { off: n + 10, len: 3 }), "runs past the buffer");
}

// ---- the other entry points (until their tests) ---------------------------------

// spec: incr.md IN-71c, IN-74
#[test]
fn empty_update_recompute_and_refill_set() {
    let m = MemoryState::default();
    let view = m.view();
    let layout = LayoutDelta::default();
    assert!(defined::recompute(&view, &layout).is_empty());
    let resolved = resolve::ResolveDelta::default();
    let rows = MemoryLayout::default();
    let stored = StoredLayout::new(&rows);
    let set = refill::refill_set(&view, &ChangedFiles::default(), &resolved, &stored, &layout, &[])
        .unwrap();
    assert!(set.units.is_empty());
}

// spec: incr.md IN-71, IN-72 2a (WP-215 condition 1(b)): random small
// links with COMDAT groups of mismatched symbol sets, weak definitions
// behind strong ones, commons and appended middle-rank files; an edit; the
// stored rows from the first link. `resolve` is called at most twice and
// its delta equals the full link of the new inputs (definers, kinds,
// values, sizes, keepers, winning commons, `at`, KEY_REFS); every
// fall-back has a reason in the full links.
#[test]
fn resolve_random_series() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut ok, mut drawn, mut two, mut falls) = (0, 0, 0, BTreeMap::<String, u32>::new());
    let total = 4000;
    for _ in 0..total {
        let keys = 2 + rng.below(5);
        let mut old = Link::new();
        let n = 2 + rng.below(6);
        for i in 0..n {
            let member = rng.below(3) == 0;
            let ments = random_resolve_ments(&mut rng, keys);
            old.add((i + 1) * R, member, ments);
        }
        if rng.below(4) == 0 {
            old.roots.push(format!("k{}", rng.below(keys)));
        }
        let mut new = old.clone();
        for _ in 0..=rng.below(2) {
            random_edit(&mut rng, &mut new, n, keys);
        }
        let j = join(&old, &new);
        assert!(j.calls <= 2);
        if new.full().iterated {
            assert!(j.result.is_err(), "the new link iterates: old {old:#?}\nnew {new:#?}");
        }
        match &j.result {
            Ok(d) => {
                same_as_full(&j, d, &new, &format!("old {old:#?}\nnew {new:#?}\ndelta {d:#?}"));
                ok += 1;
                drawn += usize::from(!j.drawn.is_empty());
                two += usize::from(j.calls == 2);
            }
            Err(e) => {
                assert!(
                    may_fall_back(&old, &new, e),
                    "an unexplained fall-back {e:?}\nold {old:#?}\nnew {new:#?}\nOLDFULL {:?}\nNEWFULL {:?}",
                    old.full(),
                    new.full()
                );
                let mut why = format!("{e:?}").split('(').next().unwrap_or_default().to_owned();
                if matches!(e, IncrReason::UpdateNotImplemented(_)) {
                    why += if new.full().clash.is_empty() { " (undefined)" } else { " (clash)" };
                } else if matches!(e, IncrReason::ClosureIterated(_)) {
                    why += if old.full().iterated { " (stored)" } else { " (feed-back)" };
                }
                *falls.entry(why).or_default() += 1;
            }
        }
    }
    eprintln!("resolve_random_series: {ok} ok, {drawn} drew in, {two} two calls, {falls:?}");
    assert!(ok * 2 > total, "{ok} of {total} updated");
    assert!(drawn > 50, "{drawn} drew in");
}

// ---- linker-defined keys (IN-71c) -------------------------------------------

/// A link's linker-defined keys as `recompute` and `refill_set` see them:
/// the chunks `(addr, size)`, the `DEFINED` values by key name, and the
/// referrers: file `f` (1-based) has one allocated unit (section `f-1`)
/// naming the keys listed for it (`INDEX` kind `GLOBAL`, `KEY_REFS`).
#[derive(Debug, Clone)]
struct Defs {
    keys: Vec<&'static str>,
    chunks: Vec<(u64, u64)>,
    values: BTreeMap<&'static str, DefValue>,
    referrers: Vec<Vec<&'static str>>,
}

const fn at(chunk: u32, offset: u64) -> DefValue {
    DefValue::At { chunk: ChunkId(chunk), offset }
}

impl Defs {
    fn id(&self, k: &str) -> u32 {
        self.keys.iter().position(|x| *x == k).unwrap() as u32
    }

    fn state(&self) -> MemoryState {
        let mut m = MemoryState::default();
        let empty = m.intern(b"");
        for k in &self.keys {
            let name = m.intern(k.as_bytes());
            m.symbols.push(SymbolRow {
                kind: sym_kind::GLOBAL,
                file: NONE,
                ordinal: 0,
                name,
                version: empty,
                at_file: 0,
                at_index: 0,
                option: false,
            });
            m.resolved.push(ResolvedRow { kind: def_kind::INTERNAL, ..ResolvedRow::default() });
        }
        for &(addr, size) in &self.chunks {
            m.chunks.push(ChunkRow {
                sh_flags: mortar_elf::consts::SHF_ALLOC,
                size,
                capacity: size,
                addr,
                ..ChunkRow::default()
            });
        }
        for (k, v) in &self.values {
            let symbol = self.id(k);
            m.defined.push(match *v {
                DefValue::Absolute(value) => DefinedRow { symbol, kind: 0, chunk: 0, value },
                DefValue::At { chunk, offset } => {
                    DefinedRow { symbol, kind: 1, chunk: chunk.0, value: offset }
                }
            });
        }
        m.files.push(FileRow::default());
        for (i, keys) in self.referrers.iter().enumerate() {
            let file = i as u32 + 1;
            m.files.push(FileRow { kind: 1, rank: u64::from(file) * R, ..FileRow::default() });
            m.sections.push(SectionRow {
                file,
                sh_flags: mortar_elf::consts::SHF_ALLOC,
                size: 8,
                ..SectionRow::default()
            });
            for k in keys {
                let key = self.id(k);
                m.index.push((
                    FileId(file),
                    IndexRow { unit: i as u32, kind: index_kind::GLOBAL, target: key },
                ));
                m.key_refs.push(KeyRefRow { key, file, ..KeyRefRow::default() });
            }
        }
        m.key_refs.sort_by_key(|r| (r.key, r.file));
        m
    }

    /// The update to `next`: the keys `recompute` says changed and the
    /// units `refill_set` refills; `slots`: the IPLT slots appended and
    /// dead (IN-28b).
    fn hop(
        &self,
        next: &Defs,
        slots: (Vec<StoredSlot>, Vec<StoredSlot>),
    ) -> (BTreeSet<&'static str>, BTreeSet<u32>) {
        let state = self.state();
        let view = state.view();
        let chunks: Vec<Chunk> = next
            .chunks
            .iter()
            .map(|&(addr, size)| Chunk {
                kind: ChunkKind::FileHeader,
                sh_type: mortar_elf::consts::SHT_PROGBITS,
                flags: mortar_elf::consts::SHF_ALLOC,
                align: 3,
                size,
                capacity: size,
                addr: Some(addr),
                offset: addr,
                shndx: Some(1),
                relro: false,
            })
            .collect();
        let mut layout = LayoutDelta::default();
        layout.placement.chunks = Column::from_vec(chunks).unwrap();
        // Each referrer's unit: stored at chunk 0 (unchanged); no file is
        // placed by the update, so `place_of` answers from the stored rows
        // (the sparse placement, WP-216 seam 4: `placement.place` empty).
        let rows = MemoryLayout {
            sections: (0..self.referrers.len())
                .map(|i| StoredSection {
                    id: SectionId(i as u32),
                    file: FileId(i as u32 + 1),
                    index: 1,
                    anchor_kind: 0,
                    anchor: Vec::new(),
                    name: b".text".to_vec(),
                    ordinal: 0,
                    sh_type: mortar_elf::consts::SHT_PROGBITS,
                    sh_flags: mortar_elf::consts::SHF_ALLOC,
                    size: 8,
                    align: 1,
                    place: Place::at(ChunkId(0), 0),
                })
                .collect(),
            ..MemoryLayout::default()
        };
        let stored = StoredLayout::new(&rows);
        layout.placement.defined = next
            .values
            .iter()
            .map(|(k, v)| Defined { symbol: SymbolId(self.id(k)), value: *v })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        layout.placement.defined.sort_by_key(|d| d.symbol);
        (layout.slots_appended, layout.slots_dead) = slots;
        let changes = defined::recompute(&view, &layout);
        let resolved = resolve::ResolveDelta::default();
        let set = refill::refill_set(
            &view,
            &ChangedFiles::default(),
            &resolved,
            &stored,
            &layout,
            &changes,
        )
        .unwrap();
        let keys = changes.iter().map(|c| self.keys[c.key.0 as usize]).collect();
        (keys, set.units.iter().map(|u| u.section.0).collect())
    }
}

/// Every hop of `links` (a first update, then each one from the previous
/// update's result: the third hop included) refills exactly the
/// referrers of the keys named for it.
fn hops(links: &[Defs], want: &[&[&'static str]]) {
    for (i, w) in want.iter().enumerate() {
        let (keys, units) = links[i].hop(&links[i + 1], (Vec::new(), Vec::new()));
        let w: BTreeSet<&'static str> = w.iter().copied().collect();
        assert_eq!(keys, w, "hop {}: the keys recompute changes", i + 1);
        let refs: BTreeSet<u32> = links[i]
            .referrers
            .iter()
            .enumerate()
            .filter(|(_, ks)| ks.iter().any(|k| w.contains(k)))
            .map(|(u, _)| u as u32)
            .collect();
        assert_eq!(units, refs, "hop {}: the units refilled", i + 1);
    }
}

/// `.text` (0), `.init_array` (1), `.data` (2), `.bss` (3) with
/// `init_array` and `bss` bytes; the start-up symbols at their ends.
fn startup(init_array: u64, bss: u64) -> Defs {
    let text = (0x1000, 0x100);
    let ia = (0x2000, init_array);
    let data = (0x2000 + init_array.max(16), 32);
    let b = (data.0 + 32, bss);
    Defs {
        keys: vec![
            "_etext",
            "__init_array_start",
            "__init_array_end",
            "_edata",
            "__bss_start",
            "_end",
            "x",
        ],
        chunks: vec![text, ia, data, b],
        values: [
            ("_etext", at(0, 0x100)),
            ("__init_array_start", at(1, 0)),
            ("__init_array_end", at(1, init_array)),
            ("_edata", at(2, 32)),
            ("__bss_start", at(3, 0)),
            ("_end", at(3, bss)),
        ]
        .into_iter()
        .collect(),
        // libc-start.o, dl-minimal-malloc.o, a reader of `_etext`.
        referrers: vec![
            vec!["__init_array_start", "__init_array_end"],
            vec!["_end"],
            vec!["_etext", "_edata"],
        ],
    }
}

// spec: incr.md IN-71c (`init_array_grows`): `.init_array` grows past its
// capacity's slack (16 -> 24 bytes: `.data` and `.bss` move), a first
// update and a third hop.
#[test]
fn init_array_grows() {
    hops(
        &[startup(16, 64), startup(24, 64), startup(32, 64), startup(40, 64)],
        &[
            &["__init_array_end", "_edata", "__bss_start", "_end"],
            &["__init_array_end", "_edata", "__bss_start", "_end"],
            &["__init_array_end", "_edata", "__bss_start", "_end"],
        ],
    );
}

// spec: incr.md IN-71c (`init_array_shrinks`): in place (the chunks after
// it stay: `startup` keeps `.data` at 16 bytes past it).
#[test]
fn init_array_shrinks() {
    hops(
        &[startup(16, 64), startup(8, 64), startup(16, 64), startup(8, 64)],
        &[&["__init_array_end"], &["__init_array_end"], &["__init_array_end"]],
    );
}

// spec: incr.md IN-71c (`init_array_grows_then_shrinks`): back to the
// first link's values on the third hop, each hop against its own stored
// rows.
#[test]
fn init_array_grows_then_shrinks() {
    let moved: &[&str] = &["__init_array_end", "_edata", "__bss_start", "_end"];
    hops(
        &[startup(16, 64), startup(24, 64), startup(16, 64), startup(16, 64)],
        &[moved, moved, &[]],
    );
}

// spec: incr.md IN-71c (`bss_grows_moves_end`): `_end` moves, nothing
// else; its referrer (`dl-minimal-malloc.o`) is refilled.
#[test]
fn bss_grows_moves_end() {
    hops(
        &[startup(16, 64), startup(16, 96), startup(16, 96), startup(16, 128)],
        &[&["_end"], &[], &["_end"]],
    );
}

// spec: incr.md IN-71c (`defsym_follows_moved_symbol`, review-wp207 M-6):
// `--defsym x=_etext+4`; an edit that grows `.text` moves `_etext`, and
// `x` with it: `x`'s referrers are refilled.
#[test]
fn defsym_follows_moved_symbol() {
    let link = |text: u64| {
        let mut d = startup(16, 64);
        d.chunks[0].1 = text;
        d.values.insert("_etext", at(0, text));
        d.values.insert("x", at(0, text + 4));
        d.referrers.push(vec!["x"]);
        d
    };
    hops(
        &[link(0x100), link(0x110), link(0x110), link(0x120)],
        &[&["_etext", "x"], &[], &["_etext", "x"]],
    );
}

/// A registry section `reg` (chunk 4) with `__start_reg`/`__stop_reg`.
fn registry(size: u64) -> Defs {
    let mut d = startup(16, 64);
    d.keys.extend(["__start_reg", "__stop_reg"]);
    d.chunks.push((0x8000, size));
    d.values.insert("__start_reg", at(4, 0));
    d.values.insert("__stop_reg", at(4, size));
    d.referrers.push(vec!["__start_reg", "__stop_reg"]);
    d
}

// spec: incr.md IN-71c (`start_stop_registry_grows_then_shrinks`)
#[test]
fn start_stop_registry_grows_then_shrinks() {
    hops(
        &[registry(16), registry(32), registry(16), registry(16)],
        &[&["__stop_reg"], &["__stop_reg"], &[]],
    );
}

/// `.eh_frame_hdr` (chunk 4) at `addr` and `__GNU_EH_FRAME_HDR` at it.
fn eh_frame(addr: u64) -> Defs {
    let mut d = startup(16, 64);
    d.keys.push("__GNU_EH_FRAME_HDR");
    d.chunks.push((addr, 64));
    d.values.insert("__GNU_EH_FRAME_HDR", at(4, 0));
    d.referrers.push(vec!["__GNU_EH_FRAME_HDR"]);
    d
}

// spec: incr.md IN-71c (`eh_frame_overflow_then_edit_unchanged_file`,
// review-in77 B-3): the overflow moves `.eh_frame_hdr` (the key's chunk
// address, its offset unchanged); then an edit of a file that changes no
// linker-defined key; then the header moves again.
#[test]
fn eh_frame_overflow_then_edit_unchanged_file() {
    hops(
        &[eh_frame(0x6000), eh_frame(0x9000), eh_frame(0x9000), eh_frame(0xA000)],
        &[&["__GNU_EH_FRAME_HDR"], &[], &["__GNU_EH_FRAME_HDR"]],
    );
}

/// `.rela.iplt` (chunk 4) of `n` entries with `__rela_iplt_start/_end`.
fn iplt(n: u64) -> Defs {
    let mut d = startup(16, 64);
    d.keys.extend(["__rela_iplt_start", "__rela_iplt_end", "resolver"]);
    d.chunks.push((0x7000, 24 * n));
    d.values.insert("__rela_iplt_start", at(4, 0));
    d.values.insert("__rela_iplt_end", at(4, 24 * n));
    d.referrers.push(vec!["__rela_iplt_start", "__rela_iplt_end"]);
    d.referrers.push(vec!["resolver"]);
    d
}

// spec: incr.md IN-71c (`ifunc_added_moves_rela_iplt_end`): an IFUNC
// added (one more `.rela.iplt` entry) moves `__rela_iplt_end`.
#[test]
fn ifunc_added_moves_rela_iplt_end() {
    hops(&[iplt(1), iplt(2), iplt(2), iplt(3)], &[&["__rela_iplt_end"], &[], &["__rela_iplt_end"]]);
}

// spec: incr.md IN-71c, IN-28b (`local_ifunc_slot_dead_and_appended`,
// review-in72 m-5): a local IFUNC's IPLT slot dies (written 0, its entry
// stays) and another's is appended: `__rela_iplt_end` moves; a local
// slot names no global, so only the start-up code's unit is refilled for
// it; a global's appended slot refills that global's referrers.
#[test]
fn local_ifunc_slot_dead_and_appended() {
    let (a, b) = (iplt(1), iplt(2));
    let local = StoredSlot {
        kind: SlotKind::Iplt,
        key: SymKey::local(SymEntry { file: FileId(1), index: mortar_elf::SymbolIndex(3) }),
        index: 0,
        refs: 1,
    };
    let (keys, units) = a.hop(&b, (vec![local], vec![local]));
    assert_eq!(keys, ["__rela_iplt_end"].into_iter().collect());
    assert_eq!(units, [3].into_iter().collect(), "the start-up code's unit only");
    // Third hop: a global IFUNC's slot appended.
    let c = iplt(3);
    let global = StoredSlot {
        kind: SlotKind::Iplt,
        key: SymKey::global(SymbolId(b.id("resolver"))),
        index: 1,
        refs: 1,
    };
    let (keys, units) = b.hop(&c, (vec![global], Vec::new()));
    assert_eq!(keys, ["__rela_iplt_end"].into_iter().collect());
    assert_eq!(units, [3, 4].into_iter().collect(), "with the global's referrer");
}

// ---- resolution (IN-71, IN-72 item 2a) ---------------------------------------

/// The update `old` -> `new` succeeds and equals the full link of `new`;
/// the files it drew in.
fn hop_ok(old: &Link, new: &Link) -> (resolve::ResolveDelta, Vec<u32>) {
    let j = join(old, new);
    let d = j
        .result
        .clone()
        .unwrap_or_else(|e| panic!("update failed: {e:?}\nold {old:#?}\nnew {new:#?}"));
    same_as_full(&j, &d, new, &format!("old {old:#?}\nnew {new:#?}\ndelta {d:#?}"));
    assert!(j.calls <= 2);
    (d, j.drawn)
}

/// The key named `k` in `link`'s stored state.
fn key_of(link: &Link, k: &str) -> SymbolId {
    let mut keys = Vec::new();
    link.state(&mut keys);
    SymbolId(keys.iter().position(|x| x == k).unwrap() as u32)
}

// spec: incr.md IN-71, IN-72 2a (a COMDAT keeper change both ways,
// `comdat_keeper_flips_twice`): the first copy goes, the second file's
// copy is kept (drawn in: its section takes a new id); the copy comes
// back at a middle rank (appended, before the second file): it wins
// again; the third hop removes it again.
#[test]
fn comdat_keeper_flips_twice() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![weak_def("t").func().group("G").val(8, 4), undef("t")]);
    let f2 = a.add(2 * R, false, vec![weak_def("t").func().group("G").val(16, 4), def("u")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![undef("t")];
    let (d, drawn) = hop_ok(&a, &b);
    assert_eq!(drawn, vec![f2], "the new keeper is drawn in");
    assert_eq!(d.reelected.len(), 1);
    assert_eq!((d.reelected[0].1, d.reelected[0].2), (FileId(f1), FileId(f2)));
    let mut c = b.clone();
    let f3 = c.add(R + R / 2, false, vec![weak_def("t").func().group("G").val(24, 4)]);
    let (d, drawn) = hop_ok(&b, &c);
    assert_eq!(drawn, vec![f2], "the old keeper is drawn in");
    assert_eq!((d.reelected[0].1, d.reelected[0].2), (FileId(f2), FileId(f3)));
    let mut e = c.clone();
    e.files[f3 as usize] = None;
    let (d, _) = hop_ok(&c, &e);
    assert_eq!((d.reelected[0].1, d.reelected[0].2), (FileId(f3), FileId(f2)));
}

// spec: incr.md IN-71 (ruling D2-6, `ResolveDelta::keepers`): a changed
// file that ADDS a COMDAT group (a new signature: no `reelected` row, no
// old keeper) and one whose group's keeper MOVES (the copy goes from the
// first file, the second keeps it): `keepers` names each signature of the
// changed file's sections with the full link's keeper (`same_as_full`
// checks every hop of every series against the model's full link).
#[test]
fn keepers_of_an_added_group_and_a_moved_keeper() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("main"), undef("h")]);
    let f2 = a.add(2 * R, false, vec![weak_def("h").func().group("H").val(8, 4)]);
    // f1 gains a copy of `H` (it becomes the keeper: lower rank) and a new
    // group `N` no file had.
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![
        def("main"),
        weak_def("h").func().group("H").val(4, 4),
        weak_def("n").func().group("N").val(12, 4),
    ];
    let (d, _) = hop_ok(&a, &b);
    let keepers: Vec<(&[u8], Option<FileId>)> =
        d.keepers.iter().map(|(s, k)| (s.as_slice(), *k)).collect();
    assert_eq!(keepers, vec![(&b"H"[..], Some(FileId(f1))), (&b"N"[..], Some(FileId(f1)))]);
    assert!(d.reelected.iter().all(|r| r.0 != b"N"), "a new signature is not re-elected");
    // f1 drops both copies: `H` moves back to f2, which is drawn in (its
    // sections take new ids) and names it: `keepers` has f2's `H` only.
    let mut c = b.clone();
    c.files[f1 as usize].as_mut().unwrap().ments = vec![def("main"), undef("h")];
    let (d, drawn) = hop_ok(&b, &c);
    assert_eq!(drawn, vec![f2], "the new keeper is drawn in");
    let keepers: Vec<(&[u8], Option<FileId>)> =
        d.keepers.iter().map(|(s, k)| (s.as_slice(), *k)).collect();
    assert_eq!(keepers, vec![(&b"H"[..], Some(FileId(f2)))]);
    assert_eq!((d.reelected[0].1, d.reelected[0].2), (FileId(f1), FileId(f2)));
    // f2 (the keeper) edited, keeping its copy: H's keeper is f2.
    let mut e = c.clone();
    e.files[f2 as usize].as_mut().unwrap().ments =
        vec![weak_def("h").func().group("H").val(8, 8), def("x")];
    let (d, _) = hop_ok(&c, &e);
    let keepers: Vec<(&[u8], Option<FileId>)> =
        d.keepers.iter().map(|(s, k)| (s.as_slice(), *k)).collect();
    assert_eq!(keepers, vec![(&b"H"[..], Some(FileId(f2)))]);
}

// spec: incr.md IN-71 (`common_first_referenced_by_changed_file`,
// review-in77 B-4): a changed file before the winning common's file
// gains a common of the key (it wins: the first by rank, the largest
// size) and, on the next hop, only a reference (the winner goes back,
// `at` stays with the changed file).
#[test]
fn common_first_referenced_by_changed_file() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("main")]);
    a.add(2 * R, false, vec![M { value: 8, size: 8, ..common("c") }]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments =
        vec![def("main"), M { value: 4, size: 32, ..common("c") }];
    let (d, drawn) = hop_ok(&a, &b);
    assert!(drawn.is_empty(), "a winning common is never drawn in: {drawn:?}");
    let c = key_of(&a, "c");
    let row = d.resolved.iter().find(|(k, _)| *k == c).unwrap().1;
    assert_eq!((row.kind, row.size, row.value), (def_kind::COMMON, 32, 4));
    let mut e = b.clone();
    e.files[f1 as usize].as_mut().unwrap().ments = vec![def("main"), undef("c")];
    let (d, drawn) = hop_ok(&b, &e);
    assert!(drawn.is_empty(), "the common's value is its stored alignment: {drawn:?}");
    let row = d.resolved.iter().find(|(k, _)| *k == c).unwrap().1;
    assert_eq!((row.kind, row.size, row.value), (def_kind::COMMON, 8, 8));
}

// spec: incr.md IN-71 (`first_mentioner_drops_then_readds_key`, three
// hops, review-in77 B-4): `at` follows the first live mentioner by rank.
#[test]
fn first_mentioner_drops_then_readds_key() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![undef("k"), def("x")]);
    a.add(2 * R, false, vec![def("k")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("x")];
    hop_ok(&a, &b);
    hop_ok(&b, &a);
    hop_ok(&a, &b);
}

// spec: incr.md IN-71 (`key_refs_single_mentioner`): the one file that
// mentions a key drops it (the key is gone, its KEY_REFS row deleted),
// then mentions it again (a new key).
#[test]
fn key_refs_single_mentioner() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("only"), def("x")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("x")];
    let (d, _) = hop_ok(&a, &b);
    assert_eq!(d.gone, vec![key_of(&a, "only")]);
    let (d, _) = hop_ok(&b, &a);
    assert_eq!(d.new_keys.len(), 1);
    assert_eq!(d.new_keys[0].name, b"only".to_vec());
}

// spec: incr.md IN-71 (a common's stored value; WP-215 R.6 5, ruled): an
// UNCHANGED file whose common has alignment 0 (`st_value` 0) becomes the
// winner: its value is not stored, `COMMONS.align` holds 1, and the full
// link writes `max(1, st_value)` (`tables::resolved_row`): the update's row
// equals the full link's, value 1.
#[test]
fn unchanged_align0_common_becomes_winner() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![common("c").val(8, 8), def("x")]);
    a.add(2 * R, false, vec![common("c").val(0, 4)]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("x")];
    let (d, drawn) = hop_ok(&a, &b);
    assert!(drawn.is_empty(), "never a draw-in for a common");
    let row = d.resolved.iter().find(|(k, _)| *k == key_of(&a, "c")).unwrap().1;
    assert_eq!((row.kind, row.value, row.size), (def_kind::COMMON, 1, 4));
    // And back: a changed file's alignment-0 common wins: 1 as well.
    let mut c = a.clone();
    c.files[f1 as usize].as_mut().unwrap().ments = vec![common("c").val(0, 2), def("x")];
    let (d, _) = hop_ok(&a, &c);
    let row = d.resolved.iter().find(|(k, _)| *k == key_of(&a, "c")).unwrap().1;
    assert_eq!((row.value, row.size), (1, 4));
}

// spec: incr.md IN-71b "Live keys" (WP-215 R.6 3, ruled): a DEAD key (no
// stored mention by a live file, no `LAZY` definition, no option) is inert
// to every reader of the update, and found and reused when it gets a
// mention again. `d` is dead in the stored state: only the `LAZY`
// member's undefined mention names it (not stored). (1) An edit of
// another file: `d` is not affected, not gone, not a reason for a
// draw-in, no fall-back (no undefined report), its row stays undefined.
// (2) The lazy member itself changes (dropping its `d`): `d` stays as
// it was (dead), not affected. (3) A live file defines `d`: its stored
// row is reused (no new key). (4) The same from a `GONE` row (a dead key
// an update left): reused. Re-election reads `GROUPS` by signature, never
// a key's rows, so a dead key never reaches it.
#[test]
fn dead_key_is_inert_and_reused() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("x")]);
    let m = a.add(2 * R, true, vec![def("z"), undef("d")]);
    let d_key = key_of(&a, "d");
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("x").val(8, 8)];
    let (delta, drawn) = hop_ok(&a, &b);
    assert!(drawn.is_empty());
    assert!(!delta.affected.contains(&d_key), "a dead key is not affected");
    assert!(!delta.gone.contains(&d_key));
    assert!(delta.resolved.iter().all(|(k, _)| *k != d_key));
    // (2) the lazy member changes.
    let mut c = a.clone();
    c.files[m as usize].as_mut().unwrap().ments = vec![def("z").val(4, 4)];
    let (delta, drawn) = hop_ok(&a, &c);
    assert!(drawn.is_empty());
    assert!(!delta.affected.contains(&d_key) && !delta.gone.contains(&d_key));
    // (3) a live file defines it: the stored row, not a new key.
    let mut e = a.clone();
    e.files[f1 as usize].as_mut().unwrap().ments = vec![def("x"), def("d")];
    let (delta, _) = hop_ok(&a, &e);
    assert!(delta.new_keys.is_empty(), "{:?}", delta.new_keys);
    assert!(delta.affected.contains(&d_key));
    let row = delta.resolved.iter().find(|(k, _)| *k == d_key).unwrap().1;
    assert_eq!(row.kind, def_kind::SECTION);
    // (4) from a GONE row (as an update leaves it).
    let j = join_patched(&a, &e, |st, keys| {
        let k = keys.iter().position(|x| x == "d").unwrap();
        st.symbols[k].kind = sym_kind::GONE;
    });
    let delta = j.result.clone().unwrap();
    same_as_full(&j, &delta, &e, "GONE d defined again");
    assert!(delta.new_keys.is_empty() && delta.affected.contains(&d_key));
}

// spec: incr.md IN-71 ("first", "least" by CURRENT rank; WP-215 rulings):
// an appended file at a middle rank defines a strong `k` and wins over
// the later stored definer; a weak one loses to the stored strong one.
#[test]
fn appended_middle_rank_definer_wins() {
    let mut a = Link::new();
    a.add(R, false, vec![undef("k")]);
    a.add(3 * R, false, vec![def("k").val(8, 8)]);
    let mut b = a.clone();
    let f = b.add(2 * R, false, vec![def("w").val(0, 0)]);
    hop_ok(&a, &b);
    let mut c = a.clone();
    c.add(2 * R, false, vec![weak_def("k").val(4, 4)]);
    hop_ok(&a, &c);
    // A strong definition at the middle rank and another at rank 3R
    // clash: the full link reports it; the update falls back by name.
    b.files[f as usize].as_mut().unwrap().ments = vec![def("k").val(4, 4)];
    assert_eq!(
        join(&a, &b).result.err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)))
    );
}

// spec: incr.md IN-71, IN-72 2a: the defining file changes to an
// unchanged file (its value and size are not stored): it is drawn in,
// once; the second call asks for nothing.
#[test]
fn unchanged_new_definer_is_drawn_in() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("k").val(8, 8), undef("u")]);
    let f2 = a.add(2 * R, false, vec![weak_def("k").val(16, 4), def("u")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![undef("k"), undef("u")];
    let (_, drawn) = hop_ok(&a, &b);
    assert_eq!(drawn, vec![f2]);
}

// spec: incr.md IN-71, S-74 (IN-72 item 2a): a hidden reference added by
// a changed file makes an unchanged definer's global hidden (a local of
// its block): the definer is drawn in.
#[test]
fn hidden_reference_draws_in_definer() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![undef("k")]);
    let f2 = a.add(2 * R, false, vec![def("k").val(8, 8)]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![undef("k").hidden()];
    let (_, drawn) = hop_ok(&a, &b);
    assert_eq!(drawn, vec![f2]);
    let (_, drawn) = hop_ok(&b, &a);
    assert_eq!(drawn, vec![f2], "and back");
}

// spec: incr.md IN-71b, §7.1 (`wrap_of_member_symbol`, WP-211): with
// `--wrap=foo` a reference to `foo` extracts the member defining
// `__wrap_foo`, not the one defining `foo`; an edit to `__real_foo`
// extracts the latter and drops the former.
#[test]
fn wrap_of_member_symbol() {
    let mut a = Link::new();
    a.wrap.push("foo".into());
    let f1 = a.add(R, false, vec![undef("foo")]);
    let real = a.add(2 * R, true, vec![def("foo")]);
    let wrap = a.add(3 * R, true, vec![def("__wrap_foo")]);
    let full = a.full();
    assert!(full.live.contains_key(&wrap) && !full.live.contains_key(&real));
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![undef("__real_foo")];
    let x = check(&a, &b);
    assert_eq!(x.extracted.iter().map(|e| e.0.0).collect::<Vec<_>>(), vec![real]);
    assert_eq!(x.unextracted.iter().map(|f| f.0).collect::<Vec<_>>(), vec![wrap]);
    hop_ok(&a, &b);
    hop_ok(&b, &a);
}

// spec: incr.md IN-71b, §4.1 step 1, §7.2 (`assign_candidate_vs_member`,
// WP-211: script assignments are refused under incremental mode (`-T`),
// so the candidate the update meets is `--defsym`'s): a `--defsym` key
// is not a target, so a reference added to it extracts no member; its
// row stays INTERNAL.
#[test]
fn assign_candidate_vs_member() {
    let mut a = Link::new();
    a.defsym.push("x".into());
    let f1 = a.add(R, false, vec![def("main")]);
    let m = a.add(2 * R, true, vec![def("x")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("main"), undef("x")];
    let x = check(&a, &b);
    assert!(x.extracted.is_empty(), "{x:?}");
    assert!(!b.full().live.contains_key(&m));
    let (d, _) = hop_ok(&a, &b);
    let row = d.resolved.iter().find(|(k, _)| *k == key_of(&a, "x")).unwrap().1;
    assert_eq!(row.kind, def_kind::INTERNAL);
}

// spec: incr.md IN-71b (`extraction_bound_is_extraction_changed`): more
// members than `FLIP_BOUND` flipping is `extraction-changed`; exactly
// the bound is an update.
#[test]
fn extraction_bound_is_extraction_changed() {
    for (n, falls) in [(extract::FLIP_BOUND, false), (extract::FLIP_BOUND + 1, true)] {
        let mut a = Link::new();
        let f1 = a.add(R, false, vec![]);
        for i in 0..n {
            a.add(2 * R + i as u64, true, vec![def(&format!("m{i}"))]);
        }
        let mut b = a.clone();
        b.files[f1 as usize].as_mut().unwrap().ments =
            (0..n).map(|i| undef(&format!("m{i}"))).collect();
        let r = update(&a, &b);
        if falls {
            assert!(matches!(r, Err(IncrReason::ExtractionChanged(_))), "{n}: {:?}", r.err());
        } else {
            assert_eq!(r.unwrap().0.len(), n + 2, "{n}: every member extracted");
        }
    }
}

// spec: incr.md IN-71, WP-215 §Q.2 (`affected_key_feeds_back_is_fallback`):
// an edit makes a live file's definition of an affected key, its
// `first`, a dropped COMDAT copy (the WP-200 shape: wb.o's `hx` in its
// copy of G, which wa.o now keeps): the full link iterates, the update
// is `closure-iterated`.
#[test]
fn affected_key_feeds_back_is_fallback() {
    let mut a = Link::new();
    a.add(R, false, vec![undef("Y"), undef("g"), undef("hx")]);
    let wa = a.add(2 * R, false, vec![def("z")]);
    a.add(
        3 * R,
        false,
        vec![weak_def("g").func().group("G"), weak_def("hx").func().group("G"), def("Y")],
    );
    a.add(4 * R, true, vec![def("hx")]);
    assert!(!a.full().iterated);
    let mut b = a.clone();
    b.files[wa as usize].as_mut().unwrap().ments = vec![weak_def("g").func().group("G")];
    assert!(b.full().iterated, "the WP-200 shape");
    assert!(matches!(join(&a, &b).result, Err(IncrReason::ClosureIterated(_))));
}

// spec: incr.md IN-71 (WP-215 D8, condition 3 of seam 3's review):
// `-u foo@V` names the key `(foo, V)`, never `foo`. Static only: no
// shared object defines it, so it is an undefined key the root refers to;
// an edit of `foo`'s definer updates `foo` and leaves `(foo, V)`
// unaffected. With a shared object's `foo@V` (the stored row SHARED, the
// file of kind 2: a dynamic link, which incremental mode refuses; the
// rows are forced here), an edit that makes the key affected is
// `update-not-implemented (resolve)` by name.
#[test]
fn undefined_versioned_root() {
    let mut a = Link::new();
    a.roots.push("foo@V".into());
    let f1 = a.add(R, false, vec![def("foo").val(0, 4), def("x")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("foo").val(4, 4), def("x")];
    let (d, _) = hop_ok(&a, &b);
    let root = key_of(&a, "foo@V");
    assert!(d.resolved.iter().all(|(k, _)| *k != root), "the root's key is not affected");
    let foo = key_of(&a, "foo");
    assert_eq!(d.resolved.iter().find(|(k, _)| *k == foo).unwrap().1.value, 4);
    // A shared object (file 2) defines `foo@V`; file 1 now mentions it.
    let mut a = a.clone();
    let so = a.add(2 * R, false, vec![]);
    let shared = |m: &mut MemoryState, keys: &[String]| {
        let k = keys.iter().position(|x| x == "foo@V").unwrap() as u32;
        m.files[so as usize].kind = 2;
        m.mentions.push((
            FileId(so),
            MentionRow {
                symbol: k,
                kind: def_kind::SHARED,
                bind: GLOBAL,
                typ: FUNC,
                other: 0,
                section: NONE,
            },
        ));
        m.key_refs.push(KeyRefRow {
            key: k,
            file: so,
            def: def_kind::SHARED,
            bind: GLOBAL,
            typ: FUNC,
            flags: 0,
            tomb: false,
            demand: 0,
        });
        m.key_refs.sort_by_key(|r| (r.key, r.file));
        m.resolved[k as usize] = ResolvedRow {
            kind: def_kind::SHARED,
            value: 0,
            section: NONE,
            ..ResolvedRow::default()
        };
    };
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![def("foo").val(4, 4), def("x")];
    assert!(join_patched(&a, &b, shared).result.is_ok(), "an edit not naming it updates");
    // The shared object itself changes: its key is affected.
    let mut b = a.clone();
    b.files[so as usize].as_mut().unwrap().ments = vec![def("y")];
    assert_eq!(
        join_patched(&a, &b, shared).result.err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))),
        "a shared definer's key affected"
    );
}

// spec: incr.md IN-71 (WP-215 condition 1(c), the U-10 line): a key whose
// definer becomes a shared object (`Def::Shared`, a stored SHARED row or
// a kind-2 mention) is `update-not-implemented (resolve)`: the update
// does not run the dynamic stage (spec/dyn.md D-11); static only today.
#[test]
fn shared_definer_is_not_implemented() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("k"), undef("u")]);
    let so = a.add(2 * R, false, vec![def("u")]);
    let shared = |m: &mut MemoryState, keys: &[String]| {
        let k = keys.iter().position(|x| x == "k").unwrap() as u32;
        m.files[so as usize].kind = 2;
        m.mentions.push((
            FileId(so),
            MentionRow {
                symbol: k,
                kind: def_kind::SHARED,
                bind: GLOBAL,
                typ: FUNC,
                other: 0,
                section: NONE,
            },
        ));
        m.key_refs.push(KeyRefRow {
            key: k,
            file: so,
            def: def_kind::SHARED,
            bind: GLOBAL,
            typ: FUNC,
            flags: 0,
            tomb: false,
            demand: 0,
        });
        m.key_refs.sort_by_key(|r| (r.key, r.file));
    };
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![undef("k"), undef("u")];
    assert_eq!(
        join_patched(&a, &b, shared).result.err(),
        Some(IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve)))
    );
}

// spec: incr.md IN-71 (`at`: the U-2 finding): `at` is the FIRST LIVE
// mentioner by rank (IN-71's words), not the first of all inputs (the
// capture's `first_mentions`): a lazy member mentioning `k` before the
// live file does not hold it. Under the other reading this test fails:
// `at` would be the member's.
#[test]
fn at_is_first_live_mentioner() {
    let mut a = Link::new();
    let m = a.add(R, true, vec![def("k"), def("other")]);
    let f2 = a.add(2 * R, false, vec![weak_undef("k"), def("x")]);
    let mut b = a.clone();
    b.files[f2 as usize].as_mut().unwrap().ments = vec![def("x"), weak_undef("k")];
    let (d, _) = hop_ok(&a, &b);
    let k = key_of(&a, "k");
    let at: Vec<(u32, u32)> = d.at.iter().filter(|x| x.0 == k).map(|x| (x.1, x.2)).collect();
    assert_eq!(at, vec![(f2, 1)], "the live file's entry, not member {m}'s");
}

// spec: incr.md IN-71 (`symbol-kind-changed`): a definition that turns
// from a section's into an absolute one while an unchanged file refers to
// the key is `symbol-kind-changed`.
#[test]
fn section_to_absolute_is_symbol_kind_changed() {
    let mut a = Link::new();
    let f1 = a.add(R, false, vec![def("k").val(8, 4)]);
    a.add(2 * R, false, vec![undef("k")]);
    let mut b = a.clone();
    b.files[f1 as usize].as_mut().unwrap().ments = vec![abs("k", 0x1234)];
    assert!(matches!(join(&a, &b).result, Err(IncrReason::SymbolKindChanged(_))));
}
