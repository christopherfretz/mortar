//! Stage 3's driver (`spec/plumb.md` §12, P-D1..P-D13): a
//! dynamic executable is read, its command line and shared objects are
//! modelled, the static checks run on the output, and stage 2's engine
//! judges every input relocation against the loader's views.

use std::collections::{BTreeSet, HashMap, HashSet};

use mortar_elf::consts::{PT_DYNAMIC, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK};

use crate::dynmodel::{Model, SoDef};
use crate::dynout::{DT_VERDEF, DynOut};
use crate::image::Image;
use crate::line::{Files, Line};
use crate::loader::{self, Loader, Params};
use crate::locate::{self, Obj};
use crate::map::{Map, OutputKind};
use crate::report::{Finding, OUTSIDE, Report, Rule};
use crate::{Input, reloc, run_engine, structure};

/// What stage 3 needs beyond stage 2's arguments.
#[allow(missing_debug_implementations)]
pub struct Ctx<'f> {
    /// The linker's words (`--inputs-from`'s line, or the mortar map's
    /// `arg` records when `None`).
    pub line: Option<Vec<String>>,
    pub files: &'f dyn Files,
}

/// The load bias of view 2 (64 KiB-aligned, far from every other address,
/// with low 32 bits that are not 0 so a 32-bit field cannot hide it).
pub const BIAS2: u64 = 0x5555_5a5a_0000;
/// Where view 3 defines the `WeakDyn` keys.
pub const FRESH: u64 = 0x6000_0000_0000;

/// Whether the output is dynamic: it has `PT_DYNAMIC` (D-1: `Exec`,
/// `Pie`, `StaticPie`).
pub fn is_dynamic(output: &[u8]) -> bool {
    Image::read(output).is_ok_and(|i| i.phdrs.iter().any(|p| p.p_type == PT_DYNAMIC))
}

/// Whether section `si` of `o` has a record naming one of `names`.
pub(crate) fn names_any(o: &Obj<'_>, si: usize, names: &HashSet<String>) -> bool {
    let (Some(t), Some(rs)) = (o.symtab, o.secs[si].relocs) else { return false };
    let Some(raw) = o.records(rs) else { return false };
    raw.iter().any(|r| {
        t.get(mortar_elf::SymbolIndex(r.1))
            .ok()
            .and_then(|s| t.name(&s).ok())
            .is_some_and(|n| names.contains(String::from_utf8_lossy(n).as_ref()))
    })
}

/// Verifies a link (§2), with stage 3 for a dynamic output. A static
/// output is verified as [`crate::verify`] does.
#[allow(clippy::too_many_lines)] // the views in order
pub fn verify_with(output: &[u8], inputs: &[Input<'_>], map: &Map, ctx: &Ctx<'_>) -> Report {
    let mut report = Report::default();
    let image = match Image::read(output) {
        Ok(i) => i,
        Err(e) => {
            report.findings.push(Finding::new(Rule::OutputUnreadable, e));
            return report;
        }
    };
    let Some(dout) = DynOut::read(&image) else {
        return crate::verify(output, inputs, map);
    };
    report.findings.extend(structure::check(&image));
    report.findings.extend(structure::check_layout(&image, map.format));
    let located = locate::locate(map, inputs, &image);
    report.findings.extend(located.findings);
    let objs = located.objs;
    let args = ctx
        .line
        .clone()
        .or_else(|| map.dynamic.as_ref().map(|d| d.args.clone()).filter(|a| !a.is_empty()));
    let Some(args) = args else {
        report.findings.push(Finding::new(
            Rule::Dyn(2),
            "a dynamic output and no command line: stage 3 needs the link's words",
        ));
        report.sort();
        return report;
    };
    let line = Line::parse(&args, ctx.files);
    // P-D1: a link stage 3 does not model is reported by name, as one
    // finding each, and nothing else is judged (§12.1).
    let mut outside: Vec<String> = line.outside.clone();
    // Stage 3's rules are written for RELA targets (AArch64, x86-64);
    // an i386 dynamic output (REL records, `DT_REL`, any kind) is not
    // modelled: one P-D1 line, nothing judged by them (§12.1).
    if image.header.e_machine == mortar_elf::consts::EM_386 {
        outside = vec!["i386 (EM_386: REL records and DT_REL; stage 3 models the RELA targets AArch64 and x86-64, dyn.md D-1)".into()];
    } else if dout.has(DT_VERDEF) {
        outside.push("the output defines versions (.gnu.version_d; dyn.md D-27, M4)".into());
    }
    if !outside.is_empty() {
        for o in outside {
            report.findings.push(Finding::new(Rule::Dyn(1), format!("{OUTSIDE}{o}")));
        }
        report.sort();
        return report;
    }
    for p in &line.problems {
        report.findings.push(Finding::new(Rule::Dyn(2), format!("the command line: {p}")));
    }
    if let Some(d) = &map.dynamic {
        report.findings.extend(map_facts(&line, d));
    }
    // file 0's definitions: the output's defined globals no input and no
    // shared object defines (linker-defined symbols).
    let mut input_defs: HashSet<String> = HashSet::new();
    for o in &objs {
        let Some(t) = o.symtab else { continue };
        for (_, s) in t.iter() {
            if s.st_shndx != 0
                && matches!(s.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
                && let Ok(n) = t.name(&s)
            {
                input_defs.insert(String::from_utf8_lossy(n).into_owned());
            }
        }
    }
    let pre: Vec<String> = image
        .symbols
        .iter()
        .filter(|s| {
            s.sym.st_shndx != 0
                && matches!(s.sym.binding(), STB_GLOBAL | STB_WEAK)
                && !input_defs.contains(&s.name)
        })
        .map(|s| s.name.clone())
        .collect();
    let mut model = Model::build(&line, &objs, &[], ctx.files);
    let file0: Vec<String> = pre
        .into_iter()
        .filter(|n| {
            !model.dsos.iter().any(|d| d.defs.get(n).is_some_and(|v| v.iter().any(SoDef::visible)))
        })
        .collect();
    if !file0.is_empty() {
        model = Model::build(&line, &objs, &file0, ctx.files);
    }
    for p in &model.problems {
        report.findings.push(Finding::new(Rule::Dyn(2), p.clone()));
    }
    for o in &model.outside {
        report.findings.push(Finding::new(Rule::Dyn(1), format!("{OUTSIDE}{o}")));
    }
    if !line.problems.is_empty() || !model.problems.is_empty() || !model.outside.is_empty() {
        // P-D2: an input the verifier cannot read is an error; nothing
        // computed without it is judged.
        report.sort();
        return report;
    }
    // The loader's order: the image's own DT_NEEDED.
    let mut needed_idx = Vec::new();
    for n in &dout.needed {
        match model.by_soname(n) {
            Some(i) => needed_idx.push(i),
            None => report.findings.push(Finding::new(
                Rule::Dyn(6),
                format!("DT_NEEDED '{n}' names no shared object of the link"),
            )),
        }
    }
    let ld = Loader {
        model: &model,
        order: model.loader_order(&needed_idx),
        machine: image.header.e_machine,
    };
    let kind = line.kind.unwrap_or(OutputKind::Exec);
    report
        .findings
        .extend(crate::dyncheck::check(&image, &dout, &line, &model, &ld, &objs, map.format));
    // View 1: bias 0.
    let p1 = Params::default();
    let v1 = loader::apply(&image, &dout, &ld, &p1, true);
    report.findings.extend(v1.findings.iter().cloned());
    let Ok(img1) = Image::read(&v1.bytes) else {
        report.sort();
        return report;
    };
    let plt = loader::plt_entries(&image);
    let mut engine = reloc::Engine::new(&img1, &objs, map.format);
    dyn_engine(&mut engine, &v1, &plt);
    run_engine(&mut engine, &image, map, None);
    let abs = std::mem::take(&mut engine.abs_words);
    if kind == OutputKind::StaticPie {
        // §12.4: no RELATIVE record on a word that holds no address of the
        // executable (what view 2 cannot see when the word is not an
        // input's absolute use).
        let known: BTreeSet<u64> = abs.iter().filter(|w| w.3).map(|w| w.0).collect();
        for r in &dout.rels {
            if loader::kind(image.header.e_machine, r.ty) == loader::Kind::Relative
                && !known.contains(&r.offset)
            {
                let placed = engine.occupied_at(r.offset);
                // A linker-made word (a GOT slot no reference left in use)
                // holding an address of the image is an address word.
                let a = r.addend.unwrap_or(0) as u64;
                let inside = image.phdrs.iter().any(|p| {
                    p.p_type == mortar_elf::consts::PT_LOAD
                        && p.p_vaddr <= a
                        && a <= p.p_vaddr.saturating_add(p.p_memsz)
                });
                if !placed && inside {
                    continue;
                }
                report.findings.push(
                    Finding::new(
                        Rule::Dyn(9),
                        if placed {
                            "a RELATIVE record on a word of an input section that no input relocation makes an address of the executable"
                        } else {
                            "a RELATIVE record on a linker-made word that holds no address of the executable the inputs ask for"
                        },
                    )
                    .at(r.offset),
                );
            }
        }
    }
    let chosen = std::mem::take(&mut engine.ifunc_chosen);
    report.findings.extend(exported_ifuncs(&image, &dout, &model, &chosen, map.format));
    let view1_findings = std::mem::take(&mut engine.findings);
    report.coverage = std::mem::take(&mut engine.cov);
    drop(engine);
    // View 2: a second bias (Pie): every word that holds an address of the
    // executable moves with it, every other word does not.
    if kind == OutputKind::Pie || image.header.e_type == mortar_elf::consts::ET_DYN {
        let p2 = Params { bias: BIAS2, ..Params::default() };
        let v2 = loader::apply(&image, &dout, &ld, &p2, false);
        let mut seen = BTreeSet::new();
        for &(at, w, v, internal) in &abs {
            if !seen.insert(at) {
                continue;
            }
            let Some(off) = file_off(&image, at, u64::from(w)) else { continue };
            let mut b = [0u8; 8];
            b[..w as usize].copy_from_slice(&v2.bytes[off..off + w as usize]);
            let found = u64::from_le_bytes(b);
            let want = if internal { v.wrapping_add(BIAS2) } else { v };
            let mask = if w == 8 { u64::MAX } else { (1u64 << (8 * w)) - 1 };
            if found & mask != want & mask {
                let mut f = Finding::new(
                    Rule::Dyn(9),
                    format!(
                        "view 2 (load bias {BIAS2:#x}): the word holds {found:#x}, the program needs {:#x}{}",
                        want & mask,
                        if internal {
                            " (no record moves it with the executable)"
                        } else if found & mask == v.wrapping_add(BIAS2) & mask {
                            " (a record moves a value that is not an address of the executable)"
                        } else {
                            ""
                        }
                    ),
                )
                .at(at);
                f.expected = Some(want & mask);
                f.found = Some(found);
                report.findings.push(f);
            }
        }
    }
    // View 3: every WeakDyn key (an undefined weak `.dynsym` entry the
    // loader finds no definition for) defined at a fresh address.
    let mut fresh: HashMap<String, u64> = HashMap::new();
    for (j, s) in dout.syms.iter().enumerate() {
        if j > 0 && !s.defined() && s.binding() == STB_WEAK && v1.dynbind.get(&s.name) == Some(&0) {
            let n = fresh.len() as u64;
            fresh.insert(s.name.clone(), FRESH + n * 0x1000);
        }
    }
    if !fresh.is_empty() {
        let p3 = Params { bias: 0, fresh: fresh.clone() };
        let v3 = loader::apply(&image, &dout, &ld, &p3, false);
        if let Ok(img3) = Image::read(&v3.bytes) {
            let names: HashSet<String> = fresh.keys().cloned().collect();
            let mut e3 = reloc::Engine::new(&img3, &objs, map.format);
            dyn_engine(&mut e3, &v3, &plt);
            run_engine(&mut e3, &image, map, Some(&names));
            let mut bad: HashMap<String, Vec<Finding>> = HashMap::new();
            let mut ok: HashMap<String, u64> = HashMap::new();
            for f in e3.findings.drain(..) {
                if let Some(s) = &f.symbol
                    && names.contains(s)
                {
                    bad.entry(s.clone()).or_default().push(f);
                }
            }
            for k in &names {
                ok.insert(k.clone(), uses_of(&objs, k));
            }
            for (k, fs) in bad {
                let n = ok.get(&k).copied().unwrap_or(0);
                // The cause, named: no record names the key (its `.dynsym`
                // entry is inert; D-25 notes that no reference writes one
                // for an absolute use in `-no-pie`).
                let recorded = dout.rels.iter().any(|r| {
                    r.sym != 0 && dout.syms.get(r.sym as usize).is_some_and(|s| s.name == k)
                });
                let cause = if recorded { "" } else { "; no dynamic record names it (D-25, W12)" };
                // Every use must reach the definition. A use whose field
                // is too narrow for it (an input's 32-bit absolute use in a
                // no-PIE executable) cannot whatever the linker writes, and
                // fails only when other uses do reach it (a split: W12, F2).
                let split = (fs.len() as u64) < n;
                for f in fs.into_iter().filter(|f| split || f.rule != Rule::Overflow) {
                    {
                        let mut g = Finding::new(
                            Rule::Dyn(9),
                            format!(
                                "view 3 ('{k}' defined by a preloaded library): this use ({n} in all) still sees the absent definition ({}){cause}",
                                f.message
                            ),
                        );
                        g.addr = f.addr;
                        g.site = f.site;
                        g.reloc = f.reloc;
                        g.symbol = Some(k.clone());
                        g.expected = f.expected;
                        g.found = f.found;
                        report.findings.push(g);
                    }
                }
            }
        }
    }
    report.findings.extend(view1_findings);
    report.sort();
    report
}

fn file_off(image: &Image<'_>, addr: u64, len: u64) -> Option<usize> {
    image.phdrs.iter().filter(|p| p.p_type == mortar_elf::consts::PT_LOAD).find_map(|p| {
        let off = addr.checked_sub(p.p_vaddr)?;
        (off.checked_add(len)? <= p.p_filesz).then(|| (p.p_offset + off) as usize)
    })
}

/// The number of input relocation records naming `name` in placed
/// sections.
fn uses_of(objs: &[Obj<'_>], name: &str) -> u64 {
    let mut n = 0;
    for o in objs {
        let Some(t) = o.symtab else { continue };
        for s in &o.secs {
            let Some(rs) = s.relocs else { continue };
            if matches!(s.place, locate::Place::Absent | locate::Place::Discarded) || !s.alloc() {
                continue;
            }
            let Some(raw) = o.records(rs) else { continue };
            for r in raw {
                if t.get(mortar_elf::SymbolIndex(r.1))
                    .ok()
                    .and_then(|x| t.name(&x).ok())
                    .is_some_and(|x| x == name.as_bytes())
                {
                    n += 1;
                }
            }
        }
    }
    n
}

/// An IFUNC the executable exports (W13): the loader gives a library the
/// resolver's result, so the executable's own uses must reach it too, and
/// x86's loader refuses a library reference to an executable IFUNC.
fn exported_ifuncs(
    image: &Image<'_>,
    out: &DynOut,
    model: &Model,
    chosen: &std::collections::BTreeMap<u64, Option<u64>>,
    format: crate::map::Format,
) -> Vec<Finding> {
    let mut v = Vec::new();
    let x86 = image.header.e_machine != mortar_elf::consts::EM_AARCH64;
    for s in out.syms.iter().skip(1) {
        if !s.defined() || s.sym_type() != mortar_elf::consts::STT_GNU_IFUNC {
            continue;
        }
        let mut f = |n: u8, m: String| {
            let mut x = Finding::new(Rule::Dyn(n), m).at(s.st_value);
            x.symbol = Some(s.name.clone());
            v.push(x);
        };
        if format == crate::map::Format::Mortar {
            f(
                7,
                format!(
                    "the executable's IFUNC '{}' is exported as STT_GNU_IFUNC (D-23, W13)",
                    s.name
                ),
            );
            continue;
        }
        let referenced = model.ref_dynamic.contains(&s.name);
        if x86 && referenced {
            f(
                7,
                format!(
                    "the executable's IFUNC '{}' is exported as STT_GNU_IFUNC and a library references it: x86's loader refuses the load (D-23, W13)",
                    s.name
                ),
            );
            continue;
        }
        match chosen.get(&s.st_value) {
            None => f(
                7,
                format!(
                    "'{}' is exported as STT_GNU_IFUNC at {:#x}, which is no resolver of the program (W13)",
                    s.name, s.st_value
                ),
            ),
            Some(Some(e)) => f(
                9,
                format!(
                    "'{}' is exported as STT_GNU_IFUNC: a library gets its resolver's result, the executable takes IPLT entry {e:#x}: two addresses for one function (D-23, W13)",
                    s.name
                ),
            ),
            Some(None) => {}
        }
    }
    v
}

/// Gives stage 2's engine a view: the loader's bindings, the TLS block
/// the view uses, the PLT entries.
/// P-D3: the map's facts of the command line (`output`, `interp`,
/// `runpath`/`rpath`, `bind`, `relro`; §3.2) agree with its words. The
/// hints are not compared: they only name what stage 3 recomputes.
fn map_facts(line: &Line, d: &crate::map::DynRecords) -> Vec<Finding> {
    use crate::map::RunpathTag;
    let mut v = Vec::new();
    let mut fact = |what: &str, map: String, words: String| {
        if map != words {
            v.push(Finding::new(
                Rule::Dyn(3),
                format!("the map's {what} record says {map}; the command line says {words}"),
            ));
        }
    };
    if let Some(k) = d.kind {
        fact("output", format!("{k:?}"), format!("{:?}", line.kind.unwrap_or(OutputKind::Exec)));
    }
    if d.interp.is_some() || line.interp.is_some() {
        fact("interp", format!("{:?}", d.interp), format!("{:?}", line.interp));
    }
    let words = (!line.rpath.is_empty()).then(|| {
        let tag = if line.new_dtags { RunpathTag::Runpath } else { RunpathTag::Rpath };
        (tag, line.rpath.join(":"))
    });
    fact("runpath/rpath", format!("{:?}", d.runpath), format!("{words:?}"));
    if let Some(now) = d.now {
        fact("bind", format!("now {now}"), format!("now {}", line.now));
    }
    if let Some(relro) = d.relro {
        fact("relro", format!("on {relro}"), format!("on {}", line.relro));
    }
    v
}

fn dyn_engine(
    e: &mut reloc::Engine<'_, '_>,
    v: &loader::View,
    plt: &std::collections::BTreeMap<u64, u64>,
) {
    e.dynbind = v.dynbind.iter().map(|(k, x)| (k.clone(), *x)).collect();
    e.dyn_tls.clone_from(&v.dyn_tls);
    e.dyn_tls_mod = v.dyn_tls_mod.iter().map(|(k, x)| (k.clone(), *x)).collect();
    e.plt = plt.clone();
    e.dynamic = true;
    if matches!(e.tls, Ok(None)) && !v.dyn_tls.is_empty() {
        e.tls = Ok(Some(reloc::TlsBlock { start: v.tls.0, tcb: v.tls.1 }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoFiles;
    impl Files for NoFiles {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
    }

    // spec: plumb.md §3.2 — the map's facts are recomputed from its `arg`
    // words and a disagreeing record is reported (P-D3).
    #[test]
    fn s3_2_map_facts_agree_with_the_words() {
        let words: Vec<String> =
            "-pie -z now -dynamic-linker /lib/ld.so.1 -rpath /a -rpath $ORIGIN -o x"
                .split(' ')
                .map(str::to_string)
                .collect();
        let line = Line::parse(&words, &NoFiles);
        let mut d = crate::map::DynRecords {
            kind: Some(OutputKind::Pie),
            interp: Some("/lib/ld.so.1".into()),
            runpath: Some((crate::map::RunpathTag::Runpath, "/a:$ORIGIN".into())),
            now: Some(true),
            relro: Some(true),
            ..Default::default()
        };
        assert!(map_facts(&line, &d).is_empty(), "{:?}", map_facts(&line, &d));
        d.kind = Some(OutputKind::Exec);
        d.interp = Some("/lib/ld-other.so".into());
        d.runpath = Some((crate::map::RunpathTag::Rpath, "/a:$ORIGIN".into()));
        d.now = Some(false);
        d.relro = Some(false);
        let f = map_facts(&line, &d);
        assert_eq!(f.len(), 5, "{f:?}");
        assert!(f.iter().all(|x| x.rule == Rule::Dyn(3)));
    }
}
