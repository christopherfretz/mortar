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
mod content;
mod dyncheck;
mod dynmodel;
mod dynmutate;
pub mod dynout;
mod dynverify;
mod erratum;
mod hasher;
mod image;
pub mod line;
mod loader;
mod locate;
pub mod map;
pub mod mutate;
mod plt;
mod property;
mod reloc;
mod spie;
pub mod report;
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
pub use report::{Coverage, Finding, OUTSIDE, Reason, Report, Rewrite, Rule, Site, Via};

/// An input file of the link, by the path the map names it with (§2.1).
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
}

/// Verifies a link (§2): the output's bytes, the inputs the map names, and
/// the map. Never panics on any input; what it cannot read is a finding.
pub fn verify(output: &[u8], inputs: &[Input<'_>], map: &Map) -> Report {
    let mut report = Report::default();
    let image = match image::Image::read(output) {
        Ok(i) => i,
        Err(e) => {
            report.findings.push(Finding::new(Rule::OutputUnreadable, e));
            return report;
        }
    };
    report.findings.extend(structure::check(&image));
    report.findings.extend(structure::check_layout(&image, map.format));
    let located = locate::locate(map, inputs, &image);
    report.findings.extend(located.findings);
    let objs = located.objs;
    let mut engine = reloc::Engine::new(&image, &objs, map.format);
    run_engine(&mut engine, &image, map, None);
    report.findings.append(&mut engine.findings);
    report.coverage = std::mem::take(&mut engine.cov);
    report.sort();
    report
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
    engine.finish_ifunc();
    engine.check_landing_pads();
    engine.finish_got();
    engine.finish_pieces();
    if only.is_some() {
        return;
    }
    engine.check_eh_frame();
    content::check_content(engine);
    erratum::check(engine, map);
    content::check_init_padding(engine);
    content::check_symbols(engine);
    symbols::check(engine);
    let props = property::check(image, objs, map.format, &mut engine.cov);
    engine.findings.extend(props);
    if map.format == Format::Mortar {
        let ids = buildid::check(image, &mut engine.cov, buildid::Style::of(map));
        engine.findings.extend(ids);
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
