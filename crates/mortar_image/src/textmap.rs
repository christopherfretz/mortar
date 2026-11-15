//! The text map (§9.3, I-120..I-122): GNU ld 2.44's `-Map` layout, for
//! people and the scripts that read GNU maps. The machine map is
//! `crate::map` (`mortar-map 1`, `--mortar-map`).

use std::fmt::Write as _;

use mortar_core::par::Pool;

/// GNU's `SECTION_NAME_MAP_LENGTH`: the column addresses start at.
const NAME_COLUMN: usize = 16;
/// The column GNU's archive lines put the referencing file at.
const ARCHIVE_COLUMN: usize = 30;

/// One line inside an output section, in address order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A placed input section (or a linker-made chunk, file `mortar`).
    Input { name: Vec<u8>, addr: u64, size: u64, file: Vec<u8> },
    /// A global symbol defined in the input section before it.
    Symbol { addr: u64, name: Vec<u8> },
    /// Bytes between two inputs: the fill pattern of a code section, or
    /// none (zeros).
    Fill { addr: u64, size: u64, pattern: Option<Vec<u8>> },
}

/// An output section: `addr` is 0 for a non-alloc one (GNU's), whose
/// items then hold offsets from its start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutSection {
    pub name: Vec<u8>,
    pub addr: u64,
    pub size: u64,
    pub items: Vec<Item>,
}

/// Everything the text map shows, in its order (I-121).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextMap {
    /// The address width in hex digits: 16 (ELFCLASS64) or 8.
    pub digits: usize,
    /// Per extracted archive member: `ARCHIVE(MEMBER)`, the file whose
    /// reference extracted it, and the symbol.
    pub archive: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// Per discarded input section: name, size, file.
    pub discarded: Vec<(Vec<u8>, u64, Vec<u8>)>,
    /// The `LOAD` lines: each input file, an archive once.
    pub loads: Vec<Vec<u8>>,
    pub sections: Vec<OutSection>,
    /// The output file as the command line named it.
    pub output: Vec<u8>,
    /// GNU's BFD target name.
    pub target: &'static str,
}

/// GNU's BFD target name for an ELF machine and class.
#[must_use]
pub fn target_name(machine: u16, class64: bool) -> &'static str {
    match (machine, class64) {
        (183, true) => "elf64-littleaarch64",
        (62, true) => "elf64-x86-64",
        (3, false) => "elf32-i386",
        (_, true) => "elf64-little",
        (_, false) => "elf32-little",
    }
}

fn text(out: &mut String, b: &[u8]) {
    out.push_str(&String::from_utf8_lossy(b));
}

/// `name` at the line's start, then the address column: on the next line
/// when the name reaches it (GNU's rule: `len >= 15`).
fn name_column(out: &mut String, lead: &str, name: &[u8]) {
    out.push_str(lead);
    text(out, name);
    let len = lead.len() + name.len();
    if len >= NAME_COLUMN - 1 {
        out.push('\n');
        out.push_str(&" ".repeat(NAME_COLUMN));
    } else {
        out.push_str(&" ".repeat(NAME_COLUMN - len));
    }
}

fn addr_size(out: &mut String, digits: usize, addr: u64, size: u64) {
    let size = format!("{size:#x}");
    let _ = write!(out, "0x{addr:0digits$x} {size:>10}");
}

fn section_text(s: &OutSection, digits: usize) -> String {
    let mut out = String::new();
    out.push('\n');
    name_column(&mut out, "", &s.name);
    addr_size(&mut out, digits, s.addr, s.size);
    out.push('\n');
    for item in &s.items {
        match item {
            Item::Input { name, addr, size, file } => {
                name_column(&mut out, " ", name);
                addr_size(&mut out, digits, *addr, *size);
                out.push(' ');
                text(&mut out, file);
                out.push('\n');
            }
            Item::Symbol { addr, name } => {
                let pad = " ".repeat(NAME_COLUMN);
                let _ = write!(out, "{pad}0x{addr:0digits$x}{pad}");
                text(&mut out, name);
                out.push('\n');
            }
            Item::Fill { addr, size, pattern } => {
                name_column(&mut out, " ", b"*fill*");
                addr_size(&mut out, digits, *addr, *size);
                if let Some(p) = pattern {
                    out.push(' ');
                    for b in p {
                        let _ = write!(out, "{b:02x}");
                    }
                }
                out.push('\n');
            }
        }
    }
    out
}

/// The text of `map` (I-121, I-122): output sections formatted in
/// parallel, concatenated in order.
#[must_use]
pub fn render(map: &TextMap, pool: &Pool) -> Vec<u8> {
    let digits = map.digits;
    let mut out = String::new();
    if !map.archive.is_empty() {
        out.push_str("Archive member included to satisfy reference by file (symbol)\n\n");
        for (member, by, symbol) in &map.archive {
            text(&mut out, member);
            if member.len() >= ARCHIVE_COLUMN {
                out.push('\n');
                out.push_str(&" ".repeat(ARCHIVE_COLUMN));
            } else {
                out.push_str(&" ".repeat(ARCHIVE_COLUMN - member.len()));
            }
            text(&mut out, by);
            out.push_str(" (");
            text(&mut out, symbol);
            out.push_str(")\n");
        }
        out.push('\n');
    }
    if !map.discarded.is_empty() {
        out.push_str("Discarded input sections\n\n");
        for (name, size, file) in &map.discarded {
            name_column(&mut out, " ", name);
            addr_size(&mut out, digits, 0, *size);
            out.push(' ');
            text(&mut out, file);
            out.push('\n');
        }
        out.push('\n');
    }
    let ones = if digits >= 16 { u64::MAX } else { (1 << (4 * digits)) - 1 };
    out.push_str("Memory Configuration\n\nName             Origin             Length             Attributes\n");
    let _ = writeln!(out, "*default*        0x{0:0digits$x} 0x{ones:0digits$x}", 0);
    out.push_str("\nLinker script and memory map\n\n");
    for l in &map.loads {
        out.push_str("LOAD ");
        text(&mut out, l);
        out.push('\n');
    }
    let parts = pool.map(map.sections.len(), |k| section_text(&map.sections[k], digits));
    for p in parts {
        out.push_str(&p);
    }
    out.push_str("OUTPUT(");
    text(&mut out, &map.output);
    let _ = writeln!(out, " {})", map.target);
    // GNU's pseudo-file of its stubs, the file the patch lines name.
    out.push_str("LOAD linker stubs\n");
    out.into_bytes()
}
