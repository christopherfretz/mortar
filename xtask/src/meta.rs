//! `cargo xtask meta`: the test producer and the dumper of Mason metadata
//! containers (`spec/meta.md` §12, "The test producer"). A front end to
//! `mortar_meta`'s writer and reader: the format has one implementation.
//!
//!   cargo xtask meta write <description> -o <out.s>
//!   cargo xtask meta write <description> --bin <out>        (one section's bytes)
//!   cargo xtask meta write <description> --object <out.o> --target <arch>
//!   cargo xtask meta write <description> --into <in.o> -o <out.o>   (objcopy)
//!   cargo xtask meta dump <object>
//!
//! `<arch>` is `x86_64`, `aarch64` or `i386`.
//!
//! # The description
//!
//! One directive per line; `#` starts a comment; fields are `name=value`
//! in any order. A value is a number (decimal or `0x` hex), a string (a
//! bare word, or `"..."` with `\\`, `\"` and `\xNN`), or hex bytes. A
//! field left out is zero (or empty); `details` of `type` defaults to
//! "none" (`0xffffffff`).
//!
//! ```text
//! section                         # a new .mason.meta section (MT-4); the first is implied
//! unit producer=4 flags=0         # a new unit (the first is implied, producer 4)
//! pad 2                           # two zero words before the next unit (MT-10)
//! summary name=S value=S flags=N
//! definition name=S|anchor=S key=N hash=HEX class=N flags=N module=S file=S line=N
//! type name=S key=N hash=HEX layout=N size=N align=N flags=N file=S line=N details=N
//! declaration name=S|anchor=S key=N ty=N size=N type_name=S flags=N
//! details hex=HEX                 # kind 4's body
//! raw kind=N version=N row_size=N row_count=N hex=HEX   # any block, as given
//! ```
//!
//! `name=S` sets the key to FNV-1a 64 of `S` (MT-13) unless `key=` says
//! otherwise; `anchor=S` writes an anchored row (name 0) whose key is that
//! of `S`; a row with neither is anchored with the `key=` given.
//!
//! The dump is a description: `meta write` of a dump gives the bytes that
//! were dumped, for every unit the writer could have written (a canonical
//! unit, MT-38). The reader's findings are `#` lines.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mortar_meta::read::{BlockHeader, FileReader, Finding, SectionAttrs, Unit};
use mortar_meta::write::{Declaration, Definition, RawBlock, Summary, Type};
use mortar_meta::{UnitBuilder, kind, producer};

pub const USAGE: &str = "usage: cargo xtask meta write <description> (-o <out.s> | --bin <out> | \
                         --object <out.o> --target x86_64|aarch64|i386 | --into <in.o> -o <out.o>)\n       \
                         cargo xtask meta dump <object>\n";

/// One item of a section: a unit, or zero words before the next one.
#[derive(Debug, Clone)]
pub enum Item {
    Unit(UnitBuilder),
    Pad(usize),
}

/// The sections a description gives, each a sequence of items.
pub type Sections = Vec<Vec<Item>>;

/// Parses a description (the module's grammar).
///
/// # Errors
/// The line and what is wrong with it.
#[allow(clippy::too_many_lines)] // one arm per directive, read top to bottom
pub fn parse(text: &str) -> Result<Sections, String> {
    let mut sections: Sections = vec![Vec::new()];
    // Whether the current section's last unit was opened by `unit` or by
    // a row (rows after a `section` or `pad` open a new implicit unit).
    let mut open = false;
    for (n, line) in text.lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", n + 1);
        let words = split(line).map_err(at)?;
        let Some((directive, rest)) = words.split_first() else { continue };
        let items = sections.last_mut().expect("one section at least");
        if directive == b"pad" {
            let [count] = rest else { return Err(at("pad takes one count".into())) };
            let count = number(count).map_err(at)?;
            items.push(Item::Pad(usize::try_from(count).map_err(|e| at(e.to_string()))?));
            open = false;
            continue;
        }
        let fields = Fields::parse(rest).map_err(at)?;
        match directive.as_slice() {
            b"section" => {
                fields.done(&[]).map_err(at)?;
                sections.push(Vec::new());
                open = false;
            }
            b"unit" => {
                let mut u = UnitBuilder::new(producer::TEST);
                u.producer = fields.num("producer", u64::from(producer::TEST)).map_err(at)?;
                fields.done(&["producer"]).map_err(at)?;
                items.push(Item::Unit(u));
                open = true;
            }
            b"summary" => {
                let row = Summary {
                    name: fields.string("name"),
                    value: fields.string("value"),
                    flags: fields.num("flags", 0).map_err(at)?,
                };
                fields.done(&["name", "value", "flags"]).map_err(at)?;
                unit(items, &mut open).summary.push(row);
            }
            b"definition" => {
                let (key, name) = fields.keyed().map_err(at)?;
                let row = Definition {
                    key,
                    name,
                    hash: fields.hash().map_err(at)?,
                    class: fields.num("class", 0).map_err(at)?,
                    flags: fields.num("flags", 0).map_err(at)?,
                    module: fields.string("module"),
                    file: fields.string("file"),
                    line: fields.num("line", 0).map_err(at)?,
                };
                fields
                    .done(&[
                        "name", "anchor", "key", "hash", "class", "flags", "module", "file", "line",
                    ])
                    .map_err(at)?;
                unit(items, &mut open).definitions.push(row);
            }
            b"type" => {
                let name = fields.string("name");
                let key = match fields.get("key") {
                    Some(k) => number(k).map_err(at)?,
                    None => mortar_meta::fnv1a64(&name),
                };
                let row = Type {
                    key,
                    name,
                    hash: fields.hash().map_err(at)?,
                    layout: fields.num("layout", 0).map_err(at)?,
                    size: fields.num("size", 0).map_err(at)?,
                    align: fields.num("align", 0).map_err(at)?,
                    flags: fields.num("flags", 0).map_err(at)?,
                    file: fields.string("file"),
                    line: fields.num("line", 0).map_err(at)?,
                    details: fields
                        .num("details", u64::from(mortar_meta::NO_DETAILS))
                        .map_err(at)?,
                };
                fields
                    .done(&[
                        "name", "key", "hash", "layout", "size", "align", "flags", "file", "line",
                        "details",
                    ])
                    .map_err(at)?;
                unit(items, &mut open).types.push(row);
            }
            b"declaration" => {
                let (key, name) = fields.keyed().map_err(at)?;
                let row = Declaration {
                    key,
                    name,
                    ty: fields.num("ty", 0).map_err(at)?,
                    size: fields.num("size", 0).map_err(at)?,
                    type_name: fields.string("type_name"),
                    flags: fields.num("flags", 0).map_err(at)?,
                };
                fields
                    .done(&["name", "anchor", "key", "ty", "size", "type_name", "flags"])
                    .map_err(at)?;
                unit(items, &mut open).declarations.push(row);
            }
            b"details" => {
                let body = fields.bytes("hex").map_err(at)?;
                fields.done(&["hex"]).map_err(at)?;
                let u = unit(items, &mut open);
                if u.details.is_some() {
                    return Err(at("kind 4 given twice in one unit".into()));
                }
                u.details = Some(body);
            }
            b"raw" => {
                let block = RawBlock {
                    kind: fields.num("kind", 0).map_err(at)?,
                    version: fields.num("version", 1).map_err(at)?,
                    row_size: fields.num("row_size", 0).map_err(at)?,
                    row_count: fields.num("row_count", 0).map_err(at)?,
                    body: fields.bytes("hex").map_err(at)?,
                };
                fields.done(&["kind", "version", "row_size", "row_count", "hex"]).map_err(at)?;
                unit(items, &mut open).raw.push(block);
            }
            other => {
                return Err(at(format!("unknown directive '{}'", String::from_utf8_lossy(other))));
            }
        }
    }
    Ok(sections)
}

/// The unit rows go to: the open one, or a new implicit one.
fn unit<'a>(items: &'a mut Vec<Item>, open: &mut bool) -> &'a mut UnitBuilder {
    if !*open {
        items.push(Item::Unit(UnitBuilder::new(producer::TEST)));
        *open = true;
    }
    match items.last_mut() {
        Some(Item::Unit(u)) => u,
        _ => unreachable!("an open unit is the last item"),
    }
}

/// The bytes of one section: its units, written by `mortar_meta`, and its
/// padding.
///
/// # Errors
/// The writer's error, with the unit's place.
pub fn section_bytes(items: &[Item]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Unit(u) => out.extend(u.finish().map_err(|e| format!("unit {i}: {e}"))?),
            Item::Pad(words) => out.resize(out.len() + 8 * words, 0),
        }
    }
    Ok(out)
}

/// GNU as source for the sections: one `.mason.meta` per section (MT-1:
/// `SHT_PROGBITS`, `SHF_EXCLUDE` alone, aligned to 8), distinct by
/// `unique` when there are several (MT-4).
///
/// # Errors
/// [`section_bytes`]'s.
pub fn assembly(sections: &Sections) -> Result<String, String> {
    let mut s = String::from("# Mason metadata: cargo xtask meta write\n");
    for (i, items) in sections.iter().enumerate() {
        let bytes = section_bytes(items)?;
        let unique = if sections.len() > 1 { format!(",unique,{}", i + 1) } else { String::new() };
        let _ = writeln!(s, "\t.section .mason.meta,\"e\",%progbits{unique}\n\t.p2align 3");
        for line in bytes.chunks(16) {
            let row: Vec<String> = line.iter().map(|b| format!("0x{b:02x}")).collect();
            let _ = writeln!(s, "\t.byte {}", row.join(","));
        }
    }
    // An executable stack is not what the test objects mean to ask for.
    s.push_str("\t.section .note.GNU-stack,\"\",%progbits\n");
    Ok(s)
}

/// The text dump of every container of the ELF object `bytes` (MT-4: in
/// section-index order, one reader for the file, so that MT-35's report
/// comes once). A section named `.mason.meta` that is not a container
/// (MT-6) is a `#` line.
///
/// # Errors
/// The object cannot be parsed.
pub fn dump(bytes: &[u8]) -> Result<String, String> {
    let object = mortar_elf::Object::parse(bytes).map_err(|e| format!("not an object: {e}"))?;
    let mut reader = FileReader::new();
    let mut out = String::new();
    let mut first = true;
    let mut index = Vec::new();
    for s in object.sections() {
        let name = s.name().map_err(|e| e.to_string())?;
        if name != mortar_meta::SECTION_NAME {
            continue;
        }
        let i = s.index().0;
        if !mortar_meta::is_container(name, s.sh_type(), s.flags()) {
            let _ = writeln!(
                out,
                "# section {i}: not a container (sh_type {:#x}, sh_flags {:#x}; MT-6)",
                s.sh_type(),
                s.flags()
            );
            continue;
        }
        let h = s.header();
        let attrs = SectionAttrs {
            index: i,
            addralign: h.sh_addralign,
            entsize: h.sh_entsize,
            link: h.sh_link,
            info: h.sh_info,
        };
        reader.section(attrs, s.data().map_err(|e| e.to_string())?);
        index.push(i);
    }
    let mut units = reader.units.iter().peekable();
    let mut findings = reader.findings.iter().peekable();
    for &i in &index {
        if !first {
            out.push_str("section\n");
        }
        first = false;
        let _ = writeln!(out, "# section {i}");
        // Units and findings of this section, by offset.
        loop {
            let u = units.peek().filter(|u| u.section == i).map(|u| u.offset);
            let f = findings.peek().filter(|f| f.section() == i).map(|f| finding_at(f));
            match (u, f) {
                (Some(a), Some(b)) if b < a => print_finding(&mut out, findings.next().unwrap()),
                (Some(_), _) => print_unit(&mut out, units.next().unwrap()),
                (None, Some(_)) => print_finding(&mut out, findings.next().unwrap()),
                (None, None) => break,
            }
        }
    }
    Ok(out)
}

fn finding_at(f: &Finding) -> u64 {
    match f {
        Finding::Malformed { unit, .. } => unit.unwrap_or(0),
        Finding::Version { unit, .. } => *unit,
    }
}

fn print_finding(out: &mut String, f: &Finding) {
    match f {
        Finding::Version { unit, major, .. } => {
            let _ = writeln!(out, "# meta-version: unit at offset {unit}: major {major}");
        }
        Finding::Malformed { .. } => {
            let _ = writeln!(out, "# meta-malformed: {}", f.reason());
        }
    }
}

/// Whether a known kind's block is the one this writer writes (version 1,
/// the v1 row size); a later version is dumped raw, so that writing the
/// dump back gives its bytes.
fn as_written(b: &BlockHeader) -> bool {
    b.version == 1 && kind::row_size(b.kind).is_some_and(|r| r == b.row_size)
}

fn print_unit(out: &mut String, u: &Unit<'_>) {
    let _ = writeln!(
        out,
        "# unit at offset {}: {} bytes, minor {}, header_size {}, flags {:#x}",
        u.offset,
        u.bytes().len(),
        u.minor,
        u.header_size,
        u.flags
    );
    let _ = writeln!(out, "unit producer={}", u.producer);
    for b in u.blocks() {
        let known = kind::row_size(b.kind).is_some();
        if known && as_written(&b) {
            print_rows(out, u, b.kind);
            continue;
        }
        let body = u
            .bytes()
            .get((b.at + mortar_meta::BLOCK_HEADER_SIZE) as usize..(b.at + b.block_size) as usize)
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "raw kind={:#06x} version={} row_size={} row_count={} hex={}",
            b.kind,
            b.version,
            b.row_size,
            b.row_count,
            hex(body)
        );
    }
}

fn print_rows(out: &mut String, u: &Unit<'_>, k: u16) {
    let name_or_anchor = |out: &mut String, name: u32, key: u64| {
        if name == 0 {
            let _ = write!(out, " key={key:#018x}");
        } else {
            let n = u.string(name);
            let _ = write!(out, " name={}", quote(n));
            if mortar_meta::fnv1a64(n) != key {
                let _ = write!(out, " key={key:#018x}");
            }
        }
    };
    match k {
        kind::SUMMARY => {
            for r in u.summary().iter() {
                let _ = writeln!(
                    out,
                    "summary name={} value={} flags={:#x}",
                    quote(u.string(r.name)),
                    quote(u.string(r.value)),
                    r.flags
                );
            }
        }
        kind::DEFINITIONS => {
            for r in u.definitions().iter() {
                out.push_str("definition");
                name_or_anchor(out, r.name, r.key);
                let _ = writeln!(
                    out,
                    " hash={} class={} flags={:#x} module={} file={} line={}",
                    hex(&r.hash),
                    r.class,
                    r.flags,
                    quote(u.string(r.module)),
                    quote(u.string(r.file)),
                    r.line
                );
            }
        }
        kind::TYPES => {
            for r in u.types().iter() {
                let _ = write!(out, "type name={}", quote(u.string(r.name)));
                if mortar_meta::fnv1a64(u.string(r.name)) != r.key {
                    let _ = write!(out, " key={:#018x}", r.key);
                }
                let _ = writeln!(
                    out,
                    " hash={} layout={:#x} size={} align={} flags={:#x} file={} line={} details={:#x}",
                    hex(&r.hash),
                    r.layout,
                    r.size,
                    r.align,
                    r.flags,
                    quote(u.string(r.file)),
                    r.line,
                    r.details
                );
            }
        }
        kind::DETAILS => {
            let _ = writeln!(out, "details hex={}", hex(u.details().unwrap_or_default()));
        }
        kind::DECLARATIONS => {
            for r in u.declarations().iter() {
                out.push_str("declaration");
                name_or_anchor(out, r.name, r.key);
                let _ = writeln!(
                    out,
                    " ty={:#018x} size={} type_name={} flags={:#x}",
                    r.ty,
                    r.size,
                    quote(u.string(r.type_name)),
                    r.flags
                );
            }
        }
        _ => {}
    }
}

/// A string as the description writes it: bare when it is a plain word.
fn quote(s: &[u8]) -> String {
    let plain = !s.is_empty()
        && s.iter().all(|&b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\' | b'#' | b'='));
    if plain {
        return String::from_utf8_lossy(s).into_owned();
    }
    let mut out = String::from("\"");
    for &b in s {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b' '..=b'~' => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out.push('"');
    out
}

fn hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "-".into();
    }
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A line's words, as bytes: `"..."` is one word (with its escapes);
/// `#` outside quotes ends the line. A `name=` prefix and its quoted value
/// are one word.
fn split(line: &str) -> Result<Vec<Vec<u8>>, String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i] == b'#' {
            break;
        }
        let mut w = Vec::new();
        while i < b.len() && !b[i].is_ascii_whitespace() {
            if b[i] != b'"' {
                w.push(b[i]);
                i += 1;
                continue;
            }
            // Mark the word as quoted (a quoted empty string is a value).
            w.push(0);
            i += 1;
            loop {
                match b.get(i) {
                    None => return Err("unterminated string".into()),
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') => match b.get(i + 1) {
                        Some(b'x') => {
                            let h = line.get(i + 2..i + 4).ok_or("short \\x escape")?;
                            w.push(u8::from_str_radix(h, 16).map_err(|e| e.to_string())?);
                            i += 4;
                        }
                        Some(&c @ (b'"' | b'\\')) => {
                            w.push(c);
                            i += 2;
                        }
                        _ => return Err("unknown escape".into()),
                    },
                    Some(&c) => {
                        w.push(c);
                        i += 1;
                    }
                }
            }
        }
        out.push(w);
    }
    Ok(out)
}

fn number(w: &[u8]) -> Result<u64, String> {
    let s = std::str::from_utf8(w).map_err(|e| e.to_string())?;
    let r = match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| format!("'{s}' is not a number"))
}

/// The `name=value` fields of a directive.
struct Fields(Vec<(Vec<u8>, Vec<u8>)>);

impl Fields {
    fn parse(words: &[Vec<u8>]) -> Result<Self, String> {
        let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for w in words {
            let eq = w
                .iter()
                .position(|&b| b == b'=')
                .ok_or_else(|| format!("'{}' is not name=value", String::from_utf8_lossy(w)))?;
            let (k, v) = (w[..eq].to_vec(), w[eq + 1..].to_vec());
            if out.iter().any(|(x, _)| *x == k) {
                return Err(format!("field '{}' given twice", String::from_utf8_lossy(&k)));
            }
            out.push((k, v));
        }
        Ok(Self(out))
    }

    /// The raw value: a quoted one starts with the 0 [`split`] put there.
    fn get(&self, name: &str) -> Option<&[u8]> {
        self.0.iter().find(|(k, _)| k == name.as_bytes()).map(|(_, v)| v.as_slice())
    }

    fn string(&self, name: &str) -> Vec<u8> {
        match self.get(name) {
            Some([0, rest @ ..]) => rest.to_vec(),
            Some(v) => v.to_vec(),
            None => Vec::new(),
        }
    }

    fn num<T: TryFrom<u64>>(&self, name: &str, default: u64) -> Result<T, String> {
        let v = self.get(name).map_or(Ok(default), number)?;
        T::try_from(v).map_err(|_| format!("{name}={v} does not fit its field"))
    }

    fn bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        let Some(v) = self.get(name) else { return Ok(Vec::new()) };
        if v == b"-" {
            return Ok(Vec::new());
        }
        if v.len() % 2 != 0 {
            return Err(format!("{name}: an odd number of hex digits"));
        }
        v.chunks(2)
            .map(|p| {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| format!("{name}: not hex"))
            })
            .collect()
    }

    fn hash(&self) -> Result<[u8; 16], String> {
        let b = self.bytes("hash")?;
        if b.is_empty() {
            return Ok([0; 16]);
        }
        b.try_into().map_err(|_| "hash= takes 16 bytes (32 hex digits)".into())
    }

    /// `name=` / `anchor=` / `key=` of kinds 2 and 5.
    fn keyed(&self) -> Result<(u64, Option<Vec<u8>>), String> {
        let key = self.get("key").map(number).transpose()?;
        match (self.get("name"), self.get("anchor")) {
            (Some(_), Some(_)) => Err("name= and anchor= exclude each other".into()),
            (Some(_), None) => {
                let n = self.string("name");
                Ok((key.unwrap_or_else(|| mortar_meta::fnv1a64(&n)), Some(n)))
            }
            (None, Some(_)) => {
                let a = self.string("anchor");
                Ok((key.unwrap_or_else(|| mortar_meta::fnv1a64(&a)), None))
            }
            (None, None) => Ok((key.unwrap_or(0), None)),
        }
    }

    fn done(&self, known: &[&str]) -> Result<(), String> {
        match self.0.iter().find(|(k, _)| !known.iter().any(|n| n.as_bytes() == k.as_slice())) {
            Some((k, _)) => Err(format!("unknown field '{}'", String::from_utf8_lossy(k))),
            None => Ok(()),
        }
    }
}

/// The assembler of a target (`spec/harness.md`'s three targets).
fn assembler(target: &str) -> Result<(&'static str, &'static [&'static str]), String> {
    match target {
        "x86_64" => Ok(("x86_64-linux-gnu-as", &["--64"])),
        "i386" => Ok(("i686-linux-gnu-as", &["--32"])),
        "aarch64" => Ok(("aarch64-linux-gnu-as", &[])),
        _ => Err(format!("unknown target '{target}' (x86_64, aarch64, i386)")),
    }
}

fn run_tool(name: &str, args: &[String]) -> Result<(), String> {
    let program = crate::mason::tool(name).ok_or_else(|| format!("{name} is not installed"))?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let o = crate::mason::run_in(&program, args, &dir, &[], crate::mason::TOOL_LIMITS)?;
    if o.end.success() {
        Ok(())
    } else {
        Err(format!("{name} failed: {}", String::from_utf8_lossy(&o.stderr)))
    }
}

/// Assembles `sections` into the relocatable object `out` for `target`.
///
/// # Errors
/// The writer's, or the assembler's.
pub fn assemble(sections: &Sections, target: &str, out: &Path) -> Result<(), String> {
    let (name, flags) = assembler(target)?;
    let s = out.with_extension("mason.s");
    std::fs::write(&s, assembly(sections)?).map_err(|e| e.to_string())?;
    let mut args: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
    args.extend(["-o".into(), out.display().to_string(), s.display().to_string()]);
    let r = run_tool(name, &args);
    let _ = std::fs::remove_file(&s);
    r
}

/// Adds one container holding `items` to the object `input`, as `out`.
/// objcopy (binutils 2.44) gives an added section `SHF_WRITE` unless told
/// `readonly`, and ignores `--set-section-alignment` for a section it adds
/// in the same run: so two runs, the second setting MT-1's alignment.
///
/// # Errors
/// The writer's, or objcopy's.
pub fn add_to(items: &[Item], input: &Path, out: &Path) -> Result<(), String> {
    let program = crate::mason::tool("objcopy").ok_or("objcopy is not installed")?;
    add_with(&program, items, input, out)
}

/// [`add_to`] with the `objcopy` given (a cross target's own).
///
/// # Errors
/// The writer's, or objcopy's.
pub fn add_with(objcopy: &Path, items: &[Item], input: &Path, out: &Path) -> Result<(), String> {
    let run = |args: &[String]| -> Result<(), String> {
        let dir = std::env::current_dir().map_err(|e| e.to_string())?;
        let o = crate::mason::run_in(objcopy, args, &dir, &[], crate::mason::TOOL_LIMITS)?;
        if o.end.success() {
            Ok(())
        } else {
            Err(format!("objcopy failed: {}", String::from_utf8_lossy(&o.stderr)))
        }
    };
    let bin = out.with_extension("mason.bin");
    let step = out.with_extension("mason.o");
    std::fs::write(&bin, section_bytes(items)?).map_err(|e| e.to_string())?;
    let r = run(&[
        "--add-section".into(),
        format!(".mason.meta={}", bin.display()),
        "--set-section-flags".into(),
        ".mason.meta=exclude,readonly".into(),
        input.display().to_string(),
        step.display().to_string(),
    ])
    .and_then(|()| {
        run(&[
            "--set-section-alignment".into(),
            ".mason.meta=8".into(),
            step.display().to_string(),
            out.display().to_string(),
        ])
    });
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&step);
    r
}

/// `cargo xtask meta ...`.
pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprint!("xtask meta: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, object] if cmd == "dump" => {
            let bytes = std::fs::read(object).map_err(|e| format!("{object}: {e}"))?;
            print!("{}", dump(&bytes)?);
            Ok(())
        }
        [cmd, description, rest @ ..] if cmd == "write" => {
            let text =
                std::fs::read_to_string(description).map_err(|e| format!("{description}: {e}"))?;
            let sections = parse(&text)?;
            let mut out: Option<PathBuf> = None;
            let mut mode: Option<(&str, &str)> = None;
            let mut target: Option<&str> = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                let mut value =
                    || it.next().map(String::as_str).ok_or(format!("{a} takes a value"));
                match a.as_str() {
                    "-o" => out = Some(PathBuf::from(value()?)),
                    "--bin" | "--object" | "--into" => mode = Some((a.as_str(), value()?)),
                    "--target" => target = Some(value()?),
                    _ => return Err(format!("unknown argument '{a}'")),
                }
            }
            match (mode, out, target) {
                (None, Some(o), None) => {
                    std::fs::write(&o, assembly(&sections)?).map_err(|e| e.to_string())
                }
                (Some(("--bin", path)), None, None) => {
                    let [items] = sections.as_slice() else {
                        return Err("--bin writes one section".into());
                    };
                    std::fs::write(path, section_bytes(items)?).map_err(|e| e.to_string())
                }
                (Some(("--object", path)), None, Some(t)) => {
                    assemble(&sections, t, Path::new(path))
                }
                (Some(("--into", input)), Some(o), None) => {
                    let [items] = sections.as_slice() else {
                        return Err("--into adds one section".into());
                    };
                    add_to(items, Path::new(input), &o)
                }
                _ => Err("one output: -o, --bin, --object with --target, or --into with -o".into()),
            }
        }
        _ => Err("unknown command".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &str = r#"
# every directive
summary name=lang value=c++ flags=1
summary name=std value="c++ 20"
definition name=_ZN1A1fEv hash=000102030405060708090a0b0c0d0e0f class=1 flags=0x3 module=- file=a.h line=7
definition anchor=_ZN1A1gEv class=2
definition key=0x1234 class=1
type name=N1A size=8 align=4 file="a b.h" line=3
type name="\x01odd\"" key=5 details=0
declaration name=_Z1hv ty=0x99 size=4 type_name=int flags=2
declaration anchor=x
details hex=0102030405060708
raw kind=0x7f00 version=3 row_size=8 row_count=1 hex=aabbccddeeff0011
pad 2
unit producer=1
summary name=a value=b
section
definition name=f
"#;

    // spec: meta.md §12 ("the test producer"), MT-38: the dump of a
    // written unit writes back to the same bytes.
    #[test]
    fn dump_writes_back() {
        let sections = parse(ALL).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].len(), 3, "a unit, a pad, a unit");
        for items in &sections {
            let bytes = section_bytes(items).unwrap();
            let mut r = FileReader::new();
            r.section(SectionAttrs::conforming(1), &bytes);
            assert!(r.findings.is_empty(), "{:?}", r.findings);
            let mut text = String::new();
            for u in &r.units {
                print_unit(&mut text, u);
            }
            let again = parse(&text).unwrap();
            let units: Vec<&Item> = items.iter().filter(|i| matches!(i, Item::Unit(_))).collect();
            let back: Vec<Vec<u8>> = again[0]
                .iter()
                .map(|i| match i {
                    Item::Unit(u) => u.finish().unwrap(),
                    Item::Pad(_) => unreachable!(),
                })
                .collect();
            let want: Vec<Vec<u8>> = units
                .iter()
                .map(|i| match i {
                    Item::Unit(u) => u.finish().unwrap(),
                    Item::Pad(_) => unreachable!(),
                })
                .collect();
            assert_eq!(back, want, "{text}");
        }
    }

    #[test]
    fn fields() {
        let s = parse("definition name=f\n").unwrap();
        let Item::Unit(u) = &s[0][0] else { panic!() };
        assert_eq!(u.definitions[0].key, mortar_meta::fnv1a64(b"f"));
        assert_eq!(u.producer, producer::TEST);
        let s = parse("declaration anchor=g\n").unwrap();
        let Item::Unit(u) = &s[0][0] else { panic!() };
        assert_eq!(
            (u.declarations[0].key, u.declarations[0].name.clone()),
            (mortar_meta::fnv1a64(b"g"), None)
        );
        for bad in [
            "summary nam=x",
            "summary name=x name=y",
            "definition name=a anchor=b",
            "frob",
            "summary name=\"x",
            "details hex=abc",
            "definition hash=00",
            "summary flags=0x100000000",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(quote(b""), "\"\"");
        assert_eq!(quote(b"a=b"), "\"a=b\"");
    }

    #[test]
    fn assembly_has_one_section_per_section() {
        let s = parse(ALL).unwrap();
        let a = assembly(&s).unwrap();
        assert_eq!(a.matches(".section .mason.meta,\"e\",%progbits,unique,").count(), 2);
        let one = assembly(&parse("summary name=a value=b\n").unwrap()).unwrap();
        assert!(one.contains(".section .mason.meta,\"e\",%progbits\n\t.p2align 3"));
    }
}
