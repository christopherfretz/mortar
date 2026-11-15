//! Test support: an owned [`View`] filled by hand (the rules' tests) or
//! from real objects and archives (the differential tests).
#![allow(dead_code)]

pub mod inputs;

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use mortar_testutil::run_with;

use mortar_core::{ArchiveId, ComdatId, FileId, OptId, SymbolId};
use mortar_elf::archive::Archive;
use mortar_elf::consts::{
    GRP_COMDAT, SHF_ALLOC, SHF_EXECINSTR, SHF_GROUP, SHF_TLS, SHF_WRITE, SHT_GROUP, SHT_NOBITS,
    SHT_PROGBITS, STB_GLOBAL, STB_WEAK, STT_FUNC, STT_NOTYPE, STT_OBJECT, STT_SECTION, STT_TLS,
    STV_DEFAULT,
};
use mortar_elf::object::Object;
use mortar_elf::{Machine, SectionIndex, SymbolIndex, SymbolSection};
use mortar_resolve::view::{FileInfo, FileKind, Groups, Mention, SectionInfo, View};

pub const TEXT: u32 = 1;
pub const DATA: u32 = 2;
pub const BSS: u32 = 3;
pub const TDATA: u32 = 4;

/// A section of a model file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MSection {
    pub name: Vec<u8>,
    pub sh_type: u32,
    pub flags: u64,
    pub entsize: u64,
    pub addralign: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
}

impl MSection {
    pub fn new(name: &str, sh_type: u32, flags: u64) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            sh_type,
            flags,
            entsize: 0,
            addralign: 8,
            size: 8,
            link: 0,
            info: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MFile {
    pub info: FileInfo,
    pub label: String,
    /// For archive members: the member's name.
    pub member: Option<String>,
    pub mentions: Vec<Mention>,
    pub groups: Groups,
    pub sections: Vec<MSection>,
    next_index: u32,
}

/// An owned view.
#[derive(Debug, Clone)]
pub struct Model {
    pub machine: Machine,
    pub files: Vec<MFile>,
    names: Vec<Vec<u8>>,
    index: HashMap<Vec<u8>, SymbolId>,
    sigs: HashMap<Vec<u8>, ComdatId>,
    archives: u32,
    /// `(path, member, ordinal)` to the `source` of its first file.
    sources: HashMap<(String, Vec<u8>, u32), u32>,
}

impl Model {
    pub fn new(machine: Machine) -> Self {
        let mut m = Self {
            machine,
            files: Vec::new(),
            names: Vec::new(),
            index: HashMap::new(),
            sigs: HashMap::new(),
            archives: 0,
            sources: HashMap::new(),
        };
        m.push_file(FileKind::Internal, "<internal>", Vec::new());
        m
    }

    fn push_file(&mut self, kind: FileKind, label: &str, sections: Vec<MSection>) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(MFile {
            info: FileInfo {
                kind,
                archive: OptId::NONE,
                group: None,
                sections: sections.len() as u32,
                source: id.0,
            },
            label: label.to_owned(),
            member: None,
            mentions: Vec::new(),
            groups: Groups::default(),
            sections,
            next_index: 1,
        });
        id
    }

    fn default_sections() -> Vec<MSection> {
        let mut tdata = MSection::new(".tdata", SHT_PROGBITS, SHF_ALLOC | SHF_WRITE | SHF_TLS);
        tdata.addralign = 4;
        vec![
            MSection { addralign: 0, size: 0, ..MSection::new("", 0, 0) },
            MSection::new(".text", SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR),
            MSection::new(".data", SHT_PROGBITS, SHF_ALLOC | SHF_WRITE),
            MSection::new(".bss", SHT_NOBITS, SHF_ALLOC | SHF_WRITE),
            tdata,
        ]
    }

    /// A named object with `.text`, `.data`, `.bss`, `.tdata`.
    pub fn obj(&mut self, label: &str) -> FileId {
        self.push_file(FileKind::Object, label, Self::default_sections())
    }

    /// A new archive mention; its members follow with [`Model::member`].
    pub fn archive(&mut self) -> ArchiveId {
        self.archives += 1;
        ArchiveId(self.archives - 1)
    }

    /// A lazy member of `archive`.
    pub fn member(&mut self, archive: ArchiveId, label: &str) -> FileId {
        let f = self.push_file(FileKind::LazyMember, label, Self::default_sections());
        self.files[f.0 as usize].info.archive = OptId::some(archive);
        f
    }

    /// A `--whole-archive` member of `archive`.
    pub fn whole(&mut self, archive: ArchiveId, label: &str) -> FileId {
        let f = self.member(archive, label);
        self.files[f.0 as usize].info.kind = FileKind::WholeMember;
        f
    }

    pub fn set_group(&mut self, file: FileId, group: u32) {
        self.files[file.0 as usize].info.group = Some(group);
    }

    /// Makes `file` the same source as `like` (one archive mentioned twice).
    pub fn same_source(&mut self, file: FileId, like: FileId) {
        self.files[file.0 as usize].info.source = self.files[like.0 as usize].info.source;
    }

    pub fn set_kind(&mut self, file: FileId, kind: FileKind) {
        self.files[file.0 as usize].info.kind = kind;
    }

    /// The key row named `name` (made if new).
    pub fn key(&mut self, name: &str) -> SymbolId {
        self.key_bytes(name.as_bytes())
    }

    pub fn key_bytes(&mut self, name: &[u8]) -> SymbolId {
        if let Some(&k) = self.index.get(name) {
            return k;
        }
        let k = SymbolId(self.names.len() as u32);
        self.names.push(name.to_vec());
        self.index.insert(name.to_vec(), k);
        k
    }

    /// The row of `name`.
    pub fn id(&self, name: &str) -> SymbolId {
        *self.index.get(name.as_bytes()).unwrap_or_else(|| panic!("no key {name}"))
    }

    pub fn try_id(&self, name: &str) -> Option<SymbolId> {
        self.index.get(name.as_bytes()).copied()
    }

    pub fn name(&self, key: SymbolId) -> String {
        String::from_utf8_lossy(&self.names[key.0 as usize]).into_owned()
    }

    /// The signature `name`, if a group has it.
    pub fn find_sig(&self, name: &[u8]) -> Option<ComdatId> {
        self.sigs.get(name).copied()
    }

    /// The row of `name` as bytes.
    pub fn try_id_bytes(&self, name: &[u8]) -> Option<SymbolId> {
        self.index.get(name).copied()
    }

    pub fn sig(&mut self, name: &[u8]) -> ComdatId {
        let n = self.sigs.len() as u32;
        *self.sigs.entry(name.to_vec()).or_insert(ComdatId(n))
    }

    /// Adds a mention; `name` with `@@V` also defines `name@V` (§9.4).
    #[allow(clippy::too_many_arguments)]
    pub fn sym(
        &mut self,
        file: FileId,
        name: &str,
        binding: u8,
        sym_type: u8,
        place: SymbolSection,
        value: u64,
        size: u64,
    ) -> SymbolIndex {
        let index = SymbolIndex(self.files[file.0 as usize].next_index);
        self.sym_at(
            file,
            index,
            name.as_bytes(),
            binding,
            sym_type,
            STV_DEFAULT,
            place,
            value,
            size,
        );
        index
    }

    #[allow(clippy::too_many_arguments)]
    fn sym_at(
        &mut self,
        file: FileId,
        index: SymbolIndex,
        name: &[u8],
        binding: u8,
        sym_type: u8,
        visibility: u8,
        place: SymbolSection,
        value: u64,
        size: u64,
    ) {
        let (key, also) = if let Some(at) = find(name, b"@@") {
            let plain = name[..at].to_vec();
            let mut versioned = plain.clone();
            versioned.push(b'@');
            versioned.extend_from_slice(&name[at + 2..]);
            let key = self.key_bytes(&plain);
            (key, OptId::some(self.key_bytes(&versioned)))
        } else {
            (self.key_bytes(name), OptId::NONE)
        };
        let f = &mut self.files[file.0 as usize];
        f.next_index = f.next_index.max(index.0 + 1);
        f.mentions.push(Mention {
            index,
            key,
            also,
            binding,
            sym_type,
            visibility,
            place,
            value,
            size,
        });
    }

    pub fn set_visibility(&mut self, file: FileId, index: SymbolIndex, visibility: u8) {
        let m = self.files[file.0 as usize].mentions.iter_mut().find(|m| m.index == index).unwrap();
        m.visibility = visibility;
    }

    pub fn def(&mut self, f: FileId, name: &str) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, STT_FUNC, SymbolSection::Section(SectionIndex(TEXT)), 0, 4)
    }
    pub fn def_in(&mut self, f: FileId, name: &str, section: u32) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, STT_FUNC, SymbolSection::Section(SectionIndex(section)), 0, 4)
    }
    pub fn weak(&mut self, f: FileId, name: &str) -> SymbolIndex {
        self.sym(f, name, STB_WEAK, STT_FUNC, SymbolSection::Section(SectionIndex(TEXT)), 0, 4)
    }
    pub fn data(&mut self, f: FileId, name: &str, size: u64) -> SymbolIndex {
        self.sym(
            f,
            name,
            STB_GLOBAL,
            STT_OBJECT,
            SymbolSection::Section(SectionIndex(DATA)),
            0,
            size,
        )
    }
    pub fn weak_data(&mut self, f: FileId, name: &str, size: u64) -> SymbolIndex {
        self.sym(f, name, STB_WEAK, STT_OBJECT, SymbolSection::Section(SectionIndex(DATA)), 0, size)
    }
    pub fn tls(&mut self, f: FileId, name: &str) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, STT_TLS, SymbolSection::Section(SectionIndex(TDATA)), 0, 4)
    }
    pub fn undef(&mut self, f: FileId, name: &str) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, STT_NOTYPE, SymbolSection::Undefined, 0, 0)
    }
    pub fn undef_typed(&mut self, f: FileId, name: &str, sym_type: u8) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, sym_type, SymbolSection::Undefined, 0, 0)
    }
    pub fn weak_undef(&mut self, f: FileId, name: &str) -> SymbolIndex {
        self.sym(f, name, STB_WEAK, STT_NOTYPE, SymbolSection::Undefined, 0, 0)
    }
    pub fn common(&mut self, f: FileId, name: &str, size: u64, align: u64) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, STT_OBJECT, SymbolSection::Common, align, size)
    }
    pub fn abs(&mut self, f: FileId, name: &str, value: u64) -> SymbolIndex {
        self.sym(f, name, STB_GLOBAL, STT_NOTYPE, SymbolSection::Absolute, value, 0)
    }

    /// Adds a section; returns its index.
    pub fn section(&mut self, f: FileId, s: MSection) -> u32 {
        let file = &mut self.files[f.0 as usize];
        file.sections.push(s);
        file.info.sections = file.sections.len() as u32;
        file.info.sections - 1
    }

    /// Adds a COMDAT group `sig` (flags `flags`) over `members`; returns
    /// the group section's index.
    pub fn group(&mut self, f: FileId, sig: &str, flags: u32, members: &[u32]) -> u32 {
        let signature = self.sig(sig.as_bytes());
        let mut s = MSection::new(".group", SHT_GROUP, 0);
        s.addralign = 4;
        let index = self.section(f, s);
        for &m in members {
            if let Some(s) = self.files[f.0 as usize].sections.get_mut(m as usize) {
                s.flags |= SHF_GROUP;
            }
        }
        self.files[f.0 as usize].groups.push(SectionIndex(index), flags, signature, members);
        index
    }

    /// A COMDAT `.text.<sig>` section in group `sig`, defining `name`
    /// there (a template instantiation).
    pub fn comdat_def(&mut self, f: FileId, sig: &str, name: &str) -> u32 {
        let s = self.section(
            f,
            MSection::new(&format!(".text.{sig}"), SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR),
        );
        self.group(f, sig, GRP_COMDAT, &[s]);
        self.sym(f, name, STB_WEAK, STT_FUNC, SymbolSection::Section(SectionIndex(s)), 0, 4);
        s
    }

    /// A label for `file` as the reference linkers print it.
    pub fn label(&self, file: FileId) -> String {
        let f = &self.files[file.0 as usize];
        match &f.member {
            Some(m) => format!("{}({m})", f.label),
            None => f.label.clone(),
        }
    }

    // ---- ELF -----------------------------------------------------------

    /// Reads a relocatable object.
    pub fn load_object(
        &mut self,
        bytes: &[u8],
        label: &str,
        kind: FileKind,
    ) -> Result<FileId, String> {
        let obj = Object::parse(bytes).map_err(|e| format!("{label}: {e:?}"))?;
        let n = obj.section_count();
        let mut sections = Vec::with_capacity(n as usize);
        for i in 0..n {
            let s = obj.section(SectionIndex(i)).map_err(|e| format!("{label}: {e:?}"))?;
            let h = s.header();
            sections.push(MSection {
                name: s.name().unwrap_or_default().to_vec(),
                sh_type: h.sh_type,
                flags: h.sh_flags,
                entsize: h.sh_entsize,
                addralign: h.sh_addralign,
                size: h.sh_size,
                link: h.sh_link,
                info: h.sh_info,
            });
        }
        let file = self.push_file(kind, label, sections);
        let symtab = obj.symbols().map_err(|e| format!("{label}: {e:?}"))?;
        if let Some(symtab) = symtab {
            for (index, sym) in symtab.globals() {
                let name = symtab.name(&sym).map_err(|e| format!("{label}: {e:?}"))?;
                let place =
                    symtab.section_of(index, &sym).map_err(|e| format!("{label}: {e:?}"))?;
                self.sym_at(
                    file,
                    index,
                    name,
                    sym.binding(),
                    sym.sym_type(),
                    sym.st_other & 3,
                    place,
                    sym.st_value,
                    sym.st_size,
                );
            }
            // Groups, with the effective signature (§6.1: a section symbol
            // signs with its section's name).
            for i in 0..n {
                let s = obj.section(SectionIndex(i)).map_err(|e| format!("{label}: {e:?}"))?;
                if s.sh_type() != SHT_GROUP {
                    continue;
                }
                let g = s.group().map_err(|e| format!("{label}: {e:?}"))?;
                let sym = symtab.get(g.signature()).map_err(|e| format!("{label}: {e:?}"))?;
                let name = if sym.sym_type() == STT_SECTION {
                    let at =
                        symtab.section_of(g.signature(), &sym).map_err(|e| format!("{e:?}"))?;
                    match at {
                        SymbolSection::Section(sec) => {
                            self.files[file.0 as usize].sections[sec.0 as usize].name.clone()
                        }
                        _ => Vec::new(),
                    }
                } else {
                    symtab.name(&sym).unwrap_or_default().to_vec()
                };
                let signature = self.sig(&name);
                let members: Vec<u32> = g.members().filter_map(Result::ok).map(|s| s.0).collect();
                self.files[file.0 as usize].groups.push(
                    SectionIndex(i),
                    g.flags(),
                    signature,
                    &members,
                );
            }
        }
        Ok(file)
    }

    /// Reads an archive mention: its members in order, lazy (or
    /// `--whole-archive`), in `group` if given.
    pub fn load_archive(
        &mut self,
        path: &Path,
        whole: bool,
        group: Option<u32>,
    ) -> Result<Vec<FileId>, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let ar = Archive::parse(&bytes).map_err(|e| format!("{}: {e:?}", path.display()))?;
        let archive = self.archive();
        let mut out = Vec::new();
        let mut ordinals: HashMap<Vec<u8>, u32> = HashMap::new();
        for member in ar.members() {
            let member = member.map_err(|e| format!("{}: {e:?}", path.display()))?;
            let Some(data) = member.data() else { continue };
            if !data.starts_with(b"\x7fELF") {
                continue;
            }
            let kind = if whole { FileKind::WholeMember } else { FileKind::LazyMember };
            let label = path.display().to_string();
            let f = self.load_object(data, &label, kind)?;
            let name = member.name().to_vec();
            let ordinal = ordinals.entry(name.clone()).or_default();
            let at = (path.display().to_string(), name, *ordinal);
            *ordinal += 1;
            let source = *self.sources.entry(at).or_insert(f.0);
            let file = &mut self.files[f.0 as usize];
            file.member = Some(String::from_utf8_lossy(member.name()).into_owned());
            file.info.archive = OptId::some(archive);
            file.info.group = group;
            file.info.source = source;
            out.push(f);
        }
        Ok(out)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl View for Model {
    fn machine(&self) -> Machine {
        self.machine
    }
    fn files(&self) -> u32 {
        self.files.len() as u32
    }
    fn keys(&self) -> u32 {
        self.names.len() as u32
    }
    fn comdats(&self) -> u32 {
        self.sigs.len() as u32
    }
    fn file(&self, file: FileId) -> FileInfo {
        self.files[file.0 as usize].info
    }
    fn mentions(&self, file: FileId, out: &mut Vec<Mention>) {
        out.extend_from_slice(&self.files[file.0 as usize].mentions);
    }
    fn mention(&self, file: FileId, index: SymbolIndex) -> Option<Mention> {
        self.files[file.0 as usize].mentions.iter().find(|m| m.index == index).copied()
    }
    fn groups(&self, file: FileId, out: &mut Groups) {
        let g = &self.files[file.0 as usize].groups;
        for info in &g.groups {
            out.push(info.section, info.flags, info.signature, g.members_of(info));
        }
    }
    fn section(&self, file: FileId, index: SectionIndex) -> Option<SectionInfo<'_>> {
        let s = self.files[file.0 as usize].sections.get(index.0 as usize)?;
        Some(SectionInfo {
            name: &s.name,
            sh_type: s.sh_type,
            flags: s.flags,
            entsize: s.entsize,
            addralign: s.addralign,
            size: s.size,
            link: s.link,
            info: s.info,
        })
    }
    fn key_name(&self, key: SymbolId) -> (&[u8], Option<&[u8]>) {
        let name = &self.names[key.0 as usize];
        match name.iter().position(|&b| b == b'@') {
            Some(at) => (&name[..at], Some(&name[at + 1..])),
            None => (name, None),
        }
    }
    fn find_key(&self, name: &[u8]) -> Option<SymbolId> {
        self.index.get(name).copied()
    }
}

// ---- processes and traces ----------------------------------------------

pub fn ok(out: &mortar_testutil::Outcome, what: &str) {
    assert!(out.success(), "{what} failed:\n{}{}", out.stdout_text(), out.stderr_text());
}

pub fn exec(program: &Path, args: &[String], dir: &Path) -> mortar_testutil::Outcome {
    let mut c = Command::new(program);
    c.args(args).current_dir(dir);
    run_with(&mut c, b"", Duration::from_mins(10))
}

pub fn canon(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_owned(), |p| p.display().to_string())
}

/// `archive(member)` with the archive's path canonical.
pub fn member_label(archive: &str, member: &str) -> String {
    format!("{}({member})", canon(archive))
}

/// `ld.bfd -t -t` (`(archive)member`) and `ld.lld --trace`
/// (`archive(member)`).
pub fn parse_trace(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for l in text.lines() {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix('(')
            && let Some(close) = rest.find(')')
        {
            out.insert(member_label(&rest[..close], &rest[close + 1..]));
        } else if l.ends_with(')')
            && let Some(open) = l.rfind('(')
        {
            out.insert(member_label(&l[..open], &l[open + 1..l.len() - 1]));
        }
    }
    out
}
