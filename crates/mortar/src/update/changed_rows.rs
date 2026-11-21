//! WP-267 P-1: U4's rows from the changed files alone (`spec/landed/incr/
//! WP-267.md` §1 rows 6, 7a, 7b, 7d, 7e, 7h, 7i; §4's P-1 row): the
//! update's [`ChangedFiles`] made from each changed file's own bytes and
//! the stored rows, with no front (no `Inputs`, no `Resolution`).
//!
//! Two steps, as IN-70 orders them:
//! - [`changed_files_of`] (U4, before `extract`/`resolve`): the member-level
//!   judgement (the bytes hashed as `detect` hashes them), the `FILES` row,
//!   the sections' identity and header columns, keys, mentions, values,
//!   `versioned`, `first_global`, the ODR copies, `debug`;
//! - [`facts_after_resolve`] (after U-3's `resolve`, whose election the
//!   COMDAT keepers are): the sections' `kind`, `KEPT` bit and output
//!   section, `FILES.properties` (of the KEPT `.note.gnu.property`
//!   sections), `meta_units`.
//!
//! Not made here (stopped, `WIP-267-p1.md`'s table): `INDEX` (M-4) and
//! `EDGES` (M-5), P-1b's; `FILES`' eh columns (the update's layout, U7's
//! `LayoutDelta::eh`); the sections' `COLLECTED` bit (U6's liveness;
//! F-8: never an input). The shadow ([`shadow`], findings only) compares
//! every field made here with front 1's (`changed::changed_files`, the
//! oracle) on the real path, which keeps using front 1's rows (P-5
//! switches).
//!
//! Refused by name: a changed archive member (M-1), `--wrap` (the keys by
//! name are false under it; `file_scan.rs`'s refusal stays), a changed
//! file that is not a relocatable object, and the per-file front's and
//! `changed_facts`' own refusals.

use std::collections::HashMap;
use std::path::Path;

use mortar_core::id::{FileId, SectionId};
use mortar_core::identity::Anchor;
use mortar_core::incr::{IncrReason, Unimplemented};
use mortar_core::plan::Plan;
use mortar_core::resolution::SectionKind;
use mortar_incr::capture;
use mortar_incr::format::{NONE, SectionFacts, SectionRow, Str, Strings, kind, sym_kind};
use mortar_incr::update::rows::{CopyRow, FileRow, MentionRow, file_flags, file_stack};
use mortar_incr::update::view::{
    Change, ChangedFile, ChangedFiles, KeyName, MentionValue, OdrRows, StateView,
};

use super::file_front::{FrontCache, Ids, front_of};

fn resolve() -> IncrReason {
    IncrReason::UpdateNotImplemented(Some(Unimplemented::Resolve))
}

fn corrupt(what: impl std::fmt::Display) -> IncrReason {
    IncrReason::StateCorrupt(format!("WP-267 P-1: {what}"))
}

fn too_much(what: &str) -> IncrReason {
    IncrReason::TooMuchChanged(format!("{what} past u32"))
}

/// `FILES.kind` of a relocatable object (the capture's `Reader::Object`).
const KIND_OBJECT: u8 = 1;

/// U4 from the changed files alone (WP-267 P-1): the update's
/// [`ChangedFiles`] for `files` (U2's `Detected::changed_files`: the files
/// of the changed paths), each opened through `cache` (`front_of`,
/// `Ids::Update`), with `positional` (U3's set). A file whose bytes hash
/// to its stored `FILES.hash` is not changed and has no row (U4's
/// member-level judgement, the hash `detect` takes). `plan` is the
/// update's line (`--wrap`, `--odr`); `cwd` resolves the stored paths.
///
/// Every field but the ones [`facts_after_resolve`] fills and the ones
/// stopped (the module's list: `index`, `edges`, the eh columns, the
/// `COLLECTED` bit) is as front 1's `changed::changed_files` makes it.
///
/// Parameters against WP-267 §4's line: `arch` moved to
/// [`facts_after_resolve`] (only `FILES.properties` reads it), `cwd`
/// added (the path's bytes are hashed before a front is opened).
///
/// # Errors
/// `update-not-implemented (resolve)`: a changed archive member (M-1), a
/// changed file that is not a relocatable object; `(scan)`: `--wrap`;
/// `input-changed`: a file that cannot be read; `front_of`'s;
/// `too-much-changed` past the `u32` id spaces.
pub fn changed_files_of(
    view: &StateView<'_>,
    cache: &mut FrontCache<'_>,
    files: &[FileId],
    positional: &[FileId],
    plan: &Plan,
    cwd: &Path,
) -> Result<ChangedFiles, IncrReason> {
    // 7d: a mention's key by name is the link's key only without `--wrap`
    // (`resolve.rs` applies it to the keys); `file_scan.rs` refuses it too.
    if !mortar_resolve::tables::option_keys(plan, &[]).wrap.is_empty() {
        return Err(IncrReason::UpdateNotImplemented(Some(Unimplemented::Scan)));
    }
    let copies = plan.diagnostics.odr.mode != mortar_core::plan::OdrMode::Off;
    let mut ids = files.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut next = u32::try_from(view.count(kind::SECTIONS)).map_err(|_| too_much("SECTIONS"))?;
    let mut strings = Strings::default();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let stored = view.file(id)?;
        if stored.flags & file_flags::GONE != 0 {
            return Err(corrupt(format!("file {} is gone and changed", id.0)));
        }
        // M-1: a changed archive's members are matched and re-read by the
        // front until P-1's member table exists.
        if stored.member != 0 || stored.kind != KIND_OBJECT {
            return Err(resolve());
        }
        let path = view.string(stored.path)?;
        let at = cwd.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&path));
        let bytes = std::fs::read(&at).map_err(|e| {
            IncrReason::InputChanged(format!("{}: not read: {e}", String::from_utf8_lossy(&path)))
        })?;
        let hash = crate::incremental::hash_bytes(mortar_input::store::content_hash(&bytes));
        if hash == stored.hash {
            continue;
        }
        // The capture's format-4 row (`capture.rs` `capture`): a plain
        // object is the whole file; `POSITIONAL` on every captured row.
        let row = FileRow {
            kind: stored.kind,
            member: 0,
            flags: file_flags::POSITIONAL,
            path: stored.path,
            mention: stored.mention,
            member_name: Str::default(),
            member_ordinal: 0,
            base: 0,
            len: bytes.len() as u64,
            hash,
            rank: stored.rank,
            round: stored.round,
            ..FileRow::default()
        };
        drop(bytes);
        let first = SectionId(next);
        let ff = front_of(cache, view, id, Ids::Update { first_section: first, row: row.clone() })?;
        let cf = file_rows_of(ff, row, first, copies, &mut strings)?;
        next = u32::try_from(cf.sections.len())
            .ok()
            .and_then(|n| next.checked_add(n))
            .ok_or_else(|| too_much("SECTIONS"))?;
        out.push(cf);
    }
    // The `FILES` rows' strings after every file's sections' (the
    // oracle's order: `file_rows`, then the rows), then every `Str` moved
    // past the state's `STRINGS`.
    for c in &mut out {
        c.row.path = strings.add(&view.string(c.row.path)?);
        c.row.member_name = Str::default();
    }
    let strings = strings.into_bytes();
    let base = u32::try_from(view.count(kind::STRINGS))
        .ok()
        .filter(|b| u32::try_from(strings.len()).is_ok_and(|n| b.checked_add(n).is_some()))
        .ok_or_else(|| too_much("the update's strings"))?;
    let rebase =
        |s: Str| if s.len == 0 { Str::default() } else { Str { off: base + s.off, len: s.len } };
    for c in &mut out {
        c.row.path = rebase(c.row.path);
        for s in &mut c.sections {
            s.anchor = rebase(s.anchor);
            s.name = rebase(s.name);
        }
    }
    Ok(ChangedFiles { files: out, appeared: Vec::new(), positional: positional.to_vec(), strings })
}

/// One changed file's rows from its front `ff` (`Ids::Update` at
/// `first`), its `Str`s in `strings` (not yet moved past `STRINGS`).
fn file_rows_of(
    ff: &super::file_front::FileFront<'_>,
    mut row: FileRow,
    first: SectionId,
    copies: bool,
    strings: &mut Strings,
) -> Result<ChangedFile, IncrReason> {
    let file = ff.file;
    // 7h: the bytes' flags and the stack note (one producer each).
    row.flags |= capture::object_flags_of(&ff.object, ff.bytes);
    row.stack = match mortar_layout::stack::file_note(&ff.object) {
        mortar_layout::stack::StackNote::Absent => file_stack::ABSENT,
        mortar_layout::stack::StackNote::NonExec => file_stack::NON_EXEC,
        mortar_layout::stack::StackNote::Exec => file_stack::EXEC,
    };
    // 7a: identity and the header's five; facts after resolve; U-4 places.
    let mut sections = Vec::with_capacity(ff.sections.len());
    for s in &ff.sections {
        let (anchor_kind, anchor) = capture::anchor_row_of(strings, s.anchor);
        sections.push(SectionRow {
            file: file.0,
            anchor_kind,
            anchor,
            name: strings.add(s.name),
            ordinal: s.ordinal,
            sh_type: s.sh_type,
            sh_flags: s.flags,
            size: s.size,
            align: s.align,
            offset: s.offset,
            place_chunk: NONE,
            place_offset: 0,
            ..SectionRow::default()
        });
    }
    // 7d: the keys by name, in the order the mentions name them.
    let mut keys: Vec<KeyName> = Vec::new();
    let mut at: HashMap<(&[u8], Option<&[u8]>), u32> = HashMap::new();
    let mut key_of = Vec::with_capacity(ff.keys.len());
    for k in &ff.keys {
        let p = if let Some(&p) = at.get(&(k.name, k.version)) {
            p
        } else {
            let p = u32::try_from(keys.len()).map_err(|_| too_much("keys"))?;
            keys.push(KeyName {
                name: k.name.to_vec(),
                version: k.version.map(<[u8]>::to_vec).unwrap_or_default(),
                kind: if k.version.is_some() { sym_kind::VERSIONED } else { sym_kind::GLOBAL },
                index: 0,
            });
            at.insert((k.name, k.version), p);
            p
        };
        key_of.push(p);
    }
    let count = u32::try_from(sections.len()).map_err(|_| too_much("sections"))?;
    let table = ff.object.symbols().map_err(|_| corrupt(format!("file {}: symbols", file.0)))?;
    let (mentions, copy_rows) = match &table {
        Some(t) => {
            let m = capture::file_mentions_of(file, t, &key_of, first.0, count);
            let comdat = |s: u32| {
                s.checked_sub(first.0)
                    .and_then(|k| ff.sections.get(k as usize))
                    .is_some_and(|x| matches!(x.anchor, Anchor::Comdat(_)))
            };
            let c = capture::file5_of(t, &m, &comdat, copies).map_err(corrupt)?;
            (m, c)
        }
        None => (Vec::new(), Vec::new()),
    };
    // 7d: the values, position for position with the mentions.
    let values = ff
        .keys
        .iter()
        .take(mentions.len())
        .map(|k| MentionValue { value: k.st_value, size: k.st_size })
        .collect();
    let mentions = mentions
        .iter()
        .map(|m| MentionRow {
            symbol: m.symbol,
            kind: m.kind,
            bind: m.bind,
            typ: m.typ,
            other: m.other,
            section: m.section,
        })
        .collect();
    let copies = copy_rows
        .into_iter()
        .map(|(key, index, size, typ)| CopyRow { key, file: file.0, index, size, typ, tomb: false })
        .collect();
    Ok(ChangedFile {
        file,
        change: Change::Changed,
        row,
        first_section: first,
        sections,
        keys,
        first_global: ff.first_global,
        mentions,
        values,
        index: Vec::new(),
        edges: Vec::new(),
        meta_units: 0,
        debug: super::object_has_debug(&ff.object),
        versioned: ff.versioned,
        odr: OdrRows { copies },
    })
}

/// 7b and the rows that read it (WP-267 P-1), after U-3's `resolve`: each
/// changed file's sections' `kind`, `KEPT` bit and output section
/// (`file_split::changed_facts` over the file's front in `cache`, the
/// COMDAT keepers `keepers` = `ResolveDelta::keepers`), its
/// `meta_units` (the sections of kind `Meta`, as resolve lists them in
/// `Resolution::meta`) and its `FILES.properties` (`object_properties`
/// over its KEPT `.note.gnu.property` notes, `arch`'s merge rules; the
/// bytes appended to `cf.strings`). The `COLLECTED` bit is not set (U6's).
///
/// # Errors
/// `changed_facts`' refusals; `update-not-implemented (resolve)` for a
/// file whose front `cache` does not hold or whose notes do not parse;
/// `too-much-changed` past the `u32` string space.
pub fn facts_after_resolve(
    view: &StateView<'_>,
    cache: &FrontCache<'_>,
    cf: &mut ChangedFiles,
    plan: &Plan,
    arch: &dyn mortar_arch::Arch,
    keepers: &[(Vec<u8>, Option<FileId>)],
) -> Result<(), IncrReason> {
    use mortar_elf::consts::SHT_NOTE;
    let base = view.count(kind::STRINGS);
    for c in &mut cf.files {
        let ff = cache.get(c.file).ok_or_else(resolve)?;
        let facts = super::file_split::changed_facts(ff, view, plan, keepers)?;
        if facts.len() != c.sections.len() {
            return Err(corrupt(format!("file {}: facts for another section count", c.file.0)));
        }
        let mut notes: Vec<(SectionId, &[u8], u64)> = Vec::new();
        for (k, (row, fact)) in c.sections.iter_mut().zip(&facts).enumerate() {
            let f = SectionFacts {
                kind: fact.kind,
                kept: fact.kept,
                collected: false,
                out: fact.out.map(|o| o.0),
            };
            capture::put_section_facts(row, &f).map_err(IncrReason::StateLimit)?;
            let s = &ff.sections[k];
            if fact.kept && s.sh_type == SHT_NOTE && s.name == b".note.gnu.property" {
                let data =
                    ff.object.section(s.index).and_then(|x| x.data()).map_err(|_| resolve())?;
                notes.push((s.id, data, s.align.max(1)));
            }
        }
        c.meta_units = u32::try_from(facts.iter().filter(|f| f.kind == SectionKind::Meta).count())
            .unwrap_or(u32::MAX);
        if notes.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(&cf_string(view, &cf.strings, c.row.path)?).into_owned();
        let obj = mortar_synth::notes::ObjectNotes { path: &path, sections: &notes };
        let processor = |t: u32| arch.property_merge(t);
        let diag = mortar_diag::Collector::new(mortar_diag::Policy::default());
        let props =
            mortar_synth::notes::object_properties(&obj, arch.facts().class, &processor, &diag)
                .map_err(|_| resolve())?;
        if props.is_empty() {
            continue;
        }
        let props: Vec<(u32, &[u8])> = props.iter().map(|(t, v)| (*t, v.as_slice())).collect();
        let bytes = mortar_incr::update::rows::encode_properties(&props);
        let off = u32::try_from(base + cf.strings.len() as u64)
            .map_err(|_| too_much("the update's strings"))?;
        let len = u32::try_from(bytes.len()).map_err(|_| too_much("a property list"))?;
        off.checked_add(len).ok_or_else(|| too_much("the update's strings"))?;
        cf.strings.extend_from_slice(&bytes);
        c.row.properties = Str { off, len };
    }
    Ok(())
}

/// A `Str` of the update's space before `cf` is whole
/// (`ChangedFiles::string`'s rule over the state and `strings`).
fn cf_string(view: &StateView<'_>, strings: &[u8], s: Str) -> Result<Vec<u8>, IncrReason> {
    let base = view.count(kind::STRINGS);
    let (a, b) = (u64::from(s.off), u64::from(s.off) + u64::from(s.len));
    if b <= base {
        return view.string(s);
    }
    let (Some(a), Some(b)) = (a.checked_sub(base), b.checked_sub(base)) else {
        return Err(corrupt("a string across the state's end"));
    };
    strings.get(a as usize..b as usize).map(<[u8]>::to_vec).ok_or_else(|| corrupt("a string"))
}

// ---- the shadow (findings only): P-1's rows against front 1's -------------

/// `changed_rows_equal_the_fronts` on the real path (WP-267 P-1 step 3):
/// with `MORTAR_UPDATE_FINDINGS` set, [`changed_files_of`] and
/// [`facts_after_resolve`] beside front 1's `changed::changed_files` (the
/// oracle, over a capture of `f` made here), compared field by field; one
/// line recorded: `P-1: equal (N files, F fields)`, `finding: P-1 ...`
/// naming the field and both values, `P-1: refused by name: ...` or `P-1:
/// not compared: ...`. Nothing is returned: the update goes on with front
/// 1's rows. Without the variable it does nothing (a plain link and a real
/// update pay nothing).
pub(super) fn shadow(
    session: &crate::incremental::Session,
    view: &StateView<'_>,
    detected: &super::Detected,
    f: &crate::front::Front<'_>,
    ctx: &super::Ctx<'_>,
    cwd: &Path,
) {
    if !super::changed::recording() {
        return;
    }
    let mut p1b = None;
    let line = shadow_line(session, view, detected, f, ctx, cwd, &mut p1b).unwrap_or_else(|l| l);
    super::changed::record(&line);
    // WP-267 P-1b: `index` and `edges`, once P-1's rows equal front 1's.
    super::changed::record(
        &p1b.unwrap_or_else(|| "P-1b: not compared: P-1's rows are not equal (its line)".into()),
    );
}

/// Front 1's capture (the oracle's `FILES` rows), as `refill_capture`
/// makes it without the refill's binding parts (no `FILES` column reads
/// them).
fn front_one_capture(
    session: &crate::incremental::Session,
    view: &StateView<'_>,
    f: &crate::front::Front<'_>,
    ctx: &super::Ctx<'_>,
    cwd: &Path,
) -> Result<mortar_incr::update::testing::MemoryState, String> {
    let meta = view.meta().map_err(|e| e.report())?;
    let tree = match meta.leaf_fn {
        0 => mortar_digest::TreeFn::L128,
        1 => mortar_digest::TreeFn::Sha1,
        _ => mortar_digest::TreeFn::Sha256,
    };
    let written = mortar_image::link::Written {
        build_id: None,
        leaves: Some((tree, Vec::new())),
        sealed: Some(meta.sealed_start..meta.sealed_end),
        file_size: 0,
        file: None,
        rows: 0,
        jobs: 0,
        members: None,
    };
    let (state, rows5) = session.state(f, ctx.store, &written, cwd)?;
    let rows5 = rows5.ok_or("no format-5 rows")?;
    capture::state5_rows(&state, rows5, false)
}

#[allow(clippy::too_many_lines, reason = "the oracle, P-1's two steps, their comparison")]
fn shadow_line(
    session: &crate::incremental::Session,
    view: &StateView<'_>,
    detected: &super::Detected,
    f: &crate::front::Front<'_>,
    ctx: &super::Ctx<'_>,
    cwd: &Path,
    p1b: &mut Option<String>,
) -> Result<String, String> {
    use mortar_incr::update::{extract, resolve as res};
    let not = |what: &str, e: &dyn std::fmt::Display| format!("P-1: not compared: {what}: {e}");
    let refused = |e: &IncrReason| format!("P-1: refused by name: {}", e.report());
    // The oracle's changed set: U4's member-level judgement by the capture.
    let capture =
        front_one_capture(session, view, f, ctx, cwd).map_err(|e| not("front 1's capture", &e))?;
    let mut front_ids = Vec::new();
    for &id in &detected.changed_files {
        let now = capture.files.get(id.0 as usize).ok_or_else(|| not("FILES", &id.0))?;
        let old = view.file(id).map_err(|e| not("FILES", &e.report()))?;
        if now.hash != old.hash {
            front_ids.push(id);
        }
    }
    let mut cache = FrontCache::new(ctx.store, ctx.pool, cwd);
    // The cost of P-1's two steps (the clock of the record), timed alone.
    let at = std::time::Instant::now();
    let mut mine = match changed_files_of(
        view,
        &mut cache,
        &detected.changed_files,
        &detected.positional,
        ctx.plan,
        cwd,
    ) {
        Ok(m) => m,
        Err(e) => return Err(refused(&e)),
    };
    let pre_ms = at.elapsed().as_secs_f64() * 1e3;
    let my_ids: Vec<FileId> = mine.files.iter().map(|c| c.file).collect();
    if my_ids != front_ids {
        return Ok(format!(
            "finding: P-1 changed set: mine {:?}, front 1's {:?}",
            my_ids.iter().map(|x| x.0).collect::<Vec<_>>(),
            front_ids.iter().map(|x| x.0).collect::<Vec<_>>()
        ));
    }
    if my_ids.is_empty() {
        return Ok("P-1: equal (0 files, 0 fields)".into());
    }
    let files: Vec<(FileId, Change)> = front_ids.iter().map(|&x| (x, Change::Changed)).collect();
    let oracle = super::changed::changed_files(f, view, &files, &detected.positional, &capture)
        .map_err(|e| not("the oracle", &e.report()))?;
    let mut cmp = Cmp::default();
    compare_pre(&mut cmp, view, &mine, &oracle).map_err(|e| not("a string", &e.report()))?;
    if let Some(d) = cmp.diff.first() {
        return Ok(format!("finding: P-1 {d} ({} differ of {} fields)", cmp.diff.len(), cmp.n));
    }
    // 7b: after resolve over P-1's rows (its election's keepers).
    let options = mortar_resolve::tables::option_keys(&f.plan, &f.inputs.link.directives);
    let mut resolve_ms = 0.0;
    let mut post_ms = 0.0;
    let post = (|| {
        let at = std::time::Instant::now();
        let ex = extract::extract(view, &mine, &options).map_err(|e| ("extract", e))?;
        if !(ex.extracted.is_empty() && ex.unextracted.is_empty()) {
            // M-1: an extraction flip's members are not P-1's rows.
            return Err(("M-1: an extraction flip", resolve()));
        }
        let r = res::resolve(view, &mine, &ex, &options).map_err(|e| ("resolve", e))?;
        resolve_ms = at.elapsed().as_secs_f64() * 1e3;
        match r {
            res::Resolution::Delta(d) => {
                let at = std::time::Instant::now();
                let r =
                    facts_after_resolve(view, &cache, &mut mine, &f.plan, f.arch.get(), &d.keepers);
                post_ms = at.elapsed().as_secs_f64() * 1e3;
                r.map(|()| d).map_err(|e| ("facts_after_resolve", e))
            }
            // M-1: a draw-in is an archive member's extraction.
            res::Resolution::DrawIn(_) => Err(("M-1: a draw-in", resolve())),
        }
    })();
    let pre = cmp.n;
    let delta = match post {
        Ok(d) => d,
        Err((step, e)) => {
            return Ok(format!(
                "P-1: pre equal ({} files, {pre} fields); post refused by name: {step}: {}",
                my_ids.len(),
                e.report()
            ));
        }
    };
    compare_post(&mut cmp, view, &mine, &oracle, &capture)
        .map_err(|e| not("a string", &e.report()))?;
    if let Some(d) = cmp.diff.first() {
        return Ok(format!("finding: P-1 {d} ({} differ of {} fields)", cmp.diff.len(), cmp.n));
    }
    *p1b = Some(super::changed_edges::shadow_line(
        view,
        &cache,
        &mut mine,
        &oracle,
        &delta,
        &f.plan,
        f.arch.get(),
        &f.pool,
    ));
    // The laps: `changed_files_of`, U-3's `extract`+`resolve` over its rows
    // (not P-1's code), `facts_after_resolve`.
    Ok(format!(
        "P-1: equal ({} files, {} fields) ms changed_files_of={pre_ms:.3} \
         resolve={resolve_ms:.3} facts_after_resolve={post_ms:.3}",
        my_ids.len(),
        cmp.n
    ))
}

/// The field-by-field comparison's count and its differences.
#[derive(Default)]
struct Cmp {
    n: usize,
    diff: Vec<String>,
}

impl Cmp {
    fn eq<T: PartialEq + std::fmt::Debug>(
        &mut self,
        what: &dyn Fn() -> String,
        mine: &T,
        front: &T,
    ) {
        self.n += 1;
        if mine != front {
            self.diff.push(format!("{}: mine {mine:?}, front 1's {front:?}", what()));
        }
    }
}

/// The fields [`changed_files_of`] makes, each against the oracle's;
/// strings by BYTES.
fn compare_pre(
    cmp: &mut Cmp,
    view: &StateView<'_>,
    mine: &ChangedFiles,
    front: &ChangedFiles,
) -> Result<(), IncrReason> {
    cmp.eq(&|| "positional".into(), &mine.positional, &front.positional);
    cmp.eq(&|| "appeared".into(), &mine.appeared, &front.appeared);
    for (m, o) in mine.files.iter().zip(&front.files) {
        let id = m.file.0;
        let w = |field: &'static str| move || format!("file {id} {field}");
        cmp.eq(&w("change"), &m.change, &o.change);
        let (a, b) = (&m.row, &o.row);
        cmp.eq(&w("row.kind"), &a.kind, &b.kind);
        cmp.eq(&w("row.member"), &a.member, &b.member);
        cmp.eq(&w("row.flags"), &a.flags, &b.flags);
        cmp.eq(&w("row.path"), &mine.string(view, a.path)?, &front.string(view, b.path)?);
        cmp.eq(&w("row.mention"), &a.mention, &b.mention);
        cmp.eq(
            &w("row.member_name"),
            &mine.string(view, a.member_name)?,
            &front.string(view, b.member_name)?,
        );
        cmp.eq(&w("row.member_ordinal"), &a.member_ordinal, &b.member_ordinal);
        cmp.eq(&w("row.base"), &a.base, &b.base);
        cmp.eq(&w("row.len"), &a.len, &b.len);
        cmp.eq(&w("row.hash"), &a.hash, &b.hash);
        cmp.eq(&w("row.rank"), &a.rank, &b.rank);
        cmp.eq(&w("row.round"), &a.round, &b.round);
        cmp.eq(&w("row.stack"), &a.stack, &b.stack);
        cmp.eq(&w("first_section"), &m.first_section, &o.first_section);
        cmp.eq(&w("sections.len"), &m.sections.len(), &o.sections.len());
        for (k, (x, y)) in m.sections.iter().zip(&o.sections).enumerate() {
            let s = |field: &'static str| move || format!("file {id} section {k} {field}");
            cmp.eq(&s("file"), &x.file, &y.file);
            cmp.eq(&s("anchor_kind"), &x.anchor_kind, &y.anchor_kind);
            cmp.eq(&s("anchor"), &mine.string(view, x.anchor)?, &front.string(view, y.anchor)?);
            cmp.eq(&s("name"), &mine.string(view, x.name)?, &front.string(view, y.name)?);
            cmp.eq(&s("ordinal"), &x.ordinal, &y.ordinal);
            cmp.eq(&s("sh_type"), &x.sh_type, &y.sh_type);
            cmp.eq(&s("sh_flags"), &x.sh_flags, &y.sh_flags);
            cmp.eq(&s("size"), &x.size, &y.size);
            cmp.eq(&s("align"), &x.align, &y.align);
            cmp.eq(&s("offset"), &x.offset, &y.offset);
            cmp.eq(&s("place"), &(x.place_chunk, x.place_offset), &(y.place_chunk, y.place_offset));
        }
        cmp.eq(&w("keys"), &m.keys, &o.keys);
        cmp.eq(&w("first_global"), &m.first_global, &o.first_global);
        cmp.eq(&w("mentions"), &m.mentions, &o.mentions);
        cmp.eq(&w("values"), &m.values, &o.values);
        cmp.eq(&w("versioned"), &m.versioned, &o.versioned);
        cmp.eq(&w("odr.copies"), &m.odr, &o.odr);
        cmp.eq(&w("debug"), &m.debug, &o.debug);
    }
    Ok(())
}

/// The fields [`facts_after_resolve`] makes, each against the oracle's;
/// the oracle's `row.properties` is a `Str` of front 1's CAPTURE buffer
/// (`changed_files` moves only `path` and `member_name`), read there.
fn compare_post(
    cmp: &mut Cmp,
    view: &StateView<'_>,
    mine: &ChangedFiles,
    front: &ChangedFiles,
    capture: &mortar_incr::update::testing::MemoryState,
) -> Result<(), IncrReason> {
    use mortar_incr::format::section_bits::KEPT;
    for (m, o) in mine.files.iter().zip(&front.files) {
        let id = m.file.0;
        let w = |field: &'static str| move || format!("file {id} {field}");
        for (k, (x, y)) in m.sections.iter().zip(&o.sections).enumerate() {
            let s = |field: &'static str| move || format!("file {id} section {k} {field}");
            cmp.eq(&s("kind"), &x.kind, &y.kind);
            cmp.eq(&s("bits.KEPT"), &(x.bits & KEPT), &(y.bits & KEPT));
            cmp.eq(&s("out"), &x.out, &y.out);
        }
        let p = o.row.properties;
        let theirs = capture
            .strings
            .get(p.off as usize..p.off as usize + p.len as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| corrupt("the capture's properties"))?;
        cmp.eq(&w("row.properties"), &mine.string(view, m.row.properties)?, &theirs);
        cmp.eq(&w("meta_units"), &m.meta_units, &o.meta_units);
    }
    Ok(())
}
