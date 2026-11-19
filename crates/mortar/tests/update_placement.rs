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
//!   the stored layout has it). Without the file's `REGIONS` rows
//!   `delta::layout` cannot keep a section in its file's region: it
//!   refuses by name (`update-not-implemented (layout)`) instead of
//!   sending every section to the overflow phase at the output section's
//!   tail (the lane's `.text: update 0x86f20, full 0xc70` and its growth
//!   step). With the first link's regions it keeps every section at the
//!   full link's offset, moves nothing and takes no growth step.
//! - M2b-2d (ruling M2b-2c 2): the regions are WP-264's stored `REGIONS`
//!   rows, and a TEST-ONLY derivation from the stored places and the
//!   `Fill` hole after each run (`chunk.rs`'s IN-30 rule, [`WithRegions`])
//!   gives the same rows: the producer checked against the rule, in every
//!   hop. `Fate::Beside`'s branch where the chunk changes size and the
//!   empty `.bss` follows its end (ruling M2b-2c 3).
//!
//! On AArch64 and x86-64.

#![allow(dead_code, reason = "support/update.rs's helpers that other tests use")]

mod support;
#[path = "support/update.rs"]
mod update;

use mortar::update::file_front::{FrontCache, Ids, front_of, front_of_changed};
use mortar::update::layout_in::layout_inputs;
use mortar_core::column::Column;
use mortar_core::id::{ChunkId, FileId, OutSecId, SectionId, SymKey, SymKind, SymbolId};
use mortar_core::incr::{
    FileDemands, IncrReason, LayoutInput, LayoutRows, PlaceNow, StoredEhRange, StoredFrag,
    StoredFragUse, StoredKeyRef, StoredLayout, StoredRegion, StoredSection, StoredSlot,
    SymtabBlock, SymtabTables, Unimplemented,
};
use mortar_core::layout::{
    Chunk, ChunkKind, Defined, Hole, HoleKind, Patch, Redirect, Segment, SegmentRules, SymPlace,
    Tls, derive_segments,
};
use mortar_elf::consts::{PT_GNU_STACK, SHF_EXECINSTR};
use mortar_elf::{Class, Machine, SectionIndex};
use mortar_incr::format::Str;
use mortar_incr::update::rows::FileRow;
use mortar_incr::update::view::{ChangedFiles, StateView};
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
    /// TEST-ONLY room after one chunk (`Beside`'s unit test): `(chunk,
    /// bytes)`, its capacity that much larger and every in-file chunk
    /// after it that much further on in the file (a segment reserve the
    /// no-libc program's full link does not leave).
    room: Option<(ChunkId, u64)>,
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
            let pad = inner
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
                capacity: end - start + pad,
            });
            *n += 1;
        }
        v.sort_by_key(|r| (r.out, r.category, r.ordinal));
        v
    }
}

impl LayoutRows for WithRegions<'_> {
    fn chunks(&self) -> Result<Vec<(ChunkId, Chunk)>, IncrReason> {
        let mut v = self.inner.chunks()?;
        if let Some((at, extra)) = self.room {
            let end = v.iter().find(|(id, _)| *id == at).map(|(_, c)| c.offset);
            for (id, c) in &mut v {
                if *id == at {
                    c.capacity += extra;
                } else if end.is_some_and(|e| c.in_file() && c.offset > e) {
                    c.offset += extra;
                }
            }
        }
        Ok(v)
    }
    fn order(&self) -> Result<Vec<ChunkId>, IncrReason> {
        self.inner.order()
    }
    fn segments(&self) -> Result<Vec<Segment>, IncrReason> {
        let stored = self.inner.segments()?;
        if self.room.is_none() {
            return Ok(stored);
        }
        // The room changes the chunks, so the segments are derived again
        // with the rules that derive the stored ones (L-125).
        let column = |v: Vec<(ChunkId, Chunk)>| {
            Column::<ChunkId, Chunk>::from_vec(v.into_iter().map(|(_, c)| c).collect())
                .expect("chunks")
        };
        let (was, now) = (column(self.inner.chunks()?), column(self.chunks()?));
        let order = self.order()?;
        let stack = stored.iter().find(|s| s.p_type == PT_GNU_STACK).expect("PT_GNU_STACK").flags;
        let pie = was.iter().any(|(_, c)| c.kind == ChunkKind::FileHeader && c.addr == Some(0));
        let rules = [0x1000u64, 0x4000, 0x10000]
            .into_iter()
            .map(|max_page| SegmentRules { class: Class::Elf64, max_page, stack, pie })
            .find(|r| derive_segments(&was, &order, r) == stored)
            .expect("the rules of the stored segments");
        Ok(derive_segments(&now, &order, &rules))
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
        let edit = || {
            std::fs::copy(line.dir.join("ms.o"), line.dir.join("m1.o")).expect("the edit");
            line.archive(&["m1.o", "m2.o"]);
        };
        identity_hop(&line, &args, &|v| member(v, "m1.o"), &edit, &what);
        judged += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(judged, 2, "both targets judged");
    }
}

/// A program without libc whose only writable data is `a.o`'s `.data`
/// word: every `.bss` input is empty, so the full link gives `.bss` no
/// chunk and places `a.o`'s empty `.bss` at the end of `.data`'s chunk
/// (layout.md L-159). `a.o` edited in one immediate (the same size).
const ASM_A64: &str = "\t.text\n\t.globl _start\n_start:\n\tmov x0, #42\n\tmov x8, #93\n\
    \tsvc #0\n\t.data\n\t.globl word\nword:\t.quad 1\n\
    \t.section .note.GNU-stack,\"\",%progbits\n";
const ASM_X64: &str = "\t.text\n\t.globl _start\n_start:\n\tmovl $42, %edi\n\tmovl $60, %eax\n\
    \tsyscall\n\t.data\n\t.globl word\nword:\t.quad 1\n\
    \t.section .note.GNU-stack,\"\",@progbits\n";

/// M2b2b-2: the empty `.bss` of a changed file whose output section has
/// no chunk (L-159) takes the full link's place, beside `.data`, instead
/// of `new-output-section: .bss`.
#[test]
fn an_empty_bss_with_no_output_chunk_stays_beside_data() {
    let judged = empty_bss_hops(
        "empty .bss",
        &|t: &str, from: &str| t.replace(from, &from.replace("42", "43")),
        true,
        0,
        &|what: &dyn Fn(&str) -> String, bss: &Placed| {
            assert_eq!(bss.out.as_deref(), Some(&b".data"[..]), "{}", what(".bss beside .data"));
        },
    );
    if mortar_testutil::tools_required() {
        assert_eq!(judged, 2, "both targets judged");
    }
}

/// `Fate::Beside`'s other branch (ruling M2b-2c 3): `a.o`'s `.data` grows
/// past its region (8 bytes and 64 of slack, to 80), so `.data`'s chunk
/// changes size; the empty `.bss`, stored at that chunk's old END, follows
/// its new end (L-159: the end of the last chunk before `.bss`). The
/// no-libc program's full link leaves `.data`'s chunk no file reserve
/// (the same edit with the stored rows is `reserve-exhausted: the PT_LOAD
/// of .data: segment reserve (file)`), so the stored layout is given 4
/// KiB of room after that chunk (TEST-ONLY, [`WithRegions::room`]).
#[test]
fn an_empty_bss_at_the_chunk_end_follows_the_new_end() {
    let judged = empty_bss_hops(
        "empty .bss, .data grown",
        &|t: &str, _from: &str| t.replace("word:\t.quad 1\n", "word:\t.quad 1\n\t.fill 72, 1, 0\n"),
        false,
        0x1000,
        &|what: &dyn Fn(&str) -> String, bss: &Placed| {
            assert_eq!(bss.out.as_deref(), Some(&b".data"[..]), "{}", what(".bss beside .data"));
            let was = bss.chunk_was.expect("the stored chunk");
            assert_ne!(bss.chunk_now, was, "{}", what("the chunk changed size"));
            assert_eq!(
                bss.offset,
                bss.chunk_now,
                "{}",
                what(&format!("at the new end ({was:#x} before)"))
            );
        },
    );
    if mortar_testutil::tools_required() {
        assert_eq!(judged, 2, "both targets judged");
    }
}

/// What an [`empty_bss_hops`] case asserts of `.bss`: given the message
/// prefixer and the placed `.bss`.
type Check = dyn Fn(&dyn Fn(&str) -> String, &Placed);

/// The no-libc `a.o` program on both targets, edited by `edit(text,
/// immediate)`, one [`hop`] (`identity` as given), `check` on the delta's
/// `.bss`. Returns the targets judged.
fn empty_bss_hops(
    case: &str,
    edit: &dyn Fn(&str, &str) -> String,
    identity: bool,
    room: u64,
    check: &Check,
) -> usize {
    let mut judged = 0;
    for (arch, label, text, from) in [
        (mortar_testutil::target::Arch::Aarch64, "aarch64 asm", ASM_A64, "#42"),
        (mortar_testutil::target::Arch::X86_64, "x86-64 asm", ASM_X64, "$42"),
    ] {
        let Some(gcc) = arch.tool("gcc") else {
            assert!(!mortar_testutil::tools_required(), "{} is required", arch.gnu("gcc"));
            continue;
        };
        let scratch = mortar_testutil::Scratch::new("wp225c-empty-bss");
        let dir = scratch.path().to_path_buf();
        let base = vec![dir.join("a.o").to_string_lossy().into_owned(), "-static".into()];
        let line =
            update::Line { _scratch: scratch, label: label.into(), dir, base, cc: gcc.clone() };
        let assemble = |src: &str| {
            let s = line.dir.join("a.s");
            std::fs::write(&s, src).expect("a.s");
            let obj = line.dir.join("a.o");
            let o = mortar_testutil::run(
                &gcc,
                &["-c".into(), s.into_os_string(), "-o".into(), obj.into_os_string()],
            );
            assert!(o.success(), "{label}: gcc: {}", o.stderr_text());
        };
        assemble(text);
        let what = |s: &str| format!("{label} {case}: {s}");
        let edited = edit(text, from);
        assert_ne!(edited, text, "{}", what("the edit changes the source"));
        let edit = || assemble(&edited);
        let pick = |v: &StateView<'_>| {
            let files = u32::try_from(v.count(mortar_incr::format::kind::FILES)).expect("FILES");
            (1..files)
                .map(FileId)
                .find(|&id| {
                    let r = v.file(id).expect("FILES");
                    r.member == 0 && v.string(r.path).expect("path").ends_with(b"a.o")
                })
                .expect("a.o")
        };
        let placed = hop(&line, &line.base, &pick, &edit, &what, identity, room).0;
        let bss = placed.iter().find(|p| p.name == ".bss").expect("a.o's .bss placed");
        eprintln!("{}", what(&format!("{bss:?}")));
        check(&what, bss);
        judged += 1;
    }
    judged
}

/// A section the delta placed: its name, its output section's name, its
/// offset in its chunk, and that chunk's size after and before the update.
#[derive(Debug)]
struct Placed {
    name: String,
    out: Option<Vec<u8>>,
    offset: u64,
    chunk_now: u64,
    chunk_was: Option<u64>,
}

/// Each placed section of the changed file.
type Sections = Vec<Placed>;

/// One identity-case hop ([`hop`] with `identity`).
fn identity_hop(
    line: &update::Line,
    args: &[String],
    pick: &dyn Fn(&StateView<'_>) -> FileId,
    edit: &dyn Fn(),
    what: &dyn Fn(&str) -> String,
) -> Sections {
    hop(line, args, pick, edit, what, true, 0).0
}

/// One hop: the state of `args`, `edit`, the front of the new inputs and
/// `delta::layout` over the changed file `pick` names, with the whole
/// front's demands of the file (TEST-ONLY, keys moved into the update's
/// ids by name, as `tests/layout_in.rs`). Without the file's `REGIONS`
/// rows the delta refuses by name. With the first link's regions
/// ([`WithRegions`]) it is judged by M2b2a-2's properties (ruling
/// M2b2a-3) against the full link: placed exactly when the full link
/// places it, in an output section of the same name, aligned, overlapping
/// no other placed section of its chunk, every piece's fragment stored or
/// new. With `identity` (the front places each section of the file where
/// the stored row has it: required first, from the bytes), also every
/// section at the full link's offset, nothing moved and no growth step.
/// Gives each placed section's name and the delta's output section name,
/// and the counts `(pieces, eh records)`. `room` > 0: the stored layout is
/// judged with that many bytes of room after the chunk of the file's
/// `.data` place ([`WithRegions::room`]; its regions the stored ones).
#[allow(clippy::too_many_lines, reason = "the hop, then the properties in order")]
fn hop(
    line: &update::Line,
    args: &[String],
    pick: &dyn Fn(&StateView<'_>) -> FileId,
    edit: &dyn Fn(),
    what: &dyn Fn(&str) -> String,
    identity: bool,
    room: u64,
) -> (Sections, usize, usize) {
    {
        let old = state(line, args, "old");
        edit();
        let _new = state(line, args, "new");
        let capture = mortar_incr::state5::read::decode(
            &std::fs::read(line.dir.join("new.state5")).expect("the new state"),
        )
        .expect("the new state decodes");
        // One store per version of the inputs (B18-1).
        let store = support::store();
        let f = whole(line, args, &store);
        let file = pick(&old);
        let nf = same_file(&f, &old, file);
        assert_eq!(nf, file, "{}", what("one FILES row in both links"));
        let (cf, delta) =
            mortar::update::changed::u5(&old, &[file], &[], &f, &capture, &mut |_| {})
                .unwrap_or_else(|e| panic!("{}", what(&format!("U4-U5: {}", e.report()))))
                .expect("the file changed");
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
            placed += 1;
            if !identity {
                continue;
            }
            let p = prev.unwrap_or_else(|| panic!("{}", what(&format!("{name}: no stored row"))));
            let (sc, so) = p.place.get().expect("a stored place");
            assert_eq!((p.size, so), (c.size, fo), "{}", what(&format!("{name}: in place")));
            let fout =
                f.layout.placement.chunks[fc].kind.out().map(|o| f.assignment.outsecs[o].name);
            let sout = stored_chunks.get(&sc).and_then(|ch| ch.kind.out()).map(out_name);
            assert_eq!(fout.map(<[u8]>::to_vec), sout, "{}", what(&format!("{name}: output")));
        }
        assert!(placed >= 1, "{}", what("the file has a placed section"));

        // The first link's REGIONS rows (WP-264's producer), and the
        // TEST-ONLY derivation from `chunk.rs`'s rule ([`WithRegions`]):
        // the same rows (ruling M2b-2c 2: the producer checked against the
        // rule).
        let count = old.count(mortar_incr::format::kind::REGIONS);
        let mut mine = stored.regions_of(file).expect("REGIONS");
        mine.sort_by_key(|r| (r.out, r.category, r.ordinal));
        eprintln!(
            "{}",
            what(&format!("REGIONS rows: {count} in the state, {} of the file", mine.len()))
        );
        assert!(!mine.is_empty(), "{}", what("the first link wrote the file's REGIONS"));
        let derived = WithRegions::derive(&rows, file);
        assert_eq!(derived, mine, "{}", what("WithRegions' rows are the stored REGIONS"));

        // The whole front's demands of the file (TEST-ONLY).
        let symbols = old.count(mortar_incr::format::kind::SYMBOLS);
        let mut demands: Vec<FileDemands> = Vec::new();
        if let Some(i5) = &f.incr5 {
            for d in i5.demands.iter().filter(|d| d.file == nf) {
                let mut v = Vec::new();
                for x in &d.demands {
                    let key = match x.key.kind() {
                        SymKind::Global(g) => {
                            let k = &f.inputs.symbols[g];
                            let ver = k.version.unwrap_or_default();
                            let id = old.find_key(k.name, ver).expect("SYMBOLS").or_else(|| {
                                let at = delta
                                    .new_keys
                                    .iter()
                                    .position(|n| n.name == k.name && n.version == ver)?;
                                Some(SymbolId(u32::try_from(symbols).ok()? + at as u32))
                            });
                            SymKey::global(id.expect("the key in the update"))
                        }
                        SymKind::Local(_) => x.key,
                    };
                    v.push(mortar_core::incr::SlotDemand { key, flags: x.flags });
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
        let machine =
            if line.label.starts_with("aarch64") { Machine::AArch64 } else { Machine::X86_64 };
        let target = mortar_layout::target::Target::of(machine);
        // Without the file's regions: refused by name, never the overflow
        // phase (M2b2b-3).
        let none = WithRegions { inner: &rows, file, regions: Vec::new(), room: None };
        let bare = mortar_layout::delta::layout(&StoredLayout::new(&none), &input, &target);
        eprintln!("{}", what(&format!("no regions: {:?}", bare.as_ref().err())));
        assert!(bare.is_err(), "{}", what("no regions: refused"));
        if identity {
            let want = IncrReason::UpdateNotImplemented(Some(Unimplemented::Layout));
            assert_eq!(bare.as_ref().err(), Some(&want), "{}", what("no regions: refused"));
        }

        // With the stored regions: judged.
        eprintln!("{}", what(&format!("stored regions {mine:?}")));
        let data = rows_of.iter().find(|r| &*r.name == b".data").and_then(|r| r.place.get());
        let roomy = WithRegions {
            inner: &rows,
            file,
            regions: mine.clone(),
            room: data.filter(|_| room > 0).map(|(ch, _)| (ch, room)),
        };
        assert!(room == 0 || roomy.room.is_some(), "{}", what("a .data place for the room"));
        let roomy_stored = StoredLayout::new(&roomy);
        let stored2 = if room > 0 { &roomy_stored } else { &stored };
        let d = mortar_layout::delta::layout(stored2, &input, &target)
            .unwrap_or_else(|e| panic!("{}", what(&format!("with regions: {}", e.report()))));
        let mut out = Vec::new();
        let mut mine: Vec<(u32, u64, u64)> = Vec::new();
        for c in &inputs.changes {
            let fid = f.resolution.section_id(nf, SectionIndex(c.index)).expect("front id");
            let full = f.layout.placement.place[fid].get();
            let now = match d.place_of(stored2, c.id).expect("place_of") {
                PlaceNow::Here(p) => p.get(),
                PlaceNow::NotPlaced => None,
                PlaceNow::Stored(_) => panic!("{}", what("a changed file's section is Stored")),
            };
            let name = String::from_utf8_lossy(c.key.name).into_owned();
            assert_eq!(
                now.is_some(),
                full.is_some(),
                "{}",
                what(&format!("{name}: placed (update {now:?}, full {full:?})"))
            );
            if identity {
                assert_eq!(
                    now.map(|x| x.1),
                    full.map(|x| x.1),
                    "{}",
                    what(&format!("{name}: the update's offset is the full link's"))
                );
            }
            let (Some((uc, uo)), Some((fc, fo))) = (now, full) else { continue };
            let uout = d.placement.chunks[uc].kind.out().map(out_name);
            let fout =
                f.layout.placement.chunks[fc].kind.out().map(|o| f.assignment.outsecs[o].name);
            assert_eq!(uout.as_deref(), fout, "{}", what(&format!("{name}: output section")));
            let addr = d.placement.chunks[uc].addr.unwrap_or(0) + uo;
            assert!(addr.is_multiple_of(c.align.max(1)), "{}", what(&format!("{name}: aligned")));
            eprintln!("{}", what(&format!("{name}: update {uo:#x}, full {fo:#x}")));
            mine.push((uc.0, uo, uo + c.size));
            out.push(Placed {
                name,
                out: uout,
                offset: uo,
                chunk_now: d.placement.chunks[uc].size,
                chunk_was: stored_chunks.get(&uc).map(|x| x.size),
            });
        }
        // No overlap with any other placed section of the same chunk.
        let files = u32::try_from(old.count(mortar_incr::format::kind::FILES)).expect("FILES");
        let mut others: Vec<(u32, u64, u64)> = Vec::new();
        for g in (1..files).map(FileId).filter(|&g| g != file) {
            for s in stored2.sections_of(g).expect("SECTIONS") {
                if let PlaceNow::Stored(p) = d.place_of(stored2, s.id).expect("place_of")
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
            let found = stored2.frags_of(key.0, key.1, key.2).expect("FRAGS").is_some()
                || d.frags.iter().any(|g| (g.out, g.class, g.hash) == key);
            assert!(found, "{}", what(&format!("piece {p:?}: no fragment")));
            pieces += 1;
        }
        if identity {
            assert!(d.moved.is_empty(), "{}", what(&format!("moved {:?}", d.moved)));
            assert!(d.growth_steps.is_empty(), "{}", what("a growth step"));
        }
        let records = inputs.eh.iter().map(|e| e.records.len()).sum();
        eprintln!(
            "{}",
            what(&format!(
                "{} changes, {} placed, {pieces} pieces, {records} eh records, {} moved, {} growth steps",
                inputs.changes.len(),
                mine.len(),
                d.moved.len(),
                d.growth_steps.len()
            ))
        );
        (out, pieces, records)
    }
}

/// `m1.o` before: C++, a `try` (`.eh_frame` with a personality).
const EH_A: &str = "extern \"C\" { int wp255d_m1_data = 1; int wp255d_throw(int); }\n\
    extern \"C\" int wp255d_m1(int x) { try { return wp255d_throw(x) + wp255d_m1_data; } \
    catch (...) { return -1; } }\n\
    extern \"C\" int wp255d_throw(int x) { if (x < 0) throw x; return x; }\n";
/// `m1.o` after, with as few new FDEs as the cases need (M2b-2a item 8):
/// the function grown (within its region's slack), strings added (a merge section, read inside
/// `wp255d_m1`: no new function), a COMDAT group added (`wp255d_inl`,
/// kept by the earlier `m8.o`, so this copy and its FDE are dropped).
const EH_C: &str = "extern \"C\" { int wp255d_m1_data = 1; int wp255d_throw(int); }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m1(int x) { static const char *const t[] = \
    { \"wp225c-alpha\", \"wp225c-beta\" }; try { return wp255d_throw(x) + \
    wp255d_inl(x) + *t[x & 1]; } \
    catch (...) { return -1; } }\n\
    extern \"C\" int wp255d_throw(int x) { if (x < 0) throw x; return x; }\n";
const M8: &str = "extern \"C\" { extern int wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m8(int x) { return wp255d_inl(x) + 8; }\n";

/// M2b-2a item 8: a C++ edit with strings, a dropped COMDAT copy and its
/// `.eh_frame` records, judged by M2b2a-2's properties with the first
/// link's regions; `.eh_frame`'s reserve holds (no new FDE).
#[test]
fn a_cpp_edit_with_strings_and_a_comdat_is_judged() {
    let mut judged = 0;
    for line in lines(&[("eha", "c++", EH_A), ("ehc", "c++", EH_C), ("m8", "c++", M8)]) {
        let what = |s: &str| format!("{} c++ eh: {s}", line.label);
        let args = line.args(&["wp255d_m8", "wp255d_m2", "wp255d_m1"]);
        std::fs::copy(line.dir.join("eha.o"), line.dir.join("m1.o")).expect("before");
        line.archive(&["m8.o", "m1.o", "m2.o"]);
        let edit = || {
            std::fs::copy(line.dir.join("ehc.o"), line.dir.join("m1.o")).expect("the edit");
            line.archive(&["m8.o", "m1.o", "m2.o"]);
        };
        let (_, pieces, records) =
            hop(&line, &args, &|v| member(v, "m1.o"), &edit, &what, false, 0);
        assert!(pieces > 0, "{}", what("the strings' pieces judged"));
        assert!(records > 0, "{}", what("the .eh_frame records judged"));
        judged += 1;
    }
    if mortar_testutil::tools_required() {
        assert_eq!(judged, 2, "both targets judged");
    }
}
