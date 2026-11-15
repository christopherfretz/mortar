//! S12, the content rule, and Y1, the symbol rule (`spec/plumb.md` §6.2,
//! §4.6): the bytes of every placed section are its input's outside the
//! relocated places, and every output symbol is where its definition went.

use mortar_elf::consts::{SHF_COMPRESSED, SHT_NOBITS, STT_FUNC, STT_GNU_IFUNC, STT_TLS};
use mortar_elf::{SymbolIndex, SymbolSection};

use crate::a64;
use crate::locate::{Place, covered};
use crate::map::Format;
use crate::reloc::Engine;
use crate::report::{Finding, Rule};

/// Sections a linker rewrites by definition and S12 does not compare:
/// `.note.gnu.property`, whose notes the linker combines (§6.2).
fn combined(name: &str) -> bool {
    name == ".note.gnu.property"
}

/// S12 (§6.2).
pub(crate) fn check_content(e: &mut Engine<'_, '_>) {
    let objs = e.objs;
    let image = e.image;
    for o in objs {
        let machine = o.elf.machine();
        for (si, s) in o.secs.iter().enumerate() {
            let Place::Whole(addr) = s.place else { continue };
            if !s.alloc() {
                nonalloc_content(e, o, si, addr);
                continue;
            }
            if s.merge()
                || s.is_eh_frame(machine)
                || combined(&s.name)
                || s.shdr.sh_size == 0
                || s.tbss()
            {
                continue;
            }
            let size = s.shdr.sh_size;
            e.cov.content_bytes += size;
            if s.shdr.sh_type == SHT_NOBITS {
                if image.is_zero(addr, size) != Some(true) {
                    e.findings.push(
                        Finding::new(Rule::Structure(12), "a NOBITS section's memory is not zero")
                            .at(addr)
                            .site(o.site(si, 0)),
                    );
                }
                continue;
            }
            let Some(out) = image.bytes(addr, size) else {
                e.findings.push(
                    Finding::new(
                        Rule::Structure(12),
                        "the section's bytes are not in the output's file image",
                    )
                    .at(addr)
                    .site(o.site(si, 0)),
                );
                continue;
            };
            let cov = covered(o, si);
            // x86-64 rewritings are recorded byte by byte (§5.9).
            let gran = if machine == mortar_elf::Machine::X86_64 { 1 } else { 4 };
            if let Some(k) = first_difference(s.data, &cov, out, gran, |k| {
                // A word an erratum 843419 patch replaced by a branch (W8).
                let p = addr + k;
                if e.rewritten_words.contains(&p) {
                    return true;
                }
                if gran == 1 {
                    return false;
                }
                let Some(inw) = s.data.get(k as usize..k as usize + 4) else { return false };
                let inw = u32::from_le_bytes([inw[0], inw[1], inw[2], inw[3]]);
                let outw = u32::from_le_bytes([
                    out[k as usize],
                    out[k as usize + 1],
                    out[k as usize + 2],
                    out[k as usize + 3],
                ]);
                // A word a TLS rewriting wrote that no record covers (W12),
                // or an x86-64 byte of a rewritten window.
                e.rewritten_words.contains(&p)
                    || (a64::erratum_843419_site(p)
                        && a64::is_ldst_unsigned_imm(inw)
                        && e.erratum_patch(p, outw) == Some(inw))
                    // W7 over an ADRP without a record: the ADR of the
                    // page the ADRP computes here (a64.md §11.3 step 1).
                    || (a64::erratum_843419_adrp(p)
                        && a64::adrp(inw, p).is_some_and(|w| a64::adr(outw, p) == Some(w)))
            }) {
                let mut f = Finding::new(
                    Rule::Structure(12),
                    format!("the output differs from the input at offset {k:#x}, which no relocation covers"),
                )
                .at(addr + k)
                .site(o.site(si, k));
                f.expected = Some(u64::from(s.data[k as usize]));
                f.found = Some(u64::from(out[k as usize]));
                e.findings.push(f);
            }
        }
    }
}

/// The length of the x86 no-operation instruction at the start of `b`:
/// `90`, or `0f 1f /0` (`nopl`/`nopw`, any operand), after any number of
/// `66` prefixes and at most one `2e` (Intel SDM, NOP; GNU ld's padding).
fn x86_nop(b: &[u8]) -> Option<usize> {
    let mut i = 0;
    while b.get(i) == Some(&0x66) {
        i += 1;
    }
    if b.get(i) == Some(&0x2e) {
        i += 1;
    }
    match *b.get(i)? {
        0x90 => Some(i + 1),
        0x0f if b.get(i + 1) == Some(&0x1f) => {
            let modrm = *b.get(i + 2)?;
            if modrm & 0x38 != 0 {
                return None;
            }
            let (md, rm) = (modrm >> 6, modrm & 7);
            let sib = usize::from(md != 3 && rm == 4);
            let base5 = sib == 1 && b.get(i + 3).is_some_and(|s| s & 7 == 5);
            let disp = match md {
                0 if rm == 5 || base5 => 4,
                1 => 1,
                2 => 4,
                _ => 0,
            };
            let n = i + 3 + sib + disp;
            (n <= b.len()).then_some(n)
        }
        _ => None,
    }
}

/// S19 (§6.2, `spec/x64.md` X-50): on x86, the fragments of `.init` and
/// `.fini` run in sequence from crti's to crtn's, so the bytes between
/// two of them are no-operation instructions: mortar's 0x90, a
/// reference's multi-byte NOPs. lld and mold write `int3` (W8, §10.2 A16).
pub(crate) fn check_init_padding(e: &mut Engine<'_, '_>) {
    let image = e.image;
    if !matches!(image.header.e_machine, 3 | 62) {
        return;
    }
    for out in
        image.sections.iter().filter(|s| s.alloc() && (s.name == ".init" || s.name == ".fini"))
    {
        let (lo, hi) = (out.shdr.sh_addr, out.shdr.sh_addr.saturating_add(out.shdr.sh_size));
        let mut parts: Vec<(u64, u64)> = e
            .objs
            .iter()
            .flat_map(|o| o.secs.iter())
            .filter_map(|s| match s.place {
                Place::Whole(a) if s.alloc() && s.name == out.name && lo <= a && a < hi => {
                    Some((a, a.saturating_add(s.shdr.sh_size)))
                }
                _ => None,
            })
            .collect();
        parts.sort_unstable();
        for w in parts.windows(2) {
            let (g0, g1) = (w[0].1, w[1].0);
            if g1 <= g0 {
                continue;
            }
            let Some(gap) = image.bytes(g0, g1 - g0) else { continue };
            let ok = if e.format == Format::Mortar {
                gap.iter().all(|&b| b == 0x90)
            } else {
                let mut k = 0;
                while k < gap.len() {
                    match x86_nop(&gap[k..]) {
                        Some(n) => k += n,
                        None => break,
                    }
                }
                k == gap.len()
            };
            if !ok {
                let mut f = Finding::new(
                    Rule::Structure(19),
                    format!(
                        "the padding between two fragments of {} is not no-operation instructions (spec/x64.md X-50)",
                        out.name
                    ),
                )
                .at(g0);
                f.found = gap.first().map(|&b| u64::from(b));
                e.findings.push(f);
            }
        }
    }
}

/// S12 for a non-alloc section placed whole (§5.8): its bytes in the
/// output section of its name, at its offset there.
#[allow(clippy::many_single_char_names)]
fn nonalloc_content(e: &mut Engine<'_, '_>, o: &crate::locate::Obj<'_>, si: usize, addr: u64) {
    let s = &o.secs[si];
    let image = e.image;
    if s.merge() || s.shdr.sh_size == 0 || s.shdr.sh_type == SHT_NOBITS || s.consumed() {
        return;
    }
    let Some(i) = image.nonalloc_section(&s.name) else { return };
    if (s.shdr.sh_flags | image.sections[i].shdr.sh_flags) & SHF_COMPRESSED != 0 {
        return;
    }
    let size = s.shdr.sh_size;
    let Some(out) = image.nonalloc_bytes(i, addr, size) else {
        e.findings.push(
            Finding::new(
                Rule::Structure(12),
                format!(
                    "the section is placed at [{addr:#x}, +{size:#x}), outside output section '{}'",
                    s.name
                ),
            )
            .at(addr)
            .site(o.site(si, 0)),
        );
        return;
    };
    e.cov.content_bytes += size;
    let cov = covered(o, si);
    if let Some(k) =
        first_difference(s.data, &cov, out, 4, |k| e.nonalloc_rewritten.contains(&(i, addr + k)))
    {
        let mut f = Finding::new(
            Rule::Structure(12),
            format!(
                "the output differs from the input at offset {k:#x}, which no relocation covers"
            ),
        )
        .at(addr + k)
        .site(o.site(si, k));
        f.expected = Some(u64::from(s.data[k as usize]));
        f.found = Some(u64::from(out[k as usize]));
        e.findings.push(f);
    }
}

/// The first offset where `out` differs from `data` outside the covered
/// ranges, skipping `gran`-byte units (4 or 1) `allowed` accepts.
fn first_difference(
    data: &[u8],
    covered: &[(u64, u64)],
    out: &[u8],
    gran: usize,
    allowed: impl Fn(u64) -> bool,
) -> Option<u64> {
    let mut ci = 0usize;
    let mut k = 0usize;
    let n = data.len().min(out.len());
    while k < n {
        while ci < covered.len() && covered[ci].1 as usize <= k {
            ci += 1;
        }
        if ci < covered.len() && covered[ci].0 as usize <= k {
            k = covered[ci].1 as usize;
            continue;
        }
        if data[k] != out[k] {
            let w = k & !(gran - 1);
            if w + gran <= n && allowed(w as u64) {
                k = w + gran;
                continue;
            }
            return Some(k as u64);
        }
        k += 1;
    }
    None
}

/// Y1 (§4.6): every defined global output symbol whose name has placed
/// definitions in the inputs has the value of one of them.
#[allow(clippy::many_single_char_names)]
pub(crate) fn check_symbols(e: &mut Engine<'_, '_>) {
    let objs = e.objs;
    let image = e.image;
    let mut names: Vec<&String> = e.defs.keys().collect();
    names.sort();
    let mut findings = Vec::new();
    let (mut checked, mut unchecked) = (0u64, 0u64);
    for name in names {
        let Some(out) = image.global(name).or_else(|| image.unique(name)) else { continue };
        if out.sym.st_shndx == 0 {
            continue;
        }
        // A TLS symbol's value is its offset in the TLS template (gABI
        // "Symbol Values"; §4.7): its address is that plus PT_TLS p_vaddr.
        let value = if out.sym.sym_type() == STT_TLS {
            let Ok(Some(b)) = e.tls else {
                unchecked += 1;
                continue;
            };
            b.start.wrapping_add(out.sym.st_value)
        } else {
            out.sym.st_value
        };
        let mut values: Vec<u64> = Vec::new();
        let mut ifunc = false;
        for &(oi, di) in &e.defs[name] {
            let o = &objs[oi];
            let Some(t) = o.symtab else { continue };
            let Ok(d) = t.get(SymbolIndex(di)) else { continue };
            if d.sym_type() == STT_GNU_IFUNC {
                ifunc = true;
            }
            match t.section_of(SymbolIndex(di), &d) {
                Ok(SymbolSection::Absolute) => values.push(d.st_value),
                Ok(SymbolSection::Section(k)) => {
                    if let Some(Place::Whole(a)) = o.secs.get(k.0 as usize).map(|s| &s.place) {
                        values.push(a.wrapping_add(d.st_value));
                    }
                }
                _ => {}
            }
        }
        if ifunc {
            // §4.8: STT_GNU_IFUNC at the resolver (GNU ld, mortar), or
            // (lld, mold) a function symbol at an IPLT entry of it.
            let gnu = out.sym.sym_type() == STT_GNU_IFUNC && values.contains(&value);
            let canonical = matches!(e.format, Format::Lld | Format::Mold)
                && out.sym.sym_type() == STT_FUNC
                && values.iter().any(|&r| e.iplt.entries_of(r).contains(&value));
            if gnu || canonical {
                checked += 1;
            } else {
                let mut f = Finding::new(
                    Rule::SymbolValue,
                    format!(
                        "IFUNC symbol '{name}' is not STT_GNU_IFUNC at its resolver{}",
                        if matches!(e.format, Format::Lld | Format::Mold) {
                            " nor a function at an IPLT entry of it"
                        } else {
                            ""
                        }
                    ),
                )
                .at(value);
                f.symbol = Some(name.clone());
                f.expected = values.first().copied();
                f.found = Some(value);
                findings.push(f);
            }
        } else if values.is_empty() {
            unchecked += 1;
        } else if values.contains(&value) {
            checked += 1;
        } else {
            let mut f = Finding::new(
                Rule::SymbolValue,
                format!("symbol '{name}' is not where any placed definition of it is"),
            )
            .at(value);
            f.symbol = Some(name.clone());
            f.expected = values.first().copied();
            f.found = Some(value);
            findings.push(f);
        }
    }
    e.cov.symbols_checked += checked;
    e.cov.symbols_unchecked += unchecked;
    e.findings.extend(findings);
}
