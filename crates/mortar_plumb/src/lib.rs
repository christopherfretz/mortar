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
mod erratum;
mod image;
mod locate;
pub mod map;
pub mod mutate;
mod property;
mod reloc;
pub mod report;
mod structure;
mod symbols;
pub mod x64;
mod x86op;

pub use locate::thin_members;
pub use map::{Format, Map, MapError, MapFile};
pub use report::{Coverage, Finding, Reason, Report, Rewrite, Rule, Site, Via};

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
    for (oi, o) in objs.iter().enumerate() {
        for (si, s) in o.secs.iter().enumerate() {
            if s.consumed() {
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
    engine.check_eh_frame();
    content::check_content(&mut engine);
    erratum::check(&mut engine, map);
    content::check_init_padding(&mut engine);
    content::check_symbols(&mut engine);
    symbols::check(&mut engine);
    let props = property::check(&image, &objs, map.format, &mut engine.cov);
    engine.findings.extend(props);
    if map.format == Format::Mortar {
        let ids = buildid::check(&image, &mut engine.cov, buildid::Style::of(map));
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
    report.findings.append(&mut engine.findings);
    report.coverage = std::mem::take(&mut engine.cov);
    report.sort();
    report
}
