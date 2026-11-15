//! P1 and P2, program properties (`spec/plumb.md` §6.6): the output's
//! `.note.gnu.property` is the merge of the inputs' notes by the rule of
//! the linker that wrote it, and `PT_GNU_PROPERTY` covers it.
//!
//! The merge rules are written from `spec/synth.md` S-94 to S-97 and
//! S-126 and the observations of its §13 table, not from mortar's code.

use std::collections::BTreeMap;

use mortar_elf::consts::{PT_GNU_PROPERTY, PT_NOTE, SHF_ALLOC, SHT_NOTE};

use crate::image::Image;
use crate::locate::Obj;
use crate::map::Format;
use crate::report::{Coverage, Finding, Rule};

const NAME: &str = ".note.gnu.property";
const NT_GNU_PROPERTY_TYPE_0: u32 = 5;
const EM_386: u16 = 3;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;

/// One note's properties: type to data.
type Props = BTreeMap<u32, Vec<u8>>;

/// How the output property of a type is formed (synth.md S-95).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merge {
    /// AND of every object, an absent one counting 0; omitted if 0.
    And,
    /// OR of the objects that have it; present if any has it.
    Or,
    /// OR of every object; present only if every object has it.
    OrAll,
    /// The largest value of the objects that have it.
    Max,
    /// No data; present if any object has it.
    Present,
    /// Copied when every object that has it has the same bytes.
    EqualOrAbsent,
}

/// What a linker does with a type: merges it, drops it (the output must
/// not have it), or the verifier does not judge it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Treat {
    Merge(Merge),
    Dropped,
    NotJudged,
}

/// The S-95 rule of a type, `None` for an unknown type and for
/// `MEMORY_SEAL`, which is an option of the link and not of its inputs.
fn rule(machine: u16, t: u32) -> Option<Merge> {
    let x86 = machine == EM_X86_64 || machine == EM_386;
    Some(match t {
        1 => Merge::Max,
        2 => Merge::Present,
        0xb000_0000..=0xb000_7fff => Merge::And,
        0xb000_8000..=0xb000_ffff => Merge::Or,
        0xc000_0000 if machine == EM_AARCH64 => Merge::And,
        0xc000_0001 if machine == EM_AARCH64 => Merge::EqualOrAbsent,
        0xc000_0002 if x86 => Merge::And,
        0xc000_8000..=0xc000_8fff if x86 => Merge::Or,
        0xc001_0000..=0xc001_7fff if x86 => Merge::OrAll,
        _ => return None,
    })
}

/// The type's treatment by the linker of `format` (§6.6's table).
fn treat(format: Format, machine: u16, t: u32) -> Treat {
    let r = rule(machine, t);
    let processor = (0xc000_0000..0xe000_0000).contains(&t);
    match format {
        Format::Mortar => {
            r.map_or(if t == 3 { Treat::NotJudged } else { Treat::Dropped }, Treat::Merge)
        }
        // GNU ld knows the generic and x86 types, not PAUTH; it copies or
        // fails on an unknown type (synth.md §13), which is not judged.
        Format::Gnu => match r {
            Some(Merge::EqualOrAbsent) | None => Treat::NotJudged,
            Some(m) => Treat::Merge(m),
        },
        // lld keeps the processor AND types and PAUTH; it drops the rest.
        Format::Lld => match r {
            Some(m @ (Merge::And | Merge::EqualOrAbsent)) if processor => Treat::Merge(m),
            Some(Merge::OrAll) => Treat::NotJudged,
            _ if t == 3 => Treat::NotJudged,
            _ => Treat::Dropped,
        },
        // mold writes no AArch64 property; on x86 it merges the
        // processor types and drops the generic ones.
        Format::Mold => match r {
            _ if machine == EM_AARCH64 => Treat::Dropped,
            Some(m) if processor => Treat::Merge(m),
            _ if t == 3 => Treat::NotJudged,
            _ => Treat::Dropped,
        },
    }
}

/// The notes of a `.note.gnu.property` section: `Err` names the defect.
fn notes(data: &[u8], align: usize) -> Result<Vec<Props>, String> {
    let u32_at = |o: usize| -> Option<u32> {
        data.get(o..o.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let pad = |n: usize, a: usize| n.checked_next_multiple_of(a);
    let mut out = Vec::new();
    let mut o = 0usize;
    while o < data.len() {
        let (Some(namesz), Some(descsz), Some(ty)) = (u32_at(o), u32_at(o + 4), u32_at(o + 8))
        else {
            return Err(format!("a note header at {o:#x} runs past the section"));
        };
        let (namesz, descsz) = (namesz as usize, descsz as usize);
        let name_at = o + 12;
        let desc_at =
            pad(name_at.checked_add(namesz).ok_or("overflow")?, align).ok_or("overflow")?;
        let end = pad(desc_at.checked_add(descsz).ok_or("overflow")?, align).ok_or("overflow")?;
        if desc_at.checked_add(descsz).is_none_or(|e| e > data.len()) {
            return Err(format!("the note at {o:#x} runs past the section"));
        }
        let name = &data[name_at..name_at + namesz];
        o = end.min(data.len()).max(o + 12);
        if name != b"GNU\0" || ty != NT_GNU_PROPERTY_TYPE_0 {
            continue;
        }
        let desc = &data[desc_at..desc_at + descsz];
        let mut props = Props::new();
        let mut p = 0usize;
        while p < desc.len() {
            let at =
                |k: usize| desc.get(k..k + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let (Some(t), Some(sz)) = (at(p), at(p + 4)) else {
                return Err(format!("a property header at {p:#x} of a note runs past it"));
            };
            let sz = sz as usize;
            let d = p + 8;
            if d.checked_add(sz).is_none_or(|e| e > desc.len()) {
                return Err(format!("property {t:#x} runs past its note"));
            }
            props.insert(t, desc[d..d + sz].to_vec());
            p = pad(d + sz, align).ok_or("overflow")?;
        }
        out.push(props);
    }
    Ok(out)
}

fn word(d: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    let n = d.len().min(8);
    a[..n].copy_from_slice(&d[..n]);
    u64::from_le_bytes(a)
}

fn bytes_of(v: u64, len: usize) -> Vec<u8> {
    v.to_le_bytes()[..len.min(8)].to_vec()
}

/// The merge of a type over `objs` (one property map per object, `None`
/// where it is absent). `Err`: the inputs disagree (PAUTH).
fn merge(m: Merge, vals: &[Option<&Vec<u8>>]) -> Result<Option<Vec<u8>>, ()> {
    let have: Vec<&Vec<u8>> = vals.iter().filter_map(|v| *v).collect();
    if have.is_empty() {
        return Ok(None);
    }
    let len = have[0].len();
    Ok(match m {
        Merge::And => {
            if have.len() < vals.len() {
                return Ok(None);
            }
            let v = have.iter().fold(u64::MAX, |a, d| a & word(d));
            (v != 0).then(|| bytes_of(v, len))
        }
        Merge::Or => Some(bytes_of(have.iter().fold(0, |a, d| a | word(d)), len)),
        Merge::OrAll => (have.len() == vals.len())
            .then(|| bytes_of(have.iter().fold(0, |a, d| a | word(d)), len)),
        Merge::Max => Some(bytes_of(have.iter().map(|d| word(d)).max().unwrap_or(0), len)),
        Merge::Present => Some(Vec::new()),
        Merge::EqualOrAbsent => {
            if have.iter().any(|d| *d != have[0]) {
                return Err(());
            }
            Some(have[0].clone())
        }
    })
}

/// One object's properties from its notes: S-126 (as separate objects)
/// for mortar, the AND types `ORed` over the notes that have them for GNU
/// ld and lld (synth.md X-33). `None` for mold when the object has
/// several notes (not observed).
fn combine(format: Format, machine: u16, ns: &[Props]) -> Option<Props> {
    if ns.len() == 1 {
        return Some(ns[0].clone());
    }
    if ns.len() > 1 && format == Format::Mold {
        return None;
    }
    let types: std::collections::BTreeSet<u32> =
        ns.iter().flat_map(|n| n.keys().copied()).collect();
    let mut out = Props::new();
    for t in types {
        let vals: Vec<Option<&Vec<u8>>> = ns.iter().map(|n| n.get(&t)).collect();
        let m = rule(machine, t);
        let v = match (format, m) {
            (Format::Gnu | Format::Lld, Some(Merge::And)) => {
                let have: Vec<&Vec<u8>> = vals.iter().filter_map(|v| *v).collect();
                let len = have[0].len();
                Some(bytes_of(have.iter().fold(0, |a, d| a | word(d)), len))
            }
            (_, Some(m)) => merge(m, &vals).ok().flatten(),
            // An unknown type: kept, for the per-type treatment to drop
            // or not judge.
            (_, None) => vals.iter().find_map(|v| v.cloned()),
        };
        if let Some(v) = v {
            out.insert(t, v);
        }
    }
    Some(out)
}

/// P1 and P2 (§6.6).
#[allow(clippy::too_many_lines)] // the inputs, the output, then P1 type by type
pub(crate) fn check(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    format: Format,
    cov: &mut Coverage,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let machine = image.header.e_machine;
    let align: usize = if image.class() == mortar_elf::Class::Elf64 { 8 } else { 4 };
    // The inputs: one property map per object, none for an object
    // without the section.
    let mut per_obj: Vec<Option<Props>> = Vec::new();
    let mut judged_and = true;
    for o in objs {
        let mut ns = Vec::new();
        for (si, s) in o.secs.iter().enumerate() {
            if s.name != NAME || s.shdr.sh_type != SHT_NOTE {
                continue;
            }
            match notes(s.data, align) {
                Ok(mut v) => ns.append(&mut v),
                Err(e) => {
                    findings.push(
                        Finding::new(Rule::InputUnreadable, format!("{}: {NAME}: {e}", o.label()))
                            .site(o.site(si, 0)),
                    );
                    cov.properties_not_judged += 1;
                    return findings;
                }
            }
        }
        let combined =
            if ns.is_empty() { Some(Props::new()) } else { combine(format, machine, &ns) };
        judged_and &= combined.is_some();
        per_obj.push(combined);
    }
    // The output.
    let outs: Vec<usize> =
        (1..image.sections.len()).filter(|&i| image.sections[i].name == NAME).collect();
    let mut found = Props::new();
    let p2 = |m: String| Finding::new(Rule::Property(2), m);
    if outs.len() > 1 {
        findings.push(p2(format!("the output has {} sections named {NAME}", outs.len())));
        return findings;
    }
    let out = outs.first().copied();
    if let Some(i) = out {
        let s = &image.sections[i];
        let sh = s.shdr;
        if sh.sh_type != SHT_NOTE || sh.sh_flags & SHF_ALLOC == 0 {
            findings
                .push(p2(format!("{NAME} is not an allocated SHT_NOTE section")).at(sh.sh_addr));
        }
        if sh.sh_addralign != align as u64 {
            findings.push(
                p2(format!("{NAME} is aligned to {}, not {align}", sh.sh_addralign)).at(sh.sh_addr),
            );
        }
        match image.section_data(i).map(|d| notes(d, align)) {
            Some(Ok(ns)) if ns.len() == 1 => found = ns.into_iter().next().unwrap_or_default(),
            Some(Ok(ns)) => {
                findings.push(
                    p2(format!("{NAME} holds {} property notes, not one", ns.len())).at(sh.sh_addr),
                );
                return findings;
            }
            Some(Err(e)) => {
                findings.push(p2(format!("{NAME}: {e}")).at(sh.sh_addr));
                return findings;
            }
            None => {
                findings.push(p2(format!("{NAME} has no contents in the file")).at(sh.sh_addr));
                return findings;
            }
        }
        // PT_GNU_PROPERTY is the section; a PT_NOTE covers it.
        let props: Vec<_> = image.phdrs.iter().filter(|p| p.p_type == PT_GNU_PROPERTY).collect();
        if props.len() != 1 || props[0].p_vaddr != sh.sh_addr || props[0].p_memsz != sh.sh_size {
            findings.push(
                p2(format!("PT_GNU_PROPERTY is not exactly {NAME} ({} such headers)", props.len()))
                    .at(sh.sh_addr),
            );
        }
        let end = sh.sh_addr.saturating_add(sh.sh_size);
        if !image.phdrs.iter().any(|p| {
            p.p_type == PT_NOTE
                && p.p_vaddr <= sh.sh_addr
                && end <= p.p_vaddr.saturating_add(p.p_memsz)
        }) {
            findings.push(p2(format!("no PT_NOTE covers {NAME}")).at(sh.sh_addr));
        }
    } else if image.phdrs.iter().any(|p| p.p_type == PT_GNU_PROPERTY) {
        findings.push(p2(format!("PT_GNU_PROPERTY without a {NAME} section")));
    }
    // P1, type by type, in ascending order.
    let at = out.map(|i| image.sections[i].shdr.sh_addr);
    let mut types: std::collections::BTreeSet<u32> = found.keys().copied().collect();
    for p in per_obj.iter().flatten() {
        types.extend(p.keys().copied());
    }
    for t in types {
        let got = found.get(&t);
        let want = match treat(format, machine, t) {
            Treat::NotJudged => {
                cov.properties_not_judged += 1;
                continue;
            }
            Treat::Dropped => None,
            Treat::Merge(m) => {
                if m == Merge::And && !judged_and {
                    cov.properties_not_judged += 1;
                    continue;
                }
                let vals: Vec<Option<&Vec<u8>>> =
                    per_obj.iter().map(|p| p.as_ref().and_then(|p| p.get(&t))).collect();
                if let Ok(v) = merge(m, &vals) {
                    v
                } else {
                    let mut f = Finding::new(
                        Rule::Property(1),
                        format!("the inputs' property {t:#x} differ, and the link is not refused"),
                    );
                    f.addr = at;
                    findings.push(f);
                    continue;
                }
            }
        };
        cov.properties_judged += 1;
        if got != want.as_ref() {
            let mut f = Finding::new(
                Rule::Property(1),
                match (&want, got) {
                    (None, _) => format!(
                        "the output has property {t:#x}, which the inputs' merge does not give"
                    ),
                    (Some(_), None) => {
                        format!("the output lacks property {t:#x}, which the inputs' merge gives")
                    }
                    _ => format!("property {t:#x} is not the merge of the inputs'"),
                },
            );
            f.addr = at;
            f.expected = want.as_deref().map(word);
            f.found = got.map(|d| word(d));
            findings.push(f);
        }
    }
    findings
}
