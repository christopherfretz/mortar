//! The inputs read by position (`spec/plumb.md` §8.1; WP-109 §6, WP-109b):
//! of each object the map names only its skeleton is resident (the file
//! header, the section header table, the section names, the symbol table
//! with its strings and extended indexes, the groups); a section's body
//! is read through the [`Source`] when a rule asks for it, and kept only
//! in a small cache. The verifier does no I/O of its own.
//!
//! The readers of `mortar_elf` give the same answers, checks and errors
//! over a skeleton as over the whole file whenever what they decode is
//! resident (`spec/elf.md` §3.0); a file whose skeleton is not enough for
//! them (a malformed one: an error that says `NotResident`) is read whole,
//! so the verdict is the whole bytes' in every case.

use std::collections::VecDeque;
use std::ops::Deref;
use std::sync::{Arc, Mutex, PoisonError};

use mortar_elf::archive::{self, Archive, Location};
use mortar_elf::consts::{SHN_XINDEX, SHT_GROUP, SHT_NOBITS, SHT_SYMTAB, SHT_SYMTAB_SHNDX};
use mortar_elf::{Error, FileHeader, Object, Part, Record, Shdr, View};

use crate::Source;
use crate::hasher::HashMap;
use crate::locate::thin_member_path;
use crate::map::{Map, MapFile};

/// Whole objects kept after a phase loaded them ([`Store::load`]): the
/// one being judged and the one before it (whose piece groups are decided
/// when the next one starts, §5.6).
const OBJECTS: usize = 2;
/// The section always resident: kiln's metadata (`spec/meta.md`).
const META: &[u8] = b".mason.meta";
/// The bytes of section bodies kept between reads.
const SECTION_BYTES: u64 = 64 << 20;

/// Where an object's bytes are: `len` bytes of file `file` of the source,
/// from `base` (an archive member's offset; 0 for a file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct At {
    pub file: usize,
    pub base: u64,
    pub len: u64,
}

/// An object's resident bytes.
#[derive(Debug)]
pub(crate) enum Skel {
    /// The skeleton.
    Parts(Vec<Part>),
    /// The whole object: a file the skeleton is not enough for.
    Whole(Box<[u8]>),
}

/// What a map's file is (§3.3).
#[derive(Debug)]
pub(crate) enum Resolved {
    /// Not among the inputs (I1).
    Missing,
    /// It cannot be read: the message (I2), without the file's name.
    Error(String),
    /// Object `k` of the store.
    Object(usize),
}

/// The bytes of a section: borrowed from a resident part, or shared with
/// the cache.
#[derive(Debug, Clone)]
pub(crate) enum Bytes<'a> {
    Ref(&'a [u8]),
    Shared(Arc<[u8]>, usize, usize),
}

impl Deref for Bytes<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Ref(b) => b,
            Bytes::Shared(a, s, e) => &a[*s..*e],
        }
    }
}

#[derive(Debug, Default)]
struct Cache {
    /// Whole objects, oldest first.
    objects: VecDeque<(At, Arc<[u8]>)>,
    /// Section bodies by (file, absolute offset, size), oldest first.
    sections: HashMap<(usize, u64, u64), Arc<[u8]>>,
    order: VecDeque<(usize, u64, u64)>,
    bytes: u64,
}

/// The inputs of a link as the verifier holds them: the skeleton of every
/// object the maps name, and the means to read the rest.
pub(crate) struct Store<'s> {
    src: &'s dyn Source,
    objects: Vec<(At, Skel)>,
    /// Per map given to [`Store::read`], per file of the map.
    resolved: Vec<Vec<Resolved>>,
    cache: Mutex<Cache>,
    /// Reads that failed after the skeletons were read: one message per
    /// file, in the order they failed.
    errors: Mutex<Vec<(usize, String)>>,
}

impl std::fmt::Debug for Store<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Store({} objects)", self.objects.len())
    }
}

/// An archive's members by name: `(header offset, where)` in file order.
type Members = HashMap<Vec<u8>, Vec<(u64, Loc)>>;

#[derive(Debug, Clone)]
pub(crate) enum Loc {
    Inline { offset: u64, size: u64 },
    External(Vec<u8>),
    ExternalMember,
}

impl<'s> Store<'s> {
    /// Resolves every file of `maps` against `src` and reads the skeleton
    /// of every object they name (§3.3).
    pub(crate) fn read(src: &'s dyn Source, maps: &[&Map]) -> Store<'s> {
        let mut by_path: HashMap<&str, usize> = HashMap::default();
        for i in 0..src.count() {
            by_path.insert(src.path(i), i);
        }
        let mut store = Store {
            src,
            objects: Vec::new(),
            resolved: Vec::new(),
            cache: Mutex::default(),
            errors: Mutex::default(),
        };
        let mut index: HashMap<At, usize> = HashMap::default();
        let mut archives: HashMap<usize, Result<Members, String>> = HashMap::default();
        for map in maps {
            let mut list = Vec::with_capacity(map.files.len());
            for f in &map.files {
                let r = store.resolve(f, &by_path, &mut archives, &mut index);
                list.push(r);
            }
            store.resolved.push(list);
        }
        store
    }

    fn resolve(
        &mut self,
        f: &MapFile,
        by_path: &HashMap<&str, usize>,
        archives: &mut HashMap<usize, Result<Members, String>>,
        index: &mut HashMap<At, usize>,
    ) -> Resolved {
        let src = self.src;
        let Some(&file) = by_path.get(f.path.as_str()) else { return Resolved::Missing };
        let at = if f.member.is_some() {
            let members = archives.entry(file).or_insert_with(|| members_of(src, file));
            let found = match members {
                Ok(m) => find_member(src, file, f, m, by_path),
                Err(e) => Err(e.clone()),
            };
            match found {
                Ok(at) => at,
                Err(e) => return Resolved::Error(e),
            }
        } else {
            At { file, base: 0, len: src.len(file) }
        };
        if let Some(&k) = index.get(&at) {
            return Resolved::Object(k);
        }
        match skeleton(src, at) {
            Ok(s) => {
                index.insert(at, self.objects.len());
                self.objects.push((at, s));
                Resolved::Object(self.objects.len() - 1)
            }
            Err(e) => Resolved::Error(e),
        }
    }

    /// File `i` of map `m` (an index into the maps given to `read`).
    pub(crate) fn resolved(&self, m: usize, i: usize) -> &Resolved {
        self.resolved.get(m).and_then(|l| l.get(i)).unwrap_or(&Resolved::Missing)
    }

    /// Object `k`: where it is, and its resident bytes as a view.
    pub(crate) fn object(&self, k: usize) -> (At, View<'_>) {
        let (at, s) = &self.objects[k];
        let view = match s {
            Skel::Parts(p) => View::parts(p, at.len),
            Skel::Whole(b) => View::whole(b),
        };
        (*at, view)
    }

    /// The bytes held resident: the skeletons (and the objects read whole).
    pub(crate) fn resident(&self) -> u64 {
        self.objects
            .iter()
            .map(|(_, s)| match s {
                Skel::Parts(p) => p.iter().map(|p| p.bytes.len() as u64).sum(),
                Skel::Whole(b) => b.len() as u64,
            })
            .sum()
    }

    /// `size` bytes of the object at `at` from `offset` (within it): from a
    /// loaded object, the cache, or the source. A read that fails is
    /// recorded ([`Store::take_errors`]) and gives no bytes.
    pub(crate) fn body(&self, at: At, offset: u64, size: u64) -> Bytes<'static> {
        let (Ok(s), Some(e)) = (usize::try_from(offset), offset.checked_add(size)) else {
            return Bytes::Ref(&[]);
        };
        if e > at.len {
            return Bytes::Ref(&[]);
        }
        let key = (at.file, at.base + offset, size);
        {
            let c = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, whole)) = c.objects.iter().find(|(a, _)| *a == at) {
                return Bytes::Shared(Arc::clone(whole), s, s + size as usize);
            }
            if let Some(b) = c.sections.get(&key) {
                return Bytes::Shared(Arc::clone(b), 0, b.len());
            }
        }
        let Some(b) = self.read_range(at.file, at.base + offset, size) else {
            return Bytes::Ref(&[]);
        };
        let len = b.len();
        if size <= SECTION_BYTES / 4 {
            let mut c = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            if c.sections.insert(key, Arc::clone(&b)).is_none() {
                c.order.push_back(key);
                c.bytes += size;
            }
            while c.bytes > SECTION_BYTES {
                let Some(old) = c.order.pop_front() else { break };
                if let Some(x) = c.sections.remove(&old) {
                    c.bytes -= x.len() as u64;
                }
            }
        }
        Bytes::Shared(b, 0, len)
    }

    /// Reads the whole object at `at` into the cache, for a phase that
    /// reads most of its sections (one read instead of one per section).
    pub(crate) fn load(&self, at: At) {
        {
            let c = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            if c.objects.iter().any(|(a, _)| *a == at) {
                return;
            }
        }
        let Some(b) = self.read_range(at.file, at.base, at.len) else { return };
        let mut c = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        c.objects.push_back((at, b));
        while c.objects.len() > OBJECTS {
            c.objects.pop_front();
        }
    }

    fn read_range(&self, file: usize, offset: u64, size: u64) -> Option<Arc<[u8]>> {
        let n = usize::try_from(size).ok()?;
        let mut b = vec![0u8; n];
        match self.src.read_at(file, offset, &mut b) {
            Ok(()) => Some(b.into()),
            Err(e) => {
                let mut errs = self.errors.lock().unwrap_or_else(PoisonError::into_inner);
                if !errs.iter().any(|(f, _)| *f == file) {
                    errs.push((file, e));
                }
                None
            }
        }
    }

    /// The reads that failed after the skeletons were read, one message per
    /// file (an I2 finding each).
    pub(crate) fn take_errors(&self) -> Vec<String> {
        let mut errs = self.errors.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *errs).into_iter().map(|(_, e)| e).collect()
    }
}

/// `size` bytes of file `file` from `offset`.
fn read(src: &dyn Source, file: usize, offset: u64, size: u64) -> Result<Vec<u8>, String> {
    let n = usize::try_from(size).map_err(|_| format!("{}: too large", src.path(file)))?;
    let mut b = vec![0u8; n];
    src.read_at(file, offset, &mut b)?;
    Ok(b)
}

/// The ranges `(offset, size)` read into parts: sorted, overlapping and
/// adjacent ones merged (a parts view's parts are disjoint and not
/// adjacent).
fn parts(
    src: &dyn Source,
    file: usize,
    base: u64,
    mut ranges: Vec<(u64, u64)>,
) -> Result<Vec<Part>, String> {
    ranges.retain(|r| r.1 > 0);
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (s, n) in ranges {
        let e = s + n;
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
        .into_iter()
        .map(|(s, e)| {
            Ok(Part { offset: s, bytes: read(src, file, base + s, e - s)?.into_boxed_slice() })
        })
        .collect()
}

/// The skeleton of the object at `at`: its file header, section header
/// table, section names, symbol table (its strings, its extended indexes)
/// and groups, or the whole object when `mortar_elf` needs more of it than
/// that (a malformed file).
pub(crate) fn skeleton(src: &dyn Source, at: At) -> Result<Skel, String> {
    let len = at.len;
    let within = |o: u64, n: u64| o.checked_add(n).is_some_and(|e| e <= len);
    let head = read(src, at.file, at.base, len.min(64))?;
    let mut ranges = vec![(0, len.min(64))];
    if let Ok(h) = FileHeader::parse(&head) {
        let class = h.class;
        let es = Shdr::size(class) as u64;
        if h.e_shoff != 0 && u64::from(h.e_shentsize) == es {
            let count = if h.e_shnum != 0 {
                Some(u64::from(h.e_shnum))
            } else if within(h.e_shoff, es) {
                Shdr::decode(class, &read(src, at.file, at.base + h.e_shoff, es)?)
                    .map(|s| s.sh_size)
            } else {
                None
            };
            if let Some(size) = count
                .filter(|c| u32::try_from(*c).is_ok())
                .and_then(|c| c.checked_mul(es))
                .filter(|size| within(h.e_shoff, *size))
            {
                ranges.push((h.e_shoff, size));
                let table = read(src, at.file, at.base + h.e_shoff, size)?;
                let hdrs: Vec<Shdr> = table
                    .chunks_exact(es as usize)
                    .filter_map(|c| Shdr::decode(class, c))
                    .collect();
                let mut want = |i: u32| {
                    if let Some(s) = hdrs.get(i as usize)
                        && s.sh_type != SHT_NOBITS
                        && within(s.sh_offset, s.sh_size)
                    {
                        ranges.push((s.sh_offset, s.sh_size));
                    }
                };
                let names = if h.e_shstrndx == SHN_XINDEX {
                    hdrs.first().map_or(0, |s| s.sh_link)
                } else {
                    u32::from(h.e_shstrndx)
                };
                want(names);
                // Kiln's metadata (`.mason.meta`, small): the ODR table
                // borrows its units for the whole check (§13).
                let strings = hdrs
                    .get(names as usize)
                    .filter(|s| s.sh_type != SHT_NOBITS && within(s.sh_offset, s.sh_size))
                    .map(|s| read(src, at.file, at.base + s.sh_offset, s.sh_size))
                    .transpose()?
                    .unwrap_or_default();
                for (i, s) in (0u32..).zip(&hdrs) {
                    let name = usize::try_from(s.sh_name)
                        .ok()
                        .and_then(|n| strings.get(n..))
                        .and_then(|b| b.split(|&c| c == 0).next());
                    if name == Some(META) {
                        want(i);
                    }
                    match s.sh_type {
                        SHT_SYMTAB => {
                            want(i);
                            want(s.sh_link);
                        }
                        SHT_SYMTAB_SHNDX | SHT_GROUP => want(i),
                        _ => {}
                    }
                }
            }
        }
    }
    let p = parts(src, at.file, at.base, ranges)?;
    let enough = {
        let view = View::parts(&p, len);
        match Object::parse_view(view) {
            Err(Error::NotResident { .. }) => false,
            Err(_) => true,
            Ok(o) => !matches!(o.symbols(), Err(Error::NotResident { .. })),
        }
    };
    if enough {
        Ok(Skel::Parts(p))
    } else {
        Ok(Skel::Whole(read(src, at.file, at.base, len)?.into_boxed_slice()))
    }
}

/// An archive's members (§2.2), from its headers read by position: the
/// magic, every member header, the special members and BSD names (what
/// `mortar_elf`'s reader decodes itself), never a member's contents. A
/// thin archive (it holds no contents) and anything that is not a regular
/// archive are read whole.
fn members_of(src: &dyn Source, file: usize) -> Result<Members, String> {
    let mut by_name: Members = HashMap::default();
    for (name, offset, loc) in members_in_order(src, file)? {
        by_name.entry(name).or_default().push((offset, loc));
    }
    Ok(by_name)
}

impl Skel {
    /// The resident bytes as a view of an object of `len` bytes.
    pub(crate) fn view(&self, len: u64) -> View<'_> {
        match self {
            Skel::Parts(p) => View::parts(p, len),
            Skel::Whole(b) => View::whole(b),
        }
    }
}

/// An archive's members in archive order: `(name, header offset, where)`
/// (the input set's file order, §3.8), read as [`members_of`] reads them.
pub(crate) fn members_in_order(
    src: &dyn Source,
    file: usize,
) -> Result<Vec<(Vec<u8>, u64, Loc)>, String> {
    let len = src.len(file);
    let magic = read(src, file, 0, len.min(8))?;
    if magic.as_slice() == archive::MAGIC {
        let mut ranges = vec![(0, 8)];
        let mut at = 8u64;
        while at < len {
            let n = (len - at).min(archive::HEADER_LEN);
            let raw = read(src, file, at, n)?;
            ranges.push((at, n));
            match archive::header_span(at, &raw, len) {
                Ok(span) => {
                    ranges.push((span.content, span.front));
                    if span.next <= at {
                        break;
                    }
                    at = span.next;
                }
                Err(_) => break,
            }
        }
        let p = parts(src, file, 0, ranges)?;
        match members_in(View::parts(&p, len)) {
            Err(archive::Error::NotResident { .. }) => {}
            r => return r.map_err(|e| e.to_string()),
        }
    }
    let whole = read(src, file, 0, len)?;
    members_in(View::whole(&whole)).map_err(|e| e.to_string())
}

fn members_in(view: View<'_>) -> Result<Vec<(Vec<u8>, u64, Loc)>, archive::Error> {
    let archive = Archive::parse_view(view)?;
    let mut out = Vec::new();
    for h in archive.headers() {
        let h = h?;
        let loc = match h.location {
            Location::Inline { offset } => Loc::Inline { offset, size: h.size },
            Location::External { path } => Loc::External(path.to_vec()),
            Location::ExternalMember { .. } => Loc::ExternalMember,
        };
        out.push((h.name.to_vec(), h.header_offset, loc));
    }
    Ok(out)
}

/// The member a map's file names (§3.3): by name, and by the header's
/// offset when the map gives it.
fn find_member(
    src: &dyn Source,
    file: usize,
    f: &MapFile,
    members: &Members,
    by_path: &HashMap<&str, usize>,
) -> Result<At, String> {
    let name = f.member.as_deref().unwrap_or("").as_bytes();
    let mut found: Option<At> = None;
    let mut count = 0;
    for (offset, loc) in members.get(name).map_or(&[][..], Vec::as_slice) {
        if f.member_offset.is_some_and(|o| o != *offset) {
            continue;
        }
        count += 1;
        match loc {
            Loc::Inline { offset, size } => {
                found = found.or(Some(At { file, base: *offset, len: *size }));
            }
            Loc::External(path) => {
                let at = thin_member_path(&f.path, path);
                match by_path.get(at.as_str()) {
                    Some(&i) => found = found.or(Some(At { file: i, base: 0, len: src.len(i) })),
                    None => {
                        return Err(format!(
                            "a member of a thin archive, '{at}', which is not among the inputs"
                        ));
                    }
                }
            }
            Loc::ExternalMember => {
                return Err(
                    "a thin archive's member that is a member of another archive (not read)".into(),
                );
            }
        }
    }
    match (found, count) {
        (Some(d), 1) => Ok(d),
        (None, _) => {
            Err(format!("the archive has no member named '{}'", String::from_utf8_lossy(name)))
        }
        _ => Err(format!(
            "the archive has {count} members named '{}' and the map does not say which",
            String::from_utf8_lossy(name)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Input;

    fn member(name: &str, body: &[u8]) -> Vec<u8> {
        let mut m = format!("{name:<16}{:<32}{:<10}`\n", "", body.len()).into_bytes();
        m.extend_from_slice(body);
        if body.len() % 2 == 1 {
            m.push(b'\n');
        }
        m
    }

    fn archive(members: &[Vec<u8>]) -> Vec<u8> {
        let mut a = archive::MAGIC.to_vec();
        for m in members {
            a.extend_from_slice(m);
        }
        a
    }

    /// The members (or the error), in archive order.
    fn shown(r: Result<Vec<(Vec<u8>, u64, Loc)>, String>) -> String {
        match r {
            Ok(m) => format!("{m:?}"),
            Err(e) => format!("error: {e}"),
        }
    }

    // spec/plumb.md §8.1 (WP-109b): an archive's members found from its
    // headers read by position are the members (or the error, word for
    // word) the reader finds in the whole bytes, for good archives and for
    // headers that lie.
    #[test]
    fn s8_1_members_by_position_are_the_whole_bytes_members() {
        let good = vec![
            member("/", &[0, 0, 0, 1, 0, 0, 0, 8, b'f', 0]),
            member("//", b"a_long_member_name.o/\n"),
            member("m.o/", b"abcd"),
            member("/0", b"xyz"),
            member("#1/12", b"bsd_named.o\0body"),
            member("m.o/", b"second"),
        ];
        let mut cases = vec![archive(&good)];
        // A member whose size runs past the end.
        let mut past = archive(&good[..3]);
        past.extend_from_slice(format!("{:<16}{:<32}{:<10}`\n", "n.o/", "", 1000).as_bytes());
        past.extend_from_slice(b"short");
        cases.push(past);
        // A size that is not a number; a bad terminator; a truncated
        // header; a long name past the table; a BSD name past the member.
        let mut bad_size = archive(&good[..3]);
        bad_size.extend_from_slice(format!("{:<16}{:<32}{:<10}`\n", "n.o/", "", "12a").as_bytes());
        cases.push(bad_size);
        let mut bad_end = archive(&good[..3]);
        bad_end.extend_from_slice(format!("{:<16}{:<32}{:<10}x\n", "n.o/", "", 2).as_bytes());
        bad_end.extend_from_slice(b"ab");
        cases.push(bad_end);
        let mut cut = archive(&good[..3]);
        cut.extend_from_slice(b"n.o/            0   ");
        cases.push(cut);
        cases.push(archive(&[good[1].clone(), member("/900", b"x")]));
        cases.push(archive(&[member("#1/40", b"short")]));
        cases.push(b"!<arch>\n".to_vec());
        cases.push(b"!<arc".to_vec());
        cases.push(b"not an archive at all".to_vec());
        for (n, a) in cases.iter().enumerate() {
            let inputs = vec![Input { path: "lib.a", bytes: a }];
            let by_position = shown(members_in_order(&inputs, 0));
            let whole = shown(members_in(View::whole(a)).map_err(|e| e.to_string()));
            assert_eq!(by_position, whole, "case {n}");
        }
    }

    // spec/plumb.md §8.1: a map naming a member of an archive whose
    // header lies is an error naming what is wrong, never a panic, and the
    // member's offset in the map selects among members of one name.
    #[test]
    fn s8_1_a_lying_member_header_is_an_error() {
        let mut a = archive(&[member("m.o/", b"abcd")]);
        a.extend_from_slice(format!("{:<16}{:<32}{:<10}`\n", "n.o/", "", 99).as_bytes());
        let inputs = vec![Input { path: "lib.a", bytes: &a }];
        let map = crate::map::parse(
            crate::Format::Mortar,
            "mortar-map 2\narg mortar\nfile 0 lib.a m.o 8\nfile 1 lib.a n.o 72\nfile 2 lib.a m.o 72\nend\n",
        )
        .unwrap();
        let store = Store::read(&inputs, &[&map]);
        for i in 0..3 {
            let Resolved::Error(e) = store.resolved(0, i) else {
                panic!("file {i}: {:?}", store.resolved(0, i));
            };
            assert!(e.contains("72") || e.contains("0x48"), "file {i}: {e}");
        }
    }
}
