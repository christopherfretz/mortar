//! What a file is, from its bytes (`spec/input.md` §2), and what the walk
//! needs to know of it: the per-file analysis that runs in parallel
//! (§12.1). Nothing here depends on the mention that named the file, so a
//! file named twice is analysed once.

use mortar_diag::{Diagnostic, RefusalClass};
use mortar_elf::archive::{self, Archive, Content};
use mortar_elf::consts::{
    DF_1_PIE, EI_CLASS, EI_DATA, ELFDATA2MSB, EM_386, EM_AARCH64, EM_X86_64, ET_CORE, ET_DYN,
    ET_EXEC, ET_REL, SHT_GROUP, em_name, et_name,
};
use mortar_elf::{Class, Error, FileHeader, Machine, Object, SharedObject};
use mortar_script::Script;

/// The name of the common symbol GCC writes into an LTO object without
/// code (§2.2).
const GNU_LTO_SLIM: &[u8] = b"__gnu_lto_slim\0";

/// The `e_ident` class byte, the data encoding, `e_type` and `e_machine`
/// of an ELF file, read raw in the file's own encoding: what the skip rule
/// of §4.4 compares, for every `e_type` and every machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElfIdent {
    pub class: u8,
    pub big_endian: bool,
    pub e_type: u16,
    pub e_machine: u16,
}

impl ElfIdent {
    /// The ident of `bytes` if they start with the ELF magic and hold the
    /// fields (20 bytes).
    #[must_use]
    pub fn of(bytes: &[u8]) -> Option<Self> {
        if !bytes.starts_with(b"\x7fELF") || bytes.len() < 20 {
            return None;
        }
        let big_endian = bytes[EI_DATA] == ELFDATA2MSB;
        let half = |at: usize| {
            let pair = [bytes[at], bytes[at + 1]];
            if big_endian { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) }
        };
        Some(Self { class: bytes[EI_CLASS], big_endian, e_type: half(16), e_machine: half(18) })
    }

    /// Whether the file is of `machine` (class and `e_machine`).
    #[must_use]
    pub fn is_of(&self, machine: Machine) -> bool {
        let class = match machine.class() {
            Class::Elf32 => 1,
            Class::Elf64 => 2,
        };
        self.class == class && self.e_machine == machine.e_machine()
    }

    /// The BFD name of the file's target, for `skipped-incompatible`
    /// (`elf32-i386`), or a description when it has none mortar knows.
    #[must_use]
    pub fn target_name(&self) -> String {
        let bits = match self.class {
            1 => "32",
            2 => "64",
            _ => "",
        };
        let endian = if self.big_endian { "big" } else { "little" };
        match (self.class, self.e_machine, self.big_endian) {
            (2, EM_AARCH64, false) => "elf64-littleaarch64".to_owned(),
            (2, EM_AARCH64, true) => "elf64-bigaarch64".to_owned(),
            (1, EM_AARCH64, false) => "elf32-littleaarch64".to_owned(),
            (2, EM_X86_64, false) => "elf64-x86-64".to_owned(),
            (1, EM_X86_64, false) => "elf32-x86-64".to_owned(),
            (1, EM_386, false) => "elf32-i386".to_owned(),
            (_, m, _) => match em_name(m) {
                Some(name) => format!("elf{bits}-{endian} {name}"),
                None => format!("elf{bits}-{endian} e_machine {m:#x}"),
            },
        }
    }
}

/// Why a file (or member) contributes nothing, decided from its bytes
/// alone: a refusal by name, or a reader's error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bad {
    /// Refused by name (§2.1 rows 4-7, §2.2, §2.4). `gnu_phrase`: the
    /// message carries GNU ld's `file not recognized: file format not
    /// recognized` after its own text (§13).
    Refused { class: RefusalClass, name: String, note: Option<&'static str>, gnu_phrase: bool },
    /// A structural error: the reader's text (§2.4, §6, §7.1).
    Malformed(String),
}

impl Bad {
    fn refused(class: RefusalClass, name: impl Into<String>) -> Self {
        Self::Refused { class, name: name.into(), note: None, gnu_phrase: false }
    }

    fn format(name: &str) -> Self {
        Self::Refused {
            class: RefusalClass::FileFormat,
            name: name.to_owned(),
            note: None,
            gnu_phrase: true,
        }
    }
}

/// A relocatable object: the reader and what its symbol-table pass found
/// (§6).
#[derive(Clone, Copy, Debug)]
pub struct ObjectFacts<'s> {
    pub object: Object<'s>,
    /// `e_shnum`, section 0 included.
    pub sections: u32,
    /// The non-local symbol-table entries (the `sym_map` span, §6).
    pub globals: u32,
    /// `SHT_GROUP` sections.
    pub groups: u32,
}

/// A shared object: what §8 needs of its dynamic table (§8.1).
#[derive(Clone, Debug)]
pub struct SharedFacts<'s> {
    pub dso: SharedObject<'s>,
    pub soname: Option<&'s [u8]>,
    /// `DT_NEEDED`, in table order.
    pub needed: Vec<&'s [u8]>,
    pub runpath: Option<&'s [u8]>,
    pub rpath: Option<&'s [u8]>,
    /// The non-local dynamic symbols.
    pub globals: u32,
}

/// One ordinary member of an archive (§7.2).
#[derive(Clone, Debug)]
pub struct MemberFacts<'s> {
    pub header_offset: u64,
    /// The member name; in a thin archive the stored path.
    pub name: &'s [u8],
    pub content: MemberContent<'s>,
}

/// Where a member's bytes are, and, for one stored in the archive, what
/// they are.
#[derive(Clone, Debug)]
pub enum MemberContent<'s> {
    /// In the archive at `offset`; `facts` filled by the member pass.
    Inline { offset: u64, len: u64, facts: Option<Box<Facts<'s>>> },
    /// Thin: the file at `path`.
    External { path: &'s [u8] },
    /// Thin, GNU's nested form: the member at `header_offset` of the
    /// archive at `archive_path`.
    ExternalMember { archive_path: &'s [u8], header_offset: u64 },
}

/// An archive: its ordinary members in archive order, and the reader's
/// error that ended them, if one did (§7.1).
#[derive(Clone, Debug)]
pub struct ArchiveFacts<'s> {
    pub thin: bool,
    pub members: Vec<MemberFacts<'s>>,
    /// The reader's error: `(text, header offset of the member it names)`.
    pub error: Option<(String, Option<u64>)>,
}

impl ArchiveFacts<'_> {
    /// The ident of the first member (in archive order) that is a
    /// relocatable object of any machine: what the skip rule and the
    /// target rule look at (§3.1, §4.4). `None` when the first such member
    /// is in another file (thin) and `thin_ident` cannot say, or there is
    /// none.
    pub fn first_relocatable(
        &self,
        thin_ident: impl Fn(&MemberContent<'_>) -> Option<Option<ElfIdent>>,
    ) -> FirstRel {
        for m in &self.members {
            let ident = match &m.content {
                MemberContent::Inline { facts, .. } => facts.as_ref().and_then(|f| f.ident),
                other => match thin_ident(other) {
                    Some(ident) => ident,
                    None => return FirstRel::Unknown,
                },
            };
            if let Some(ident) = ident.filter(|i| i.e_type == ET_REL) {
                return FirstRel::Found(ident);
            }
        }
        FirstRel::None
    }
}

/// [`ArchiveFacts::first_relocatable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstRel {
    Found(ElfIdent),
    None,
    /// A thin member that has not been read yet comes first.
    Unknown,
}

/// A linker script (§5): the parse, over the file's bytes.
#[derive(Clone, Debug)]
pub struct ScriptFacts {
    pub parsed: Result<Script, Box<Diagnostic>>,
}

/// What a file or member is.
#[derive(Clone, Debug)]
pub enum Kind<'s> {
    /// A file of length 0 (an empty script, §2.3).
    Empty,
    Object(Box<ObjectFacts<'s>>),
    Shared(Box<SharedFacts<'s>>),
    Archive(ArchiveFacts<'s>),
    Script(ScriptFacts),
    /// An archive member that is no relocatable object (§7.3): what it is.
    NotObject(&'static str),
    Bad(Bad),
    /// No format mortar can name (§2.1 row 9).
    Unknown,
}

/// The analysis of a file or member.
#[derive(Clone, Debug)]
pub struct Facts<'s> {
    /// The raw ELF ident, for every file that starts like an ELF file.
    pub ident: Option<ElfIdent>,
    pub kind: Kind<'s>,
}

/// §2.4: a reader's error as what the file is.
fn header_error(error: Error) -> Bad {
    match error {
        Error::BigEndian => Bad::refused(RefusalClass::FileFormat, "big-endian ELF"),
        Error::UnsupportedMachine { e_machine } => Bad::refused(
            RefusalClass::Machine,
            em_name(e_machine).map_or_else(|| format!("{e_machine:#x}"), str::to_owned),
        ),
        Error::UnsupportedClass { class, e_machine } => Bad::refused(
            RefusalClass::FileFormat,
            match (class, e_machine) {
                (Class::Elf32, EM_X86_64) => "x32".to_owned(),
                (Class::Elf32, EM_AARCH64) => "ILP32".to_owned(),
                (Class::Elf64, EM_386) => "ELF64 EM_386".to_owned(),
                (class, m) => format!("{class:?} e_machine {m:#x}"),
            },
        ),
        other => Bad::Malformed(other.to_string()),
    }
}

/// The name of an `e_type` mortar refuses (§2.2).
fn e_type_name(e_type: u16) -> String {
    et_name(e_type).map_or_else(|| format!("{e_type:#x}"), str::to_owned)
}

/// Sniffs and analyses `bytes` (§2.1, §2.2): a file in input position, or,
/// with `member`, an archive member (§7.3: rows 1 and 8 do not apply).
/// An archive's inline members are listed but not analysed here
/// ([`analyse_member`] does that, in parallel).
#[must_use]
pub fn analyse(bytes: &[u8], member: bool) -> Facts<'_> {
    let ident = ElfIdent::of(bytes);
    let kind = sniff(bytes, member);
    Facts { ident, kind }
}

/// Analyses an archive member's bytes (§7.3).
#[must_use]
pub fn analyse_member(bytes: &[u8]) -> Facts<'_> {
    analyse(bytes, true)
}

fn sniff(bytes: &[u8], member: bool) -> Kind<'_> {
    if bytes.is_empty() {
        return if member { Kind::NotObject("empty") } else { Kind::Empty };
    }
    if bytes.starts_with(b"\x7fELF") {
        return elf(bytes, member);
    }
    if Archive::sniff(bytes).is_some() {
        if member {
            return Kind::NotObject("an archive");
        }
        return Kind::Archive(archive_facts(bytes));
    }
    let starts = |magic: &[&[u8]]| magic.iter().any(|m| bytes.starts_with(m));
    if starts(&[b"BC\xC0\xDE", b"\xDE\xC0\x17\x0B"]) {
        return Kind::Bad(Bad::format("LLVM bitcode"));
    }
    if starts(&[
        b"\xFE\xED\xFA\xCE",
        b"\xFE\xED\xFA\xCF",
        b"\xCE\xFA\xED\xFE",
        b"\xCF\xFA\xED\xFE",
        b"\xCA\xFE\xBA\xBE",
    ]) {
        return Kind::Bad(Bad::format("Mach-O"));
    }
    if bytes.starts_with(b"\0asm") {
        return Kind::Bad(Bad::format("WebAssembly"));
    }
    if bytes.len() >= 20 && starts(&[b"\x4C\x01", b"\x64\x86", b"\x64\xAA"]) {
        return Kind::Bad(Bad::format("COFF"));
    }
    if member {
        return Kind::NotObject("not an ELF file");
    }
    if mortar_script::looks_like_script(bytes) {
        // The path is put in place by the walk (one file, many spellings).
        return Kind::Script(ScriptFacts { parsed: mortar_script::parse(bytes, "") });
    }
    Kind::Unknown
}

fn elf(bytes: &[u8], member: bool) -> Kind<'_> {
    let header = match FileHeader::parse(bytes) {
        Ok(header) => header,
        Err(e) => return Kind::Bad(header_error(e)),
    };
    match header.e_type {
        ET_REL => match Object::parse(bytes) {
            Ok(object) => object_facts(object),
            Err(e) => Kind::Bad(header_error(e)),
        },
        ET_DYN if member => Kind::NotObject("a shared object"),
        ET_EXEC if member => Kind::NotObject("an executable (ET_EXEC)"),
        ET_CORE if member => Kind::NotObject("a core file (ET_CORE)"),
        _ if member => Kind::NotObject("an ELF file that is not a relocatable object"),
        ET_DYN => shared_facts(bytes),
        ET_EXEC => Kind::Bad(Bad::refused(RefusalClass::ObjectType, "ET_EXEC")),
        e_type => Kind::Bad(Bad::refused(RefusalClass::ObjectType, e_type_name(e_type))),
    }
}

/// The symbol-table pass of §6: every non-local entry's name read (the
/// interning slot of stage B), the `SHT_GROUP` signatures, the LTO check
/// of §2.2.
fn object_facts(object: Object<'_>) -> Kind<'_> {
    let mut globals = 0u32;
    let mut groups = 0u32;
    let table = match object.symbols() {
        Ok(table) => table,
        Err(e) => return Kind::Bad(Bad::Malformed(e.to_string())),
    };
    if let Some(table) = &table {
        let strings = table.strings().bytes();
        let mut slim = false;
        for (_, sym) in table.iter() {
            let at = sym.st_name as usize;
            if strings.get(at..at + GNU_LTO_SLIM.len()) == Some(GNU_LTO_SLIM) {
                slim = true;
            }
        }
        if slim {
            return Kind::Bad(Bad::Refused {
                class: RefusalClass::FileFormat,
                name: "GCC LTO object".to_owned(),
                note: Some(
                    "mortar does not perform link-time optimization; compile without -flto, or with -ffat-lto-objects",
                ),
                gnu_phrase: false,
            });
        }
        for (_, sym) in table.globals() {
            if let Err(e) = table.name(&sym) {
                return Kind::Bad(Bad::Malformed(e.to_string()));
            }
            globals += 1;
        }
    }
    for section in object.sections() {
        if section.sh_type() != SHT_GROUP {
            continue;
        }
        let signature = section.group().and_then(|group| {
            let table =
                table.as_ref().ok_or(Error::SectionIndexOutOfRange { index: 0, count: 0 })?;
            let sym = table.get(group.signature())?;
            table.name(&sym)
        });
        if let Err(e) = signature {
            return Kind::Bad(Bad::Malformed(e.to_string()));
        }
        groups += 1;
    }
    Kind::Object(Box::new(ObjectFacts {
        object,
        sections: object.section_count(),
        globals,
        groups,
    }))
}

/// §8.1: what is read of a shared object, and its refusals.
fn shared_facts(bytes: &[u8]) -> Kind<'_> {
    let dso = match SharedObject::parse(bytes) {
        Ok(dso) => dso,
        Err(e) => return Kind::Bad(header_error(e)),
    };
    if dso.section_table().is_empty() {
        return Kind::Bad(Bad::refused(
            RefusalClass::Feature,
            "shared object without section headers",
        ));
    }
    let malformed = |e: Error| Kind::Bad(Bad::Malformed(e.to_string()));
    let dynamic = match dso.dynamic() {
        Ok(Some(dynamic)) => dynamic,
        Ok(None) => {
            return Kind::Bad(Bad::Malformed("a shared object without a dynamic table".to_owned()));
        }
        Err(e) => return malformed(e),
    };
    match dynamic.flags_1() {
        Ok(flags) if flags & DF_1_PIE != 0 => {
            return Kind::Bad(Bad::Refused {
                class: RefusalClass::ObjectType,
                name: "DF_1_PIE".to_owned(),
                note: Some("a position-independent executable cannot be linked against"),
                gnu_phrase: false,
            });
        }
        Ok(_) => {}
        Err(e) => return malformed(e),
    }
    let soname = match dynamic.soname() {
        Ok(soname) => soname,
        Err(e) => return malformed(e),
    };
    let runpath = match dynamic.runpath() {
        Ok(v) => v,
        Err(e) => return malformed(e),
    };
    let rpath = match dynamic.rpath() {
        Ok(v) => v,
        Err(e) => return malformed(e),
    };
    let mut needed = Vec::new();
    for name in dynamic.needed() {
        match name {
            Ok(name) => needed.push(name),
            Err(e) => return malformed(e),
        }
    }
    let mut globals = 0u32;
    match dso.symbols() {
        Ok(Some(table)) => {
            for (_, sym) in table.globals() {
                if let Err(e) = table.name(&sym) {
                    return malformed(e);
                }
                globals += 1;
            }
        }
        Ok(None) => {}
        Err(e) => return malformed(e),
    }
    Kind::Shared(Box::new(SharedFacts { dso, soname, needed, runpath, rpath, globals }))
}

/// Lists an archive's ordinary members (§7.1, §7.2); inline members are
/// analysed later.
fn archive_facts(bytes: &[u8]) -> ArchiveFacts<'_> {
    let archive = match Archive::parse(bytes) {
        Ok(a) => a,
        Err(e) => {
            // A malformed symbol index is a warning once `Archive::parse`
            // can skip it (spec/archive.md change of spec/input.md §17,
            // WP-020d); until then every reader error is fatal.
            return ArchiveFacts {
                thin: bytes.starts_with(archive::THIN_MAGIC),
                members: Vec::new(),
                error: Some((e.to_string(), None)),
            };
        }
    };
    let thin = archive.kind() == archive::Kind::Thin;
    let mut members = Vec::new();
    let mut error = None;
    for m in archive.members() {
        match m {
            Ok(m) => {
                let content = match m.content() {
                    Content::Inline { offset, data } => {
                        MemberContent::Inline { offset, len: data.len() as u64, facts: None }
                    }
                    Content::External { path } => MemberContent::External { path },
                    Content::ExternalMember { archive_path, header_offset } => {
                        MemberContent::ExternalMember { archive_path, header_offset }
                    }
                };
                members.push(MemberFacts {
                    header_offset: m.header_offset(),
                    name: m.name(),
                    content,
                });
            }
            Err(e) => {
                error = Some((e.to_string(), None));
            }
        }
    }
    ArchiveFacts { thin, members, error }
}
