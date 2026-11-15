//! A stand-in for `mortar_input` (WP-021) in tests: the `Inputs` table of
//! a static link line from its objects and archives, read whole into
//! leaked buffers. Numbering follows `spec/model.md` §2.3 (keys and
//! signatures by first mention); `Inputs::check` must accept the result.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mortar_core::inputs::{
    Archive, ArchiveKind, Blob, Comdat, ContentHash, File, FileBytes, GroupOrdinal, InputFlags,
    Inputs, LinkInputs, LinkTarget, Member, Origin, Reader, StoreRef, SymbolKey,
};
use mortar_core::intern::SymName;
use mortar_core::{ArchiveId, Column, ComdatId, FileId, IdRange, Span, SymbolId};
use mortar_elf::archive::Archive as ArFile;
use mortar_elf::consts::{SHT_GROUP, STT_SECTION};
use mortar_elf::{Machine, Object, SectionIndex, SymbolSection};

/// One input of a line, in command-line order.
#[derive(Debug, Clone)]
pub enum Item {
    Object(PathBuf),
    /// `group`: any number naming the `--start-group` region.
    Archive {
        path: PathBuf,
        whole: bool,
        group: Option<u32>,
    },
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

#[derive(Default)]
struct Builder {
    files: Vec<File<'static>>,
    groups: Vec<Option<GroupOrdinal>>,
    sym_map: Vec<SymbolId>,
    symbols: Vec<SymbolKey<'static>>,
    keys: HashMap<(&'static [u8], Option<&'static [u8]>), SymbolId>,
    comdats: Vec<Comdat<'static>>,
    sigs: HashMap<&'static [u8], ComdatId>,
    archives: Vec<Archive<'static>>,
    ordinals: HashMap<Option<u32>, u32>,
}

/// The key of a mention's name (§2.3): `foo` for `foo` and `foo@@V`,
/// `(foo, V)` for `foo@V`.
pub fn split_name(name: &[u8]) -> (&[u8], Option<&[u8]>) {
    match name.iter().position(|&b| b == b'@') {
        Some(at) if name[at..].starts_with(b"@@") => (&name[..at], None),
        Some(at) => (&name[..at], Some(&name[at + 1..])),
        None => (name, None),
    }
}

impl Builder {
    fn push(
        &mut self,
        path: &'static [u8],
        data: &'static [u8],
        member: Option<Member<'static>>,
        origin: Origin,
        state: InputFlags,
        group: Option<GroupOrdinal>,
    ) -> Result<(), String> {
        let label = String::from_utf8_lossy(path).into_owned();
        let obj = Object::parse(data).map_err(|e| format!("{label}: {e}"))?;
        let start = self.sym_map.len() as u32;
        if let Some(t) = obj.symbols().map_err(|e| format!("{label}: {e}"))? {
            for (_, sym) in t.globals() {
                let (name, version) = split_name(t.name(&sym).map_err(|e| format!("{e}"))?);
                let next = SymbolId(self.symbols.len() as u32);
                let id = *self.keys.entry((name, version)).or_insert(next);
                if id == next {
                    // The key's hash, as the input phase computes it
                    // (`spec/model.md` §9): resolution's index reads it.
                    let hash = mortar_core::intern::InternKey::hash(&SymName { name, version });
                    self.symbols.push(SymbolKey { name, version, hash });
                }
                self.sym_map.push(id);
            }
            for i in 0..obj.section_count() {
                let s = obj.section(SectionIndex(i)).map_err(|e| format!("{e}"))?;
                if s.sh_type() != SHT_GROUP {
                    continue;
                }
                let g = s.group().map_err(|e| format!("{e}"))?;
                let sym = t.get(g.signature()).map_err(|e| format!("{e}"))?;
                let name: &'static [u8] = if sym.sym_type() == STT_SECTION {
                    match t.section_of(g.signature(), &sym).map_err(|e| format!("{e}"))? {
                        SymbolSection::Section(sec) => {
                            obj.section(sec).and_then(|s| s.name()).map_err(|e| format!("{e}"))?
                        }
                        _ => b"",
                    }
                } else {
                    t.name(&sym).map_err(|e| format!("{e}"))?
                };
                let next = ComdatId(self.comdats.len() as u32);
                if *self.sigs.entry(name).or_insert(next) == next {
                    self.comdats.push(Comdat { name });
                }
            }
        }
        let syms = Span { start, len: self.sym_map.len() as u32 - start };
        self.files.push(File {
            path,
            member,
            origin,
            state,
            reader: Reader::Object(obj),
            bytes: Some(FileBytes { blob: Blob(0), base: 0, len: data.len() as u64 }),
            sections: obj.section_count(),
            syms,
            hash: ContentHash(0),
        });
        self.groups.push(group);
        Ok(())
    }

    fn group(&mut self, group: Option<u32>) -> Option<GroupOrdinal> {
        let group = group?;
        let next = self.ordinals.len() as u32 + 1;
        Some(GroupOrdinal(*self.ordinals.entry(Some(group)).or_insert(next)))
    }
}

/// The `Inputs` of a static link of `items`.
pub fn build(machine: Machine, items: &[Item]) -> Result<Inputs<'static>, String> {
    let mut b = Builder::default();
    b.files.push(File {
        path: b"",
        member: None,
        origin: Origin::Internal,
        state: InputFlags::empty(),
        reader: Reader::Internal,
        bytes: None,
        sections: 0,
        syms: Span::default(),
        hash: ContentHash(0),
    });
    b.groups.push(None);
    for (input, item) in items.iter().enumerate() {
        let origin = Origin::Input { input: input as u32, arg: input as u32 + 1, entry: None };
        match item {
            Item::Object(path) => {
                let data = leak(read(path)?);
                let p = leak(path.display().to_string().into_bytes());
                b.push(p, data, None, origin, InputFlags::empty(), None)?;
            }
            Item::Archive { path, whole, group } => {
                let data = leak(read(path)?);
                let p = leak(path.display().to_string().into_bytes());
                let ar = ArFile::parse(data).map_err(|e| format!("{}: {e}", path.display()))?;
                let id = ArchiveId(b.archives.len() as u32);
                let first = FileId(b.files.len() as u32);
                let group = b.group(*group);
                let state = if *whole { InputFlags::WHOLE_ARCHIVE } else { InputFlags::empty() };
                let mut ordinals: HashMap<&[u8], u32> = HashMap::new();
                for m in ar.members() {
                    let m = m.map_err(|e| format!("{e}"))?;
                    let Some(bytes) = m.data() else { continue };
                    if !bytes.starts_with(b"\x7fELF") {
                        continue;
                    }
                    let ordinal = ordinals.entry(m.name()).or_default();
                    let member = Member {
                        archive: id,
                        name: m.name(),
                        ordinal: *ordinal,
                        offset: m.header_offset(),
                        thin: None,
                    };
                    *ordinal += 1;
                    b.push(p, bytes, Some(member), origin, state, group)?;
                }
                b.archives.push(Archive {
                    path: p,
                    kind: ArchiveKind::Gnu,
                    origin,
                    members: IdRange::new(first, FileId(b.files.len() as u32)),
                });
            }
        }
    }
    let files = b.files.len() as u32;
    let mut link = LinkInputs::of_files(files);
    link.group = Column::from_vec(b.groups).map_err(|e| format!("{e:?}"))?;
    let inputs = Inputs {
        target: LinkTarget::of(machine),
        files: Column::from_vec(b.files).map_err(|e| format!("{e:?}"))?,
        sym_map: b.sym_map.into_boxed_slice(),
        symbols: Column::from_vec(b.symbols).map_err(|e| format!("{e:?}"))?,
        comdats: Column::from_vec(b.comdats).map_err(|e| format!("{e:?}"))?,
        archives: Column::from_vec(b.archives).map_err(|e| format!("{e:?}"))?,
        link,
        store: StoreRef::none(),
    };
    inputs.check().map_err(|e| e.to_string())?;
    Ok(inputs)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether resolution over the `Inputs` view (`b`) and over the model of
/// the same files (`a`) agree, key by key and signature by signature by
/// name (their ids differ: the model numbers option names and `foo@V` of
/// `foo@@V` too).
#[allow(clippy::many_single_char_names, reason = "a and b are the two results, m the model")]
pub fn same(
    m: &super::Model,
    a: &mortar_resolve::Resolved,
    inputs: &Inputs<'_>,
    b: &mortar_resolve::Resolved,
) -> Result<(), String> {
    use mortar_resolve::View;
    if a.file_live != b.file_live {
        return Err("file_live differs".to_owned());
    }
    if a.section_base != b.section_base {
        return Err("section_base differs".to_owned());
    }
    for (k, key) in inputs.symbols.iter() {
        let mut name = key.name.to_vec();
        if let Some(v) = key.version {
            name.push(b'@');
            name.extend_from_slice(v);
        }
        let label = String::from_utf8_lossy(&name).into_owned();
        let mk = m.try_id_bytes(&name).ok_or_else(|| format!("{label}: no model key"))?;
        let (i, j) = (mk.0 as usize, k.0 as usize);
        if a.def[i] != b.def[j] || a.props[i] != b.props[j] || a.clash[i] != b.clash[j] {
            return Err(format!(
                "{label}: model {:?} {:?}, inputs {:?} {:?}",
                a.def[i], a.props[i], b.def[j], b.props[j]
            ));
        }
    }
    let sig = |c: ComdatId| -> Result<ComdatId, String> {
        let name = inputs.comdats[c].name;
        m.find_sig(name)
            .ok_or_else(|| format!("{}: no model signature", String::from_utf8_lossy(name)))
    };
    for (c, _) in inputs.comdats.iter() {
        if a.keepers.keeper[sig(c)?.0 as usize] != b.keepers.keeper[c.0 as usize] {
            return Err(format!("comdat {}: keepers differ", c.0));
        }
    }
    for (x, y) in a.sections.iter().zip(&b.sections) {
        let yc = match y.comdat.get() {
            Some(c) => mortar_core::OptId::some(sig(c)?),
            None => mortar_core::OptId::NONE,
        };
        if (x.kind, x.state, x.reloc, x.comdat) != (y.kind, y.state, y.reloc, yc) {
            return Err(format!("sections differ: {x:?} / {y:?}"));
        }
    }
    let ids = |r: &mortar_resolve::Resolved| -> Vec<(mortar_resolve::problem::DiagKey, &str)> {
        r.problems.iter().map(|p| (p.key, p.id())).collect()
    };
    if ids(a) != ids(b) {
        return Err(format!("problems differ: {:?} / {:?}", ids(a), ids(b)));
    }
    let _ = m.files();
    Ok(())
}
