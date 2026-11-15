//! What the tools print, shared by the L1 tests: number formats, the
//! spellings `readelf` and `llvm-readobj` use, a parser for
//! `llvm-readobj`'s block output, and running a tool that must not warn.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64, SHN_X86_64_LCOMMON};
use mortar_elf::{Class, Sym, SymbolSection};
use mortar_testutil::{Outcome, run_with};

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A number as the tools print it: `0x` and hexadecimal, or decimal.
pub fn number(s: &str) -> u64 {
    let s = s.trim();
    let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => s.parse(),
    };
    parsed.unwrap_or_else(|_| panic!("not a number: `{s}`"))
}

pub fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim(), 16).unwrap_or_else(|_| panic!("not hexadecimal: `{s}`"))
}

/// The number in the last parentheses of `s`: `SHT_RELA (0x4)` is 4.
pub fn in_parens(s: &str) -> u64 {
    let open = s.rfind('(').unwrap_or_else(|| panic!("no `(` in `{s}`"));
    let close = s[open..].find(')').map_or_else(|| panic!("`{s}`"), |at| open + at);
    number(&s[open + 1..close])
}

/// `name (n)`: the text before the last ` (` and the number in it.
pub fn name_and_number(s: &str) -> (&str, u64) {
    match s.rsplit_once(" (") {
        Some((name, rest)) => (name, number(rest.trim_end_matches(')'))),
        None => panic!("not `name (number)`: `{s}`"),
    }
}

pub fn run_tool(program: &Path, args: &[&str], file: &Path, timeout: Duration) -> String {
    let mut command = Command::new(program);
    command.args(args).arg(file);
    let outcome: Outcome = run_with(&mut command, &[], timeout);
    assert!(
        outcome.success(),
        "{} {args:?} {}: {}",
        program.display(),
        file.display(),
        outcome.stderr_text()
    );
    // A tool that warns about a file it read has found something the reader
    // did not: neither tool warns about a well-formed object.
    assert!(
        outcome.stderr.is_empty(),
        "{} {args:?} {}: {}",
        program.display(),
        file.display(),
        outcome.stderr_text()
    );
    outcome.stdout_text()
}

pub fn readelf_symbol_type(sym_type: u8) -> &'static str {
    match sym_type {
        0 => "NOTYPE",
        1 => "OBJECT",
        2 => "FUNC",
        3 => "SECTION",
        4 => "FILE",
        5 => "COMMON",
        6 => "TLS",
        10 => "IFUNC",
        other => panic!("symbol type {other}: add readelf's spelling"),
    }
}

pub fn readelf_binding(binding: u8) -> &'static str {
    match binding {
        0 => "LOCAL",
        1 => "GLOBAL",
        2 => "WEAK",
        10 => "UNIQUE",
        other => panic!("binding {other}: add readelf's spelling"),
    }
}

pub fn readelf_visibility(sym: &Sym, e_machine: u16) -> String {
    let mut shown =
        ["DEFAULT", "INTERNAL", "HIDDEN", "PROTECTED"][usize::from(sym.visibility())].to_owned();
    let other = sym.st_other & !3;
    if other != 0 {
        if e_machine == EM_AARCH64 && other == mortar_elf::consts::STO_AARCH64_VARIANT_PCS {
            shown.push_str(" [VARIANT_PCS]");
        } else {
            panic!("st_other {:#x}: add readelf's spelling", sym.st_other);
        }
    }
    shown
}

pub fn readelf_section_index(section: SymbolSection, e_machine: u16) -> String {
    match section {
        SymbolSection::Undefined => "UND".to_owned(),
        SymbolSection::Absolute => "ABS".to_owned(),
        SymbolSection::Common => "COM".to_owned(),
        SymbolSection::Section(index) => index.0.to_string(),
        SymbolSection::Reserved(SHN_X86_64_LCOMMON) if e_machine == EM_X86_64 => {
            "LARGE_COM".to_owned()
        }
        SymbolSection::Reserved(other) => panic!("st_shndx {other:#x}: add readelf's spelling"),
    }
}

/// The type of a note as each tool prints it: (`readelf`, `llvm-readobj`).
pub fn note_type(owner: &[u8], n_type: u32) -> (&'static str, &'static str) {
    match (owner, n_type) {
        (b"GNU", 1) => ("NT_GNU_ABI_TAG (ABI version tag)", "NT_GNU_ABI_TAG (ABI version tag)"),
        (b"GNU", 3) => (
            "NT_GNU_BUILD_ID (unique build ID bitstring)",
            "NT_GNU_BUILD_ID (unique build ID bitstring)",
        ),
        (b"GNU", 5) => ("NT_GNU_PROPERTY_TYPE_0", "NT_GNU_PROPERTY_TYPE_0 (property note)"),
        // LLVM 22 does not name the SystemTap and FDO types.
        (b"stapsdt", 3) => ("NT_STAPSDT (SystemTap probe descriptors)", "Unknown (0x00000003)"),
        (b"FDO", 0xcafe_1a7e) => ("FDO_PACKAGING_METADATA", "Unknown (0xcafe1a7e)"),
        _ => panic!("note {} type {n_type:#x}: add the tools' spellings", text(owner)),
    }
}

/// The names of the set bits of `word`, lowest first, from `names`
/// (bit 0 first); a bit without a name fails.
pub fn bits(word: u32, names: &[&str]) -> String {
    let mut out = Vec::new();
    for bit in 0..32 {
        if word & (1 << bit) != 0 {
            let name = names.get(bit).unwrap_or_else(|| panic!("bit {bit} of {word:#x}"));
            out.push(*name);
        }
    }
    out.join(", ")
}

pub const X86_ISA: [&str; 4] = ["x86-64-baseline", "x86-64-v2", "x86-64-v3", "x86-64-v4"];

pub const X86_FEATURE_2: [&str; 12] = [
    "x86", "x87", "MMX", "XMM", "YMM", "ZMM", "FXSR", "XSAVE", "XSAVEOPT", "XSAVEC", "TMM", "MASK",
];

/// A program property as each tool prints it: (`readelf`, `llvm-readobj`).
pub fn property_text(e_machine: u16, class: Class, pr_type: u32, data: &[u8]) -> (String, String) {
    let word = || {
        let bytes: [u8; 4] = data.try_into().unwrap_or_else(|_| panic!("property {pr_type:#x}"));
        u32::from_le_bytes(bytes)
    };
    let x86 = matches!(e_machine, EM_X86_64 | EM_386);
    let same = |s: String| (s.clone(), s);
    match pr_type {
        1 => {
            let mut bytes = [0u8; 8];
            bytes[..data.len()].copy_from_slice(data);
            assert_eq!(data.len(), class.word_size());
            same(format!("stack size: {:#x}", u64::from_le_bytes(bytes)))
        }
        0xc000_0000 if e_machine == EM_AARCH64 => {
            let names = bits(word(), &["BTI", "PAC", "GCS"]);
            (format!("AArch64 feature: {names}"), format!("aarch64 feature: {names}"))
        }
        0xc000_0002 if x86 => {
            same(format!("x86 feature: {}", bits(word(), &["IBT", "SHSTK", "LAM_U48", "LAM_U57"])))
        }
        0xc000_8002 | 0xc001_0002 if x86 => {
            let kind = if pr_type == 0xc000_8002 { "needed" } else { "used" };
            let names = bits(word(), &X86_ISA);
            let llvm = if names.is_empty() { "<None>".to_owned() } else { names.clone() };
            (format!("x86 ISA {kind}: {names}"), format!("x86 ISA {kind}: {llvm}"))
        }
        0xc000_8001 | 0xc001_0001 if x86 => {
            let kind = if pr_type == 0xc000_8001 { "needed" } else { "used" };
            let names = bits(word(), &X86_FEATURE_2);
            let llvm = if names.is_empty() { "<None>".to_owned() } else { names.clone() };
            (format!("x86 feature {kind}: {names}"), format!("x86 feature {kind}: {llvm}"))
        }
        other => panic!("property {other:#x}: add the tools' spellings"),
    }
}

/// A size column: decimal, or hexadecimal with `0x` when large.
pub fn number_of(s: &str) -> u64 {
    number(s)
}

/// An ABI tag note as (`readelf`, `llvm-readobj` OS, ABI).
pub fn abi_tag(desc: &[u8]) -> (String, String, String) {
    let words: Vec<u32> =
        desc.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().expect("a word"))).collect();
    assert_eq!(words.len(), 4, "an ABI tag is four words");
    let os = match words[0] {
        0 => "Linux",
        other => panic!("ABI tag OS {other}: add its name"),
    };
    let abi = format!("{}.{}.{}", words[1], words[2], words[3]);
    (format!("OS: {os}, ABI: {abi}"), os.to_owned(), abi)
}

/// A block of `llvm-readobj`'s output: the line that opens it and its
/// lines and blocks in order.
#[derive(Debug, Default)]
pub struct Node {
    pub head: String,
    pub items: Vec<Item>,
}

#[derive(Debug)]
pub enum Item {
    Line(String),
    Node(Node),
}

impl Node {
    /// Parses the indented `Key: value` / `Key {` ... `}` output.
    pub fn parse(output: &str) -> Node {
        let mut stack = vec![Node::default()];
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line == "}" || line == "]" {
                let node = stack.pop().expect("an open block");
                stack.last_mut().expect("the root").items.push(Item::Node(node));
                continue;
            }
            let opens = line.ends_with(" {")
                || line.ends_with(" [")
                || line == "{"
                || line == "["
                || (line.contains(" [ (") && line.ends_with(')'));
            if opens {
                let head = line.trim_end_matches(['{', '[']).trim_end().to_owned();
                stack.push(Node { head, items: Vec::new() });
            } else {
                stack.last_mut().expect("a block").items.push(Item::Line(line.to_owned()));
            }
        }
        assert_eq!(stack.len(), 1, "llvm-readobj output has unclosed blocks");
        stack.pop().expect("the root")
    }

    pub fn nodes(&self, head: &str) -> impl Iterator<Item = &Node> {
        let spaced = format!("{head} ");
        let head = head.to_owned();
        self.items.iter().filter_map(move |item| match item {
            Item::Node(node) if node.head == head || node.head.starts_with(&spaced) => Some(node),
            _ => None,
        })
    }

    pub fn node(&self, head: &str) -> &Node {
        self.nodes(head).next().unwrap_or_else(|| panic!("no block `{head}` in `{}`", self.head))
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|item| match item {
            Item::Line(line) => Some(line.as_str()),
            Item::Node(_) => None,
        })
    }

    /// The value of `key: value`, or of a block `key [ (value)`.
    pub fn get(&self, key: &str) -> &str {
        let prefix = format!("{key}:");
        for item in &self.items {
            match item {
                Item::Line(line) => {
                    if let Some(value) = line.strip_prefix(&prefix) {
                        return value.strip_prefix(' ').unwrap_or(value);
                    }
                }
                Item::Node(node) => {
                    if let Some(value) = node.head.strip_prefix(&format!("{key} [ ")) {
                        return value;
                    }
                }
            }
        }
        panic!("no `{key}` in `{}`", self.head)
    }

    pub fn num(&self, key: &str) -> u64 {
        number(self.get(key))
    }
}
