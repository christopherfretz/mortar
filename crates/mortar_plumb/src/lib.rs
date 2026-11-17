//! The independent verifier (`spec/plumb.md`): given a finished link's
//! output, its inputs and a map of where each input section went, it
//! recomputes what the output must hold from the specifications (gABI,
//! AAELF64, the A64 encodings) and reports every difference.
//!
//! It shares nothing with the linker but the ELF reader and the diagnostic
//! type (`docs/AGENTS.md`, rule 10).

#![forbid(unsafe_code)]

pub mod a64;
pub mod anomaly;
pub mod buildid;
mod cfa;
mod content;
pub mod dead;
mod deaddwarf;
mod dyncheck;
mod dynfields;
mod dynmodel;
mod dynmutate;
pub mod dynout;
mod dynverify;
pub mod equiv;
mod erratum;
mod fate;
mod gdbdwarf;
mod gdbindex;
mod gdbnames;
mod hasher;
mod image;
mod inset;
pub mod line;
mod loader;
mod locate;
pub mod map;
mod maprec;
mod meta;
pub mod mutate;
mod odrtable;
pub mod opts;
mod plt;
pub mod prof;
mod property;
mod reloc;
pub mod report;
pub mod shared;
pub mod sharedflow;
mod spie;
mod store;
mod structure;
pub mod sweep;
mod symbols;
pub mod x64;
pub mod x86;
mod x86op;

pub use dynverify::{Ctx, is_dynamic, verify_with};

/// The verifier's highest stage (`spec/plumb.md` §1): 3 since WP-050 —
/// [`verify_with`] judges dynamic executables (§12). A caller that must
/// know whether a dynamic output is judged (the link-test harness's
/// probe, `spec/harness.md` "L3") reads this instead of probing.
pub const STAGE: u8 = 3;
pub use line::{Files, HostFiles};
pub use locate::thin_members;
pub use map::{Format, Map, MapError, MapFile};
pub use report::{
    Coverage, Finding, MetaCoverage, OUTSIDE, Reason, Report, Rewrite, Rule, Site, Via,
};

/// An input file of the link, by the path the map names it with (§2.1).
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

/// The link's input files, read by position (§8.1; WP-109 §6.3): the
/// verifier does no I/O of its own, the caller reads. Implemented by the
/// command over open files and by `[Input]` (whole bytes) for the tests.
///
/// A source is a list of files by path (§2.2: a thin archive's members
/// are files of their own); the members of a regular archive are found by
/// the verifier, which reads the archive's headers through the source.
pub trait Source: Sync {
    /// The number of files.
    fn count(&self) -> usize;
    /// The path of file `i`, as the map names it.
    fn path(&self, i: usize) -> &str;
    /// The length of file `i` in bytes.
    fn len(&self, i: usize) -> u64;
    /// Exactly `buf.len()` bytes of file `i` from `offset`. A read that
    /// cannot be made whole (past the end, a file that shrank since it was
    /// opened, an I/O error) is an error naming the file, never a panic.
    ///
    /// # Errors
    /// The message names the file and what failed.
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String>;
}

/// Whole bytes as a source (the tests', and the callers' that hold every
/// input in memory): `&inputs[..]` or `&inputs` (a `Vec`).
impl Source for &[Input<'_>] {
    fn count(&self) -> usize {
        <[Input<'_>]>::len(self)
    }
    fn path(&self, i: usize) -> &str {
        self.get(i).map_or("", |f| f.path)
    }
    fn len(&self, i: usize) -> u64 {
        self.get(i).map_or(0, |f| f.bytes.len() as u64)
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        whole_read_at(self, i, offset, buf)
    }
}

impl Source for Vec<Input<'_>> {
    fn count(&self) -> usize {
        <[Input<'_>]>::len(self)
    }
    fn path(&self, i: usize) -> &str {
        self.get(i).map_or("", |f| f.path)
    }
    fn len(&self, i: usize) -> u64 {
        self.get(i).map_or(0, |f| f.bytes.len() as u64)
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        whole_read_at(self, i, offset, buf)
    }
}

fn whole_read_at(files: &[Input<'_>], i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
    let Some(f) = files.get(i) else { return Err(format!("no input {i}")) };
    let got =
        usize::try_from(offset).ok().and_then(|at| f.bytes.get(at..at.checked_add(buf.len())?));
    match got {
        Some(b) => {
            buf.copy_from_slice(b);
            Ok(())
        }
        None => Err(format!(
            "{}: {} bytes at offset {offset:#x} are past its end ({} bytes)",
            f.path,
            buf.len(),
            f.bytes.len()
        )),
    }
}

/// Verifies a link (§2) whose inputs are read through `src`, with stage 3
/// for a dynamic output (§12; a static output is judged as [`verify`]
/// judges it). The verdict is [`verify_with`]'s on the same bytes: only the
/// objects' skeletons are held, the rest is read when a rule needs it
/// (§8.1).
pub fn verify_from(output: &[u8], src: &dyn Source, map: &Map, ctx: &Ctx<'_>) -> Report {
    dynverify::verify_src(output, src, map, ctx)
}

/// Verifies a link (§2): the output's bytes, the inputs the map names, and
/// the map. Never panics on any input; what it cannot read is a finding.
pub fn verify(output: &[u8], inputs: &[Input<'_>], map: &Map) -> Report {
    verify_static(output, &inputs, map)
}

/// The I2 findings of the reads that failed after the skeletons were read
/// (§8.1: a file that changed or shrank since it was opened), in the
/// report's order.
pub(crate) fn read_errors(store: &store::Store<'_>, report: &mut Report) {
    let errors = store.take_errors();
    if !errors.is_empty() {
        report.findings.extend(errors.into_iter().map(|e| Finding::new(Rule::InputUnreadable, e)));
        report.sort();
    }
}

/// [`verify`] over a source.
pub(crate) fn verify_static(output: &[u8], src: &dyn Source, map: &Map) -> Report {
    let store = store::Store::read(src, &[map]);
    prof::mark("skeletons");
    let mut report = verify_static_in(output, &store, 0, map);
    read_errors(&store, &mut report);
    report
}

/// [`verify`] over the inputs of map `m` of `store` (the caller reports
/// the reads that fail, [`read_errors`]).
pub(crate) fn verify_static_in(
    output: &[u8],
    store: &store::Store<'_>,
    m: usize,
    map: &Map,
) -> Report {
    let mut report = Report::default();
    let image = match image::Image::read(output) {
        Ok(i) => i,
        Err(e) => {
            report.findings.push(Finding::new(Rule::OutputUnreadable, e));
            return report;
        }
    };
    prof::mark("image");
    report.findings.extend(structure::check(&image));
    report.findings.extend(structure::check_layout(&image, map));
    prof::mark("structure");
    let located = locate::locate(map, store, m, &image);
    report.findings.extend(located.findings);
    let objs = located.objs;
    prof::mark("locate");
    if prof::enabled() {
        gauges(store, map, &objs);
    }
    let mut engine = reloc::Engine::new(&image, &objs, map.format);
    prof::mark("engine new");
    engine.exports = opts::of(map).and_then(|o| fate::static_exports(map, &o));
    run_engine(&mut engine, &image, map, None);
    report.findings.append(&mut engine.findings);
    // M8 (§3.3): an output with no dynamic section has no COPY record.
    if map.format == Format::Mortar && dynout::DynOut::read(&image).is_none() {
        report.findings.extend(maprec::copies(map, &[]));
    }
    report.coverage = std::mem::take(&mut engine.cov);
    // The line's options (§3.2a), from a mortar map's `arg` records.
    let opts = opts::of(map);
    if let Some(o) = &opts {
        report.findings.extend(opts::unread_findings(o));
        // S10 (§3.2a): a static output's PT_GNU_STACK's PF_X is the
        // line's (L-84); a dynamic output's is P-D4's, in stage 3.
        let dynamic = dynout::DynOut::read(&image).is_some();
        if let Some((m, at)) = dyncheck::stack(&image, o.execstack, &objs, map.format)
            && !dynamic
        {
            report.findings.push(Finding::new(Rule::Structure(10), m).at(at));
        }
    }
    // Stage 4 (§13).
    let meta = odrtable::check(&image, &objs, map, opts.as_ref(), None, &mut report.coverage.meta);
    report.findings.extend(meta);
    report.sort();
    prof::mark("odr table, sort");
    report
}

/// The sizes of the inputs, the map's rows and the located objects
/// (WP-109's table), under [`prof::enabled`].
fn gauges(store: &store::Store<'_>, map: &Map, objs: &[locate::Obj<'_>]) {
    use std::mem::size_of;
    let n = |len: usize, size: usize| (len * size) as u64;
    prof::gauge("inputs held (skeletons, objects read whole)", store.resident());
    prof::gauge("map files", n(map.files.len(), size_of::<MapFile>()));
    prof::gauge("map placed", n(map.placed.len(), size_of::<map::Placed>()));
    prof::gauge("map discarded", n(map.discarded.len(), size_of::<map::Discarded>()));
    prof::gauge("map nonalloc", n(map.nonalloc.len(), size_of::<map::NonAlloc>()));
    prof::gauge("map chunks", n(map.chunks.len(), size_of::<map::Chunk>()));
    prof::gauge("map defined", n(map.defined.len(), size_of::<map::Defined>()));
    let secs: usize = objs.iter().map(|o| o.secs.len()).sum();
    let names: usize = objs.iter().flat_map(|o| &o.secs).map(|s| s.name.capacity()).sum();
    let pieces: usize = objs
        .iter()
        .flat_map(|o| &o.secs)
        .map(|s| match &s.place {
            locate::Place::Pieces(p) => p.capacity() * size_of::<locate::Piece>(),
            _ => 0,
        })
        .sum();
    prof::gauge(
        &format!("objs ({}) secs ({secs})", objs.len()),
        n(secs, size_of::<locate::Sec<'_>>()),
    );
    prof::gauge("objs section names", names as u64);
    prof::gauge("objs pieces", pieces as u64);
    let syms: usize = objs.iter().map(|o| o.symtab.map_or(0, |t| t.len() as usize)).sum();
    prof::gauge(&format!("input symbols ({syms}), not held"), 0);
}

/// Stage 2's judgement over every placed section, then the checks that
/// follow it (§5, §6). `only` limits the sections to those with a record
/// naming one of the names (stage 3, view 3).
pub(crate) fn run_engine(
    engine: &mut reloc::Engine<'_, '_>,
    image: &image::Image<'_>,
    map: &Map,
    only: Option<&std::collections::HashSet<String>>,
) {
    let objs = engine.objs;
    for (oi, o) in objs.iter().enumerate() {
        if prof::enabled() && oi % 400 == 399 {
            prof::mark(&format!("records of {} objects", oi + 1));
            engine.gauges();
        }
        // Every placed section is judged: one read of the object (§8.1).
        if only.is_none() {
            o.load();
        }
        for (si, s) in o.secs.iter().enumerate() {
            if s.consumed() {
                continue;
            }
            if let Some(names) = only
                && !dynverify::names_any(o, si, names)
            {
                continue;
            }
            match s.place {
                locate::Place::Whole(_) => engine.cov.sections_placed += 1,
                locate::Place::Pieces(_) => engine.cov.sections_pieces += 1,
                locate::Place::Merged => engine.cov.sections_merged += 1,
                locate::Place::Discarded => engine.cov.sections_discarded += 1,
                locate::Place::Absent | locate::Place::Rewritten | locate::Place::Ambiguous => {
                    engine.cov.sections_absent += 1;
                }
            }
            engine.section(oi, si);
        }
    }
    prof::mark("records (engine.section)");
    if prof::enabled() {
        engine.gauges();
    }
    engine.finish_ifunc();
    engine.check_landing_pads();
    engine.finish_got();
    engine.finish_pieces();
    prof::mark("finish ifunc, landing pads, got, pieces");
    if only.is_some() {
        return;
    }
    engine.check_eh_frame();
    prof::mark("eh_frame");
    content::check_content(engine);
    prof::mark("content (S12)");
    erratum::check(engine, map);
    content::check_init_padding(engine);
    content::check_symbols(engine);
    prof::mark("erratum, padding, content symbols");
    symbols::check(engine);
    prof::mark("symbols");
    let (gdb, line) = gdbindex::check(image, objs, map);
    engine.findings.extend(gdb);
    engine.cov.gdb_index = line;
    prof::mark("gdb_index");
    let props = property::check(image, objs, map.format, &mut engine.cov);
    engine.findings.extend(props);
    prof::mark("properties");
    if map.claims_coverage() {
        dead::check(engine, map);
        prof::mark("dead ranges");
    }
    if map.format == Format::Mortar {
        let ids = buildid::check(image, &mut engine.cov, buildid::Style::of(map));
        engine.findings.extend(ids);
        if let Some(o) = opts::of(map) {
            engine.findings.extend(buildid::line_rules(image, map, &o));
        }
        prof::mark("build id");
        // M10 (§3.3): every section without a place has a reason.
        if only.is_none()
            && let Some(o) = opts::of(map)
        {
            engine.findings.extend(fate::check(objs, map, &o, engine.exports.as_ref()));
        }
        // M6, M7 (§3.3): the allocation records, in every mortar map.
        engine.findings.extend(maprec::check(image, objs, map));
        prof::mark("allocation records");
    }
    if image.header.e_machine == mortar_elf::consts::EM_X86_64 {
        use mortar_elf::consts::{SHF_ALLOC, SHF_X86_64_LARGE};
        engine.cov.placement_not_judged = image
            .sections
            .iter()
            .filter(|s| {
                s.shdr.sh_flags & (SHF_ALLOC | SHF_X86_64_LARGE) == SHF_ALLOC | SHF_X86_64_LARGE
            })
            .map(|s| s.name.clone())
            .collect();
    }
}
