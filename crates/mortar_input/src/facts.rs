//! What a file is, from its bytes (`spec/input.md` §2), and what the walk
//! needs to know of it: the per-file analysis that runs in parallel
//! (§12.1). Nothing here depends on the mention that named the file, so a
//! file named twice is analysed once.

use mortar_core::intern::SymName;
use mortar_diag::{Diagnostic, RefusalClass};
use mortar_elf::archive::{self, Archive, Location};
use mortar_elf::consts::{
    DF_1_PIE, EI_CLASS, EI_DATA, ELFDATA2MSB, EM_386, EM_AARCH64, EM_X86_64, ET_CORE, ET_DYN,
    ET_EXEC, ET_REL, SHT_GROUP, STT_SECTION, em_name, et_name,
};
use mortar_elf::{
    Class, Error, FileHeader, Machine, Object, Section, SharedObject, SymbolSection, SymbolTable,
    View,
};
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
#[derive(Clone, Debug)]
pub struct ObjectFacts<'s> {
    pub object: Object<'s>,
    /// `e_shnum`, section 0 included.
    pub sections: u32,
    /// The non-local symbol-table entries (the `sym_map` span, §6).
    pub globals: u32,
    /// `SHT_GROUP` sections.
    pub groups: u32,
    /// The names of the non-local entries, in table order, and the
    /// effective signatures of the groups, in section order: what the
    /// interner numbers (§6), read here once (WP-032a: the interner's two
    /// passes decoded them again, a group's signature at ~50 ns).
    pub keys: ObjectKeys<'s>,
}

/// The keys of an object's mentions, as its symbol-table pass read them.
#[derive(Clone, Debug, Default)]
pub struct ObjectKeys<'s> {
    pub names: Vec<&'s [u8]>,
    pub signatures: Vec<&'s [u8]>,
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
    /// What each version index names (`spec/resolve.md` §9.2), for the
    /// keys of the dynamic symbols.
    pub versions: VersionSlots<'s>,
}

/// What a version index of a shared object names: index `i` is slot `i`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerSlot<'s> {
    /// No version (`VER_NDX_LOCAL`, `VER_NDX_GLOBAL`, the base definition).
    None,
    /// A version the object defines.
    Defined(&'s [u8]),
    /// A version the object needs from another.
    Needed(&'s [u8]),
}

/// The version slots of a shared object; empty when it has no
/// `.gnu.version` (every key is then unversioned).
#[derive(Clone, Debug, Default)]
pub struct VersionSlots<'s> {
    pub slots: Box<[VerSlot<'s>]>,
}

impl<'s> VersionSlots<'s> {
    /// The key of dynamic symbol `index` named `name` (§6, `spec/model.md`
    /// §2.3, `spec/resolve.md` §9.2): a hidden defined version or a needed
    /// version is part of the key; a default one is not.
    #[must_use]
    pub fn key(&self, dso: &SharedObject<'s>, index: u32, name: &'s [u8]) -> SymName<'s> {
        let versym = dso.versym().ok().flatten().and_then(|t| t.get(index as usize));
        let Some(versym) = versym else { return SymName::plain(name) };
        match self.slots.get(usize::from(versym.index())) {
            Some(VerSlot::Defined(v)) if versym.hidden() => SymName::versioned(name, v),
            Some(VerSlot::Needed(v)) => SymName::versioned(name, v),
            _ => SymName::plain(name),
        }
    }
}

/// The version slots of `dso`, every global dynamic symbol's index
/// checked to name exactly one version.
fn version_slots<'s>(
    dso: &SharedObject<'s>,
    table: Option<&SymbolTable<'s>>,
) -> Result<VersionSlots<'s>, String> {
    let e = |e: Error| e.to_string();
    let Some(versym) = dso.versym().map_err(e)? else { return Ok(VersionSlots::default()) };
    let mut slots: Vec<Option<VerSlot<'s>>> = vec![Some(VerSlot::None), Some(VerSlot::None)];
    let mut put = |index: u16, slot: VerSlot<'s>| -> Result<(), String> {
        let i = usize::from(index);
        if slots.len() <= i {
            slots.resize(i + 1, None);
        }
        if i >= 2 && slots[i].is_some() {
            return Err(Error::AmbiguousVersionIndex { index }.to_string());
        }
        slots[i] = Some(slot);
        Ok(())
    };
    for d in dso.version_definitions().map_err(e)?.into_iter().flatten() {
        let d = d.map_err(e)?;
        let slot = if d.is_base() { VerSlot::None } else { VerSlot::Defined(d.name().map_err(e)?) };
        put(d.index(), slot)?;
    }
    for n in dso.version_needs().map_err(e)?.into_iter().flatten() {
        for entry in n.map_err(e)?.entries().map_err(e)? {
            let entry = entry.map_err(e)?;
            put(entry.index(), VerSlot::Needed(entry.name().map_err(e)?))?;
        }
    }
    if let Some(table) = table {
        if versym.len() < table.len() as usize {
            return Err(format!(
                ".gnu.version has {} entries for {} dynamic symbols",
                versym.len(),
                table.len()
            ));
        }
        for (index, _) in table.globals() {
            let v = versym.get(index.0 as usize).map_or(0, mortar_elf::Versym::index);
            if slots.get(usize::from(v)).copied().flatten().is_none() {
                return Err(Error::UnknownVersionIndex { symbol: index.0, index: v }.to_string());
            }
        }
    }
    Ok(VersionSlots { slots: slots.into_iter().map(|s| s.unwrap_or(VerSlot::None)).collect() })
}

/// The effective signature of a `SHT_GROUP` section (`spec/model.md`
/// §2.3, `spec/resolve.md` §6.1): its signature symbol's name, or, for an
/// `STT_SECTION` symbol, the name of the section it refers to.
pub fn effective_signature<'s>(
    object: &Object<'s>,
    table: Option<&SymbolTable<'s>>,
    group: &Section<'s>,
) -> Result<&'s [u8], Error> {
    let group_info = group.group()?;
    let table = table.ok_or(Error::SectionIndexOutOfRange { index: 0, count: 0 })?;
    let index = group_info.signature();
    let sym = table.get(index)?;
    if sym.sym_type() == STT_SECTION
        && let SymbolSection::Section(section) = table.section_of(index, &sym)?
    {
        return object.section(section)?.name();
    }
    table.name(&sym)
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
    /// A malformed symbol index (§7.1: a warning, the index is not used).
    pub index_error: Option<String>,
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

/// Sniffs and analyses a file (§2.1, §2.2): one in input position, or,
/// with `member`, an archive member (§7.3: rows 1 and 8 do not apply).
/// An archive's inline members are listed but not analysed here
/// ([`analyse_member`] does that, in parallel).
///
/// `view` is the file's bytes as far as the store holds them (§11.3): the
/// whole file, or the staged store's parts, which hold everything this
/// analysis and the readers after it read (`spec/landed/input/WP-021b.md`).
/// A structure that is not resident is an internal error of the store,
/// reported as a malformed file, never a panic.
#[must_use]
pub fn analyse(bytes: &[u8], member: bool) -> Facts<'_> {
    analyse_view(View::whole(bytes), member)
}

/// [`analyse`] over the store's view of a file (§11.3).
#[must_use]
pub fn analyse_view(view: View<'_>, member: bool) -> Facts<'_> {
    let head = FileHeader::head(&view).unwrap_or_default();
    let ident = ElfIdent::of(head);
    let kind = sniff(view, head, member);
    Facts { ident, kind }
}

/// Analyses an archive member's bytes (§7.3).
#[must_use]
pub fn analyse_member(bytes: &[u8]) -> Facts<'_> {
    analyse(bytes, true)
}

/// The whole of a file the analysis reads as one slice (a script, a
/// shared object), or the store's internal error.
fn whole(view: View<'_>) -> Result<&[u8], Kind<'_>> {
    view.as_slice().ok_or_else(|| {
        Kind::Bad(Bad::Malformed(Error::NotResident { offset: 0, size: view.len() }.to_string()))
    })
}

fn sniff<'s>(view: View<'s>, bytes: &'s [u8], member: bool) -> Kind<'s> {
    if view.is_empty() {
        return if member { Kind::NotObject("empty") } else { Kind::Empty };
    }
    if bytes.starts_with(b"\x7fELF") {
        return elf(view, bytes, member);
    }
    if Archive::sniff(bytes).is_some() {
        if member {
            return Kind::NotObject("an archive");
        }
        return Kind::Archive(archive_facts(view));
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
    if view.len() >= 20 && starts(&[b"\x4C\x01", b"\x64\x86", b"\x64\xAA"]) {
        return Kind::Bad(Bad::format("COFF"));
    }
    if member {
        return Kind::NotObject("not an ELF file");
    }
    let bytes = match whole(view) {
        Ok(bytes) => bytes,
        Err(bad) => return bad,
    };
    if mortar_script::looks_like_script(bytes) {
        // The path is put in place by the walk (one file, many spellings).
        return Kind::Script(ScriptFacts { parsed: mortar_script::parse(bytes, "") });
    }
    Kind::Unknown
}

fn elf<'s>(view: View<'s>, bytes: &'s [u8], member: bool) -> Kind<'s> {
    let header = match FileHeader::parse(bytes) {
        Ok(header) => header,
        Err(e) => return Kind::Bad(header_error(e)),
    };
    match header.e_type {
        ET_REL => match Object::parse_view(view) {
            Ok(object) => object_facts(object),
            Err(e) => Kind::Bad(header_error(e)),
        },
        ET_DYN if member => Kind::NotObject("a shared object"),
        ET_EXEC if member => Kind::NotObject("an executable (ET_EXEC)"),
        ET_CORE if member => Kind::NotObject("a core file (ET_CORE)"),
        _ if member => Kind::NotObject("an ELF file that is not a relocatable object"),
        ET_DYN => match whole(view) {
            Ok(bytes) => shared_facts(bytes),
            Err(bad) => bad,
        },
        ET_EXEC => Kind::Bad(Bad::refused(RefusalClass::ObjectType, "ET_EXEC")),
        e_type => Kind::Bad(Bad::refused(RefusalClass::ObjectType, e_type_name(e_type))),
    }
}

/// The symbol-table pass of §6: every non-local entry's name read (the
/// interning slot of stage B), the `SHT_GROUP` signatures, the LTO check
/// of §2.2.
fn object_facts(object: Object<'_>) -> Kind<'_> {
    let mut keys = ObjectKeys::default();
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
        keys.names.reserve(table.globals().len());
        for (_, sym) in table.globals() {
            match table.name(&sym) {
                Ok(name) => keys.names.push(name),
                Err(e) => return Kind::Bad(Bad::Malformed(e.to_string())),
            }
        }
    }
    for section in object.sections() {
        if section.sh_type() != SHT_GROUP {
            continue;
        }
        match effective_signature(&object, table.as_ref(), &section) {
            Ok(signature) => keys.signatures.push(signature),
            Err(e) => return Kind::Bad(Bad::Malformed(e.to_string())),
        }
    }
    Kind::Object(Box::new(ObjectFacts {
        object,
        sections: object.section_count(),
        globals: count(keys.names.len()),
        groups: count(keys.signatures.len()),
        keys,
    }))
}

/// A count of at most `u32::MAX` (symbols: `TooManySymbols`; groups:
/// sections).
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
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
    let table = match dso.symbols() {
        Ok(table) => table,
        Err(e) => return malformed(e),
    };
    if let Some(table) = &table {
        for (_, sym) in table.globals() {
            if let Err(e) = table.name(&sym) {
                return malformed(e);
            }
            globals += 1;
        }
    }
    let versions = match version_slots(&dso, table.as_ref()) {
        Ok(v) => v,
        Err(e) => return Kind::Bad(Bad::Malformed(e)),
    };
    Kind::Shared(Box::new(SharedFacts { dso, soname, needed, runpath, rpath, globals, versions }))
}

/// Lists an archive's ordinary members (§7.1, §7.2); inline members are
/// analysed later.
fn archive_facts(view: View<'_>) -> ArchiveFacts<'_> {
    let archive = match Archive::parse_view(view) {
        Ok(a) => a,
        Err(e) => {
            let head = view.prefix(archive::THIN_MAGIC.len() as u64).unwrap_or_default();
            return ArchiveFacts {
                thin: head.starts_with(archive::THIN_MAGIC),
                members: Vec::new(),
                error: Some((e.to_string(), None)),
                index_error: None,
            };
        }
    };
    let thin = archive.kind() == archive::Kind::Thin;
    let mut members = Vec::new();
    let mut error = None;
    // The headers, not the members: a member's content need not be
    // resident (§11.3); the member pass reads what it needs of it.
    for m in archive.headers() {
        match m {
            Ok(m) => {
                let content = match m.location {
                    Location::Inline { offset } => {
                        MemberContent::Inline { offset, len: m.size, facts: None }
                    }
                    Location::External { path } => MemberContent::External { path },
                    Location::ExternalMember { archive_path, header_offset } => {
                        MemberContent::ExternalMember { archive_path, header_offset }
                    }
                };
                members.push(MemberFacts { header_offset: m.header_offset, name: m.name, content });
            }
            Err(e) => {
                error = Some((e.to_string(), None));
            }
        }
    }
    let index_error = match archive.symbol_index() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    ArchiveFacts { thin, members, error, index_error }
}
