//! The verifier on a mortar output (`spec/plumb.md` §2), and the checks
//! every pipeline test applies to an output: the readers (readelf,
//! llvm-readelf) without a warning, and eu-elflint with only the findings
//! listed here, each with its reason.

use std::path::Path;

use mortar_elf::consts::{SHF_MERGE, SHT_REL, SHT_RELA};
use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection};
use mortar_testutil::{run, tool};

/// The inputs a map names, read from `dir` (with a thin archive's members).
fn inputs(map: &mortar_plumb::map::Map, dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for path in map.paths() {
        let Ok(b) = std::fs::read(dir.join(path)) else { continue };
        for member in mortar_plumb::thin_members(path, &b) {
            if let Ok(m) = std::fs::read(dir.join(&member)) {
                files.push((member, m));
            }
        }
        files.push((path.to_owned(), b));
    }
    files
}

/// `mortar_plumb` on `out`, with mortar's map `map` and the inputs it
/// names (relative to `dir`): the findings, but for the verifier's known
/// merge-piece case ([`known_merge_piece`]), whose count is returned too.
///
/// # Panics
///
/// If the output or the map cannot be read, or the map does not parse.
pub fn verify(out: &Path, map: &Path, dir: &Path) -> (Vec<mortar_plumb::Finding>, usize) {
    let text = std::fs::read_to_string(map).expect("the map");
    let parsed = mortar_plumb::map::parse(mortar_plumb::Format::Mortar, &text)
        .unwrap_or_else(|e| panic!("the map does not parse: {e:?}"));
    let files = inputs(&parsed, dir);
    let inputs: Vec<mortar_plumb::Input<'_>> =
        files.iter().map(|(p, b)| mortar_plumb::Input { path: p, bytes: b }).collect();
    let report = mortar_plumb::verify(&std::fs::read(out).expect("the output"), &inputs, &parsed);
    let (known, rest): (Vec<_>, Vec<_>) =
        report.findings.into_iter().partition(|f| known_merge_piece(&files, f));
    (rest, known.len())
}

/// [`verify`], and no finding may remain.
pub fn clean(out: &Path, map: &Path, dir: &Path) -> usize {
    let (rest, known) = verify(out, map, dir);
    assert!(rest.is_empty(), "plumb on {}: {rest:#?}", out.display());
    known
}

/// Whether `f` is the verifier's known merge-piece case (WP-028c fixes it
/// on its branch, not yet on main; `xtask/src/plumb.rs` has the same
/// predicate for `cargo xtask linktest`): an R1 finding whose relocation's
/// symbol is defined in an `SHF_MERGE` input section. plumb's `reloc.rs`
/// `in_section` takes a section symbol's `st_value` to a piece before
/// adding A, and matches a piece with an inclusive end, so a label at the
/// start of a piece reads the piece before it.
pub fn known_merge_piece(inputs: &[(String, Vec<u8>)], f: &mortar_plumb::Finding) -> bool {
    if f.rule != mortar_plumb::Rule::Field {
        return false;
    }
    let Some(site) = &f.site else { return false };
    let Some(data) = object(inputs, &site.file, site.member.as_deref()) else { return false };
    let Ok(obj) = Object::parse(data) else { return false };
    let Ok(Some(symtab)) = obj.symbols() else { return false };
    for rs in obj.sections() {
        let Ok(target) = rs.info_section() else { continue };
        if !matches!(rs.sh_type(), SHT_RELA | SHT_REL)
            || obj.section(target).ok().and_then(|t| t.name().ok()) != Some(site.section.as_bytes())
        {
            continue;
        }
        let syms: Vec<u32> = if rs.sh_type() == SHT_RELA {
            rs.rela()
                .map(|r| r.iter().filter(|x| x.r_offset == site.offset).map(|x| x.r_sym).collect())
        } else {
            rs.rel()
                .map(|r| r.iter().filter(|x| x.r_offset == site.offset).map(|x| x.r_sym).collect())
        }
        .unwrap_or_default();
        if let Some(r_sym) = syms.first() {
            let Ok(sym) = symtab.get(SymbolIndex(*r_sym)) else { return false };
            return match symtab.section_of(SymbolIndex(*r_sym), &sym) {
                Ok(SymbolSection::Section(i)) => merge(&obj, i),
                Ok(SymbolSection::Undefined) => {
                    symtab.name(&sym).is_ok_and(|n| defined_in_merge(inputs, n))
                }
                _ => false,
            };
        }
    }
    false
}

fn merge(o: &Object<'_>, i: SectionIndex) -> bool {
    o.section(i).is_ok_and(|s| s.flags() & SHF_MERGE != 0)
}

fn object<'a>(
    inputs: &'a [(String, Vec<u8>)],
    path: &str,
    member: Option<&str>,
) -> Option<&'a [u8]> {
    let (_, bytes) = inputs.iter().find(|(p, _)| p == path)?;
    let Some(member) = member else { return Some(bytes) };
    let archive = mortar_elf::archive::Archive::parse(bytes).ok()?;
    archive.members().filter_map(Result::ok).find(|m| m.name() == member.as_bytes())?.data()
}

fn defined_in_merge(inputs: &[(String, Vec<u8>)], name: &[u8]) -> bool {
    let in_object = |data: &[u8]| {
        let Ok(obj) = Object::parse(data) else { return false };
        let Ok(Some(symtab)) = obj.symbols() else { return false };
        symtab.globals().any(|(i, s)| {
            symtab.name(&s).is_ok_and(|n| n == name)
                && matches!(symtab.section_of(i, &s), Ok(SymbolSection::Section(x)) if merge(&obj, x))
        })
    };
    inputs.iter().any(|(_, bytes)| match mortar_elf::archive::Archive::parse(bytes) {
        Ok(a) => a.members().filter_map(Result::ok).filter_map(|m| m.data()).any(in_object),
        Err(_) => in_object(bytes),
    })
}

/// readelf and llvm-readelf read `out` with status 0 and nothing on
/// standard error (a warning is a failure). A missing tool is skipped.
pub fn readers(out: &Path) {
    for (name, args) in [
        ("readelf", &["-W", "-a"][..]),
        ("readelf", &["--debug-dump=frames"][..]),
        ("llvm-readelf", &["-a"][..]),
    ] {
        let Some(t) = tool(name) else { continue };
        let mut a: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        a.push(out.into());
        let o = run(t, a);
        assert!(o.success(), "{name} {args:?}: {}", o.stderr_text());
        assert_eq!(o.stderr_text(), "", "{name} {args:?} warns on {}", out.display());
    }
}

/// eu-elflint's findings on a static executable that are not mortar's to
/// fix, as (text the line contains, text it ends with, reason).
pub const ELFLINT_KNOWN: &[(&str, &str, &str)] = &[
    // L-64's `.relro_padding` is NOBITS with a `.rel*` name, which elflint
    // takes for a relocation section.
    ("'.relro_padding' has wrong ", "", "lld's and mold's outputs have the same lines"),
    // `R_AARCH64_IRELATIVE` has no symbol; elflint does not know the type
    // in a static executable and wants a symbol index.
    ("'.rela.plt': relocation ", "invalid type", "GNU ld, lld and mold write the same records"),
    (
        "'.rela.plt': relocation ",
        "invalid symbol index",
        "GNU ld, lld and mold write the same records",
    ),
    // `__ehdr_start` is valued at the file header, below section 1
    // (synth.md S-122's header exception).
    ("(__ehdr_start): st_value out of bounds", "", "GNU ld, lld and mold value it the same"),
    // SystemTap's probe notes in libstdc++.a, a note type elflint does
    // not know.
    ("'.note.stapsdt': unknown object file note type 3", "", "GNU ld's output has the same lines"),
    // elflint finds a NOBITS section's segment by file offset over
    // `[p_offset, p_offset + p_memsz)`. The RELRO segment's memsz ends at
    // a maxpage boundary (L-64's padding, 64 KiB here), so its offset
    // range covers the next segment's `.bss` offset (an undisplaced NOBITS
    // offset, L-69, as GNU ld and lld write it); by address, which the
    // loader uses, `.bss` is contained. lld's output has the same shape
    // with a 4 KiB padding. plumb's S rules check sections against
    // segments by address.
    (
        "'.bss' not fully contained in segment of program header entry ",
        "",
        "L-64's RELRO padding and elflint's offset heuristic",
    ),
];

/// eu-elflint `--gnu-ld` on `out`: no finding but [`ELFLINT_KNOWN`]. The
/// known ones seen are returned.
pub fn elflint(out: &Path) -> Vec<String> {
    let Some(t) = tool("eu-elflint") else { return Vec::new() };
    let o = run(t, [std::ffi::OsStr::new("--gnu-ld"), out.as_os_str()]);
    let text = o.stdout_text();
    let mut known = Vec::new();
    let mut rest = Vec::new();
    for l in text.lines().filter(|l| *l != "No errors") {
        if ELFLINT_KNOWN.iter().any(|(k, tail, _)| l.contains(k) && l.ends_with(tail)) {
            known.push(l.to_owned());
        } else {
            rest.push(l);
        }
    }
    assert!(
        rest.is_empty(),
        "eu-elflint on {}: {}{}",
        out.display(),
        rest.join("\n"),
        o.stderr_text()
    );
    known
}
