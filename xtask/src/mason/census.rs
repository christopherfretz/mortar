//! `cargo xtask mason census`: what relocatable objects contain, by
//! producer, so that kiln + hod's objects can be held against GCC's and
//! Clang's (`tests/mason/README.md` "Census"). Counts per group of files:
//! section classes (name pattern, type, flags), relocation types, symbol
//! kinds, COMDAT groups, `.eh_frame` CIE shapes, notes and GNU properties.
//! Archives are read member by member.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mortar_elf::archive::{Archive, Content};
use mortar_elf::consts::{SHT_GROUP, SHT_NOTE, SHT_RELA};
use mortar_elf::{Machine, Object};

/// The counts of one group of objects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub objects: usize,
    /// `<class> <type> <flags>`.
    pub sections: BTreeMap<String, usize>,
    /// `<relocation> in <section class>`.
    pub relocations: BTreeMap<String, usize>,
    /// `<type> <binding> <visibility> <where>`.
    pub symbols: BTreeMap<String, usize>,
    /// `comdat` / `plain`, with the member count.
    pub groups: BTreeMap<String, usize>,
    /// CIE shapes and the FDE count.
    pub eh_frame: BTreeMap<String, usize>,
    /// `<owner> type <n>` and `property <type>=<value>`.
    pub notes: BTreeMap<String, usize>,
    /// Objects that could not be read, with why.
    pub errors: Vec<String>,
}

/// A section name's class: the part a compiler derives from a symbol
/// (`.text._Z3foov`, `.rela.data.rel.ro.local._ZTV1A`) becomes `*`.
pub fn class(name: &str) -> String {
    const STEMS: &[&str] = &[
        ".rela.text",
        ".rela.data.rel.ro",
        ".rela.data.rel",
        ".rela.data",
        ".rela.rodata",
        ".rela.tdata",
        ".rela.init_array",
        ".rela.fini_array",
        ".rela.gcc_except_table",
        ".text",
        ".data.rel.ro",
        ".data.rel",
        ".data",
        ".bss.rel.ro",
        ".bss",
        ".tbss",
        ".tdata",
        ".rodata",
        ".gcc_except_table",
        ".init_array",
        ".fini_array",
    ];
    // Merge-string sections keep their entity size.
    if name.starts_with(".rodata.str") || name.starts_with(".rodata.cst") {
        return name.to_owned();
    }
    for stem in STEMS {
        if let Some(rest) = name.strip_prefix(stem) {
            if rest.is_empty() {
                return name.to_owned();
            }
            if let Some(tail) = rest.strip_prefix('.') {
                // `.text.startup`, `.data.rel.ro.local`, `.init_array.101`.
                if matches!(tail, "startup" | "exit" | "hot" | "unlikely" | "local") {
                    return name.to_owned();
                }
                if tail.bytes().all(|b| b.is_ascii_digit()) {
                    return format!("{stem}.<prio>");
                }
                if let Some(t) = tail.strip_prefix("local.") {
                    let _ = t;
                    return format!("{stem}.local.*");
                }
                return format!("{stem}.*");
            }
        }
    }
    name.to_owned()
}

/// `SHF_*` as letters, readelf's way.
pub fn flags(f: u64) -> String {
    const LETTERS: &[(u64, char)] = &[
        (0x1, 'W'),
        (0x2, 'A'),
        (0x4, 'X'),
        (0x10, 'M'),
        (0x20, 'S'),
        (0x40, 'I'),
        (0x80, 'L'),
        (0x200, 'G'),
        (0x400, 'T'),
        (0x800, 'C'),
        (0x8000_0000, 'E'),
        (0x0010_0000, 'R'),
    ];
    let mut s: String = LETTERS.iter().filter(|(b, _)| f & b != 0).map(|(_, c)| *c).collect();
    let known: u64 = LETTERS.iter().map(|(b, _)| b).sum();
    if f & !known != 0 {
        let _ = write!(s, "+{:#x}", f & !known);
    }
    if s.is_empty() { "-".to_owned() } else { s }
}

fn uleb(d: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *d.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= u64::from(b & 0x7f) << shift;
        }
        shift += 7;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
}

fn sleb(d: &[u8], at: &mut usize) -> Option<i64> {
    let mut v = 0i64;
    let mut shift = 0;
    loop {
        let b = *d.get(*at)?;
        *at += 1;
        if shift < 64 {
            v |= i64::from(b & 0x7f) << shift;
        }
        shift += 7;
        if b & 0x80 == 0 {
            if shift < 64 && b & 0x40 != 0 {
                v |= -1i64 << shift;
            }
            return Some(v);
        }
    }
}

/// The shapes of an `.eh_frame` section's records: one key per CIE
/// (`CIE v<version> "<augmentation>" R=<enc> L=<enc> P=<enc> code=<n>
/// data=<n> ra=<n>`) and `FDE` for each FDE.
pub fn eh_frame(d: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(len) = d.get(at..at + 4) {
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]) as usize;
        if len == 0 {
            out.push("terminator".to_owned());
            break;
        }
        if len == 0xffff_ffff {
            out.push("64-bit length (unsupported here)".to_owned());
            break;
        }
        let start = at + 4;
        let end = start + len;
        let Some(rec) = d.get(start..end) else {
            out.push("truncated record".to_owned());
            break;
        };
        at = end;
        if rec.get(..4) != Some(&[0, 0, 0, 0]) {
            out.push("FDE".to_owned());
            continue;
        }
        out.push(cie(rec).unwrap_or_else(|| "CIE (unreadable)".to_owned()));
    }
    out
}

fn cie(rec: &[u8]) -> Option<String> {
    let version = *rec.get(4)?;
    let mut at = 5;
    let nul = rec.get(at..)?.iter().position(|&b| b == 0)?;
    let aug = String::from_utf8_lossy(&rec[at..at + nul]).into_owned();
    at += nul + 1;
    let code = uleb(rec, &mut at)?;
    let data = sleb(rec, &mut at)?;
    let ra = if version == 1 {
        at += 1;
        u64::from(*rec.get(at - 1)?)
    } else {
        uleb(rec, &mut at)?
    };
    let mut parts = format!("CIE v{version} \"{aug}\" code={code} data={data} ra={ra}");
    if aug.starts_with('z') {
        let _len = uleb(rec, &mut at)?;
        for c in aug.chars().skip(1) {
            match c {
                'R' => {
                    let _ = write!(parts, " R={:#04x}", rec.get(at)?);
                    at += 1;
                }
                'L' => {
                    let _ = write!(parts, " L={:#04x}", rec.get(at)?);
                    at += 1;
                }
                'P' => {
                    let enc = *rec.get(at)?;
                    let _ = write!(parts, " P={enc:#04x}");
                    at += 1;
                    at += match enc & 0x0f {
                        0x02 | 0x0a => 2,
                        0x03 | 0x0b => 4,
                        0x00 | 0x04 | 0x0c => 8,
                        _ => return Some(parts + " P=?"),
                    };
                }
                'S' | 'B' | 'G' => {}
                _ => return Some(parts + " (unknown augmentation)"),
            }
        }
    }
    Some(parts)
}

impl Census {
    /// Adds one file: an object or an archive's members.
    pub fn add_file(&mut self, path: &Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return self.errors.push(format!("{}: {e}", path.display())),
        };
        if Archive::sniff(&bytes).is_some() {
            let archive = match Archive::parse(&bytes) {
                Ok(a) => a,
                Err(e) => return self.errors.push(format!("{}: {e}", path.display())),
            };
            for m in archive.members() {
                match m {
                    Ok(m) => {
                        if let Content::Inline { data, .. } = m.content() {
                            let name = format!(
                                "{}({})",
                                path.display(),
                                String::from_utf8_lossy(m.name())
                            );
                            self.add_object(&name, data);
                        }
                    }
                    Err(e) => self.errors.push(format!("{}: {e}", path.display())),
                }
            }
        } else {
            self.add_object(&path.display().to_string(), &bytes);
        }
    }

    /// Adds one object.
    pub fn add_object(&mut self, name: &str, bytes: &[u8]) {
        if let Err(e) = self.object(bytes) {
            self.errors.push(format!("{name}: {e}"));
        }
    }

    #[allow(clippy::too_many_lines)] // one pass over sections, then symbols
    fn object(&mut self, bytes: &[u8]) -> Result<(), mortar_elf::Error> {
        let obj = Object::parse(bytes)?;
        let machine: Machine = obj.machine();
        self.objects += 1;
        let bump = |m: &mut BTreeMap<String, usize>, k: String| *m.entry(k).or_default() += 1;
        let mut names = Vec::new();
        for s in obj.sections() {
            names.push(String::from_utf8_lossy(s.name()?).into_owned());
        }
        for s in obj.sections() {
            let name = &names[s.index().0 as usize];
            if s.index().0 == 0 {
                continue;
            }
            let ty = machine
                .section_type_name(s.sh_type())
                .map_or_else(|| format!("{:#x}", s.sh_type()), str::to_owned);
            bump(&mut self.sections, format!("{} {ty} {}", class(name), flags(s.flags())));
            match s.sh_type() {
                SHT_RELA => {
                    let target =
                        names.get(s.info_section()?.0 as usize).map_or("?", String::as_str);
                    for r in &s.rela()? {
                        let n = machine
                            .reloc_name(r.r_type)
                            .map_or_else(|| format!("type {}", r.r_type), str::to_owned);
                        bump(&mut self.relocations, format!("{n} in {}", class(target)));
                    }
                }
                SHT_GROUP => {
                    let g = s.group()?;
                    let kind = if g.is_comdat() { "comdat" } else { "plain" };
                    bump(&mut self.groups, format!("{kind} group"));
                    bump(
                        &mut self.groups,
                        format!("{kind} group with {} members", g.members().count()),
                    );
                }
                SHT_NOTE => {
                    for n in s.notes()? {
                        let n = n?;
                        let owner = String::from_utf8_lossy(n.name()).into_owned();
                        bump(&mut self.notes, format!("{name}: {owner} type {:#x}", n.n_type));
                        if let Some(props) = n.gnu_properties() {
                            for p in props {
                                let p = p?;
                                let v = p.as_u32().map_or_else(
                                    || format!("{} bytes", p.data.len()),
                                    |v| format!("{v:#x}"),
                                );
                                bump(&mut self.notes, format!("property {:#x}={v}", p.pr_type));
                            }
                        }
                    }
                }
                _ => {}
            }
            if name == ".eh_frame" {
                for k in eh_frame(s.data()?) {
                    bump(&mut self.eh_frame, k);
                }
            }
        }
        if let Some(symtab) = obj.symbols()? {
            for (i, sym) in symtab.iter() {
                if i.0 == 0 {
                    continue;
                }
                let ty = match sym.st_info & 0xf {
                    0 => "NOTYPE",
                    1 => "OBJECT",
                    2 => "FUNC",
                    3 => "SECTION",
                    4 => "FILE",
                    5 => "COMMON",
                    6 => "TLS",
                    10 => "IFUNC",
                    _ => "type?",
                };
                let bind = match sym.st_info >> 4 {
                    0 => "LOCAL",
                    1 => "GLOBAL",
                    2 => "WEAK",
                    10 => "UNIQUE",
                    _ => "bind?",
                };
                let vis =
                    ["DEFAULT", "INTERNAL", "HIDDEN", "PROTECTED"][usize::from(sym.st_other & 3)];
                let at = match sym.st_shndx {
                    0 => "undefined".to_owned(),
                    0xfff1 => "absolute".to_owned(),
                    0xfff2 => "common".to_owned(),
                    _ => match symtab.section_of(i, &sym) {
                        Ok(mortar_elf::SymbolSection::Section(x)) => {
                            names.get(x.0 as usize).map_or_else(|| "?".to_owned(), |n| class(n))
                        }
                        _ => "special".to_owned(),
                    },
                };
                let mut key = format!("{ty} {bind} {vis} in {at}");
                if sym.st_other & !3 != 0 {
                    let _ = write!(key, " other={:#x}", sym.st_other);
                }
                let name = String::from_utf8_lossy(symtab.name(&sym)?).into_owned();
                if name.starts_with("$x") || name.starts_with("$d") {
                    key = format!("mapping symbol {}", &name[..2]);
                }
                bump(&mut self.symbols, key);
            }
        }
        Ok(())
    }

    /// The census as text.
    pub fn render(&self, title: &str) -> String {
        let mut s = format!("## {title}: {} objects\n", self.objects);
        for (head, m) in [
            ("sections", &self.sections),
            ("relocations", &self.relocations),
            ("symbols", &self.symbols),
            ("groups", &self.groups),
            ("eh_frame", &self.eh_frame),
            ("notes", &self.notes),
        ] {
            let _ = writeln!(s, "### {head}");
            for (k, n) in m {
                let _ = writeln!(s, "{n:8}  {k}");
            }
        }
        for e in &self.errors {
            let _ = writeln!(s, "error: {e}");
        }
        s
    }
}

/// One of a census's tables.
type Table = fn(&Census) -> &BTreeMap<String, usize>;

/// Keys present in some groups and not in others, per table: the
/// differences a reviewer reads first.
pub fn compare(groups: &[(String, Vec<PathBuf>)]) -> Result<String, String> {
    let censuses: Vec<(String, Census)> = groups
        .iter()
        .map(|(n, files)| {
            let mut c = Census::default();
            for f in files {
                c.add_file(f);
            }
            (n.clone(), c)
        })
        .collect();
    let mut s = String::new();
    for (n, c) in &censuses {
        s.push_str(&c.render(n));
    }
    let _ = writeln!(
        s,
        "## differences (key: count per {})",
        censuses.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" / ")
    );
    let tables: [(&str, Table); 6] = [
        ("sections", |c| &c.sections),
        ("relocations", |c| &c.relocations),
        ("symbols", |c| &c.symbols),
        ("groups", |c| &c.groups),
        ("eh_frame", |c| &c.eh_frame),
        ("notes", |c| &c.notes),
    ];
    for (head, get) in tables {
        let keys: BTreeSet<&String> = censuses.iter().flat_map(|(_, c)| get(c).keys()).collect();
        let _ = writeln!(s, "### {head}");
        for k in keys {
            let counts: Vec<usize> =
                censuses.iter().map(|(_, c)| get(c).get(k).copied().unwrap_or(0)).collect();
            if counts.contains(&0) {
                let c: Vec<String> = counts.iter().map(usize::to_string).collect();
                let _ = writeln!(s, "{:>14}  {k}", c.join(" / "));
            }
        }
    }
    Ok(s)
}

/// `census [--group <name> <path>...]...`; paths alone form one group.
pub fn main(args: &[String]) -> Result<ExitCode, String> {
    let mut groups: Vec<(String, Vec<PathBuf>)> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--group" {
            let n = it.next().ok_or("--group needs a name")?;
            groups.push((n.clone(), Vec::new()));
        } else {
            if groups.is_empty() {
                groups.push(("objects".into(), Vec::new()));
            }
            if let Some(g) = groups.last_mut() {
                g.1.push(PathBuf::from(a));
            }
        }
    }
    if groups.iter().all(|(_, f)| f.is_empty()) {
        return Err("census needs files".into());
    }
    print!("{}", compare(&groups)?);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(class(".text._Z3foov"), ".text.*");
        assert_eq!(class(".rela.text._Z3foov"), ".rela.text.*");
        assert_eq!(class(".text"), ".text");
        assert_eq!(class(".text.startup"), ".text.startup");
        assert_eq!(class(".rodata.str1.1"), ".rodata.str1.1");
        assert_eq!(class(".data.rel.ro.local._ZTV1A"), ".data.rel.ro.local.*");
        assert_eq!(class(".init_array.101"), ".init_array.<prio>");
        assert_eq!(class(".debug_info"), ".debug_info");
    }

    #[test]
    fn flag_letters() {
        assert_eq!(flags(0x6), "AX");
        assert_eq!(flags(0x32), "AMS");
        assert_eq!(flags(0), "-");
        assert_eq!(flags(0x1_0000_0000), "+0x100000000");
    }

    #[test]
    fn eh_frame_cie_shape() {
        // A CIE as GCC writes it on AArch64: "zR", code 4, data -8, ra 30,
        // FDE encoding pcrel|sdata4; then an FDE (id non-zero), then the end.
        let mut d = vec![];
        let cie = [0u8, 0, 0, 0, 1, b'z', b'R', 0, 4, 0x78, 30, 1, 0x1b, 0, 0, 0];
        d.extend((cie.len() as u32).to_le_bytes());
        d.extend(cie);
        d.extend(8u32.to_le_bytes());
        d.extend([20u8, 0, 0, 0, 0, 0, 0, 0]);
        d.extend(0u32.to_le_bytes());
        assert_eq!(
            eh_frame(&d),
            ["CIE v1 \"zR\" code=4 data=-8 ra=30 R=0x1b", "FDE", "terminator"]
        );
    }
}
