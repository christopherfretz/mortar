//! WP-225 M2b-2b: the join's placement.
//!
//! - Ruling M2b2a-1: `front_of_changed` reads an `Ids::Update` row's
//!   `path` through `ChangedFiles::string` (the update's string space), so
//!   an APPEARED file (no stored row; its path a string of the update's
//!   buffer, past `STRINGS`) opens; `front_of` (the state's `STRINGS`
//!   only) refuses the same row `(resolve)`.
//!
//! - M2b-2c, finding M2b2b-3: a same-size edit of one function (the
//!   identity case: the full link of the new inputs places the file where
//!   the stored layout has it). The state the first link writes holds NO
//!   `REGIONS` row (no full-link producer), so `delta::layout` cannot keep
//!   a section in its file's region: it refuses by name
//!   (`update-not-implemented (layout)`) instead of sending every section
//!   to the overflow phase at the output section's tail (the lane's
//!   `.text: update 0x86f20, full 0xc70` and its growth step). Given the
//!   first link's regions (a TEST-ONLY derivation from the stored places
//!   and the `Fill` hole after each run, `chunk.rs`'s IN-30 rule), it keeps
//!   every section at the full link's offset, moves nothing and takes no
//!   growth step.
//!
//! On AArch64 and x86-64.

#![allow(dead_code, reason = "support/update.rs's helpers that other tests use")]

mod support;
#[path = "support/update.rs"]
mod update;

use mortar::update::file_front::{FrontCache, Ids, front_of, front_of_changed};
use mortar::update::layout_in::layout_inputs;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId};
use mortar_core::incr::{
    FileDemands, IncrReason, LayoutInput, LayoutRows, PlaceNow, StoredEhRange, StoredFrag,
    StoredFragUse, StoredKeyRef, StoredLayout, StoredRegion, StoredSection, StoredSlot,
    SymtabBlock, SymtabTables, Unimplemented,
};
use mortar_core::layout::{
    Chunk, ChunkKind, Defined, Hole, HoleKind, Patch, Redirect, Segment, SymPlace, Tls,
};
use mortar_elf::consts::SHF_EXECINSTR;
use mortar_elf::{Machine, SectionIndex};
use mortar_incr::format::Str;
use mortar_incr::update::rows::FileRow;
use mortar_incr::update::view::ChangedFiles;
use mortar_merge::phase::ArchRelocs;
use update::{first_free, lines, member, pool, same_file, state, whole};

#[test]
fn an_appeared_file_opens_through_the_update_strings() {
    for line in lines(&[]) {
        line.archive(&["m1.o", "m2.o"]);
        let view = state(&line, &line.args(&["wp255d_m2", "wp255d_m1"]), "ap");
        // `m1e.o`: named on no line of the state, its path a string the
        // state does not hold.
        let path = b"m1e.o".to_vec();
        let bytes = std::fs::read(line.dir.join("m1e.o")).expect("m1e.o");
        let base = u32::try_from(view.count(mortar_incr::format::kind::STRINGS)).expect("STRINGS");
        let changed = ChangedFiles { strings: path.clone(), ..ChangedFiles::default() };
        let len = u32::try_from(path.len()).expect("a short path");
        let row = FileRow {
            path: Str { off: base, len },
            len: bytes.len() as u64,
            hash: mortar_input::store::content_hash(&bytes).0.to_le_bytes(),
            rank: u64::from(u32::MAX) << 20,
            ..FileRow::default()
        };
        assert_eq!(changed.string(&view, row.path).expect("the path"), path, "{}", line.label);
        let file = FileId(u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("F"));
        let first = SectionId(first_free(&view));
        let ids = Ids::Update { first_section: first, row: row.clone() };

        // The state's strings alone: the path is past them.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let r = front_of(&mut cache, &view, file, ids.clone()).map(|x| x.file);
        let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve));
        assert_eq!(r, Err(want), "{}: front_of", line.label);

        // Through the update's strings: it opens, with the update's ids.
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff = front_of_changed(&mut cache, &view, &changed, file, ids)
            .unwrap_or_else(|e| panic!("{}: front_of_changed: {}", line.label, e.report()));
        assert_eq!(ff.file, file, "{}", line.label);
        assert_eq!(ff.row, row, "{}: the row it was given", line.label);
        let object = mortar_elf::Object::parse(&bytes).expect("m1e.o parses");
        assert_eq!(ff.sections.len(), object.section_count() as usize, "{}", line.label);
        for (k, s) in ff.sections.iter().enumerate() {
            assert_eq!(s.id, SectionId(first.0 + k as u32), "{}: section {k}", line.label);
            assert!(s.place.is_none(), "{}: an appeared section has no stored place", line.label);
        }
        assert!(
            ff.keys.iter().any(|k| k.name == b"wp255d_m1" && k.key.is_some()),
            "{}: its keys are the state's by name",
            line.label
        );
        // A string past the update's buffer is refused, not read.
        let short = ChangedFiles { strings: b"m1e".to_vec(), ..ChangedFiles::default() };
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ids = Ids::Update { first_section: first, row };
        let r = front_of_changed(&mut cache, &view, &short, file, ids).map(|x| x.file);
        assert!(matches!(r, Err(IncrReason::StateCorrupt(_))), "{}: {r:?}", line.label);
    }
}

/// `m1.o` with one instruction changed (`add` to `eor`/`xor`: the same
/// size on both targets): the identity case.
const M1_SAME: &str =
    "int wp255d_m1_data = 1;\nint wp255d_m1(int x) { return x ^ wp255d_m1_data; }\n";

/// The stored layout with the first link's regions of one file added
/// (TEST-ONLY: `chunk.rs`'s IN-30 rule read back from the stored rows, a
/// region per run of the file's placed sections of one `(chunk, category)`
/// in a body chunk, its capacity the run and the `Fill` hole right after
/// it). Every other row is `inner`'s.
struct WithRegions<'a> {
    inner: &'a dyn LayoutRows,
    file: FileId,
    regions: Vec<StoredRegion>,
}

impl WithRegions<'_> {
    fn derive(inner: &dyn LayoutRows, file: FileId) -> Vec<StoredRegion> {
        let chunks: std::collections::BTreeMap<ChunkId, Chunk> =
            inner.chunks().expect("CHUNKS").into_iter().collect();
        let mut runs: std::collections::BTreeMap<(ChunkId, u8), (u64, u64, OutSecId)> =
            std::collections::BTreeMap::new();
        for s in inner.sections_of(file).expect("SECTIONS") {
            let Some((ch, off)) = s.place.get() else { continue };
            let ChunkKind::Body(p) = chunks[&ch].kind else { continue };
            let cat = if s.sh_flags & SHF_EXECINSTR != 0 {
                mortar_layout::order::text_category(&s.name)
            } else {
                0
            };
            let e = runs.entry((ch, cat)).or_insert((off, off + s.size, p.out));
            e.0 = e.0.min(off);
            e.1 = e.1.max(off + s.size);
        }
        let mut ordinal: std::collections::BTreeMap<(OutSecId, u8), u32> =
            std::collections::BTreeMap::new();
        let mut v = Vec::new();
        for ((ch, cat), (start, end, out)) in runs {
            let fill = inner
                .holes_in(ch)
                .expect("HOLES")
                .iter()
                .find(|h| h.offset == end && h.kind == HoleKind::Fill)
                .map_or(0, |h| h.size);
            let n = ordinal.entry((out, cat)).or_insert(0);
            v.push(StoredRegion {
                file,
                out,
                category: cat,
                ordinal: *n,
                chunk: ch,
                offset: start,
                capacity: end - start + fill,
            });
            *n += 1;
        }
        v.sort_by_key(|r| (r.out, r.category, r.ordinal));
        v
    }
}

impl LayoutRows for WithRegions<'_> {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        self.inner.chunks()
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        self.inner.order()
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        self.inner.segments()
    }
    fn outsecs(&self) -> Result<Vec<(OutSecId, Vec<u8>)>, IncrReason> {
        self.inner.outsecs()
    }
    fn sections_of(&self, file: FileId) -> Result<Vec<StoredSection>, IncrReason> {
        self.inner.sections_of(file)
    }
    fn regions_of(&self, file: FileId) -> Result<Vec<StoredRegion>, IncrReason> {
        if file == self.file { Ok(self.regions.clone()) } else { self.inner.regions_of(file) }
    }
    fn holes_in(&self, chunk: ChunkId) -> Result<Vec<Hole>, IncrReason> {
        self.inner.holes_in(chunk)
    }
    fn frags_of(
        &self,
        out: OutSecId,
        class: u32,
        hash: [u8; 16],
    ) -> Result<Option<StoredFrag>, IncrReason> {
        self.inner.frags_of(out, class, hash)
    }
    fn frag_uses_of(&self, file: FileId) -> Result<Vec<StoredFragUse>, IncrReason> {
        self.inner.frag_uses_of(file)
    }
    fn eh_range_of(&self, file: FileId) -> Result<Option<StoredEhRange>, IncrReason> {
        self.inner.eh_range_of(file)
    }
    fn eh_ranges(&self) -> Result<Vec<(FileId, u64, StoredEhRange)>, IncrReason> {
        self.inner.eh_ranges()
    }
    fn key_refs_of(&self, file: FileId) -> Result<Vec<StoredKeyRef>, IncrReason> {
        self.inner.key_refs_of(file)
    }
    fn section(&self, id: SectionId) -> Result<Option<StoredSection>, IncrReason> {
        self.inner.section(id)
    }
    fn slots(&self) -> Result<Vec<StoredSlot>, IncrReason> {
        self.inner.slots()
    }
    fn patches(&self) -> Result<Vec<Patch>, IncrReason> {
        self.inner.patches()
    }
    fn redirects(&self) -> Result<Vec<Redirect>, IncrReason> {
        self.inner.redirects()
    }
    fn sym_place(&self) -> Result<Vec<SymPlace>, IncrReason> {
        self.inner.sym_place()
    }
    fn defined(&self) -> Result<Vec<Defined>, IncrReason> {
        self.inner.defined()
    }
    fn tls_and_entry(&self) -> Result<(Option<Tls>, Option<u64>), IncrReason> {
        self.inner.tls_and_entry()
    }
    fn symtab_block(&self, file: FileId) -> Result<Option<SymtabBlock>, IncrReason> {
        self.inner.symtab_block(file)
    }
    fn symtab_tables(&self) -> Result<(SymtabBlock, SymtabTables), IncrReason> {
        self.inner.symtab_tables()
    }
}

#[test]
fn a_same_size_edit_keeps_its_place_given_its_regions() {
    let mut judged = 0;
    for line in lines(&[("ms", "c", M1_SAME)]) {
        let what = |s: &str| format!("{} same-size edit: {s}", line.label);
        let args = line.args(&["wp255d_m2", "wp255d_m1"]);
        line.archive(&["m1.o", "m2.o"]);
        let old = state(&line, &args, "old");
        std::fs::copy(line.dir.join("ms.o"), line.dir.join("m1.o")).expect("the edit");
        line.archive(&["m1.o", "m2.o"]);
        let _new = state(&line, &args, "new");
        let capture = mortar_incr::state5::read::decode(
            &std::fs::read(line.dir.join("new.state5")).expect("the new state"),
        )
        .expect("the new state decodes");
        // One store per version of the inputs (B18-1).
        let store = support::store();
        let f = whole(&line, &args, &store);
        let file = member(&old, "m1.o");
        let nf = same_file(&f, &old, file);
        assert_eq!(nf, file, "{}", what("one FILES row in both links"));
        let (cf, delta) =
            mortar::update::changed::u5(&old, &[file], &[], &f, &capture, &mut |_| {})
                .unwrap_or_else(|e| panic!("{}", what(&format!("U4-U5: {}", e.report()))))
                .expect("m1.o changed");
        let rows = mortar_incr::state5::LayoutOf { view: &old };
        let stored = StoredLayout::new(&rows);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let relocs = ArchRelocs(f.arch.get());
        let inputs =
            layout_inputs(&mut cache, &old, &stored, &f.plan, &cf, &delta, &relocs, &f.pool)
                .unwrap_or_else(|e| panic!("{}", what(&format!("layout_inputs: {}", e.report()))));

        // Bytes first: the identity case. Each placed section of m1.o is
        // where the stored row has it (offset and output section name),
        // with the stored size.
        let outsecs = old.outsecs().expect("OUTSECS");
        let out_name = |o: OutSecId| old.string(outsecs[o.0 as usize].name).expect("name");
        let rows_of = stored.sections_of(file).expect("SECTIONS");
        let stored_chunks: std::collections::BTreeMap<ChunkId, Chunk> =
            rows.chunks().expect("CHUNKS").into_iter().collect();
        let mut placed = 0;
        for c in &inputs.changes {
            let fid = f.resolution.section_id(nf, SectionIndex(c.index)).expect("front id");
            let full = f.layout.placement.place[fid].get();
            let prev = c.old.and_then(|o| rows_of.iter().find(|r| r.id == o));
            let name = String::from_utf8_lossy(c.key.name).into_owned();
            eprintln!(
                "{}",
                what(&format!(
                    "{name}: size {} old {:?} (stored size {:?} at {:?}), full {full:?}",
                    c.size,
                    c.old,
                    prev.map(|p| p.size),
                    prev.map(|p| p.place.get())
                ))
            );
            let Some((fc, fo)) = full else { continue };
            let p = prev.unwrap_or_else(|| panic!("{}", what(&format!("{name}: no stored row"))));
            let (sc, so) = p.place.get().expect("a stored place");
            assert_eq!((p.size, so), (c.size, fo), "{}", what(&format!("{name}: in place")));
            let fout =
                f.layout.placement.chunks[fc].kind.out().map(|o| f.assignment.outsecs[o].name);
            let sout = stored_chunks.get(&sc).and_then(|ch| ch.kind.out()).map(out_name);
            assert_eq!(fout.map(<[u8]>::to_vec), sout, "{}", what(&format!("{name}: output")));
            placed += 1;
        }
        assert!(placed >= 1, "{}", what("m1.o has a placed section"));

        // The first link wrote no REGIONS row (no full-link producer).
        let count = old.count(mortar_incr::format::kind::REGIONS);
        let mine = stored.regions_of(file).expect("REGIONS");
        eprintln!(
            "{}",
            what(&format!("REGIONS rows: {count} in the state, {} of m1.o", mine.len()))
        );

        // The layout, with the whole front's demands of m1.o (TEST-ONLY;
        // this C file has none that are keys of another file).
        let demands: Vec<FileDemands> = Vec::new();
        let input = LayoutInput {
            changes: &inputs.changes,
            eh: &inputs.eh,
            pieces: &inputs.pieces,
            demands: &demands,
            debug: false,
        };
        let machine =
            if line.label.starts_with("aarch64") { Machine::AArch64 } else { Machine::X86_64 };
        let target = mortar_layout::target::Target::of(machine);
        let plain = mortar_layout::delta::layout(&stored, &input, &target);
        if mine.is_empty() {
            // Refused by name, never the overflow phase.
            let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout));
            assert_eq!(plain.as_ref().err(), Some(&want), "{}", what("no regions: refused"));
        }

        // Given the first link's regions: every placed section kept at the
        // full link's offset, nothing moved, no growth step.
        let with = WithRegions { inner: &rows, file, regions: WithRegions::derive(&rows, file) };
        eprintln!("{}", what(&format!("derived regions {:?}", with.regions)));
        let stored2 = StoredLayout::new(&with);
        let d = mortar_layout::delta::layout(&stored2, &input, &target)
            .unwrap_or_else(|e| panic!("{}", what(&format!("with regions: {}", e.report()))));
        for c in &inputs.changes {
            let fid = f.resolution.section_id(nf, SectionIndex(c.index)).expect("front id");
            let full = f.layout.placement.place[fid].get();
            let now = match d.place_of(&stored2, c.id).expect("place_of") {
                PlaceNow::Here(p) => p.get(),
                PlaceNow::NotPlaced => None,
                PlaceNow::Stored(_) => panic!("{}", what("a changed file's section is Stored")),
            };
            let name = String::from_utf8_lossy(c.key.name).into_owned();
            assert_eq!(
                now.map(|x| x.1),
                full.map(|x| x.1),
                "{}",
                what(&format!("{name}: the update's offset is the full link's"))
            );
        }
        assert!(d.moved.is_empty(), "{}", what(&format!("moved {:?}", d.moved)));
        assert!(d.growth_steps.is_empty(), "{}", what("a growth step"));
        judged += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(judged, 2, "both targets judged");
    }
}
