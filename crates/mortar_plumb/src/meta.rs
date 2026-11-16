//! A second reader of the Mason metadata container (`spec/meta.md` §4,
//! MT-10 to MT-14, MT-25, MT-35; `spec/plumb.md` §13.1). It shares no
//! code with `mortar_meta`: the format is small on purpose, and a second
//! reader is the point (`spec/odr.md` §12.5). It never panics: every
//! malformation is a message, and the units around it are read when the
//! framing still says where they are.

/// The magic of a unit (MT-10).
const MAGIC: &[u8; 4] = b"MASN";
/// The fixed header of a unit, and of a block (MT-10, MT-11).
const UNIT_HEADER: usize = 32;
const BLOCK_HEADER: usize = 16;

/// FNV-1a 64 (MT-13).
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub(crate) fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o.checked_add(2)?)?.try_into().ok()?))
}
pub(crate) fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?))
}
pub(crate) fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o.checked_add(8)?)?.try_into().ok()?))
}

/// A block of a unit that parses (MT-11).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Block<'a> {
    pub kind: u16,
    pub version: u16,
    pub row_size: usize,
    /// `row_count × row_size` bytes.
    pub rows: &'a [u8],
}

impl<'a> Block<'a> {
    pub fn rows(&self) -> impl Iterator<Item = &'a [u8]> + use<'a> {
        let size = self.row_size.max(1);
        let rows = if self.row_size == 0 { &[][..] } else { self.rows };
        rows.chunks_exact(size)
    }
}

/// A unit that parses: major 1 and none of MT-14's malformations.
#[derive(Debug, Clone)]
pub(crate) struct Unit<'a> {
    /// The unit's bytes, `unit_size` of them.
    pub bytes: &'a [u8],
    pub strtab: &'a [u8],
    pub blocks: Vec<Block<'a>>,
}

impl<'a> Unit<'a> {
    /// The string at `off` (MT-12): `None` past the table or without a NUL.
    pub fn string(&self, off: u32) -> Option<&'a [u8]> {
        string_in(self.strtab, off)
    }
    /// The block of `kind`, if the unit has one the reader knows: version
    /// 1 or later (MT-25: later versions append) and rows at least
    /// `min_row` bytes.
    pub fn block(&self, kind: u16, min_row: usize) -> Option<&Block<'a>> {
        self.blocks
            .iter()
            .find(|b| b.kind == kind && b.version >= 1 && b.row_size >= min_row && min_row > 0)
    }
}

fn string_in(strtab: &[u8], off: u32) -> Option<&[u8]> {
    if off == 0 {
        return Some(&[]);
    }
    let rest = strtab.get(off as usize..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[..end])
}

/// One container section, read.
#[derive(Debug, Clone, Default)]
pub(crate) struct Section<'a> {
    pub units: Vec<Unit<'a>>,
    /// MT-14's and MT-35's findings, one line each, with the unit's offset.
    pub problems: Vec<String>,
}

/// Row sizes of version 1 (meta.md §7), and the offsets of their string
/// fields (MT-26, `spec/odr.md` O-15, O-17).
const KIND1_ROW: usize = 16;
pub(crate) const KIND2_ROW: usize = 48;
pub(crate) const KIND3_ROW: usize = 56;
const KIND1_STRINGS: &[usize] = &[0, 4];
const KIND2_STRINGS: &[usize] = &[24, 32, 36];
const KIND3_STRINGS: &[usize] = &[32, 44];

/// Reads the units of a container section's bytes (MT-10).
pub(crate) fn read(data: &[u8]) -> Section<'_> {
    let mut out = Section::default();
    let mut at = 0usize;
    while at < data.len() {
        let Some(word) = data.get(at..at + 8) else {
            out.problems.push(format!("unit at offset {at}: header cut short (MT-14)"));
            break;
        };
        if word == [0u8; 8] {
            at += 8;
            continue;
        }
        if &word[..4] != MAGIC {
            out.problems.push(format!("unit at offset {at}: wrong magic (MT-10)"));
            break;
        }
        let Some(size) = u32_at(data, at + 8).map(|s| s as usize) else {
            out.problems.push(format!("unit at offset {at}: header cut short (MT-14)"));
            break;
        };
        if !size.is_multiple_of(8)
            || size < UNIT_HEADER
            || at.checked_add(size).is_none_or(|e| e > data.len())
        {
            out.problems.push(format!("unit at offset {at}: unit_size {size} not usable (MT-14)"));
            break;
        }
        let bytes = &data[at..at + size];
        match u16_at(bytes, 4) {
            Some(1) => match unit(bytes) {
                Ok((strtab, blocks)) => out.units.push(Unit { bytes, strtab, blocks }),
                Err(e) => out.problems.push(format!("unit at offset {at}: {e} (MT-14)")),
            },
            Some(m) => out.problems.push(format!("unit at offset {at}: major {m} (MT-35)")),
            None => {}
        }
        at += size;
    }
    out
}

type Parsed<'a> = (&'a [u8], Vec<Block<'a>>);

fn unit(u: &[u8]) -> Result<Parsed<'_>, String> {
    let field = |o| u32_at(u, o).map(|v| v as usize).ok_or_else(|| "header cut short".to_string());
    let header = field(12)?;
    let count = field(16)?;
    let st_off = field(20)?;
    let st_size = field(24)?;
    if header < UNIT_HEADER || header > u.len() {
        return Err(format!("header_size {header} outside the unit"));
    }
    let strtab = if st_size == 0 && st_off == 0 {
        &[][..]
    } else {
        if st_off < header {
            return Err("the string table overlaps the header".into());
        }
        let s = st_off
            .checked_add(st_size)
            .and_then(|e| u.get(st_off..e))
            .ok_or("the string table runs past the unit")?;
        if s.first().is_some_and(|&b| b != 0) {
            return Err("the string table does not start with the empty string".into());
        }
        s
    };
    let mut blocks = Vec::new();
    let mut at = header;
    let mut last: Option<u16> = None;
    for i in 0..count {
        let h = u
            .get(at..at.saturating_add(BLOCK_HEADER))
            .ok_or(format!("block {i} runs past the unit"))?;
        let kind = u16_at(h, 0).unwrap_or(0);
        let version = u16_at(h, 2).unwrap_or(0);
        let size = u32_at(h, 4).unwrap_or(0) as usize;
        let row_size = u32_at(h, 8).unwrap_or(0) as usize;
        let rows = u32_at(h, 12).unwrap_or(0) as usize;
        if size < BLOCK_HEADER
            || !size.is_multiple_of(8)
            || at.checked_add(size).is_none_or(|e| e > u.len())
        {
            return Err(format!("block {i} runs past the unit"));
        }
        if last.is_some_and(|k| k >= kind) {
            return Err(format!("block {i}: kinds not strictly increasing"));
        }
        last = Some(kind);
        let body = row_size
            .checked_mul(rows)
            .filter(|&n| n <= size - BLOCK_HEADER)
            .ok_or(format!("block {i}: row_count x row_size runs past the block"))?;
        let b = Block {
            kind,
            version,
            row_size,
            rows: &u[at + BLOCK_HEADER..at + BLOCK_HEADER + body],
        };
        check_rows(&b, strtab).map_err(|e| format!("block {i}: {e}"))?;
        blocks.push(b);
        at += size;
    }
    Ok((strtab, blocks))
}

/// The string fields and the order of the kinds this reader knows.
fn check_rows(b: &Block<'_>, strtab: &[u8]) -> Result<(), String> {
    let (min, strings) = match b.kind {
        1 => (KIND1_ROW, KIND1_STRINGS),
        2 => (KIND2_ROW, KIND2_STRINGS),
        3 => (KIND3_ROW, KIND3_STRINGS),
        _ => return Ok(()),
    };
    if b.version == 0 || b.row_size < min {
        return Ok(());
    }
    let mut prev: Option<&[u8]> = None;
    for row in b.rows() {
        for &o in strings {
            let off = u32_at(row, o).unwrap_or(0);
            string_in(strtab, off).ok_or("a string offset past the table or without a NUL")?;
        }
        if let Some(p) = prev
            && !ordered(b.kind, p, row, strtab)
        {
            return Err("rows not sorted".into());
        }
        prev = Some(row);
    }
    Ok(())
}

/// Whether row `b` may follow row `a` (MT-26: kind 1 by name bytes,
/// names unique; kinds 2 and 3 by key, then name bytes when both rows
/// are named).
fn ordered(kind: u16, a: &[u8], b: &[u8], strtab: &[u8]) -> bool {
    let name = |r: &[u8], o| string_in(strtab, u32_at(r, o).unwrap_or(0)).unwrap_or(&[]);
    if kind == 1 {
        return name(a, 0) < name(b, 0);
    }
    let (ka, kb) = (u64_at(a, 0).unwrap_or(0), u64_at(b, 0).unwrap_or(0));
    let o = if kind == 2 { 24 } else { 32 };
    let both_named = u32_at(a, o).unwrap_or(0) != 0 && u32_at(b, o).unwrap_or(0) != 0;
    ka < kb || (ka == kb && (!both_named || name(a, o) <= name(b, o)))
}

/// MT-32's bytes from the units that parse: each followed by zeros to a
/// multiple of 8 (a unit's size is one already).
pub(crate) fn kept<'u>(units: impl Iterator<Item = &'u Unit<'u>>) -> Vec<u8> {
    let mut v = Vec::new();
    for u in units {
        v.extend_from_slice(u.bytes);
        while v.len() % 8 != 0 {
            v.push(0);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `.mason.meta` sections of a fixture object (written by the
    /// test producer, `cargo xtask meta`; `tests/meta/gen.sh`).
    fn sections(name: &str) -> Vec<Vec<u8>> {
        let path = format!("{}/tests/meta/aarch64/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap();
        let elf = mortar_elf::Object::parse(&bytes).unwrap();
        elf.sections()
            .filter(|s| s.name().is_ok_and(|n| n == b".mason.meta"))
            .map(|s| s.data().unwrap().to_vec())
            .collect()
    }

    /// One unit: `blocks` as (kind, row size, rows), then the string table.
    fn unit(blocks: &[(u16, u32, &[u8])], strtab: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        for (kind, row_size, rows) in blocks {
            let count = if *row_size == 0 { 0 } else { rows.len() as u32 / row_size };
            let size = (16 + rows.len()).div_ceil(8) * 8;
            body.extend_from_slice(&kind.to_le_bytes());
            body.extend_from_slice(&1u16.to_le_bytes());
            body.extend_from_slice(&(size as u32).to_le_bytes());
            body.extend_from_slice(&row_size.to_le_bytes());
            body.extend_from_slice(&count.to_le_bytes());
            body.extend_from_slice(rows);
            body.resize(body.len().div_ceil(8) * 8, 0);
        }
        let st_off = if strtab.is_empty() { 0 } else { 32 + body.len() };
        let mut st = strtab.to_vec();
        st.resize(st.len().div_ceil(8) * 8, 0);
        let size = 32 + body.len() + st.len();
        let mut u = b"MASN".to_vec();
        u.extend_from_slice(&1u16.to_le_bytes());
        u.extend_from_slice(&0u16.to_le_bytes());
        for v in [size, 32, blocks.len(), st_off, strtab.len()] {
            u.extend_from_slice(&(v as u32).to_le_bytes());
        }
        u.extend_from_slice(&4u16.to_le_bytes());
        u.extend_from_slice(&0u16.to_le_bytes());
        u.extend_from_slice(&body);
        u.extend_from_slice(&st);
        u
    }

    fn def(key: u64, name: u32) -> Vec<u8> {
        let mut r = vec![0u8; 48];
        r[0..8].copy_from_slice(&key.to_le_bytes());
        r[24..28].copy_from_slice(&name.to_le_bytes());
        r[30] = 1;
        r
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn reads_the_test_producers_units() {
        // a.o: one unit (summary, 5 definitions, 1 type); b.o: two units
        // with a padding word between them (`meta dump` says the same).
        let a = sections("a.o");
        assert_eq!(a.len(), 1);
        let s = read(&a[0]);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.units.len(), 1);
        let kinds: Vec<u16> = s.units[0].blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [1, 2, 3]);
        assert_eq!(s.units[0].block(2, KIND2_ROW).unwrap().rows().count(), 5);
        let b = sections("b.o");
        let s = read(&b[0]);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.units.len(), 2);
        let k = kept(s.units.iter());
        assert_eq!(k.len(), s.units[0].bytes.len() + s.units[1].bytes.len());
        // The named row of b.o: `_Z2hvv`, its key FNV-1a 64 of the name.
        let u = &s.units[0];
        let named =
            u.block(2, KIND2_ROW).unwrap().rows().find(|r| u32_at(r, 24) != Some(0)).unwrap();
        assert_eq!(u.string(u32_at(named, 24).unwrap()), Some(&b"_Z2hvv"[..]));
        assert_eq!(u64_at(named, 0), Some(fnv1a(b"_Z2hvv")));
    }

    #[test]
    fn fnv1a_is_mt13s() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// Each malformation of MT-14 and MT-35: one problem, no unit, and the
    /// next unit read when the framing says where it is.
    #[test]
    fn malformed_units_are_named_and_skipped() {
        let good = unit(&[(2, 48, &def(5, 0))], b"");
        let named = |s: &[u8]| unit(&[(2, 48, &[def(1, 1), def(1, 3)].concat())], s);
        let mut cases: Vec<(&str, Vec<u8>, bool)> = Vec::new();
        let mut m = good.clone();
        m[0] = b'X';
        cases.push(("wrong magic", m, false));
        let mut m = good.clone();
        m[8] = 0x44;
        cases.push(("unit_size", m, false));
        let mut m = good.clone();
        m[4] = 2;
        cases.push(("major", m, true));
        let rows = [def(1, 0), def(2, 0)].concat();
        let two = unit(&[(3, 48, &rows), (2, 48, &rows)], b"");
        cases.push(("kinds not strictly increasing", two, true));
        let mut m = good.clone();
        m[44] = 2; // row_count 2 in a block of one row
        cases.push(("row_count x row_size", m, true));
        let unsorted = unit(&[(2, 48, &[def(2, 0), def(1, 0)].concat())], b"");
        cases.push(("rows not sorted", unsorted, true));
        cases.push(("rows not sorted", named(b"\0b\0a\0"), true));
        cases.push(("does not start with the empty string", named(b"xa\0b\0"), true));
        cases.push(("a string offset", unit(&[(2, 48, &def(1, 40))], b"\0a\0"), true));
        let mut m = named(b"\0a\0b\0");
        m[20] = 8; // the string table over the header
        cases.push(("overlaps the header", m, true));
        let mut m = named(b"\0a\0b\0");
        m[24] = 200;
        cases.push(("runs past the unit", m, true));
        for (what, bad, next_read) in cases {
            let mut sec = bad.clone();
            sec.extend_from_slice(&good);
            let s = read(&sec);
            assert_eq!(s.problems.len(), 1, "{what}: {:?}", s.problems);
            assert!(s.problems[0].contains(what), "{what}: {:?}", s.problems);
            assert_eq!(s.units.len(), usize::from(next_read), "{what}");
        }
        // Two named rows of one key in name order, and padding: fine.
        let mut sec = vec![0u8; 16];
        sec.extend_from_slice(&named(b"\0a\0b\0"));
        let s = read(&sec);
        assert!(s.problems.is_empty() && s.units.len() == 1, "{:?}", s.problems);
    }

    /// The reader never panics: every truncation and every single-byte
    /// change of the fixtures' containers.
    #[test]
    fn hostile_containers_never_panic() {
        for name in ["a.o", "b.o"] {
            for sec in sections(name) {
                for n in 0..sec.len() {
                    let _ = read(&sec[..n]);
                }
                for i in 0..sec.len() {
                    for v in [0u8, 1, 0x7f, 0x80, 0xff] {
                        let mut m = sec.clone();
                        m[i] = v;
                        let s = read(&m);
                        let _ = kept(s.units.iter());
                    }
                }
            }
        }
    }
}
