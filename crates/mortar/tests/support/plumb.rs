//! The verifier on a mortar output (`spec/plumb.md` §2), and the checks
//! every pipeline test applies to an output: the readers (readelf,
//! llvm-readelf) without a warning, and eu-elflint with only the findings
//! listed here, each with its reason.

use std::path::Path;

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
/// names (relative to `dir`): its findings.
///
/// # Panics
///
/// If the output or the map cannot be read, or the map does not parse.
pub fn verify(out: &Path, map: &Path, dir: &Path) -> Vec<mortar_plumb::Finding> {
    let text = std::fs::read_to_string(map).expect("the map");
    let parsed = mortar_plumb::map::parse(mortar_plumb::Format::Mortar, &text)
        .unwrap_or_else(|e| panic!("the map does not parse: {e:?}"));
    let files = inputs(&parsed, dir);
    let inputs: Vec<mortar_plumb::Input<'_>> =
        files.iter().map(|(p, b)| mortar_plumb::Input { path: p, bytes: b }).collect();
    mortar_plumb::verify(&std::fs::read(out).expect("the output"), &inputs, &parsed).findings
}

/// A verifier finding (brought back by WP-062, `spec/landed/incr/WP-062.md`
/// §3 finding 1): the verifier disagrees with `spec/synth.md` S-50 rule 1
/// on an incremental-mode link, where the
/// per-file CIEs (IN-34a) keep `crtbeginT.o`'s first `.eh_frame` record.
/// Rule 1 makes `r = 0` the start of the output `.eh_frame` "whatever `E`
/// holds (nothing, a kept first record, or a removed one)"; the verifier
/// takes the kept record's place (rule 2) for `__EH_FRAME_BEGIN__` (Y4)
/// and for `crtbeginT.o`'s references to it (R1). A plain link removes
/// that CIE as a duplicate and never meets the case.
#[must_use]
pub fn s50_rule1_verifier_finding(f: &mortar_plumb::Finding) -> bool {
    use mortar_plumb::Rule;
    match f.rule {
        Rule::SymbolTable(4) => f.symbol.as_deref() == Some("__EH_FRAME_BEGIN__"),
        Rule::Field => {
            f.symbol.as_deref() == Some(".eh_frame")
                && f.site.as_ref().is_some_and(|s| s.file.ends_with("crtbeginT.o"))
        }
        _ => false,
    }
}

/// A verifier finding (brought back by WP-062, `spec/landed/incr/WP-062.md`
/// §3 finding 9): P-I3 takes an input section of size 0 as one byte
/// (`mortar_plumb` `dead.rs` `named_by_relocations`, `sh_size.max(1)`),
/// so a zero-size section placed at a hole's start (libgfortran's
/// `.bss.empty_string.N`, the last sections of their region) is "in" the
/// hole, though it covers no byte and its address is the region's end
/// (the boundary N3 admits for `arr+size`). True exactly for a P-I3
/// finding whose named section, read from the input under `dir`, has
/// size 0.
#[must_use]
pub fn p_i3_zero_size_verifier_finding(f: &mortar_plumb::Finding, dir: &Path) -> bool {
    if f.rule != mortar_plumb::Rule::Incr(3) {
        return false;
    }
    let Some(rest) = f.message.split_once(" names '").map(|(_, r)| r) else { return false };
    let Some((section, rest)) = rest.split_once("' of ") else { return false };
    let Some(file) = rest.strip_suffix(", which is placed in a dead range") else { return false };
    let (path, member) = match file.strip_suffix(')').and_then(|f| f.rsplit_once('(')) {
        Some((p, m)) => (p, Some(m)),
        None => (file, None),
    };
    let Ok(bytes) = std::fs::read(dir.join(path)) else { return false };
    let object = match member {
        None => bytes.as_slice(),
        Some(m) => {
            let Ok(a) = mortar_elf::archive::Archive::parse(&bytes) else { return false };
            let found = a.members().flatten().find(|x| {
                let n = x.name();
                n == m.as_bytes() || n.strip_suffix(b"/") == Some(m.as_bytes())
            });
            let Some(data) = found.and_then(|x| x.data()) else { return false };
            data
        }
    };
    let Ok(o) = mortar_elf::Object::parse(object) else { return false };
    o.sections().any(|s| s.name().is_ok_and(|n| n == section.as_bytes()) && s.size() == 0)
}

/// [`verify`], and no finding may remain.
pub fn clean(out: &Path, map: &Path, dir: &Path) {
    let findings = verify(out, map, dir);
    assert!(findings.is_empty(), "plumb on {}: {findings:#?}", out.display());
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
    // `R_AARCH64_IRELATIVE` has no symbol; elflint does not know the type
    // in a static executable and wants a symbol index.
    ("'.rela.plt': relocation ", "invalid type", "GNU ld, lld and mold write the same records"),
    (
        "'.rela.plt': relocation ",
        "invalid symbol index",
        "GNU ld, lld and mold write the same records",
    ),
    // i386's (WP-030c): `R_386_IRELATIVE` in `.rel.plt`, the same records.
    ("'.rel.plt': relocation ", "invalid symbol index", "GNU ld's output has the same lines"),
    // x86-64's `SHF_X86_64_LARGE` (x64.md X-52, L-36a), a processor flag
    // elflint does not know.
    (
        "contains invalid processor-specific flag(s) 0x10000000",
        "",
        "GNU ld, lld and mold keep the flag the same",
    ),
    // `STO_AARCH64_VARIANT_PCS` (0x80) in `.symtab` (synth.md S-127,
    // WP-059): elflint knows no processor-specific `st_other` bit. libgcc's
    // `__arm_tpidr2_save` and `__arm_za_disable` carry it.
    ("): unknown bit set in st_other", "", "GNU ld's and lld's outputs have the same lines"),
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
    // elflint places a TLS symbol of an executable by its section's
    // `sh_offset − p_offset` of PT_TLS. `.tbss`'s offset is undisplaced
    // (L-69, as GNU ld and lld write it), so when `.tdata` ends short of
    // `.tbss`'s alignment the section starts lower by offset than by
    // address, and a symbol near the block's end (glibc's `thread_arena`,
    // `current`, `__libc_dlerror_result`; x86-64, WP-029b) is outside by
    // offset, inside by address and within `p_memsz`, which is what the
    // loader uses.
    (
        "does not fit completely in referenced section ",
        "'.tbss'",
        "GNU ld's output has the same lines (elflint's offset heuristic)",
    ),
    (
        "st_value out of bounds of referenced section ",
        "'.tbss'",
        "GNU ld's output has the same lines (elflint's offset heuristic)",
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
