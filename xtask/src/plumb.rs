//! `cargo xtask plumb`: the independent verifier on a finished link
//! (`spec/plumb.md` §8).
//!
//!   cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar
//!                     [--inputs-from <response file>|map]
//!
//! The inputs are the files the map names, read from the paths it gives
//! (relative ones from the current directory). With a response file, only
//! the map's files that name the same file as one of its entries are read;
//! the others are missing inputs (rule I1).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mortar_plumb::{Format, Input};

pub const USAGE: &str = "usage: cargo xtask plumb <output> --map <file> --map-format gnu|lld|mold|mortar \
                         [--inputs-from <response file>|map]\n";

/// The command line of `xtask plumb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub output: PathBuf,
    pub map: PathBuf,
    pub format: Format,
    /// `None`: the map itself.
    pub inputs_from: Option<PathBuf>,
}

impl Args {
    /// Parses the arguments after `plumb`.
    pub fn parse(args: &[String]) -> Result<Args, String> {
        let mut output = None;
        let mut map = None;
        let mut format = None;
        let mut inputs_from = None;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value =
                |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--map" => map = Some(PathBuf::from(value("--map")?)),
                "--map-format" => {
                    let v = value("--map-format")?;
                    format = Some(
                        Format::from_name(&v).ok_or_else(|| format!("unknown map format '{v}'"))?,
                    );
                }
                "--inputs-from" => {
                    let v = value("--inputs-from")?;
                    inputs_from = (v != "map").then(|| PathBuf::from(v));
                }
                s if s.starts_with('-') => return Err(format!("unknown option '{s}'")),
                s if output.is_none() => output = Some(PathBuf::from(s)),
                s => return Err(format!("a second output '{s}'")),
            }
        }
        Ok(Args {
            output: output.ok_or("no output file")?,
            map: map.ok_or("no --map")?,
            format: format.ok_or("no --map-format")?,
            inputs_from,
        })
    }
}

/// The entries of a response file: words separated by white space, with
/// single and double quotes and backslash escapes, as GNU ld reads `@file`.
/// Options (words starting with `-`) are skipped, and so is the value of
/// `-o`.
pub fn response_entries(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (_, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (_, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    let mut out = Vec::new();
    let mut skip = false;
    for w in words {
        if skip {
            skip = false;
        } else if w == "-o" {
            skip = true;
        } else if !w.starts_with('-') {
            out.push(w);
        }
    }
    out
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Verifies the link that wrote `output` and `map` (in `format`). The
/// inputs are the files the map names, a relative path read from `base`;
/// with `allowed`, only those of its files. An unreadable input is a
/// missing input (I1) in the report.
///
/// # Errors
/// The output or the map cannot be read, or the map does not parse.
pub fn verify_link(
    output: &Path,
    map: &Path,
    format: Format,
    base: &Path,
    allowed: Option<&BTreeSet<PathBuf>>,
) -> Result<Verified, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let output = read(output)?;
    let map_bytes = read(map)?;
    let text = String::from_utf8_lossy(&map_bytes);
    let parsed = mortar_plumb::map::parse(format, &text)
        .map_err(|e| format!("{}:{}: {}", map.display(), e.line, e.message))?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in parsed.paths() {
        let full = base.join(path);
        if allowed.is_some_and(|a| !a.contains(&canonical(&full))) {
            continue;
        }
        if let Ok(b) = std::fs::read(&full) {
            // A thin archive's members are files of their own (§2.2).
            for member in mortar_plumb::thin_members(path, &b) {
                if let Ok(m) = std::fs::read(base.join(&member)) {
                    files.push((member, m));
                }
            }
            files.push((path.to_string(), b));
        }
    }
    let inputs: Vec<Input<'_>> = files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
    let report = mortar_plumb::verify(&output, &inputs, &parsed);
    drop(inputs);
    Ok((output, report, files))
}

/// [`verify_link`]'s answer: the output's bytes, the report, and the
/// inputs read (path as the map names it, bytes).
pub type Verified = (Vec<u8>, mortar_plumb::Report, Vec<(String, Vec<u8>)>);

/// Whether `f` is the verifier's known merge-piece case (WP-028c, fixed
/// on its branch and not yet on main; WP-027c's hand-back names it): an
/// R1 finding (`Rule::Field`) whose relocation's symbol is defined in an
/// `SHF_MERGE` input section. plumb's `reloc.rs` `in_section` takes a
/// section symbol's `st_value` to a piece before adding A (S-17 resolves
/// `st_value + A`), and matches a piece with an inclusive end, so a label
/// at the start of a piece reads the piece before it. Nothing else is
/// filtered: an R1 into any other section, or any other rule, still fails.
#[must_use]
pub fn known_merge_piece(inputs: &[(String, Vec<u8>)], f: &mortar_plumb::Finding) -> bool {
    use mortar_elf::consts::{SHF_MERGE, SHT_REL, SHT_RELA};
    use mortar_elf::{Object, SectionIndex, SymbolIndex, SymbolSection};
    if f.rule != mortar_plumb::Rule::Field {
        return false;
    }
    let Some(site) = &f.site else { return false };
    let Some(data) = input_object(inputs, &site.file, site.member.as_deref()) else {
        return false;
    };
    let Ok(obj) = Object::parse(data) else { return false };
    let Ok(Some(symtab)) = obj.symbols() else { return false };
    let merge = |o: &Object<'_>, i: SectionIndex| {
        o.section(i).is_ok_and(|s| s.flags() & SHF_MERGE != 0)
    };
    for rs in obj.sections() {
        let Ok(target) = rs.info_section() else { continue };
        if !matches!(rs.sh_type(), SHT_RELA | SHT_REL)
            || obj.section(target).ok().and_then(|t| t.name().ok()) != Some(site.section.as_bytes())
        {
            continue;
        }
        let syms: Vec<u32> = if rs.sh_type() == SHT_RELA {
            rs.rela().map(|r| r.iter().filter(|x| x.r_offset == site.offset).map(|x| x.r_sym).collect())
        } else {
            rs.rel().map(|r| r.iter().filter(|x| x.r_offset == site.offset).map(|x| x.r_sym).collect())
        }
        .unwrap_or_default();
        for r_sym in syms {
            let Ok(sym) = symtab.get(SymbolIndex(r_sym)) else { continue };
            match symtab.section_of(SymbolIndex(r_sym), &sym) {
                Ok(SymbolSection::Section(i)) => return merge(&obj, i),
                Ok(SymbolSection::Undefined) => {
                    let Ok(name) = symtab.name(&sym) else { return false };
                    return defined_in_merge(inputs, name);
                }
                _ => return false,
            }
        }
    }
    false
}

/// The bytes of an input object: `path`, or its archive's `member`.
fn input_object<'a>(
    inputs: &'a [(String, Vec<u8>)],
    path: &str,
    member: Option<&str>,
) -> Option<&'a [u8]> {
    let (_, bytes) = inputs.iter().find(|(p, _)| p == path)?;
    let Some(member) = member else { return Some(bytes) };
    let archive = mortar_elf::archive::Archive::parse(bytes).ok()?;
    archive.members().filter_map(Result::ok).find(|m| m.name() == member.as_bytes())?.data()
}

/// Whether some input object defines the global `name` in an `SHF_MERGE`
/// section.
fn defined_in_merge(inputs: &[(String, Vec<u8>)], name: &[u8]) -> bool {
    use mortar_elf::consts::SHF_MERGE;
    use mortar_elf::{Object, SymbolSection};
    let in_object = |data: &[u8]| {
        let Ok(obj) = Object::parse(data) else { return false };
        let Ok(Some(symtab)) = obj.symbols() else { return false };
        symtab.globals().any(|(i, s)| {
            symtab.name(&s).is_ok_and(|n| n == name)
                && matches!(symtab.section_of(i, &s), Ok(SymbolSection::Section(x))
                    if obj.section(x).is_ok_and(|sec| sec.flags() & SHF_MERGE != 0))
        })
    };
    inputs.iter().any(|(_, bytes)| match mortar_elf::archive::Archive::parse(bytes) {
        Ok(a) => a.members().filter_map(Result::ok).filter_map(|m| m.data()).any(in_object),
        Err(_) => in_object(bytes),
    })
}

/// Runs the command; prints the findings and the coverage. `Ok(clean)`.
pub fn run(args: &Args) -> Result<bool, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let allowed: Option<BTreeSet<PathBuf>> = match &args.inputs_from {
        None => None,
        Some(rsp) => {
            let t = read(rsp)?;
            Some(
                response_entries(&String::from_utf8_lossy(&t))
                    .iter()
                    .map(|e| canonical(Path::new(e)))
                    .collect(),
            )
        }
    };
    let (_, report, _) =
        verify_link(&args.output, &args.map, args.format, Path::new(""), allowed.as_ref())?;
    print!("{}", report.render(&args.output.display().to_string()));
    print!("{}", report.summary());
    Ok(report.clean())
}

/// The command's entry point: exit 0 when clean, 1 on any finding, 2 on
/// a usage or read error.
pub fn main(args: &[String]) -> std::process::ExitCode {
    use std::process::ExitCode;
    match Args::parse(args).and_then(|a| run(&a)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprint!("xtask plumb: {message}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    // spec/plumb.md §8.
    #[test]
    fn s8_arguments() {
        let a = Args::parse(&s(&["a.out", "--map", "m", "--map-format", "lld"])).unwrap();
        assert_eq!(a.format, Format::Lld);
        assert_eq!(a.inputs_from, None);
        let a = Args::parse(&s(&[
            "a.out",
            "--map",
            "m",
            "--map-format",
            "gnu",
            "--inputs-from",
            "r.rsp",
        ]))
        .unwrap();
        assert_eq!(a.inputs_from, Some(PathBuf::from("r.rsp")));
        assert!(Args::parse(&s(&["a.out", "--map", "m"])).is_err());
        assert!(Args::parse(&s(&["a.out", "--map", "m", "--map-format", "x"])).is_err());
        assert!(Args::parse(&s(&["a.out", "b.out", "--map", "m", "--map-format", "gnu"])).is_err());
    }

    // spec/plumb.md §8.
    #[test]
    fn s8_response_files() {
        assert_eq!(
            response_entries("-o out 'a b.o' \"c.o\" -static d\\ e.o\n--gc-sections lib.a"),
            ["a b.o", "c.o", "d e.o", "lib.a"]
        );
    }
}
