//! S12, the content rule, and Y1, the symbol rule (`spec/plumb.md` §6.2,
//! §4.6): the bytes of every placed section are its input's outside the
//! relocated places, and every output symbol is where its definition went.

use mortar_elf::consts::{SHT_NOBITS, STT_GNU_IFUNC, STT_TLS};
use mortar_elf::{SymbolIndex, SymbolSection};

use crate::a64;
use crate::locate::{Place, covered};
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
            if !s.alloc()
                || s.merge()
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
            if let Some(k) = first_difference(s.data, &cov, out, |k| {
                // A word an erratum 843419 patch replaced by a branch (W8).
                let p = addr + k;
                let Some(inw) = s.data.get(k as usize..k as usize + 4) else { return false };
                let inw = u32::from_le_bytes([inw[0], inw[1], inw[2], inw[3]]);
                let outw = u32::from_le_bytes([
                    out[k as usize],
                    out[k as usize + 1],
                    out[k as usize + 2],
                    out[k as usize + 3],
                ]);
                a64::erratum_843419_site(p)
                    && a64::is_ldst_unsigned_imm(inw)
                    && e.erratum_patch(p, outw) == Some(inw)
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

/// The first offset where `out` differs from `data` outside the covered
/// ranges, skipping 4-byte words `allowed` accepts.
fn first_difference(
    data: &[u8],
    covered: &[(u64, u64)],
    out: &[u8],
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
            let w = k & !3;
            if w + 4 <= n && allowed(w as u64) {
                k = w + 4;
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
        if out.sym.st_shndx == 0 || out.sym.sym_type() == STT_TLS {
            continue;
        }
        let mut values: Vec<u64> = Vec::new();
        let mut ifunc = false;
        for &(oi, di) in &e.defs[name] {
            let o = &objs[oi];
            let Some(t) = o.symtab else { continue };
            let Ok(d) = t.get(SymbolIndex(di)) else { continue };
            // An IFUNC's output symbol is its canonical PLT entry (stage 2).
            if d.sym_type() == STT_GNU_IFUNC {
                values.clear();
                ifunc = true;
                break;
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
        if ifunc || values.is_empty() {
            unchecked += 1;
        } else if values.contains(&out.sym.st_value) {
            checked += 1;
        } else {
            let mut f = Finding::new(
                Rule::SymbolValue,
                format!("symbol '{name}' is not where any placed definition of it is"),
            )
            .at(out.sym.st_value);
            f.symbol = Some(name.clone());
            f.expected = values.first().copied();
            f.found = Some(out.sym.st_value);
            findings.push(f);
        }
    }
    e.cov.symbols_checked += checked;
    e.cov.symbols_unchecked += unchecked;
    e.findings.extend(findings);
}
