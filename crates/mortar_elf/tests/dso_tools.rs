//! L1 (`docs/VALIDATION.md`): the shared-object reader agrees with GNU
//! `readelf` and with `llvm-readobj` on real shared objects, `spec/elf.md`
//! §6.13. Every field the tools print of the dynamic table, the dynamic
//! symbols and their versions, the version definitions and needs, the
//! program headers and the program properties is compared with what the
//! reader gives for it.
//!
//! Nothing binary is checked in. The corpus is every `ET_DYN` file of the
//! three targets on the machine (each inode once), a deterministic sample
//! of it stripped of its section headers by `llvm-objcopy --strip-sections`
//! (the dynamic-only path must give the identical view), and a versioned
//! C library built here with GNU ld, lld and mold for each target and each
//! hash style.

mod support;

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mortar_elf::consts::{
    DF_1_DIRECT, DF_1_GLOBAL, DF_1_GROUP, DF_1_INITFIRST, DF_1_INTERPOSE, DF_1_LOADFLTR,
    DF_1_NODEFLIB, DF_1_NODELETE, DF_1_NODUMP, DF_1_NOOPEN, DF_1_NOW, DF_1_ORIGIN, DF_1_PIE,
    DF_BIND_NOW, DF_ORIGIN, DF_STATIC_TLS, DF_SYMBOLIC, DF_TEXTREL, DT_AARCH64_BTI_PLT,
    DT_AARCH64_PAC_PLT, DT_AARCH64_VARIANT_PCS, DT_FLAGS, DT_FLAGS_1, DT_NEEDED, DT_PLTREL,
    DT_RPATH, DT_RUNPATH, DT_SONAME, EM_386, EM_AARCH64, EM_X86_64, ET_DYN, PF_R, PF_W, PF_X,
    STT_SECTION, VER_FLG_BASE, VER_FLG_INFO, VER_FLG_WEAK, pt_name,
};
use mortar_elf::{Class, Entry, Error, FileHeader, SharedObject, SymbolSection};
use mortar_testutil::{DEFAULT_TIMEOUT, Scratch, run_with, tool, tools_required};
use support::dso::{DsoView, SymView, dso_view, text};
use support::oracle::{
    Node, hex, in_parens, name_and_number, number, property_text, readelf_binding,
    readelf_section_index, readelf_symbol_type, readelf_visibility, run_tool,
};

/// The largest corpus file prints millions of lines.
const TIMEOUT: Duration = Duration::from_mins(3);

// ---- the subject --------------------------------------------------------------

/// A shared object, and what the reader says about it.
struct Subject {
    path: PathBuf,
    class: Class,
    e_machine: u16,
    /// By the section headers when the file has them.
    view: DsoView,
    section_names: Vec<String>,
    /// The file has sections, and the dynamic table alone does not give
    /// the symbol count because its GNU hash table hashes no symbol and it
    /// has no `DT_HASH` (§6.6): the one documented way the entry paths may
    /// differ. `readelf -D` and `llvm-readobj` cannot list them either.
    unhashed: bool,
}

impl Subject {
    fn read(path: &Path) -> Subject {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        Self::of(path, &bytes)
    }

    fn of(path: &Path, bytes: &[u8]) -> Subject {
        let at = path.display();
        let object = SharedObject::parse(bytes).unwrap_or_else(|e| panic!("{at}: {e}"));
        let class = object.class();
        let e_machine = object.header().ehdr.e_machine;
        let section_names = object
            .section_table()
            .iter()
            .map(|section| section.name().map(text).unwrap_or_default())
            .collect();
        let view = dso_view(bytes, None).unwrap_or_else(|e| panic!("{at}: {e}"));
        let mut unhashed = false;
        if !object.section_table().is_empty() {
            // §6.3: the dynamic table alone says the same.
            match dso_view(bytes, Some(Entry::Dynamic)) {
                Ok(dynamic) => assert_eq!(dynamic, view, "{at}: the entry paths disagree"),
                Err(Error::UnhashedSymbolCount { .. }) => unhashed = true,
                Err(e) => panic!("{at}: by the dynamic table: {e}"),
            }
        }
        Subject { path: path.to_owned(), class, e_machine, view, section_names, unhashed }
    }

    fn section_name(&self, index: u32) -> String {
        self.section_names.get(index as usize).cloned().unwrap_or_default()
    }
}

/// The name of a dynamic symbol with its version, as each tool prints it:
/// (`readelf`, `llvm-readobj`). A section symbol without a name has its
/// section's name when the file has sections.
fn shown_names(subject: &Subject, symbol: &SymView) -> (String, String) {
    let mut base = symbol.name.clone();
    if base.is_empty()
        && symbol.sym.sym_type() == STT_SECTION
        && let SymbolSection::Section(index) = symbol.section
    {
        base = subject.section_name(index.0);
    }
    let Some(version) = &symbol.version else { return (base.clone(), base) };
    let name = version.name.clone().unwrap_or_default();
    // binutils 2.44 prints a version's own symbol (`VER_1`, absolute, of
    // version VER_1) without its version.
    if version.kind == 'D' && name == base {
        return (base.clone(), format!("{base}@@{name}"));
    }
    match version.kind {
        'D' if version.hidden => (format!("{base}@{name}"), format!("{base}@{name}")),
        'D' => (format!("{base}@@{name}"), format!("{base}@@{name}")),
        'N' => (format!("{base}@{name} ({})", version.index), format!("{base}@{name}")),
        _ => (base.clone(), base),
    }
}

// ---- spellings ----------------------------------------------------------------

const DF_NAMES: [(u64, &str); 5] = [
    (DF_ORIGIN, "ORIGIN"),
    (DF_SYMBOLIC, "SYMBOLIC"),
    (DF_TEXTREL, "TEXTREL"),
    (DF_BIND_NOW, "BIND_NOW"),
    (DF_STATIC_TLS, "STATIC_TLS"),
];

const DF_1_NAMES: [(u64, &str); 14] = [
    (DF_1_NOW, "NOW"),
    (DF_1_GLOBAL, "GLOBAL"),
    (DF_1_GROUP, "GROUP"),
    (DF_1_NODELETE, "NODELETE"),
    (DF_1_LOADFLTR, "LOADFLTR"),
    (DF_1_INITFIRST, "INITFIRST"),
    (DF_1_NOOPEN, "NOOPEN"),
    (DF_1_ORIGIN, "ORIGIN"),
    (DF_1_DIRECT, "DIRECT"),
    (DF_1_INTERPOSE, "INTERPOSE"),
    (DF_1_NODEFLIB, "NODEFLIB"),
    (DF_1_NODUMP, "NODUMP"),
    (DF_1_PIE, "PIE"),
    (0, ""),
];

/// The names of the flags set in `value`, in bit order, space-separated;
/// a bit without a name fails.
fn flag_names(value: u64, names: &[(u64, &str)]) -> String {
    let mut shown = Vec::new();
    let mut left = value;
    for &(bit, name) in names {
        if bit != 0 && value & bit != 0 {
            shown.push(name);
            left &= !bit;
        }
    }
    assert_eq!(left, 0, "flags {value:#x}: add the tools' spelling of {left:#x}");
    shown.join(" ")
}

/// A dynamic entry's value as both tools print it, after the tag and its
/// name, checked against `d_val`, or for a string-valued tag against
/// `string`, the string the reader found for it.
fn check_dynamic_value(
    e_machine: u16,
    (d_tag, d_val): (i64, u64),
    string: Option<&str>,
    shown: &str,
    llvm: bool,
) -> bool {
    let shown = shown.trim();
    let quoted = |label: &str| {
        shown
            .strip_prefix(label)
            .and_then(|s| s.strip_prefix(" ["))
            .and_then(|s| s.strip_suffix(']'))
    };
    match d_tag {
        DT_NEEDED => quoted("Shared library:") == string,
        DT_SONAME => quoted("Library soname:") == string,
        DT_RPATH => quoted("Library rpath:") == string,
        DT_RUNPATH => quoted("Library runpath:") == string,
        DT_FLAGS => shown == flag_names(d_val, &DF_NAMES),
        DT_FLAGS_1 => {
            let names = flag_names(d_val, &DF_1_NAMES);
            if llvm { shown == names } else { shown == format!("Flags: {names}").trim_end() }
        }
        DT_PLTREL => shown == if d_val == 7 { "RELA" } else { "REL" },
        // readelf prints no value for these.
        DT_AARCH64_BTI_PLT | DT_AARCH64_PAC_PLT | DT_AARCH64_VARIANT_PCS
            if !llvm && e_machine == EM_AARCH64 =>
        {
            shown.is_empty()
        }
        _ => {
            let value = shown.strip_suffix(" (bytes)").unwrap_or(shown);
            number(value) == d_val
        }
    }
}

/// The string the reader found for each dynamic entry of a string-valued
/// tag, in table order.
fn dynamic_strings(view: &DsoView) -> Vec<Option<&str>> {
    let mut needed = view.needed.iter();
    view.dynamic
        .iter()
        .map(|&(d_tag, _)| match d_tag {
            DT_NEEDED => needed.next().map(String::as_str),
            DT_SONAME => view.soname.as_deref(),
            DT_RPATH => view.rpath.as_deref(),
            DT_RUNPATH => view.runpath.as_deref(),
            _ => None,
        })
        .collect()
}

/// `vd_flags` / `vna_flags` as `readelf` prints them.
fn readelf_version_flags(flags: u16) -> String {
    if flags == 0 {
        return "none".to_owned();
    }
    let mut shown = Vec::new();
    for (bit, name) in [(VER_FLG_BASE, "BASE"), (VER_FLG_WEAK, "WEAK"), (VER_FLG_INFO, "INFO")] {
        if flags & bit != 0 {
            shown.push(name);
        }
    }
    if flags & !(VER_FLG_BASE | VER_FLG_WEAK | VER_FLG_INFO) != 0 {
        shown.push("<unknown>");
    }
    shown.join(" | ")
}

fn readelf_segment_type(p_type: u32, e_machine: u16) -> String {
    match pt_name(p_type) {
        Some(name) => name.trim_start_matches("PT_").to_owned(),
        None if e_machine == EM_AARCH64 && p_type == 0x7000_0002 => "AARCH64_MEMTAG_MTE".to_owned(),
        None => panic!("segment type {p_type:#x}: add readelf's spelling"),
    }
}

fn readelf_segment_flags(p_flags: u32) -> String {
    let flag = |bit, c| if p_flags & bit != 0 { c } else { ' ' };
    [flag(PF_R, 'R'), flag(PF_W, 'W'), flag(PF_X, 'E')]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// The program properties as each tool prints them.
fn properties_text(subject: &Subject, llvm: bool) -> Option<Vec<String>> {
    let properties = subject.view.properties.as_ref()?;
    Some(
        properties
            .iter()
            .map(|(pr_type, data)| {
                let (readelf, llvm_text) =
                    property_text(subject.e_machine, subject.class, *pr_type, data);
                if llvm { llvm_text } else { readelf }
            })
            .collect(),
    )
}

// ---- what was compared ----------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    files: u64,
    segments: u64,
    dynamic: u64,
    symbols: u64,
    versym: u64,
    definitions: u64,
    needs: u64,
    properties: u64,
}

impl Counts {
    fn add(&mut self, other: Counts) {
        self.files += other.files;
        self.segments += other.segments;
        self.dynamic += other.dynamic;
        self.symbols += other.symbols;
        self.versym += other.versym;
        self.definitions += other.definitions;
        self.needs += other.needs;
        self.properties += other.properties;
    }
}

// ---- readelf --------------------------------------------------------------------

/// The output split into blocks: a heading line and the lines up to the
/// next heading.
fn readelf_blocks(output: &str) -> Vec<(&str, Vec<&str>)> {
    const HEADINGS: [&str; 10] = [
        "Elf file type is",
        "Program Headers:",
        " Section to Segment mapping:",
        "Dynamic section at offset",
        "Symbol table '",
        "Symbol table for image",
        "Version symbols section",
        "Version definition section",
        "Version needs section",
        "Displaying notes found",
    ];
    let mut blocks: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in output.lines() {
        if HEADINGS.iter().any(|h| line.starts_with(h))
            || line.starts_with("No version information")
        {
            blocks.push((line, Vec::new()));
        } else if let Some(block) = blocks.last_mut()
            && !line.trim().is_empty()
        {
            block.1.push(line);
        }
    }
    blocks
}

/// Compares the symbol lines of `readelf -s` (by section or `-D`) with the
/// view, after the heading line.
fn readelf_symbols(subject: &Subject, lines: &[&str], at: &str) -> u64 {
    let symbols = &subject.view.symbols;
    assert_eq!(lines.len(), symbols.len() + 1, "{at}: readelf's symbol count");
    for (index, (symbol, line)) in symbols.iter().zip(&lines[1..]).enumerate() {
        let what = || format!("{at}: dynamic symbol {index}: `{line}`");
        let (shown_index, rest) = line.trim_start().split_once(": ").unwrap_or_default();
        assert_eq!(shown_index, index.to_string(), "{}", what());
        let mut fields = rest.split(' ').filter(|f| !f.is_empty());
        let mut next = || fields.next().unwrap_or_default();
        let sym = &symbol.sym;
        assert_eq!(hex(next()), sym.st_value, "{}", what());
        assert_eq!(number(next()), sym.st_size, "{}", what());
        assert_eq!(next(), readelf_symbol_type(sym.sym_type()), "{}", what());
        assert_eq!(next(), readelf_binding(sym.binding()), "{}", what());
        let visibility = readelf_visibility(sym, subject.e_machine);
        let mut shown = next().to_owned();
        for _ in 0..visibility.matches(' ').count() {
            shown.push(' ');
            shown.push_str(next());
        }
        assert_eq!(shown, visibility, "{}", what());
        let ndx = next();
        assert_eq!(ndx, readelf_section_index(symbol.section, subject.e_machine), "{}", what());
        let end = ndx.as_ptr() as usize - line.as_ptr() as usize + ndx.len();
        let name = line.get(end + 1..).unwrap_or("");
        assert_eq!(name, shown_names(subject, symbol).0, "{}", what());
    }
    symbols.len() as u64
}

/// Compares `readelf -dW --dyn-syms -V -lW -n` with the reader.
#[allow(clippy::too_many_lines)] // one part of the output after another
fn check_readelf(readelf: &Path, subject: &Subject) -> Counts {
    let at = subject.path.display().to_string();
    let view = &subject.view;
    let output =
        run_tool(readelf, &["-dW", "--dyn-syms", "-V", "-lW", "-n"], &subject.path, TIMEOUT);
    let mut counts = Counts { files: 1, ..Counts::default() };
    let mut seen = BTreeSet::new();
    let mut property_notes = 0;
    for (heading, lines) in readelf_blocks(&output) {
        let kind = heading.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        seen.insert(kind);
        if heading == "Program Headers:" {
            let rows: Vec<&&str> =
                lines[1..].iter().filter(|l| !l.trim_start().starts_with("[Requesting")).collect();
            assert_eq!(rows.len(), view.segments.len(), "{at}: program header count");
            for (index, (phdr, line)) in view.segments.iter().zip(rows).enumerate() {
                let what = || format!("{at}: program header {index}: `{line}`");
                let fields: Vec<&str> = line.split_whitespace().collect();
                assert!(fields.len() >= 7, "{}", what());
                assert_eq!(
                    fields[0],
                    readelf_segment_type(phdr.p_type, subject.e_machine),
                    "{}",
                    what()
                );
                let numbers: Vec<u64> = fields[1..6].iter().map(|f| number(f)).collect();
                assert_eq!(
                    numbers,
                    [phdr.p_offset, phdr.p_vaddr, phdr.p_paddr, phdr.p_filesz, phdr.p_memsz],
                    "{}",
                    what()
                );
                let last = fields.len() - 1;
                assert_eq!(number(fields[last]), phdr.p_align, "{}", what());
                assert_eq!(
                    fields[6..last].join(" "),
                    readelf_segment_flags(phdr.p_flags).replace("  ", " "),
                    "{}",
                    what()
                );
                counts.segments += 1;
            }
        } else if let Some(rest) = heading.strip_prefix("Dynamic section at offset ") {
            let (offset, count) = rest.split_once(" contains ").unwrap_or_default();
            assert_eq!(number(offset), view.dynamic_offset, "{at}: {heading}");
            let count = count.split_whitespace().next().map_or(0, number);
            assert_eq!(count as usize, view.dynamic.len(), "{at}: {heading}");
            assert_eq!(lines.len(), view.dynamic.len() + 1, "{at}: dynamic entries");
            let strings = dynamic_strings(view);
            for ((&(d_tag, d_val), line), string) in
                view.dynamic.iter().zip(&lines[1..]).zip(strings)
            {
                let what = || format!("{at}: `{line}`");
                let line = line.trim_start();
                let (tag, rest) = line.split_once(' ').unwrap_or_default();
                assert_eq!(number(tag), d_tag as u64 & tag_mask(subject.class), "{}", what());
                let close = rest.find(')').unwrap_or_else(|| panic!("{}", what()));
                assert!(
                    check_dynamic_value(
                        subject.e_machine,
                        (d_tag, d_val),
                        string,
                        &rest[close + 1..],
                        false
                    ),
                    "{}",
                    what()
                );
                counts.dynamic += 1;
            }
        } else if heading.starts_with("Symbol table '.dynsym' contains ") {
            counts.symbols += readelf_symbols(subject, &lines, &at);
        } else if heading.starts_with("Version symbols section") {
            let mut shown = Vec::new();
            for line in &lines[1..] {
                let (_, mut rest) = line.split_once(':').unwrap_or_default();
                while let Some(open) = rest.find('(') {
                    let close = rest[open..].find(')').map_or(rest.len(), |c| open + c);
                    let index = rest[..open].trim();
                    let (index, hidden) = match index.strip_suffix('h') {
                        Some(index) => (index, true),
                        None => (index, false),
                    };
                    // The index is hexadecimal, without `0x`.
                    shown.push((hex(index), hidden, rest[open + 1..close].to_owned()));
                    rest = &rest[(close + 1).min(rest.len())..];
                }
            }
            assert_eq!(shown.len(), view.symbols.len(), "{at}: version symbols");
            for (index, (symbol, (number, hidden, name))) in
                view.symbols.iter().zip(&shown).enumerate()
            {
                let version = symbol.version.as_ref().unwrap_or_else(|| panic!("{at}: no version"));
                let expected = match version.index {
                    0 => "*local*".to_owned(),
                    1 => "*global*".to_owned(),
                    _ => version.name.clone().unwrap_or_default(),
                };
                assert_eq!(
                    (*number, *hidden, name.trim()),
                    (u64::from(version.index), version.hidden, expected.as_str()),
                    "{at}: version of symbol {index}"
                );
                counts.versym += 1;
            }
        } else if heading.starts_with("Version definition section") {
            let mut definitions = view.definitions.iter();
            let mut current = None;
            let mut parents = Vec::new();
            let finish = |current: Option<&support::dso::DefView>, parents: &mut Vec<String>| {
                if let Some(def) = current {
                    assert_eq!(
                        parents[..],
                        def.names[1..],
                        "{at}: parents of version {}",
                        def.index
                    );
                }
                parents.clear();
            };
            for line in &lines[1..] {
                let (_, rest) = line.split_once(": ").unwrap_or_default();
                if let Some(parent) = rest.strip_prefix("Parent ") {
                    let (_, name) = parent.split_once(": ").unwrap_or_default();
                    parents.push(name.to_owned());
                    continue;
                }
                finish(current, &mut parents);
                let def = definitions.next().unwrap_or_else(|| panic!("{at}: more definitions"));
                let expected = format!(
                    "Rev: 1  Flags: {}  Index: {}  Cnt: {}  Name: {}",
                    readelf_version_flags(def.flags),
                    def.index,
                    def.names.len(),
                    def.names.first().map_or("", String::as_str)
                );
                assert_eq!(rest, expected, "{at}: `{line}`");
                current = Some(def);
                counts.definitions += 1;
            }
            finish(current, &mut parents);
            assert!(definitions.next().is_none(), "{at}: fewer definitions");
        } else if heading.starts_with("Version needs section") {
            let mut needs = view.needs.iter();
            let mut entries = [].iter();
            for line in &lines[1..] {
                let (_, rest) = line.split_once(": ").unwrap_or_default();
                if let Some(entry) = rest.trim_start().strip_prefix("Name: ") {
                    let (name, index, flags, _) =
                        entries.next().unwrap_or_else(|| panic!("{at}: more need entries"));
                    let expected = format!(
                        "{name}  Flags: {}  Version: {index}",
                        readelf_version_flags(*flags)
                    );
                    assert_eq!(entry, expected, "{at}: `{line}`");
                    counts.needs += 1;
                } else {
                    assert!(entries.next().is_none(), "{at}: fewer need entries");
                    let need = needs.next().unwrap_or_else(|| panic!("{at}: more needs"));
                    let expected =
                        format!("Version: 1  File: {}  Cnt: {}", need.file, need.entries.len());
                    assert_eq!(rest, expected, "{at}: `{line}`");
                    entries = need.entries.iter();
                }
            }
            assert!(entries.next().is_none() && needs.next().is_none(), "{at}: fewer needs");
        } else if heading.starts_with("Displaying notes found") {
            for line in lines.iter().filter(|l| l.contains("\tNT_GNU_PROPERTY_TYPE_0\t")) {
                let description = line.rsplit('\t').next().unwrap_or_default().trim();
                if let Some(expected) = properties_text(subject, false)
                    && description == format!("Properties: {}", expected.join(", "))
                {
                    property_notes += 1;
                    counts.properties += expected.len() as u64;
                }
            }
        }
    }
    // What the reader found, readelf showed.
    let shows = |kind: &str| seen.contains(kind);
    assert_eq!(shows("Program Headers:"), !view.segments.is_empty(), "{at}");
    assert_eq!(shows("Dynamic section"), !view.dynamic.is_empty(), "{at}");
    assert_eq!(shows("Version definition"), !view.definitions.is_empty(), "{at}");
    assert_eq!(shows("Version needs"), !view.needs.is_empty(), "{at}");
    if view.properties.is_some() {
        assert!(property_notes >= 1, "{at}: readelf shows no matching property note");
    }
    counts
}

/// The bits of `d_tag` a class stores.
fn tag_mask(class: Class) -> u64 {
    match class {
        Class::Elf32 => 0xffff_ffff,
        Class::Elf64 => u64::MAX,
    }
}

// ---- llvm-readobj ---------------------------------------------------------------

/// Compares `llvm-readobj --dynamic-table --dyn-syms --version-info
/// --program-headers --notes` with the reader.
#[allow(clippy::too_many_lines)] // one part of the output after another
fn check_llvm(readobj: &Path, subject: &Subject) -> Counts {
    let at = subject.path.display().to_string();
    let view = &subject.view;
    let args = ["--dynamic-table", "--dyn-syms", "--version-info", "--program-headers", "--notes"];
    let output = run_tool(readobj, &args, &subject.path, TIMEOUT);
    let root = Node::parse(&output);
    let mut counts = Counts { files: 1, ..Counts::default() };
    if let Some(soname) = &view.soname {
        assert_eq!(root.get("LoadName"), soname, "{at}");
    }

    let shown: Vec<&Node> = root.node("ProgramHeaders").nodes("ProgramHeader").collect();
    assert_eq!(shown.len(), view.segments.len(), "{at}: program headers");
    for (index, (phdr, node)) in view.segments.iter().zip(shown).enumerate() {
        let what = format!("{at}: program header {index}");
        assert_eq!(in_parens(node.get("Type")), u64::from(phdr.p_type), "{what}");
        assert_eq!(node.num("Offset"), phdr.p_offset, "{what}");
        assert_eq!(node.num("VirtualAddress"), phdr.p_vaddr, "{what}");
        assert_eq!(node.num("PhysicalAddress"), phdr.p_paddr, "{what}");
        assert_eq!(node.num("FileSize"), phdr.p_filesz, "{what}");
        assert_eq!(node.num("MemSize"), phdr.p_memsz, "{what}");
        assert_eq!(in_parens(node.get("Flags")), u64::from(phdr.p_flags), "{what}");
        assert_eq!(node.num("Alignment"), phdr.p_align, "{what}");
        counts.segments += 1;
    }

    if !view.dynamic.is_empty() {
        let table = root.node("DynamicSection");
        let entries = table.head.trim_start_matches("DynamicSection [ (");
        let entries = entries.split_whitespace().next().map_or(0, number);
        assert_eq!(entries as usize, view.dynamic.len(), "{at}: {}", table.head);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), view.dynamic.len() + 1, "{at}: dynamic entries");
        let strings = dynamic_strings(view);
        for ((&(d_tag, d_val), line), string) in view.dynamic.iter().zip(&lines[1..]).zip(strings) {
            let what = || format!("{at}: `{line}`");
            let mut parts = line.splitn(2, ' ');
            let tag = parts.next().unwrap_or_default();
            assert_eq!(number(tag), d_tag as u64 & tag_mask(subject.class), "{}", what());
            let rest = parts.next().unwrap_or_default().trim_start();
            let value = rest.split_once(' ').map_or("", |(_, value)| value);
            assert!(
                check_dynamic_value(subject.e_machine, (d_tag, d_val), string, value, true),
                "{}",
                what()
            );
            counts.dynamic += 1;
        }
    }

    let shown: Vec<&Node> =
        root.nodes("DynamicSymbols").flat_map(|node| node.nodes("Symbol")).collect();
    assert_eq!(shown.len(), view.symbols.len(), "{at}: dynamic symbols");
    for (index, (symbol, node)) in view.symbols.iter().zip(&shown).enumerate() {
        let what = format!("{at}: dynamic symbol {index}");
        let sym = &symbol.sym;
        let (name, st_name) = name_and_number(node.get("Name"));
        assert_eq!(st_name, u64::from(sym.st_name), "{what}");
        assert_eq!(name.trim(), shown_names(subject, symbol).1, "{what}");
        assert_eq!(node.num("Value"), sym.st_value, "{what}");
        assert_eq!(node.num("Size"), sym.st_size, "{what}");
        assert_eq!(in_parens(node.get("Binding")), u64::from(sym.binding()), "{what}");
        assert_eq!(in_parens(node.get("Type")), u64::from(sym.sym_type()), "{what}");
        let other = node.get("Other");
        let other = if other.starts_with('(') { in_parens(other) } else { number(other) };
        assert_eq!(other, u64::from(sym.st_other), "{what}");
        assert_eq!(in_parens(node.get("Section")), u64::from(sym.st_shndx), "{what}");
        counts.symbols += 1;
    }

    let versions: Vec<&Node> =
        root.nodes("VersionSymbols").flat_map(|node| node.nodes("Symbol")).collect();
    if versions.is_empty() {
        assert!(view.symbols.iter().all(|s| s.version.is_none()), "{at}: llvm shows no versions");
    } else {
        assert_eq!(versions.len(), view.symbols.len(), "{at}: version symbols");
        for (index, (symbol, node)) in view.symbols.iter().zip(versions).enumerate() {
            let version = symbol.version.as_ref().unwrap_or_else(|| panic!("{at}: no version"));
            assert_eq!(node.num("Version"), u64::from(version.index), "{at}: version {index}");
            counts.versym += 1;
        }
    }

    let definitions: Vec<&Node> =
        root.nodes("VersionDefinitions").flat_map(|node| node.nodes("Definition")).collect();
    assert_eq!(definitions.len(), view.definitions.len(), "{at}: version definitions");
    for (def, node) in view.definitions.iter().zip(definitions) {
        let what = format!("{at}: version definition {}", def.index);
        assert_eq!(node.num("Version"), 1, "{what}");
        assert_eq!(in_parens(node.get("Flags")), u64::from(def.flags), "{what}");
        assert_eq!(node.num("Index"), u64::from(def.index), "{what}");
        assert_eq!(node.num("Hash"), u64::from(def.hash), "{what}");
        assert_eq!(node.get("Name"), def.names[0], "{what}");
        let predecessors = node.get("Predecessors").trim_start_matches('[').trim_end_matches(']');
        let predecessors: Vec<&str> = predecessors.split(", ").filter(|p| !p.is_empty()).collect();
        assert_eq!(predecessors, def.names[1..], "{what}");
        counts.definitions += 1;
    }

    let needs: Vec<&Node> =
        root.nodes("VersionRequirements").flat_map(|node| node.nodes("Dependency")).collect();
    assert_eq!(needs.len(), view.needs.len(), "{at}: version needs");
    for (need, node) in view.needs.iter().zip(needs) {
        let what = format!("{at}: version need {}", need.file);
        assert_eq!(node.num("Version"), 1, "{what}");
        assert_eq!(node.num("Count"), need.entries.len() as u64, "{what}");
        assert_eq!(node.get("FileName"), need.file, "{what}");
        let entries: Vec<&Node> = node.node("Entries").nodes("Entry").collect();
        assert_eq!(entries.len(), need.entries.len(), "{what}");
        for ((name, index, flags, hash), entry) in need.entries.iter().zip(entries) {
            assert_eq!(entry.num("Hash"), u64::from(*hash), "{what}: {name}");
            assert_eq!(in_parens(entry.get("Flags")), u64::from(*flags), "{what}: {name}");
            assert_eq!(entry.num("Index"), u64::from(*index), "{what}: {name}");
            assert_eq!(entry.get("Name"), name, "{what}");
            counts.needs += 1;
        }
    }

    if let Some(expected) = properties_text(subject, true) {
        let found = root
            .nodes("NoteSections")
            .chain(root.nodes("NoteSegments"))
            .flat_map(|n| n.nodes("NoteSection").chain(n.nodes("NoteSegment")))
            .flat_map(|n| n.nodes("Notes"))
            .flat_map(|n| n.nodes(""))
            .filter(|note| note.get("Type").starts_with("NT_GNU_PROPERTY_TYPE_0"))
            .any(|note| note.node("Property").lines().collect::<Vec<_>>() == expected);
        assert!(found, "{at}: llvm-readobj shows no matching property note");
        counts.properties += expected.len() as u64;
    }
    counts
}

// ---- the corpus -------------------------------------------------------------------

struct Tools {
    readelf: PathBuf,
    readobj: PathBuf,
}

fn tools() -> Option<Tools> {
    Some(Tools { readelf: tool("readelf")?, readobj: tool("llvm-readobj")? })
}

/// The directories the corpus is drawn from.
fn corpus_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = [
        "/usr/lib/aarch64-linux-gnu",
        "/lib/aarch64-linux-gnu",
        "/usr/x86_64-linux-gnu/lib",
        "/usr/i686-linux-gnu/lib",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Ok(entries) = std::fs::read_dir("/usr/lib") {
        let mut more: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter_map(|name| {
                if name.starts_with("gcc") {
                    Some(PathBuf::from("/usr/lib").join(name))
                } else if name.starts_with("llvm-") {
                    Some(PathBuf::from("/usr/lib").join(name).join("lib"))
                } else {
                    None
                }
            })
            .collect();
        more.sort();
        roots.extend(more);
    }
    roots
}

/// The `e_machine` of an `ET_DYN` file of one of the three targets, read
/// from its first bytes.
fn dso_machine(path: &Path) -> Option<u16> {
    use std::io::Read as _;
    let mut head = [0u8; 64];
    let mut file = std::fs::File::open(path).ok()?;
    let read = file.read(&mut head).ok()?;
    let header = FileHeader::parse(&head[..read]).ok()?;
    let ok = header.ehdr.e_type == ET_DYN
        && matches!(
            (header.class, header.ehdr.e_machine),
            (Class::Elf64, EM_AARCH64 | EM_X86_64) | (Class::Elf32, EM_386)
        );
    ok.then_some(header.ehdr.e_machine)
}

/// Every `ET_DYN` file of machine `e_machine` under the corpus roots, each
/// inode once (the first path in sorted order), sorted.
fn corpus(e_machine: u16) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs: Vec<PathBuf> = corpus_roots();
    let mut seen_dirs = BTreeSet::new();
    while let Some(dir) = dirs.pop() {
        let Ok(meta) = std::fs::metadata(&dir) else { continue };
        if !seen_dirs.insert((meta.dev(), meta.ino())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() || kind.is_symlink() {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut inodes = BTreeSet::new();
    files
        .into_iter()
        .filter(|path| {
            std::fs::metadata(path).is_ok_and(|m| m.is_file() && inodes.insert((m.dev(), m.ino())))
        })
        .filter(|path| dso_machine(path) == Some(e_machine))
        .collect()
}

fn machine_label(e_machine: u16) -> &'static str {
    support::machine_name(e_machine)
}

/// §6.13: the corpus of one target, every file through both tools.
/// A file whose dynamic table alone does not give its symbol count
/// (§6.6): stripped of its sections, `readelf -D` cannot list its
/// symbols, `llvm-readobj` lists none, and the reader refuses by name.
fn unhashed_agrees(tools: &Tools, path: &Path) {
    let at = path.display();
    let objcopy = tool("llvm-objcopy").expect("llvm-objcopy");
    let scratch = Scratch::new("dso-unhashed");
    let out = scratch.join("stripped");
    let mut command = Command::new(&objcopy);
    command.arg("--strip-sections").arg(path).arg(&out);
    let outcome = run_with(&mut command, &[], TIMEOUT);
    assert!(outcome.success(), "{at}: {}", outcome.stderr_text());
    let output = run_tool(&tools.readelf, &["-D", "-W", "-s"], &out, TIMEOUT);
    assert!(
        output.contains("Dynamic symbol information is not available"),
        "{at}: readelf -D lists symbols the reader refuses to count:\n{output}"
    );
    let output = run_tool(&tools.readobj, &["--dyn-syms"], &out, TIMEOUT);
    assert!(output.contains("DynamicSymbols [\n]"), "{at}: llvm-readobj lists symbols:\n{output}");
    let bytes = std::fs::read(&out).expect("the stripped file");
    let object = SharedObject::parse(&bytes).expect("parse");
    assert!(matches!(object.symbols(), Err(Error::UnhashedSymbolCount { .. })), "{at}");
}

fn corpus_agrees(e_machine: u16) {
    let Some(tools) = tools() else { return };
    let files = corpus(e_machine);
    if files.is_empty() {
        assert!(!tools_required(), "no shared objects for {}", machine_label(e_machine));
        return;
    }
    let started = Instant::now();
    let (mut readelf, mut llvm) = (Counts::default(), Counts::default());
    let mut unhashed = Vec::new();
    for path in &files {
        let subject = Subject::read(path);
        readelf.add(check_readelf(&tools.readelf, &subject));
        llvm.add(check_llvm(&tools.readobj, &subject));
        if subject.unhashed {
            unhashed_agrees(&tools, path);
            unhashed.push(path.display().to_string());
        }
    }
    eprintln!(
        "{} corpus: {} files in {:?}\n  readelf      {readelf:?}\n  llvm-readobj {llvm:?}\n  \
         no symbol count by the dynamic table alone (GNU hash hashes nothing, no DT_HASH): \
         {unhashed:?}",
        machine_label(e_machine),
        files.len(),
        started.elapsed()
    );
}

#[test]
fn s6_13_corpus_aarch64() {
    corpus_agrees(EM_AARCH64);
}

#[test]
fn s6_13_corpus_x86_64() {
    corpus_agrees(EM_X86_64);
}

#[test]
fn s6_13_corpus_i386() {
    corpus_agrees(EM_386);
}

// ---- section-stripped files -------------------------------------------------------

/// Compares `readelf -D -W -s` (the dynamic table's symbols) with the view.
fn check_readelf_dynamic_symbols(readelf: &Path, subject: &Subject) -> u64 {
    let at = subject.path.display().to_string();
    let output = run_tool(readelf, &["-D", "-W", "-s"], &subject.path, TIMEOUT);
    let blocks = readelf_blocks(&output);
    let Some((heading, lines)) =
        blocks.iter().find(|(heading, _)| heading.starts_with("Symbol table for image"))
    else {
        assert!(subject.view.symbols.is_empty(), "{at}: readelf -D shows no symbols");
        return 0;
    };
    let expected = format!(
        "Symbol table for image contains {} entr{}:",
        subject.view.symbols.len(),
        if subject.view.symbols.len() == 1 { "y" } else { "ies" }
    );
    assert_eq!(*heading, expected, "{at}");
    readelf_symbols(subject, lines, &at)
}

/// §6.3, §6.13: `llvm-objcopy --strip-sections` of `path`; the reader's view
/// of the result, by the dynamic table, is the view of the original by
/// its sections, and agrees with `readelf -D`.
fn stripped_agrees(tools: &Tools, objcopy: &Path, scratch: &Scratch, path: &Path) -> u64 {
    let original = Subject::read(path);
    let out = scratch.join(format!(
        "{}.stripped",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let mut command = Command::new(objcopy);
    command.arg("--strip-sections").arg(path).arg(&out);
    let outcome = run_with(&mut command, &[], TIMEOUT);
    assert!(outcome.success(), "{}: {}", path.display(), outcome.stderr_text());
    let bytes = std::fs::read(&out).expect("the stripped file");
    let object = SharedObject::parse(&bytes).expect("parse");
    assert_eq!(object.entry(), Entry::Dynamic, "{}", path.display());
    if original.unhashed {
        // §6.6: refused by name, as by the original's dynamic table.
        assert!(
            matches!(object.symbols(), Err(Error::UnhashedSymbolCount { .. })),
            "{}",
            path.display()
        );
        std::fs::remove_file(&out).expect("remove the stripped file");
        return 0;
    }
    let stripped = Subject::of(&out, &bytes);
    assert_eq!(stripped.view, original.view, "{}: the stripped view differs", path.display());
    let shown = check_readelf_dynamic_symbols(&tools.readelf, &stripped);
    std::fs::remove_file(&out).expect("remove the stripped file");
    shown
}

/// Every `stride`-th file of the target's corpus (by sorted path), files
/// over 64 MiB left out.
fn stripped_sample(e_machine: u16, stride: usize) {
    let Some(tools) = tools() else { return };
    let Some(objcopy) = tool("llvm-objcopy") else { return };
    let scratch = Scratch::new("dso-stripped");
    let files: Vec<PathBuf> = corpus(e_machine)
        .into_iter()
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= 64 << 20))
        .step_by(stride)
        .collect();
    let mut symbols = 0;
    for path in &files {
        symbols += stripped_agrees(&tools, &objcopy, &scratch, path);
    }
    eprintln!(
        "{} stripped: {} files (every {stride}th), {symbols} symbols compared with readelf -D",
        machine_label(e_machine),
        files.len()
    );
}

#[test]
fn s6_13_stripped_aarch64() {
    stripped_sample(EM_AARCH64, 16);
}

#[test]
fn s6_13_stripped_x86_64() {
    stripped_sample(EM_X86_64, 2);
}

#[test]
fn s6_13_stripped_i386() {
    stripped_sample(EM_386, 2);
}

// ---- a versioned library, built here ------------------------------------------------

const LIB_SOURCE: &str = r#"
int foo_v1(void) { return 1; }
int foo_v2(void) { return 2; }
int old_impl(void) { return 3; }
__asm__(".symver foo_v1, foo@VER_1");
__asm__(".symver foo_v2, foo@@VER_2");
__asm__(".symver old_impl, old@VER_1");
extern int depfn(void) __attribute__((weak));
int call(void) { return depfn ? depfn() : 0; }
int data_obj = 5;
const int ro_obj = 7;
__thread int tls_var;
"#;

const LIB_MAP: &str = "
VER_1 { global: foo; old; data_obj; ro_obj; local: *; };
VER_2 { global: foo; call; tls_var; } VER_1;
";

const DEP_SOURCE: &str = "int depfn(void) { return 4; }\n";
const DEP_MAP: &str = "DEP_1 { global: depfn; local: *; };\n";

struct Target {
    name: &'static str,
    gcc: &'static str,
    triple: &'static str,
    bfd: &'static str,
}

const TARGETS: [Target; 3] = [
    Target {
        name: "aarch64",
        gcc: "aarch64-linux-gnu-gcc",
        triple: "aarch64-linux-gnu",
        bfd: "aarch64-linux-gnu-ld.bfd",
    },
    Target {
        name: "x86-64",
        gcc: "x86_64-linux-gnu-gcc",
        triple: "x86_64-linux-gnu",
        bfd: "x86_64-linux-gnu-ld.bfd",
    },
    Target {
        name: "i386",
        gcc: "i686-linux-gnu-gcc",
        triple: "i686-linux-gnu",
        bfd: "i686-linux-gnu-ld.bfd",
    },
];

/// The target's C compiler: the prefixed driver, or the native one when it
/// is the target's.
fn find_gcc(target: &Target) -> Option<PathBuf> {
    if let Some(found) = mortar_testutil::find_tool(target.gcc) {
        return Some(found);
    }
    let native = mortar_testutil::find_tool("gcc")?;
    let machine = run_with(Command::new(&native).arg("-dumpmachine"), &[], DEFAULT_TIMEOUT);
    machine.stdout_text().trim().starts_with(target.triple).then_some(native)
}

fn run_ok(command: &mut Command, what: &str) {
    let outcome = run_with(command, &[], DEFAULT_TIMEOUT);
    assert!(outcome.success(), "{what}: {}{}", outcome.stdout_text(), outcome.stderr_text());
}

/// §6.13: the versioned library, linked by each linker with each hash
/// style, read by both paths and both tools.
fn versioned_library(target: &Target) {
    let Some(tools) = tools() else { return };
    let Some(objcopy) = tool("llvm-objcopy") else { return };
    let Some(gcc) = find_gcc(target) else {
        assert!(!tools_required(), "no gcc for {} (MORTAR_REQUIRE_TOOLS=1)", target.name);
        return;
    };
    let linkers: Vec<(&str, PathBuf)> = [("bfd", target.bfd), ("lld", "ld.lld"), ("mold", "mold")]
        .into_iter()
        .filter_map(|(label, name)| Some((label, tool(name)?)))
        .collect();
    let scratch = Scratch::new("dso-versioned");
    let lib_c = scratch.write("lib.c", LIB_SOURCE);
    let dep_c = scratch.write("dep.c", DEP_SOURCE);
    let lib_map = scratch.write("lib.map", LIB_MAP);
    let dep_map = scratch.write("dep.map", DEP_MAP);
    let (lib_o, dep_o) = (scratch.join("lib.o"), scratch.join("dep.o"));
    for (source, object) in [(&lib_c, &lib_o), (&dep_c, &dep_o)] {
        run_ok(
            Command::new(&gcc).args(["-fPIC", "-O1", "-c"]).arg(source).arg("-o").arg(object),
            "compile",
        );
    }
    let (mut readelf, mut llvm) = (Counts::default(), Counts::default());
    let mut built = 0;
    for (label, linker) in &linkers {
        for style in ["gnu", "sysv", "both"] {
            let dep = scratch.join(format!("libdep-{label}-{style}.so"));
            let lib = scratch.join(format!("libt-{label}-{style}.so"));
            let hash = format!("--hash-style={style}");
            run_ok(
                Command::new(linker)
                    .args(["-shared", "-soname", "libdep.so", &hash, "--version-script"])
                    .arg(&dep_map)
                    .arg(&dep_o)
                    .arg("-o")
                    .arg(&dep),
                &format!("{label} {style}: libdep"),
            );
            run_ok(
                Command::new(linker)
                    .args(["-shared", "-soname", "libt.so", &hash, "--version-script"])
                    .arg(&lib_map)
                    .arg(&lib_o)
                    .arg(&dep)
                    .arg("-o")
                    .arg(&lib),
                &format!("{label} {style}: libt"),
            );
            let subject = Subject::read(&lib);
            let at = format!("{} {label} {style}", target.name);
            check_built(&subject, label, &at);
            readelf.add(check_readelf(&tools.readelf, &subject));
            llvm.add(check_llvm(&tools.readobj, &subject));
            stripped_agrees(&tools, &objcopy, &scratch, &lib);
            built += 1;
        }
    }
    eprintln!(
        "{} versioned library: {built} links ({})\n  readelf      {readelf:?}\n  llvm-readobj {llvm:?}",
        target.name,
        linkers.iter().map(|(l, _)| *l).collect::<Vec<_>>().join(", ")
    );
}

/// What the library must say whatever linked it.
fn check_built(subject: &Subject, linker: &str, at: &str) {
    let view = &subject.view;
    assert_eq!(view.soname.as_deref(), Some("libt.so"), "{at}");
    assert_eq!(view.needed, ["libdep.so"], "{at}");
    let find = |name: &str, default: bool| {
        view.symbols
            .iter()
            .find(|s| {
                s.name == name
                    && s.version.as_ref().is_some_and(|v| v.default == default && v.kind == 'D')
            })
            .unwrap_or_else(|| panic!("{at}: no {name} with default {default}"))
    };
    let foo_1 = find("foo", false);
    assert_eq!(foo_1.version.as_ref().and_then(|v| v.name.as_deref()), Some("VER_1"), "{at}");
    let foo_2 = find("foo", true);
    assert_eq!(foo_2.version.as_ref().and_then(|v| v.name.as_deref()), Some("VER_2"), "{at}");
    let old = find("old", false);
    assert!(old.version.as_ref().is_some_and(|v| v.hidden), "{at}: old is a hidden version");
    let depfn =
        view.symbols.iter().find(|s| s.name == "depfn").unwrap_or_else(|| panic!("{at}: no depfn"));
    assert_eq!(depfn.sym.binding(), 2, "{at}: depfn is a weak reference");
    let version = depfn.version.as_ref().expect("a version");
    assert_eq!(
        (version.kind, version.name.as_deref(), version.file.as_deref()),
        ('N', Some("DEP_1"), Some("libdep.so")),
        "{at}"
    );
    let ver_2 = view.definitions.iter().find(|d| d.names[0] == "VER_2");
    let parents = ver_2.map_or_else(|| panic!("{at}: no VER_2"), |d| d.names[1..].to_vec());
    // GNU ld records that VER_2 inherits from VER_1; lld and mold do not.
    let expected: &[&str] = if linker == "bfd" { &["VER_1"] } else { &[] };
    assert_eq!(parents, expected, "{at}: the parents of VER_2");
}

#[test]
fn s6_13_versioned_library_aarch64() {
    versioned_library(&TARGETS[0]);
}

#[test]
fn s6_13_versioned_library_x86_64() {
    versioned_library(&TARGETS[1]);
}

#[test]
fn s6_13_versioned_library_i386() {
    versioned_library(&TARGETS[2]);
}
