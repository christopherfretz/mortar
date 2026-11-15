//! L1 (`docs/VALIDATION.md`): the reader agrees with GNU `readelf` and with
//! `llvm-readobj` on real objects, `spec/elf.md` §4.3. Each file of the
//! corpus is read by the reader and printed by both tools; every field the
//! tools print is compared with what the reader gives for it. The relocation
//! type names are compared with the tools' over every number (§2.3).
//!
//! Nothing binary is checked in: the corpus is the system's crt objects,
//! sampled members of its `libc.a` and `libstdc++.a`, objects compiled here
//! by GCC and Clang for the three targets, objects with 70 000 sections
//! assembled here, and the test builder's own objects.

mod support;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, SHF_MERGE, SHN_X86_64_LCOMMON, SHT_GNU_VERDEF, SHT_GNU_VERNEED,
    SHT_GNU_VERSYM, SHT_GROUP, SHT_NOTE, SHT_REL, SHT_RELA, SHT_SYMTAB, SHT_SYMTAB_SHNDX,
    STT_SECTION,
};
use mortar_elf::{Chdr, Class, Object, Rel, Rela, Shdr, Sym, SymbolSection};
use mortar_testutil::{DEFAULT_TIMEOUT, Outcome, Scratch, run_with, tool, tools_required};
use support::{
    Builder, TARGETS, enc_rel, enc_rela, enc_sym, rel_size, rela_size, sample, sym_size,
};

/// Output of a tool on a file with 70 000 sections is tens of megabytes.
const LONG_TIMEOUT: Duration = Duration::from_mins(5);

// ---- what the reader says --------------------------------------------------

/// A section as the reader gives it.
#[derive(Debug, Clone)]
struct SectionView {
    name: Vec<u8>,
    shdr: Shdr,
    chdr: Option<Chdr>,
}

/// A symbol as the reader gives it.
#[derive(Debug, Clone)]
struct SymbolView {
    /// The name the tools print: the symbol's own, or for a section
    /// symbol without one the name of its section.
    shown: Vec<u8>,
    /// The name as `readelf` shows it: a section's name is cut at 256
    /// bytes there.
    readelf: String,
    sym: Sym,
    section: SymbolSection,
}

/// A relocation of either kind.
#[derive(Debug, Clone, Copy)]
struct RelocView {
    offset: u64,
    sym: u32,
    r_type: u32,
    addend: Option<i64>,
}

#[derive(Debug, Clone)]
struct GroupView {
    index: u32,
    flags: u32,
    /// The signature symbol's name.
    signature: Vec<u8>,
    /// The signature as `readelf` shows it: for a section symbol without a
    /// name, its section's name (§4.3). `llvm-readobj` shows the name.
    signature_shown: String,
    members: Vec<u32>,
}

#[derive(Debug, Clone)]
struct NoteView {
    owner: Vec<u8>,
    n_type: u32,
    desc: Vec<u8>,
    properties: Option<Vec<(u32, Vec<u8>)>>,
}

/// Everything compared, read through the reader's public interface.
#[derive(Debug)]
struct View {
    class: Class,
    e_machine: u16,
    header: mortar_elf::FileHeader,
    count: u32,
    names: u32,
    sections: Vec<SectionView>,
    symbols: Vec<SymbolView>,
    /// Per relocation section, in section order: its index and records.
    relocations: Vec<(u32, Vec<RelocView>)>,
    groups: Vec<GroupView>,
    /// Per note section, in section order: its index and notes.
    notes: Vec<(u32, Vec<NoteView>)>,
}

impl View {
    fn section_name(&self, index: u32) -> &[u8] {
        &self.sections[index as usize].name
    }
}

/// Reads `bytes` with the reader. A well-formed object reads without an
/// error (§4): any error fails the test, naming the file.
#[allow(clippy::too_many_lines)] // one kind of record after another
fn view(bytes: &[u8], what: &str) -> View {
    let fail = |error: mortar_elf::Error| -> ! { panic!("{what}: the reader says: {error}") };
    let object = Object::parse(bytes).unwrap_or_else(|e| fail(e));
    let table = object.section_table();
    let mut sections = Vec::new();
    for section in object.sections() {
        let name = section.name().unwrap_or_else(|e| fail(e)).to_vec();
        let chdr = section.compression().unwrap_or_else(|e| fail(e)).map(|c| c.header);
        // §3.10: what the reader says of a mergeable section is what its
        // header says, and the header is compared below.
        let merge = section.merge().unwrap_or_else(|e| fail(e));
        assert_eq!(merge.is_some(), section.flags() & SHF_MERGE != 0, "{what}: merge");
        if let Some(merge) = merge {
            assert_eq!(merge.entsize, section.header().sh_entsize, "{what}: merge entsize");
        }
        section.alignment().unwrap_or_else(|e| fail(e));
        section.data().unwrap_or_else(|e| fail(e));
        sections.push(SectionView { name, shdr: *section.header(), chdr });
    }
    let mut symbols = Vec::new();
    let symtab = object.symbols().unwrap_or_else(|e| fail(e));
    if let Some(table) = &symtab {
        for (index, sym) in table.iter() {
            let section = table.section_of(index, &sym).unwrap_or_else(|e| fail(e));
            let mut shown = table.name(&sym).unwrap_or_else(|e| fail(e)).to_vec();
            let mut readelf = text(&shown);
            if shown.is_empty()
                && sym.sym_type() == STT_SECTION
                && let SymbolSection::Section(at) = section
            {
                shown.clone_from(&sections[at.0 as usize].name);
                readelf = readelf_section_name(&shown);
            }
            symbols.push(SymbolView { shown, readelf, sym, section });
        }
    }
    let mut relocations = Vec::new();
    let mut groups = Vec::new();
    let mut notes = Vec::new();
    for section in object.sections() {
        let index = section.index().0;
        match section.sh_type() {
            SHT_RELA => {
                let rela = section.rela().unwrap_or_else(|e| fail(e));
                let records = rela
                    .iter()
                    .map(|r: Rela| RelocView {
                        offset: r.r_offset,
                        sym: r.r_sym,
                        r_type: r.r_type,
                        addend: Some(r.r_addend),
                    })
                    .collect();
                relocations.push((index, records));
            }
            SHT_REL => {
                let rel = section.rel().unwrap_or_else(|e| fail(e));
                let records = rel
                    .iter()
                    .map(|r: Rel| RelocView {
                        offset: r.r_offset,
                        sym: r.r_sym,
                        r_type: r.r_type,
                        addend: None,
                    })
                    .collect();
                relocations.push((index, records));
            }
            SHT_GROUP => {
                let group = section.group().unwrap_or_else(|e| fail(e));
                // The signature is a symbol of the object's one symbol
                // table, shown as the tools show symbols.
                let symtab = symtab.as_ref().map(mortar_elf::SymbolTable::section);
                assert_eq!(symtab, Some(group.symtab()), "{what}: group {index}");
                let symbol: &SymbolView = symbols
                    .get(group.signature().0 as usize)
                    .unwrap_or_else(|| panic!("{what}: the signature of group {index}"));
                let signature = table
                    .symbol_table(group.symtab(), None)
                    .and_then(|t| t.name(&symbol.sym).map(<[u8]>::to_vec))
                    .unwrap_or_else(|e| fail(e));
                let signature_shown = symbol.readelf.clone();
                let members =
                    group.members().map(|m| m.unwrap_or_else(|e| fail(e)).0).collect::<Vec<_>>();
                groups.push(GroupView {
                    index,
                    flags: group.flags(),
                    signature,
                    signature_shown,
                    members,
                });
            }
            SHT_NOTE => {
                let mut list = Vec::new();
                for note in section.notes().unwrap_or_else(|e| fail(e)) {
                    let note = note.unwrap_or_else(|e| fail(e));
                    let properties = note.gnu_properties().map(|properties| {
                        properties
                            .map(|p| p.unwrap_or_else(|e| fail(e)))
                            .map(|p| (p.pr_type, p.data.to_vec()))
                            .collect()
                    });
                    list.push(NoteView {
                        owner: note.name().to_vec(),
                        n_type: note.n_type,
                        desc: note.desc().to_vec(),
                        properties,
                    });
                }
                notes.push((index, list));
            }
            _ => {}
        }
    }
    View {
        class: object.class(),
        e_machine: object.machine().e_machine(),
        header: *object.header(),
        count: object.section_count(),
        names: table.names().section(),
        sections,
        symbols,
        relocations,
        groups,
        notes,
    }
}

// ---- tool output: helpers ---------------------------------------------------

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A number as the tools print it: `0x` and hexadecimal, or decimal.
fn number(s: &str) -> u64 {
    let s = s.trim();
    let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => s.parse(),
    };
    parsed.unwrap_or_else(|_| panic!("not a number: `{s}`"))
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim(), 16).unwrap_or_else(|_| panic!("not hexadecimal: `{s}`"))
}

/// The number in the last parentheses of `s`: `SHT_RELA (0x4)` is 4.
fn in_parens(s: &str) -> u64 {
    let open = s.rfind('(').unwrap_or_else(|| panic!("no `(` in `{s}`"));
    let close = s[open..].find(')').map_or_else(|| panic!("`{s}`"), |at| open + at);
    number(&s[open + 1..close])
}

/// `name (n)`: the text before the last ` (` and the number in it.
fn name_and_number(s: &str) -> (&str, u64) {
    match s.rsplit_once(" (") {
        Some((name, rest)) => (name, number(rest.trim_end_matches(')'))),
        None => panic!("not `name (number)`: `{s}`"),
    }
}

fn run_tool(program: &Path, args: &[&str], file: &Path, timeout: Duration) -> String {
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

/// What was compared, per tool.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    files: u64,
    sections: u64,
    symbols: u64,
    relocations: u64,
    groups: u64,
    members: u64,
    notes: u64,
    properties: u64,
    compressed: u64,
    mergeable: u64,
}

impl Totals {
    fn add(&mut self, other: Totals) {
        self.files += other.files;
        self.sections += other.sections;
        self.symbols += other.symbols;
        self.relocations += other.relocations;
        self.groups += other.groups;
        self.members += other.members;
        self.notes += other.notes;
        self.properties += other.properties;
        self.compressed += other.compressed;
        self.mergeable += other.mergeable;
    }
}

// ---- names the tools print ---------------------------------------------------

/// A section name as `readelf` lists sections: binutils 2.44 prints at
/// most 256 bytes of it, without a mark (§4.3).
fn readelf_section_name(name: &[u8]) -> String {
    text(&name[..name.len().min(256)])
}

/// The spelling of a section type in `readelf`'s section list.
fn readelf_section_type(view: &View, sh_type: u32) -> String {
    let special = match sh_type {
        SHT_SYMTAB_SHNDX => Some("SYMTAB SECTION INDICES"),
        SHT_GNU_VERDEF => Some("VERDEF"),
        SHT_GNU_VERNEED => Some("VERNEED"),
        SHT_GNU_VERSYM => Some("VERSYM"),
        _ => None,
    };
    if let Some(special) = special {
        return special.to_owned();
    }
    let name = mortar_elf::consts::sht_name(view.e_machine, sh_type)
        .unwrap_or_else(|| panic!("section type {sh_type:#x} has no name in the reader"));
    name.strip_prefix("SHT_").unwrap_or(name).to_owned()
}

fn readelf_symbol_type(sym_type: u8) -> &'static str {
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

fn readelf_binding(binding: u8) -> &'static str {
    match binding {
        0 => "LOCAL",
        1 => "GLOBAL",
        2 => "WEAK",
        10 => "UNIQUE",
        other => panic!("binding {other}: add readelf's spelling"),
    }
}

fn readelf_visibility(sym: &Sym, e_machine: u16) -> String {
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

fn readelf_section_index(section: SymbolSection, e_machine: u16) -> String {
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

/// How `llvm-readobj` names where a symbol is: its label and number.
fn llvm_section_index(view: &View, section: SymbolSection) -> (String, u64) {
    match section {
        SymbolSection::Undefined => ("Undefined".to_owned(), 0),
        SymbolSection::Absolute => ("Absolute".to_owned(), 0xfff1),
        SymbolSection::Common => ("Common".to_owned(), 0xfff2),
        SymbolSection::Section(index) => (text(view.section_name(index.0)), u64::from(index.0)),
        SymbolSection::Reserved(SHN_X86_64_LCOMMON) => ("Processor Specific".to_owned(), 0xff02),
        SymbolSection::Reserved(other) => panic!("st_shndx {other:#x}: add LLVM's spelling"),
    }
}

/// The type of a note as each tool prints it: (`readelf`, `llvm-readobj`).
fn note_type(owner: &[u8], n_type: u32) -> (&'static str, &'static str) {
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
fn bits(word: u32, names: &[&str]) -> String {
    let mut out = Vec::new();
    for bit in 0..32 {
        if word & (1 << bit) != 0 {
            let name = names.get(bit).unwrap_or_else(|| panic!("bit {bit} of {word:#x}"));
            out.push(*name);
        }
    }
    out.join(", ")
}

const X86_ISA: [&str; 4] = ["x86-64-baseline", "x86-64-v2", "x86-64-v3", "x86-64-v4"];
const X86_FEATURE_2: [&str; 12] = [
    "x86", "x87", "MMX", "XMM", "YMM", "ZMM", "FXSR", "XSAVE", "XSAVEOPT", "XSAVEC", "TMM", "MASK",
];

/// A program property as each tool prints it: (`readelf`, `llvm-readobj`).
fn property_text(e_machine: u16, class: Class, pr_type: u32, data: &[u8]) -> (String, String) {
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

// ---- readelf ------------------------------------------------------------------

/// Compares `readelf -W -h -t -g -r -s -n` with the reader.
#[allow(clippy::too_many_lines)] // one part of the output after another
fn check_readelf(readelf: &Path, file: &Path, view: &View, timeout: Duration) -> Totals {
    let at = file.display();
    let output = run_tool(readelf, &["-W", "-h", "-t", "-g", "-r", "-s", "-n"], file, timeout);
    let mut totals = Totals { files: 1, ..Totals::default() };
    let mut lines = output.lines().peekable();

    // The file header: `  Key:   value`.
    while lines.next_if(|line| *line != "ELF Header:").is_some() {}
    assert_eq!(lines.next(), Some("ELF Header:"), "{at}: no file header");
    let mut header = BTreeMap::new();
    while let Some(line) = lines.next_if(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').unwrap_or_else(|| panic!("{at}: `{line}`"));
        // `Version` appears twice: e_ident's, then e_version.
        let key = key.trim().to_owned();
        let key = if header.contains_key(&key) { format!("{key} 2") } else { key };
        header.insert(key, value.trim().to_owned());
    }
    let h = &view.header;
    let escaped = |field: u16, value: u32| {
        if u32::from(field) == value { value.to_string() } else { format!("{field} ({value})") }
    };
    let expected = [
        ("Class", h.class.name().to_owned()),
        ("Data", "2's complement, little endian".to_owned()),
        ("Version", format!("{} (current)", h.ei_version)),
        (
            "OS/ABI",
            match h.ei_osabi {
                0 => "UNIX - System V",
                3 => "UNIX - GNU",
                other => panic!("{at}: EI_OSABI {other}: add readelf's spelling"),
            }
            .to_owned(),
        ),
        ("ABI Version", h.ei_abiversion.to_string()),
        ("Type", "REL (Relocatable file)".to_owned()),
        (
            "Machine",
            match view.e_machine {
                EM_AARCH64 => "AArch64",
                EM_X86_64 => "Advanced Micro Devices X86-64",
                _ => "Intel 80386",
            }
            .to_owned(),
        ),
        ("Version 2", format!("{:#x}", h.e_version)),
        ("Entry point address", format!("{:#x}", h.e_entry)),
        ("Start of program headers", format!("{} (bytes into file)", h.e_phoff)),
        ("Start of section headers", format!("{} (bytes into file)", h.e_shoff)),
        ("Flags", format!("{:#x}", h.e_flags)),
        ("Size of this header", format!("{} (bytes)", h.e_ehsize)),
        ("Size of program headers", format!("{} (bytes)", h.e_phentsize)),
        ("Number of program headers", h.e_phnum.to_string()),
        ("Size of section headers", format!("{} (bytes)", h.e_shentsize)),
        ("Number of section headers", escaped(h.e_shnum, view.count)),
        ("Section header string table index", escaped(h.e_shstrndx, view.names)),
    ];
    for (key, value) in expected {
        assert_eq!(header.get(key), Some(&value), "{at}: file header `{key}`");
    }

    // The sections, three or four lines each: `[n] name`, the numbers, the
    // flags, and the compression header if there is one.
    while lines.next_if(|line| *line != "Section Headers:").is_some() {}
    assert_eq!(lines.next(), Some("Section Headers:"), "{at}: no section headers");
    for _ in 0..3 {
        lines.next();
    }
    for (index, section) in view.sections.iter().enumerate() {
        let s = &section.shdr;
        let first = lines.next().unwrap_or_else(|| panic!("{at}: section {index} missing"));
        let (shown_index, name) = first
            .trim_start()
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("] "))
            .unwrap_or_else(|| panic!("{at}: `{first}`"));
        assert_eq!(shown_index.trim(), index.to_string(), "{at}: `{first}`");
        assert_eq!(name, readelf_section_name(&section.name), "{at}: name of section {index}");
        let numbers = lines.next().unwrap_or_default();
        let fields: Vec<&str> = numbers.split_whitespace().collect();
        let (kind, fields) = fields.split_at(fields.len().saturating_sub(7));
        assert_eq!(kind.join(" "), readelf_section_type(view, s.sh_type), "{at}: type of {index}");
        let fields: Vec<u64> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| if i < 4 { hex(f) } else { number(f) })
            .collect();
        let shown = [
            s.sh_addr,
            s.sh_offset,
            s.sh_size,
            s.sh_entsize,
            u64::from(s.sh_link),
            u64::from(s.sh_info),
            s.sh_addralign,
        ];
        assert_eq!(fields, shown, "{at}: section {index}: addr off size es lk inf al");
        let flags = lines.next().unwrap_or_default().trim();
        let flags = flags.strip_prefix('[').and_then(|f| f.split_once(']')).map(|(f, _)| hex(f));
        assert_eq!(flags, Some(s.sh_flags), "{at}: flags of section {index}");
        if let Some(chdr) = section.chdr {
            let line = lines.next().unwrap_or_default();
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            let kind = match chdr.ch_type {
                1 => "ZLIB",
                2 => "ZSTD",
                other => panic!("{at}: ch_type {other}"),
            };
            assert_eq!(parts.len(), 3, "{at}: `{line}`");
            assert_eq!(parts[0], kind, "{at}: compression of section {index}");
            assert_eq!(hex(parts[1]), chdr.ch_size, "{at}: ch_size of section {index}");
            assert_eq!(number(parts[2]), chdr.ch_addralign, "{at}: ch_addralign of {index}");
            totals.compressed += 1;
        }
        if s.sh_flags & SHF_MERGE != 0 {
            totals.mergeable += 1;
        }
        totals.sections += 1;
    }
    assert!(lines.peek().is_none_or(|line| line.is_empty()), "{at}: more sections than read");

    // Groups: `COMDAT group section [ n] `.group' [signature] contains k
    // sections:`, a heading, `[ m] name` per member.
    let mut groups = view.groups.iter();
    loop {
        let Some(line) = lines.next() else { panic!("{at}: output ends before relocations") };
        if line.starts_with("There are no section groups") {
            assert!(view.groups.is_empty(), "{at}: readelf sees no groups");
            break;
        }
        if line.starts_with("Relocation section") || line.starts_with("There are no reloc") {
            assert!(groups.next().is_none(), "{at}: readelf shows fewer groups");
            // Put nothing back: handle the line below.
            check_readelf_relocations(Some(line), &mut lines, view, &at.to_string(), &mut totals);
            return check_readelf_rest(&mut lines, view, &at.to_string(), totals);
        }
        let Some(rest) = line
            .strip_prefix("COMDAT group section [")
            .or_else(|| line.strip_prefix("group section ["))
        else {
            continue;
        };
        let group = groups.next().unwrap_or_else(|| panic!("{at}: readelf shows more groups"));
        assert_eq!(line.starts_with("COMDAT"), group.flags & 1 != 0, "{at}: `{line}`");
        let (index, rest) = rest.split_once(']').unwrap_or_default();
        assert_eq!(number(index), u64::from(group.index), "{at}: `{line}`");
        let expected = format!(
            " `{}' [{}] contains {} sections:",
            readelf_section_name(view.section_name(group.index)),
            group.signature_shown,
            group.members.len()
        );
        assert_eq!(rest, expected, "{at}: group {}", group.index);
        lines.next(); // `[Index]    Name`
        for &member in &group.members {
            let line = lines.next().unwrap_or_default().trim();
            let (index, name) = line
                .strip_prefix('[')
                .and_then(|l| l.split_once(']'))
                .unwrap_or_else(|| panic!("{at}: `{line}`"));
            assert_eq!(number(index), u64::from(member), "{at}: member of {}", group.index);
            assert_eq!(
                name.trim(),
                readelf_section_name(view.section_name(member)),
                "{at}: `{line}`"
            );
            totals.members += 1;
        }
        totals.groups += 1;
    }
    check_readelf_relocations(None, &mut lines, view, &at.to_string(), &mut totals);
    check_readelf_rest(&mut lines, view, &at.to_string(), totals)
}

type Lines<'a> = std::iter::Peekable<std::str::Lines<'a>>;

/// The relocation sections: `Relocation section 'name' at offset 0x.. contains
/// n entries:`, a heading, one line per record.
fn check_readelf_relocations(
    mut pending: Option<&str>,
    lines: &mut Lines<'_>,
    view: &View,
    at: &str,
    totals: &mut Totals,
) {
    let mut sections = view.relocations.iter();
    loop {
        let line = match pending.take() {
            Some(line) => line,
            None => match lines.peek() {
                Some(line) if line.starts_with("Symbol table") => break,
                Some(line) if line.starts_with("Displaying notes") => break,
                Some(_) => lines.next().unwrap_or_default(),
                None => break,
            },
        };
        if line.starts_with("There are no relocations") {
            assert!(view.relocations.is_empty(), "{at}: readelf sees no relocations");
            continue;
        }
        let Some(rest) = line.strip_prefix("Relocation section '") else { continue };
        let (index, records) =
            sections.next().unwrap_or_else(|| panic!("{at}: readelf shows more relocations"));
        let section = &view.sections[*index as usize];
        let entries = if records.len() == 1 { "entry" } else { "entries" };
        let expected = format!(
            "{}' at offset {:#x} contains {} {entries}:",
            readelf_section_name(&section.name),
            section.shdr.sh_offset,
            records.len()
        );
        assert_eq!(rest, expected, "{at}: relocation section {index}");
        lines.next(); // the heading
        for (position, record) in records.iter().enumerate() {
            let line = lines.next().unwrap_or_default();
            let what = || format!("{at}: relocation {position} of section {index}: `{line}`");
            let fields: Vec<&str> = line.split_whitespace().collect();
            assert!(fields.len() >= 3, "{}", what());
            assert_eq!(hex(fields[0]), record.offset, "{}", what());
            let info = hex(fields[1]);
            let (sym, r_type) = match view.class {
                Class::Elf32 => ((info >> 8) as u32, (info & 0xff) as u32),
                Class::Elf64 => ((info >> 32) as u32, info as u32),
            };
            assert_eq!((sym, r_type), (record.sym, record.r_type), "{}", what());
            assert!(type_name_agrees(view, record.r_type, fields[2]), "{}", what());
            if let Some(addend) = record.addend {
                let n = fields.len();
                let shown = match fields[n - 2] {
                    "+" => hex(fields[n - 1]).cast_signed(),
                    "-" => hex(fields[n - 1]).cast_signed().wrapping_neg(),
                    _ => panic!("{}: no addend", what()),
                };
                assert_eq!(shown, addend, "{}", what());
            }
            totals.relocations += 1;
        }
    }
    assert!(sections.next().is_none(), "{at}: readelf shows fewer relocation sections");
}

/// The symbol table, then the notes.
fn check_readelf_rest(lines: &mut Lines<'_>, view: &View, at: &str, mut totals: Totals) -> Totals {
    // Symbols: `n: value size type bind vis[ [other]] ndx name`.
    while lines.next_if(|line| line.is_empty()).is_some() {}
    if lines.peek().is_some_and(|line| line.starts_with("Symbol table")) {
        let line = lines.next().unwrap_or_default();
        let expected = format!(
            "Symbol table '.symtab' contains {} entr{}:",
            view.symbols.len(),
            if view.symbols.len() == 1 { "y" } else { "ies" }
        );
        assert_eq!(line, expected, "{at}");
        lines.next(); // the heading
        for (index, symbol) in view.symbols.iter().enumerate() {
            let line = lines.next().unwrap_or_default();
            let what = || format!("{at}: symbol {index}: `{line}`");
            let (shown_index, rest) = line.trim_start().split_once(": ").unwrap_or_default();
            assert_eq!(shown_index, index.to_string(), "{}", what());
            let mut fields = rest.split(' ').filter(|f| !f.is_empty());
            let mut next = || fields.next().unwrap_or_default();
            let sym = &symbol.sym;
            assert_eq!(hex(next()), sym.st_value, "{}", what());
            assert_eq!(number_of(next()), sym.st_size, "{}", what());
            assert_eq!(next(), readelf_symbol_type(sym.sym_type()), "{}", what());
            assert_eq!(next(), readelf_binding(sym.binding()), "{}", what());
            let visibility = readelf_visibility(sym, view.e_machine);
            let mut shown = next().to_owned();
            for _ in 0..visibility.matches(' ').count() {
                shown.push(' ');
                shown.push_str(next());
            }
            assert_eq!(shown, visibility, "{}", what());
            let ndx = next();
            assert_eq!(ndx, readelf_section_index(symbol.section, view.e_machine), "{}", what());
            // The name is the rest of the line after one space.
            let end = ndx.as_ptr() as usize - line.as_ptr() as usize + ndx.len();
            let name = line.get(end + 1..).unwrap_or("");
            assert_eq!(name, symbol.readelf, "{}", what());
            totals.symbols += 1;
        }
    } else {
        assert!(view.symbols.is_empty(), "{at}: readelf shows no symbol table");
    }

    // Notes: `Displaying notes found in: name`, a heading, then per note a
    // line `owner size<TAB>type<TAB>description` and continuation lines.
    let mut sections = view.notes.iter();
    while let Some(line) = lines.next() {
        let Some(name) = line.strip_prefix("Displaying notes found in: ") else {
            assert!(line.is_empty(), "{at}: unexpected `{line}`");
            continue;
        };
        let (index, notes) =
            sections.next().unwrap_or_else(|| panic!("{at}: readelf shows more notes"));
        assert_eq!(
            name,
            readelf_section_name(view.section_name(*index)),
            "{at}: note section {index}"
        );
        lines.next(); // the heading
        for note in notes {
            let line = lines.next().unwrap_or_default();
            let what = || format!("{at}: note in section {index}: `{line}`");
            let parts: Vec<&str> = line.split('\t').collect();
            assert!(parts.len() >= 2, "{}", what());
            let mut head = parts[0].split_whitespace();
            assert_eq!(head.next(), Some(text(&note.owner).as_str()), "{}", what());
            assert_eq!(head.next().map(number), Some(note.desc.len() as u64), "{}", what());
            let (kind, _) = note_type(&note.owner, note.n_type);
            assert_eq!(parts[1].trim(), kind, "{}", what());
            let description = parts.get(2).map_or("", |d| d.trim());
            if let Some(properties) = &note.properties {
                let shown: Vec<String> = properties
                    .iter()
                    .map(|(t, d)| property_text(view.e_machine, view.class, *t, d).0)
                    .collect();
                let expected = format!("Properties: {}", shown.join(", "));
                assert_eq!(description, expected, "{}", what());
                totals.properties += properties.len() as u64;
            } else if note.owner == b"FDO" {
                // The descriptor is a NUL-terminated string, padded.
                let json = note.desc.split(|&b| b == 0).next().unwrap_or_default();
                assert_eq!(description, format!("Packaging Metadata: {}", text(json)));
            } else if (note.owner.as_slice(), note.n_type) == (b"GNU", 1) {
                assert_eq!(description, abi_tag(&note.desc).0, "{}", what());
            }
            // Continuation lines (stapsdt probes) are not compared.
            while lines.next_if(|l| l.starts_with("    ")).is_some() {}
            totals.notes += 1;
        }
    }
    assert!(sections.next().is_none(), "{at}: readelf shows fewer note sections");
    totals
}

/// A size column: decimal, or hexadecimal with `0x` when large.
fn number_of(s: &str) -> u64 {
    number(s)
}

/// Whether the reader's name of relocation type `r_type` is `shown`, the
/// tool's, or the reader leaves the type unnamed on purpose (§2.3).
fn type_name_agrees(view: &View, r_type: u32, shown: &str) -> bool {
    match mortar_elf::reloc::name(view.e_machine, r_type) {
        Some(ours) => ours == shown,
        None => unnamed_on_purpose(view.e_machine, r_type),
    }
}

/// An ABI tag note as (`readelf`, `llvm-readobj` OS, ABI).
fn abi_tag(desc: &[u8]) -> (String, String, String) {
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

// ---- llvm-readobj ---------------------------------------------------------------

/// A block of `llvm-readobj`'s output: the line that opens it and its
/// lines and blocks in order.
#[derive(Debug, Default)]
struct Node {
    head: String,
    items: Vec<Item>,
}

#[derive(Debug)]
enum Item {
    Line(String),
    Node(Node),
}

impl Node {
    /// Parses the indented `Key: value` / `Key {` ... `}` output.
    fn parse(output: &str) -> Node {
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

    fn nodes(&self, head: &str) -> impl Iterator<Item = &Node> {
        let spaced = format!("{head} ");
        let head = head.to_owned();
        self.items.iter().filter_map(move |item| match item {
            Item::Node(node) if node.head == head || node.head.starts_with(&spaced) => Some(node),
            _ => None,
        })
    }

    fn node(&self, head: &str) -> &Node {
        self.nodes(head).next().unwrap_or_else(|| panic!("no block `{head}` in `{}`", self.head))
    }

    fn lines(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|item| match item {
            Item::Line(line) => Some(line.as_str()),
            Item::Node(_) => None,
        })
    }

    /// The value of `key: value`, or of a block `key [ (value)`.
    fn get(&self, key: &str) -> &str {
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

    fn num(&self, key: &str) -> u64 {
        number(self.get(key))
    }
}

/// Compares `llvm-readobj` with the reader.
#[allow(clippy::too_many_lines)] // one part of the output after another
fn check_llvm(readobj: &Path, file: &Path, view: &View, timeout: Duration) -> Totals {
    let at = file.display();
    let args = [
        "--file-headers",
        "--sections",
        "--symbols",
        "--relocations",
        "--expand-relocs",
        "--section-groups",
        "--notes",
    ];
    let output = run_tool(readobj, &args, file, timeout);
    let root = Node::parse(&output);
    let mut totals = Totals { files: 1, ..Totals::default() };

    let h = &view.header;
    let header = root.node("ElfHeader");
    let ident = header.node("Ident");
    let class = match h.class {
        Class::Elf32 => 1,
        Class::Elf64 => 2,
    };
    assert_eq!(in_parens(ident.get("Class")), class, "{at}: class");
    assert_eq!(in_parens(ident.get("DataEncoding")), 1, "{at}: encoding");
    assert_eq!(ident.num("FileVersion"), u64::from(h.ei_version), "{at}: EI_VERSION");
    assert_eq!(in_parens(ident.get("OS/ABI")), u64::from(h.ei_osabi), "{at}: EI_OSABI");
    assert_eq!(ident.num("ABIVersion"), u64::from(h.ei_abiversion), "{at}: EI_ABIVERSION");
    assert_eq!(in_parens(header.get("Type")), u64::from(h.e_type), "{at}: e_type");
    assert_eq!(in_parens(header.get("Machine")), u64::from(h.e_machine), "{at}: e_machine");
    let fields = [
        ("Version", u64::from(h.e_version)),
        ("Entry", h.e_entry),
        ("ProgramHeaderOffset", h.e_phoff),
        ("SectionHeaderOffset", h.e_shoff),
        ("HeaderSize", u64::from(h.e_ehsize)),
        ("ProgramHeaderEntrySize", u64::from(h.e_phentsize)),
        ("ProgramHeaderCount", u64::from(h.e_phnum)),
        ("SectionHeaderEntrySize", u64::from(h.e_shentsize)),
    ];
    for (key, value) in fields {
        assert_eq!(header.num(key), value, "{at}: {key}");
    }
    assert_eq!(in_parens(header.get("Flags")), u64::from(h.e_flags), "{at}: e_flags");
    let escaped = |field: u16, value: u32| {
        if u32::from(field) == value { value.to_string() } else { format!("{field} ({value})") }
    };
    assert_eq!(header.get("SectionHeaderCount"), escaped(h.e_shnum, view.count), "{at}: count");
    assert_eq!(
        header.get("StringTableSectionIndex"),
        escaped(h.e_shstrndx, view.names),
        "{at}: e_shstrndx"
    );

    // Sections.
    let shown: Vec<&Node> = root.node("Sections").nodes("Section").collect();
    assert_eq!(shown.len(), view.sections.len(), "{at}: section count");
    for (index, (node, section)) in shown.iter().zip(&view.sections).enumerate() {
        let s = &section.shdr;
        let what = format!("{at}: section {index}");
        assert_eq!(node.num("Index"), index as u64, "{what}");
        let (name, sh_name) = name_and_number(node.get("Name"));
        assert_eq!((name, sh_name), (text(&section.name).as_str(), u64::from(s.sh_name)), "{what}");
        let (kind, sh_type) = name_and_number(node.get("Type"));
        assert_eq!(sh_type, u64::from(s.sh_type), "{what}: type");
        if kind.starts_with("SHT_") {
            let ours = mortar_elf::consts::sht_name(view.e_machine, s.sh_type);
            assert!(
                ours.is_some_and(|ours| ours.eq_ignore_ascii_case(kind)),
                "{what}: LLVM names the type {kind}, the reader {ours:?}"
            );
        }
        assert_eq!(in_parens(node.get("Flags")), s.sh_flags, "{what}: flags");
        let fields = [
            ("Address", s.sh_addr),
            ("Offset", s.sh_offset),
            ("Size", s.sh_size),
            ("Link", u64::from(s.sh_link)),
            ("Info", u64::from(s.sh_info)),
            ("AddressAlignment", s.sh_addralign),
            ("EntrySize", s.sh_entsize),
        ];
        for (key, value) in fields {
            assert_eq!(node.num(key), value, "{what}: {key}");
        }
        totals.sections += 1;
        if s.sh_flags & SHF_MERGE != 0 {
            totals.mergeable += 1;
        }
    }

    // Symbols.
    let shown: Vec<&Node> = root.node("Symbols").nodes("Symbol").collect();
    assert_eq!(shown.len(), view.symbols.len(), "{at}: symbol count");
    for (index, (node, symbol)) in shown.iter().zip(&view.symbols).enumerate() {
        let sym = &symbol.sym;
        let what = format!("{at}: symbol {index}");
        let (name, st_name) = name_and_number(node.get("Name"));
        assert_eq!(name, text(&symbol.shown), "{what}: name");
        assert_eq!(st_name, u64::from(sym.st_name), "{what}: st_name");
        assert_eq!(node.num("Value"), sym.st_value, "{what}: value");
        assert_eq!(node.num("Size"), sym.st_size, "{what}: size");
        assert_eq!(in_parens(node.get("Binding")), u64::from(sym.binding()), "{what}: binding");
        assert_eq!(in_parens(node.get("Type")), u64::from(sym.sym_type()), "{what}: type");
        let other = node.get("Other");
        let other = if other.starts_with('(') { in_parens(other) } else { number(other) };
        assert_eq!(other, u64::from(sym.st_other), "{what}: st_other");
        let (label, value) = name_and_number(node.get("Section"));
        assert_eq!(
            (label.to_owned(), value),
            llvm_section_index(view, symbol.section),
            "{what}: section"
        );
        totals.symbols += 1;
    }

    // Relocations, expanded: one block per record.
    let shown: Vec<&Node> = root.node("Relocations").nodes("Section").collect();
    assert_eq!(shown.len(), view.relocations.len(), "{at}: relocation sections");
    for (node, (index, records)) in shown.iter().zip(&view.relocations) {
        let head = format!("Section ({index}) {}", text(view.section_name(*index)));
        assert_eq!(node.head, head, "{at}: relocation section");
        let entries: Vec<&Node> = node.nodes("Relocation").collect();
        assert_eq!(entries.len(), records.len(), "{at}: relocations of section {index}");
        for (position, (entry, record)) in entries.iter().zip(records).enumerate() {
            let what = format!("{at}: relocation {position} of section {index}");
            assert_eq!(entry.num("Offset"), record.offset, "{what}: offset");
            let (kind, r_type) = name_and_number(entry.get("Type"));
            assert_eq!(r_type, u64::from(record.r_type), "{what}: type");
            assert!(type_name_agrees(view, record.r_type, kind), "{what}: type name {kind}");
            let (_, sym) = name_and_number(entry.get("Symbol"));
            assert_eq!(sym, u64::from(record.sym), "{what}: symbol");
            match record.addend {
                Some(addend) => {
                    let shown = entry.num("Addend");
                    let shown = match view.class {
                        Class::Elf32 => i64::from((shown as u32).cast_signed()),
                        Class::Elf64 => shown.cast_signed(),
                    };
                    assert_eq!(shown, addend, "{what}: addend");
                }
                None => assert!(
                    entry.lines().all(|line| !line.starts_with("Addend")),
                    "{what}: LLVM shows an addend for REL"
                ),
            }
            totals.relocations += 1;
        }
    }

    // Groups.
    let shown: Vec<&Node> = root.node("Groups").nodes("Group").collect();
    assert_eq!(shown.len(), view.groups.len(), "{at}: groups");
    for (node, group) in shown.iter().zip(&view.groups) {
        let what = format!("{at}: group {}", group.index);
        let section = &view.sections[group.index as usize];
        assert_eq!(node.num("Index"), u64::from(group.index), "{what}");
        assert_eq!(name_and_number(node.get("Name")).0, text(&section.name), "{what}");
        assert_eq!(node.num("Link"), u64::from(section.shdr.sh_link), "{what}: link");
        assert_eq!(node.num("Info"), u64::from(section.shdr.sh_info), "{what}: info");
        assert_eq!(in_parens(node.get("Type")), u64::from(group.flags), "{what}: flags");
        assert_eq!(node.get("Signature"), text(&group.signature), "{what}: signature");
        let members: Vec<(String, u64)> = node
            .node("Section(s) in group")
            .lines()
            .map(|line| {
                let (name, index) = name_and_number(line);
                (name.to_owned(), index)
            })
            .collect();
        let expected: Vec<(String, u64)> =
            group.members.iter().map(|&m| (text(view.section_name(m)), u64::from(m))).collect();
        assert_eq!(members, expected, "{what}: members");
        totals.groups += 1;
        totals.members += members.len() as u64;
    }

    // Notes.
    let shown: Vec<&Node> = root.node("NoteSections").nodes("NoteSection").collect();
    assert_eq!(shown.len(), view.notes.len(), "{at}: note sections");
    for (node, (index, notes)) in shown.iter().zip(&view.notes) {
        let what = format!("{at}: note section {index}");
        let section = &view.sections[*index as usize];
        assert_eq!(node.get("Name"), text(&section.name), "{what}");
        assert_eq!(node.num("Offset"), section.shdr.sh_offset, "{what}: offset");
        assert_eq!(node.num("Size"), section.shdr.sh_size, "{what}: size");
        let entries: Vec<&Node> = node.node("Notes").nodes("").collect();
        assert_eq!(entries.len(), notes.len(), "{what}: notes");
        for (entry, note) in entries.iter().zip(notes) {
            assert_eq!(entry.get("Owner"), text(&note.owner), "{what}: owner");
            assert_eq!(entry.num("Data size"), note.desc.len() as u64, "{what}: size");
            let (_, kind) = note_type(&note.owner, note.n_type);
            assert_eq!(entry.get("Type"), kind, "{what}: type");
            if let Some(properties) = &note.properties {
                let lines: Vec<&str> = entry.node("Property").lines().collect();
                let expected: Vec<String> = properties
                    .iter()
                    .map(|(t, d)| property_text(view.e_machine, view.class, *t, d).1)
                    .collect();
                assert_eq!(lines, expected, "{what}: properties");
                totals.properties += properties.len() as u64;
            } else if (note.owner.as_slice(), note.n_type) == (b"GNU", 1) {
                let (_, os, abi) = abi_tag(&note.desc);
                assert_eq!((entry.get("OS"), entry.get("ABI")), (os.as_str(), abi.as_str()));
            }
            totals.notes += 1;
        }
    }
    for section in &view.sections {
        if section.chdr.is_some() {
            totals.compressed += 1;
        }
    }
    totals
}

// ---- the corpus ---------------------------------------------------------------

/// The two tools, or `None` (the test skips) when one is missing.
struct Tools {
    readelf: PathBuf,
    readobj: PathBuf,
}

fn tools() -> Option<Tools> {
    Some(Tools { readelf: tool("readelf")?, readobj: tool("llvm-readobj")? })
}

/// What a test compared, by tool.
#[derive(Debug, Default)]
struct Tally {
    readelf: Totals,
    llvm: Totals,
}

impl Tally {
    fn check(&mut self, tools: &Tools, file: &Path, timeout: Duration) {
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let view = view(&bytes, &file.display().to_string());
        self.readelf.add(check_readelf(&tools.readelf, file, &view, timeout));
        self.llvm.add(check_llvm(&tools.readobj, file, &view, timeout));
    }

    fn report(&self, what: &str) {
        eprintln!("{what}:\n  readelf      {:?}\n  llvm-readobj {:?}", self.readelf, self.llvm);
    }
}

/// A target: the GCC driver prefix and Clang's `--target`.
#[derive(Debug, Clone, Copy)]
struct Target {
    name: &'static str,
    gcc_prefix: &'static str,
    triple: &'static str,
    e_machine: u16,
}

/// Each target's compilers are found by their prefixed names first, which
/// Debian gives the host's own compilers too (AArch64's on the development
/// machine, x86-64's on the hosted x86-64 lane); the cross compilers are
/// named by prefix only.
const TARGET_LIST: [Target; 3] = [
    Target {
        name: "aarch64",
        gcc_prefix: "aarch64-linux-gnu-",
        triple: "aarch64-linux-gnu",
        e_machine: EM_AARCH64,
    },
    Target {
        name: "x86-64",
        gcc_prefix: "x86_64-linux-gnu-",
        triple: "x86_64-linux-gnu",
        e_machine: EM_X86_64,
    },
    Target {
        name: "i386",
        gcc_prefix: "i686-linux-gnu-",
        triple: "i686-linux-gnu",
        e_machine: EM_386,
    },
];

/// The target's GCC driver for `language` (`gcc`, `g++`, `gfortran`): the
/// prefixed name, or the plain one when it is the native compiler for the
/// target.
fn find_gcc(target: &Target, language: &str) -> Option<PathBuf> {
    let prefixed = format!("{}{language}", target.gcc_prefix);
    if let Some(found) = mortar_testutil::find_tool(&prefixed) {
        return Some(found);
    }
    let native = mortar_testutil::find_tool(language)?;
    let machine = run_with(Command::new(&native).arg("-dumpmachine"), &[], DEFAULT_TIMEOUT);
    machine.stdout_text().trim().starts_with(target.triple).then_some(native)
}

/// [`find_gcc`], for a driver the test needs: missing, the test skips, or
/// fails under `MORTAR_REQUIRE_TOOLS=1`.
fn gcc(target: &Target, language: &str) -> Option<PathBuf> {
    let found = find_gcc(target, language);
    if found.is_none() {
        assert!(!tools_required(), "no {language} for {} (MORTAR_REQUIRE_TOOLS=1)", target.name);
        eprintln!("note: no {language} for {}; skipping it", target.name);
    }
    found
}

/// `<gcc> -print-file-name=<name>`, when that is a file.
fn installed(gcc: &Path, name: &str) -> Option<PathBuf> {
    let outcome =
        run_with(Command::new(gcc).arg(format!("-print-file-name={name}")), &[], DEFAULT_TIMEOUT);
    let path = PathBuf::from(outcome.stdout_text().trim());
    path.is_absolute().then_some(path).filter(|p| p.is_file())
}

const CRT_OBJECTS: [&str; 12] = [
    "crt1.o",
    "crti.o",
    "crtn.o",
    "Scrt1.o",
    "rcrt1.o",
    "gcrt1.o",
    "crtbegin.o",
    "crtend.o",
    "crtbeginS.o",
    "crtendS.o",
    "crtbeginT.o",
    "crtfastmath.o",
];

fn system_crt_objects(target: &Target) {
    let Some(tools) = tools() else { return };
    let Some(cc) = gcc(target, "gcc") else { return };
    let mut tally = Tally::default();
    for name in CRT_OBJECTS {
        let path = installed(&cc, name);
        assert!(path.is_some() || !tools_required(), "{}: no {name}", target.name);
        if let Some(path) = path {
            tally.check(&tools, &path, DEFAULT_TIMEOUT);
        }
    }
    tally.report(&format!("crt objects, {}", target.name));
}

/// §4.3: the system's crt objects of each target.
#[test]
fn s4_3_crt_objects_aarch64() {
    system_crt_objects(&TARGET_LIST[0]);
}

#[test]
fn s4_3_crt_objects_x86_64() {
    system_crt_objects(&TARGET_LIST[1]);
}

#[test]
fn s4_3_crt_objects_i386() {
    system_crt_objects(&TARGET_LIST[2]);
}

/// The ordinary members of a GNU archive: (name, bytes). A minimal walk
/// written from the format (ar(5), the gABI "Archive File"): enough to
/// take members out of the system's archives without the archive reader.
fn archive_members(bytes: &[u8]) -> Vec<(String, &[u8])> {
    assert!(bytes.starts_with(b"!<arch>\n"), "a regular archive");
    let mut at = 8;
    let mut long_names: &[u8] = &[];
    let mut members = Vec::new();
    while at + 60 <= bytes.len() {
        let header = &bytes[at..at + 60];
        let size: usize = text(&header[48..58]).trim().parse().expect("a member size");
        let data = &bytes[at + 60..at + 60 + size];
        let field = text(&header[..16]);
        let field = field.trim_end();
        if field == "//" {
            long_names = data;
        } else if field != "/" && field != "/SYM64/" {
            let name = match field.strip_prefix('/') {
                Some(offset) => {
                    let rest = &long_names[offset.parse::<usize>().expect("an offset")..];
                    let end = rest.windows(2).position(|w| w == b"/\n").expect("a long name");
                    text(&rest[..end])
                }
                None => field.trim_end_matches('/').to_owned(),
            };
            members.push((name, data));
        }
        at += 60 + size + (size & 1);
    }
    members
}

/// Every `stride`-th member of the archive `<gcc> -print-file-name=name`
/// that is an ELF object, from the `phase`-th on.
fn archive_sample(target: &Target, name: &str, stride: usize, phase: usize) {
    let Some(tools) = tools() else { return };
    let Some(cc) = gcc(target, "gcc") else { return };
    let Some(path) = installed(&cc, name) else {
        assert!(!tools_required(), "{}: no {name}", target.name);
        return;
    };
    let bytes = std::fs::read(&path).expect("the archive");
    let scratch = Scratch::new("elf-members");
    let mut tally = Tally::default();
    let members = archive_members(&bytes);
    for (position, (member, data)) in members.iter().enumerate().skip(phase).step_by(stride) {
        if !data.starts_with(b"\x7fELF") {
            continue;
        }
        let file = scratch.write(&format!("{position}-{member}"), data);
        tally.check(&tools, &file, DEFAULT_TIMEOUT);
        std::fs::remove_file(&file).expect("a scratch file");
    }
    tally.report(&format!(
        "{name}, {}: every {stride}th of {} members from {phase}",
        target.name,
        members.len()
    ));
}

/// §4.3: sampled members of the system's C library.
#[test]
fn s4_3_libc_members_aarch64() {
    archive_sample(&TARGET_LIST[0], "libc.a", 12, 0);
}

#[test]
fn s4_3_libc_members_x86_64() {
    archive_sample(&TARGET_LIST[1], "libc.a", 12, 5);
}

#[test]
fn s4_3_libc_members_i386() {
    archive_sample(&TARGET_LIST[2], "libc.a", 12, 9);
}

/// §4.3: sampled members of the system's C++ library (COMDAT groups,
/// templates, exceptions, notes of every kind the toolchain writes).
#[test]
fn s4_3_libstdcxx_members_aarch64() {
    archive_sample(&TARGET_LIST[0], "libstdc++.a", 4, 0);
}

#[test]
fn s4_3_libstdcxx_members_x86_64() {
    archive_sample(&TARGET_LIST[1], "libstdc++.a", 4, 1);
}

#[test]
fn s4_3_libstdcxx_members_i386() {
    archive_sample(&TARGET_LIST[2], "libstdc++.a", 4, 2);
}

// ---- objects compiled here -------------------------------------------------------

/// C: TLS of every model, common, weak, hidden and protected symbols, an
/// IFUNC, strings for `SHF_MERGE`, static locals.
const C_SOURCE: &str = r#"
__thread int tls_counter = 7;
__thread int tls_zero;
static __thread long tls_local;
int common_array[16];
int initialized = 42;
static int local_static;
const char *greeting = "hello, world";
const char *other = "another string";
double scale = 2.5;
__attribute__((weak)) int weak_function(int x) { return x + 1; }
__attribute__((weak)) extern int weak_undefined;
__attribute__((visibility("hidden"))) int hidden_function(void) { return 2; }
__attribute__((visibility("protected"))) int protected_data = 3;
extern int external_function(int);
static int resolve_impl(int x) { return x * 2; }
static int (*resolve(void))(int) { return resolve_impl; }
int ifunc_function(int) __attribute__((ifunc("resolve")));
int use(int x) {
    tls_local += x;
    return external_function(x) + weak_function(x) + tls_counter + tls_zero + common_array[x & 15]
        + local_static++ + (&weak_undefined ? weak_undefined : 0) + ifunc_function(x) + greeting[0]
        + other[1] + (int)tls_local + hidden_function() + protected_data + (int)(scale * x);
}
"#;

/// C++: templates, inline functions and static locals (COMDAT groups),
/// virtual functions, `thread_local`, an anonymous namespace, exceptions.
const CXX_SOURCE: &str = r"
template <typename T> struct Box {
    T value;
    T get() const { return value; }
    static int count;
};
template <typename T> int Box<T>::count = 0;
inline int twice(int x) { return 2 * x; }
struct Base {
    virtual ~Base() {}
    virtual int f() { return 1; }
};
struct Derived : Base {
    int f() override { return 2; }
};
inline int &counter() {
    static int c;
    return c;
}
thread_local int tls_cxx = 1;
namespace {
int anon() { return 5; }
}
int entry(int x) {
    Box<int> b{x};
    Box<long> l{x};
    Derived d;
    Base &r = d;
    return twice(b.get()) + (int)l.get() + r.f() + counter()++ + Box<int>::count + anon() + tls_cxx;
}
int thrower(int x) {
    if (x) throw x;
    return 0;
}
int catcher(int x) {
    try { return thrower(x); } catch (int e) { return e; }
}
";

/// Fortran: a module (module variables, a parameter), a common block, I/O.
const FORTRAN_SOURCE: &str = "
module shapes
  implicit none
  real(8), parameter :: pi = 3.14159265358979d0
  integer :: shared_count = 0
contains
  function area(r) result(a)
    real(8), intent(in) :: r
    real(8) :: a
    a = pi * r * r
    shared_count = shared_count + 1
  end function area
end module shapes

subroutine legacy(n, x)
  integer n
  real x(n)
  common /blk/ total, cnt
  real total
  integer cnt
  integer i
  do i = 1, n
    total = total + x(i)
  end do
  cnt = n
  print *, 'total', total
end subroutine legacy
";

/// Option sets every compiler gets: optimized with sections per function
/// and debug information, compressed debug information both ways,
/// position-independent with common symbols.
const FLAG_SETS: [&[&str]; 4] = [
    &["-O1", "-g", "-ffunction-sections", "-fdata-sections"],
    &["-O2", "-g", "-gz=zlib"],
    &["-O0", "-g", "-gz=zstd", "-fPIC"],
    &["-O1", "-fcommon", "-fno-asynchronous-unwind-tables"],
];

/// Options for one target only: GNU property notes, the large data model.
fn target_flags(target: &Target) -> &'static [&'static [&'static str]] {
    match target.e_machine {
        EM_AARCH64 => &[&["-O1", "-mbranch-protection=standard"]],
        EM_X86_64 => &[
            &["-O1", "-fcf-protection", "-Wa,-mx86-used-note=yes"],
            &["-O1", "-fcommon", "-mcmodel=medium", "-mlarge-data-threshold=8"],
        ],
        _ => &[&["-O1", "-fcf-protection", "-Wa,-mx86-used-note=yes"]],
    }
}

/// Compiles `source` (named `file`, which gives the language) with
/// `compiler args`, in `scratch`, and returns the object.
fn compile(
    scratch: &Scratch,
    compiler: &Path,
    args: &[&str],
    file: &str,
    source: &str,
    serial: usize,
) -> Option<PathBuf> {
    let input = scratch.write(file, source);
    let object = scratch.join(format!("{serial}.o"));
    let mut command = Command::new(compiler);
    command.args(args).arg("-c").arg(&input).arg("-o").arg(&object);
    command.current_dir(scratch.path());
    let outcome = run_with(&mut command, &[], DEFAULT_TIMEOUT);
    if !outcome.success() {
        // An option the toolchain does not have (a compression the
        // assembler was built without) skips that one build only.
        let stderr = outcome.stderr_text();
        let unsupported = stderr.contains("unrecognized")
            || stderr.contains("unsupported")
            || stderr.contains("not supported")
            || stderr.contains("unknown argument");
        assert!(unsupported, "{} {args:?} {file}: {stderr}", compiler.display());
        eprintln!("note: {} {args:?} {file}: not supported here; skipped", compiler.display());
        return None;
    }
    Some(object)
}

fn compiled(language: &str, file: &str, source: &str) {
    let Some(tools) = tools() else { return };
    let scratch = Scratch::new("elf-compiled");
    let mut tally = Tally::default();
    let mut serial = 0;
    for target in &TARGET_LIST {
        let mut compilers: Vec<(PathBuf, Vec<String>)> = Vec::new();
        let gnu = match language {
            "c" => gcc(target, "gcc"),
            "c++" => gcc(target, "g++"),
            // There is no gfortran for i386 here; GCC's i386 objects are
            // covered by C and C++.
            _ if target.e_machine == EM_386 => find_gcc(target, "gfortran"),
            _ => gcc(target, "gfortran"),
        };
        if let Some(gnu) = gnu {
            compilers.push((gnu, Vec::new()));
        }
        let clang = match language {
            "c" => tool("clang"),
            "c++" => tool("clang++"),
            _ => None, // no Fortran front end of LLVM is installed
        };
        if let Some(clang) = clang {
            compilers.push((clang, vec![format!("--target={}", target.triple)]));
        }
        for (compiler, base) in &compilers {
            for flags in FLAG_SETS.iter().chain(target_flags(target)) {
                let mut args: Vec<&str> = base.iter().map(String::as_str).collect();
                args.extend_from_slice(flags);
                serial += 1;
                let Some(object) = compile(&scratch, compiler, &args, file, source, serial) else {
                    continue;
                };
                let bytes = std::fs::read(&object).expect("the object");
                assert_eq!(
                    Object::parse(&bytes).map(|o| o.machine().e_machine()),
                    Ok(target.e_machine),
                    "{} {args:?}",
                    compiler.display()
                );
                tally.check(&tools, &object, DEFAULT_TIMEOUT);
            }
        }
    }
    tally.report(&format!("{language} objects compiled here"));
}

/// §4.3: C objects of GCC and Clang for each target.
#[test]
fn s4_3_compiled_c() {
    compiled("c", "c.c", C_SOURCE);
}

/// §4.3: C++ objects of GCC and Clang for each target.
#[test]
fn s4_3_compiled_cxx() {
    compiled("c++", "x.cpp", CXX_SOURCE);
}

/// §4.3: Fortran objects of gfortran for the targets it is installed for.
#[test]
fn s4_3_compiled_fortran() {
    compiled("fortran", "f.f90", FORTRAN_SOURCE);
}

/// §4.3: the test builder's objects (`support::sample`), which hold one
/// of every record the reader has an accessor for, in both classes.
#[test]
fn s4_3_built_samples() {
    let Some(tools) = tools() else { return };
    let scratch = Scratch::new("elf-samples");
    let mut tally = Tally::default();
    for (class, e_machine) in TARGETS {
        let file = scratch.write(&format!("sample-{e_machine}.o"), sample(class, e_machine).bytes);
        tally.check(&tools, &file, DEFAULT_TIMEOUT);
    }
    tally.report("builder samples");
}

// ---- extended section numbering ---------------------------------------------------

/// Assembly for 70 000 sections, each with a word of data, a global symbol
/// in every thousandth and in the last one, and relocations against two of
/// the symbols whose sections are numbered above 0xff00.
fn many_sections() -> String {
    let mut out = String::with_capacity(4 << 20);
    for i in 0..70_000u32 {
        let _ = writeln!(out, "\t.section .s{i},\"aw\",%progbits");
        if i % 1000 == 999 {
            let _ = writeln!(out, "\t.globl g{i}\ng{i}:");
        }
        let _ = writeln!(out, "\t.long {i}");
    }
    out.push_str("\t.section .refs,\"aw\",%progbits\n\t.long g69999\n\t.long g65999\n");
    out
}

/// §3.2, §3.5, §4.3: an object of each target with 70 000 sections: the
/// count and the name table index are escaped in section header 0, the
/// symbols' section indexes in `SHT_SYMTAB_SHNDX`.
#[test]
fn s4_3_extended_section_numbering() {
    let Some(tools) = tools() else { return };
    let scratch = Scratch::new("elf-many");
    let source = scratch.write("many.s", many_sections());
    let mut tally = Tally::default();
    for target in &TARGET_LIST {
        let Some(cc) = gcc(target, "gcc") else { continue };
        let object = scratch.join(format!("many-{}.o", target.name));
        let mut command = Command::new(&cc);
        command.arg("-c").arg(&source).arg("-o").arg(&object);
        let outcome = run_with(&mut command, &[], LONG_TIMEOUT);
        assert!(outcome.success(), "{}: {}", cc.display(), outcome.stderr_text());
        let bytes = std::fs::read(&object).expect("the object");
        let parsed = Object::parse(&bytes).expect("the object");
        assert!(parsed.section_count() > 70_000, "{}", target.name);
        assert_eq!(parsed.header().e_shnum, 0, "{}: e_shnum escaped", target.name);
        let symbols = parsed.symbols().expect("symbols").expect("a symbol table");
        assert!(symbols.has_extended_indexes(), "{}", target.name);
        tally.check(&tools, &object, LONG_TIMEOUT);
    }
    tally.report("70 000 sections");
}

// ---- relocation type names ----------------------------------------------------------

/// An object for `e_machine` with one relocation of each type in `types`,
/// RELA for the 64-bit machines and REL for i386, against symbol 1.
fn relocation_sweep(class: Class, e_machine: u16, types: &[u32]) -> Vec<u8> {
    let mut b = Builder::new(class, e_machine);
    let text = b.progbits(".text", 1, 0x6, vec![0; types.len() * 8]);
    let symtab = text + 2;
    let records: Vec<u8> = types
        .iter()
        .enumerate()
        .flat_map(|(i, &r_type)| {
            let r_offset = i as u64 * 8;
            match e_machine {
                EM_386 => enc_rel(class, &Rel { r_offset, r_sym: 1, r_type }),
                _ => enc_rela(class, &Rela { r_offset, r_sym: 1, r_type, r_addend: 0 }),
            }
        })
        .collect();
    let (sh_type, entsize, name) = match e_machine {
        EM_386 => (SHT_REL, rel_size(class), ".rel.text"),
        _ => (SHT_RELA, rela_size(class), ".rela.text"),
    };
    b.add(
        name,
        Shdr {
            sh_type,
            sh_flags: 0x40,
            sh_link: symtab,
            sh_info: text,
            sh_addralign: class.word_size() as u64,
            sh_entsize: entsize,
            ..Shdr::default()
        },
        records,
    );
    let symbols: Vec<u8> = [Sym::default(), Sym { st_name: 1, st_info: 0x10, ..Sym::default() }]
        .iter()
        .flat_map(|s| enc_sym(class, s))
        .collect();
    let index = b.add(
        ".symtab",
        Shdr {
            sh_type: SHT_SYMTAB,
            sh_link: symtab + 1,
            sh_info: 1,
            sh_addralign: class.word_size() as u64,
            sh_entsize: sym_size(class),
            ..Shdr::default()
        },
        symbols,
    );
    assert_eq!(index, symtab);
    b.progbits(".strtab", 3, 0, b"\0s\0".to_vec());
    b.build().bytes
}

/// The type number and name in each line of `readelf -rW`; the name is
/// `None` when readelf does not name the type.
fn readelf_types(output: &str, class: Class, count: usize) -> Vec<(u32, Option<String>)> {
    let mut lines = output.lines().skip_while(|line| !line.starts_with("Relocation section"));
    lines.next();
    lines.next();
    let types: Vec<(u32, Option<String>)> = lines
        .take(count)
        .map(|line| {
            let mut fields = line.split_whitespace().skip(1);
            let info = hex(fields.next().unwrap_or_default());
            let r_type = match class {
                Class::Elf32 => info & 0xff,
                Class::Elf64 => info & 0xffff_ffff,
            };
            let name = fields.next().unwrap_or_default();
            (r_type as u32, name.starts_with("R_").then(|| name.to_owned()))
        })
        .collect();
    assert_eq!(types.len(), count, "readelf -r lines");
    types
}

/// The type number and name in each record of `llvm-readobj -r
/// --expand-relocs`; the name is `None` when LLVM does not name the type.
fn llvm_types(output: &str, count: usize) -> Vec<(u32, Option<String>)> {
    let root = Node::parse(output);
    let section = root.node("Relocations").nodes("Section").next().map(|s| {
        s.nodes("Relocation")
            .map(|r| {
                let (name, r_type) = name_and_number(r.get("Type"));
                (r_type as u32, name.starts_with("R_").then(|| name.to_owned()))
            })
            .collect::<Vec<_>>()
    });
    let types = section.unwrap_or_default();
    assert_eq!(types.len(), count, "llvm-readobj -r records");
    types
}

/// Where binutils 2.44 and LLVM 22 name a type and the reader, on purpose,
/// does not: the AArch64 ILP32 types, which only `ELFCLASS32` objects use
/// and the reader refuses (§2.3), and i386 type 200, which binutils names
/// `R_386_USED_BY_INTEL_200` and no ABI defines.
fn unnamed_on_purpose(e_machine: u16, r_type: u32) -> bool {
    match e_machine {
        EM_AARCH64 => (1..=188).contains(&r_type),
        EM_386 => r_type == 200,
        _ => false,
    }
}

/// §2.3: over every relocation type number up to 1100 (and a few beyond)
/// for the 64-bit machines and every one of the 256 for i386, the reader's
/// name is the one the tools print; where both tools name a type and
/// disagree, it is one of the two and the difference is pinned; where
/// neither names it, the reader does not either.
#[test]
fn s2_3_relocation_names_agree_with_the_tools() {
    let Some(tools) = tools() else { return };
    let scratch = Scratch::new("elf-relocs");
    let mut differences = Vec::new();
    let mut named = 0;
    let (mut only_gnu, mut only_llvm) = (Vec::new(), Vec::new());
    for (class, e_machine) in TARGETS {
        let types: Vec<u32> = match e_machine {
            EM_386 => (0..256).collect(),
            _ => (0..=1100).chain([4095, 4096, 0x1_0000, 0xffff_ffff]).collect(),
        };
        let bytes = relocation_sweep(class, e_machine, &types);
        // The reader decodes the type field of each record as the tools do.
        let decoded: Vec<u32> = Object::parse(&bytes)
            .expect("the sweep object")
            .sections()
            .find(|s| matches!(s.sh_type(), SHT_RELA | SHT_REL))
            .map(|s| match e_machine {
                EM_386 => s.rel().expect("REL records").iter().map(|r| r.r_type).collect(),
                _ => s.rela().expect("RELA records").iter().map(|r| r.r_type).collect(),
            })
            .expect("a relocation section");
        assert_eq!(decoded, types, "e_machine {e_machine}: decoded types");
        let file = scratch.write(&format!("relocs-{e_machine}.o"), bytes);
        let gnu = run_tool(&tools.readelf, &["-rW"], &file, DEFAULT_TIMEOUT);
        let gnu = readelf_types(&gnu, class, types.len());
        let llvm = run_tool(&tools.readobj, &["-r", "--expand-relocs"], &file, DEFAULT_TIMEOUT);
        let llvm = llvm_types(&llvm, types.len());
        for ((&r_type, (gnu_type, gnu)), (llvm_type, llvm)) in types.iter().zip(&gnu).zip(&llvm) {
            assert_eq!((*gnu_type, *llvm_type), (r_type, r_type), "e_machine {e_machine}");
            let ours = mortar_elf::reloc::name(e_machine, r_type);
            let what = format!(
                "e_machine {e_machine} type {r_type}: readelf {gnu:?}, llvm {llvm:?}, reader \
                 {ours:?}"
            );
            if ours.is_some() {
                named += 1;
            }
            match (gnu, llvm) {
                (None, None) => assert_eq!(ours, None, "{what}"),
                _ if unnamed_on_purpose(e_machine, r_type) => assert_eq!(ours, None, "{what}"),
                (Some(gnu), Some(llvm)) if gnu != llvm => {
                    assert!(ours == Some(gnu.as_str()) || ours == Some(llvm.as_str()), "{what}");
                    differences.push((e_machine, r_type, gnu.clone(), llvm.clone(), ours));
                }
                (Some(name), None) | (None, Some(name)) => {
                    assert_eq!(ours, Some(name.as_str()), "{what}");
                    let list = if gnu.is_some() { &mut only_gnu } else { &mut only_llvm };
                    list.push((e_machine, r_type));
                }
                (Some(name), Some(_)) => assert_eq!(ours, Some(name.as_str()), "{what}"),
            }
        }
    }
    eprintln!("relocation types: {named} named by the reader");
    eprintln!("  named by readelf only (e_machine, type): {only_gnu:?}");
    eprintln!("  named by llvm-readobj only: {only_llvm:?}");
    eprintln!("  named differently: {differences:?}");
    // The differences between binutils 2.44 and LLVM 22 that §2.3 records.
    let pinned = |list: &[(u16, &[u32])]| -> Vec<(u16, u32)> {
        list.iter().flat_map(|&(m, types)| types.iter().map(move |&t| (m, t))).collect()
    };
    let only_readelf: &[(u16, &[u32])] = &[
        (EM_AARCH64, &[256]),
        (EM_X86_64, &[38, 39, 40, 46, 47, 48, 49, 51, 250, 251]),
        (EM_386, &[38, 250, 251]),
    ];
    let llvm_auth: Vec<u32> = (314..=317).chain(580..=597).chain(1041..=1044).collect();
    assert_eq!(only_gnu, pinned(only_readelf), "types only readelf names");
    assert_eq!(only_llvm, pinned(&[(EM_AARCH64, &llvm_auth)]), "types only LLVM names");
    assert!(differences.is_empty(), "types the tools name differently");
}
