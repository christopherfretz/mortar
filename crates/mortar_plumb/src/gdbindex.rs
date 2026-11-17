//! `.gdb_index`, the format rules (`spec/plumb.md` §6.10, P-G1 to P-G5):
//! what every producer's index must be for gdb to read it right, judged
//! against the output's own debug information. Mortar's index is then
//! held to `spec/synth.md` §17 exactly by [`crate::gdbnames`] (P-G6 to
//! P-G9). The index is untrusted: every offset, count and probe is
//! bounded, and what cannot be read is a finding.

use std::collections::BTreeMap;

use mortar_elf::consts::{SHF_ALLOC, SHF_COMPRESSED};

use crate::gdbdwarf::{self, Addr, Range, Unit, View};
use crate::image::Image;
use crate::locate::Obj;
use crate::map::{Format, Map};
use crate::report::{Finding, Rule};

/// The section's name.
pub(crate) const NAME: &str = ".gdb_index";

pub(crate) fn finding(n: u8, msg: impl Into<String>) -> Finding {
    Finding::new(Rule::GdbIndex(n), format!("{NAME}: {}", msg.into()))
}

/// gdb's `mapped_index_string_hash`, versions 5 and later (S-G7).
pub(crate) fn hash(name: &[u8]) -> u32 {
    name.iter().fold(0u32, |h, &c| {
        h.wrapping_mul(67).wrapping_add(u32::from(c.to_ascii_lowercase())).wrapping_sub(113)
    })
}

/// The probe sequence of `h` in a table of `t` slots (a power of two):
/// at most `t` slots, the first `h & m`, then by `((h × 17) & m) | 1`.
pub(crate) fn probe(h: u32, t: u32) -> impl Iterator<Item = u32> {
    let m = t.wrapping_sub(1);
    let step = (h.wrapping_mul(17) & m) | 1;
    let mut slot = h & m;
    (0..t).map(move |_| {
        let s = slot;
        slot = slot.wrapping_add(step) & m;
        s
    })
}

/// A parsed index (P-G1).
#[derive(Debug)]
pub(crate) struct Index<'a> {
    pub data: &'a [u8],
    pub version: u32,
    /// CU list, TU list, address area, symbol table, shortcut table
    /// (version 9; else the constant pool's), constant pool.
    pub offs: [u32; 6],
    pub cus: Vec<(u64, u64)>,
    pub tus: usize,
    pub area: Vec<(u64, u64, u32)>,
    pub slots: Vec<(u32, u32)>,
}

impl<'a> Index<'a> {
    pub fn pool(&self) -> &'a [u8] {
        &self.data[self.offs[5] as usize..]
    }
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from(u32_at(b, o)) | u64::from(u32_at(b, o + 4)) << 32
}

/// P-G1: the header and the areas.
pub(crate) fn parse(data: &[u8]) -> Result<Index<'_>, String> {
    if data.len() < 4 {
        return Err(format!("{} bytes, shorter than a header", data.len()));
    }
    let version = u32_at(data, 0);
    if !(7..=9).contains(&version) {
        return Err(format!("version {version} (gdb reads 7, 8 and 9)"));
    }
    let head = if version == 9 { 28 } else { 24 };
    if data.len() < head {
        return Err(format!("{} bytes, shorter than a version-{version} header", data.len()));
    }
    let w: Vec<u32> = (1..head / 4).map(|i| u32_at(data, 4 * i)).collect();
    let offs = if version == 9 {
        [w[0], w[1], w[2], w[3], w[4], w[5]]
    } else {
        [w[0], w[1], w[2], w[3], w[4], w[4]]
    };
    if offs[0] as usize != head {
        return Err(format!("the CU list at {:#x}, not at the header's end {head:#x}", offs[0]));
    }
    for i in 0..5 {
        if offs[i] > offs[i + 1] {
            return Err(format!(
                "the offsets are not in order ({:#x} before {:#x})",
                offs[i],
                offs[i + 1]
            ));
        }
    }
    if offs[5] as usize > data.len() {
        return Err(format!(
            "the constant pool at {:#x}, past the section's end {:#x}",
            offs[5],
            data.len()
        ));
    }
    let names = ["CU list", "TU list", "address area", "symbol table", "shortcut table"];
    for (i, sz) in [16u32, 24, 20, 8, 8].into_iter().enumerate() {
        let n = offs[i + 1] - offs[i];
        if n % sz != 0 {
            return Err(format!("the {} is {n:#x} bytes, not whole {sz}-byte entries", names[i]));
        }
    }
    let at = |i: usize| offs[i] as usize;
    let cus = (at(0)..at(1)).step_by(16).map(|o| (u64_at(data, o), u64_at(data, o + 8))).collect();
    let tus = (at(2) - at(1)) / 24;
    let area = (at(2)..at(3))
        .step_by(20)
        .map(|o| (u64_at(data, o), u64_at(data, o + 8), u32_at(data, o + 16)))
        .collect();
    let slots = (at(3)..at(4)).step_by(8).map(|o| (u32_at(data, o), u32_at(data, o + 4))).collect();
    Ok(Index { data, version, offs, cus, tus, area, slots })
}

/// The output's sections as the reader sees them: relocated values.
pub(crate) struct OutView<'i, 'a> {
    pub image: &'i Image<'a>,
}

impl View for OutView<'_, '_> {
    fn bytes(&self, s: usize) -> &[u8] {
        self.image.section_data(s).unwrap_or(&[])
    }
    fn field(&self, s: usize, off: u64, w: u8) -> Result<Addr, String> {
        let b = self.bytes(s);
        let mut r = gdbdwarf::Rd::new(b, usize::try_from(off).unwrap_or(usize::MAX));
        Ok((None, r.n(w as usize)?))
    }
    fn offset_into(&self, s: usize, off: u64, name: &str) -> Result<(usize, u64), String> {
        let (_, v) = self.field(s, off, 4)?;
        let t =
            self.image.nonalloc_section(name).ok_or_else(|| format!("no {name} in the output"))?;
        Ok((t, v))
    }
    fn input(&self) -> bool {
        false
    }
}

/// One slot of the symbol table that holds a name, read (P-G4, P-G5).
#[derive(Debug)]
pub(crate) struct Entry<'a> {
    pub slot: u32,
    pub name: &'a [u8],
    pub values: Vec<u32>,
}

/// The output's `.debug_info` units (P-G2's walk): the section, its
/// units, and the first that cannot be read.
pub(crate) struct OutUnits {
    pub sec: Option<usize>,
    pub units: Vec<Unit>,
    pub error: Option<(u64, String)>,
}

pub(crate) fn out_units(image: &Image<'_>) -> OutUnits {
    let Some(sec) = image.nonalloc_section(".debug_info") else {
        return OutUnits { sec: None, units: Vec::new(), error: None };
    };
    let (units, error) = gdbdwarf::units(image.section_data(sec).unwrap_or(&[]));
    OutUnits { sec: Some(sec), units, error }
}

/// The allocated sections of the output, `[start, end)`.
fn alloc_extents(image: &Image<'_>) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = image
        .sections
        .iter()
        .filter(|s| s.shdr.sh_flags & SHF_ALLOC != 0 && s.shdr.sh_size > 0)
        .map(|s| (s.shdr.sh_addr, s.shdr.sh_addr.saturating_add(s.shdr.sh_size)))
        .collect();
    v.sort_unstable();
    v
}

fn inside(ext: &[(u64, u64)], lo: u64, hi: u64) -> bool {
    lo < hi && ext.iter().any(|&(a, b)| a <= lo && hi <= b)
}

/// Judges every `.gdb_index` of the output (§6.10).
/// The findings, and a line for the coverage report when the output
/// has an index.
pub(crate) fn check(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    line: Option<&[String]>,
) -> (Vec<Finding>, Option<String>) {
    let mut out = Vec::new();
    let found: Vec<usize> =
        image.sections.iter().enumerate().filter(|(_, s)| s.name == NAME).map(|(i, _)| i).collect();
    let mortar = map.format == Format::Mortar;
    if mortar {
        crate::gdbnames::presence(image, objs, map, line, &found, &mut out);
    }
    let Some(&si) = found.first() else { return (out, None) };
    let s = &image.sections[si];
    if s.shdr.sh_flags & SHF_ALLOC != 0 {
        out.push(finding(1, "the section is allocated"));
    }
    let Some(data) = image.section_data(si) else {
        out.push(finding(1, "the section has no bytes in the file"));
        return (out, Some("unreadable".into()));
    };
    let index = match parse(data) {
        Ok(i) => i,
        Err(e) => {
            out.push(finding(1, e));
            return (out, Some("unreadable".into()));
        }
    };
    let units = out_units(image);
    let compressed =
        units.sec.is_some_and(|i| image.sections[i].shdr.sh_flags & SHF_COMPRESSED != 0);
    let before = out.len();
    if !compressed {
        cu_list(&index, &units, &mut out);
        area(image, &index, &units, mortar, &mut out);
    }
    let entries = table(&index, &mut out);
    if mortar {
        let clean = out.len() == before;
        crate::gdbnames::content(image, objs, map, &index, &units, &entries, clean, &mut out);
    }
    let line = format!(
        "version {}, {} units, {} address entries, {} names; rules {}{}",
        index.version,
        index.cus.len(),
        index.area.len(),
        entries.len(),
        if mortar { "P-G1 to P-G9" } else { "P-G1 to P-G5" },
        if compressed { " (P-G2, P-G3 not judged: compressed .debug_info)" } else { "" }
    );
    (out, Some(line))
}

/// P-G2.
fn cu_list(index: &Index<'_>, units: &OutUnits, out: &mut Vec<Finding>) {
    if let Some((off, e)) = &units.error {
        out.push(finding(2, format!("the output's .debug_info cannot be walked at {off:#x}: {e}")));
    }
    let want: Vec<(u64, u64)> =
        units.units.iter().filter(|u| !u.is_type_unit()).map(|u| (u.off, u.len)).collect();
    let n = want.len().max(index.cus.len());
    let mut said = 0;
    for i in 0..n {
        let (w, f) = (want.get(i), index.cus.get(i));
        if w == f {
            continue;
        }
        said += 1;
        if said > 8 {
            continue;
        }
        let msg = match (w, f) {
            (Some(w), Some(f)) => format!(
                "CU list entry {i} is (offset {:#x}, length {:#x}); the output's .debug_info has unit (offset {:#x}, length {:#x})",
                f.0, f.1, w.0, w.1
            ),
            (Some(w), None) => format!(
                "the CU list has no entry {i}; the output's .debug_info has unit (offset {:#x}, length {:#x})",
                w.0, w.1
            ),
            (None, Some(f)) => format!(
                "CU list entry {i} (offset {:#x}, length {:#x}) is not a unit of the output's .debug_info",
                f.0, f.1
            ),
            (None, None) => unreachable!("one side has entry i"),
        };
        out.push(finding(2, msg));
    }
    if said > 8 {
        out.push(finding(2, format!("and {} more CU list entries differ", said - 8)));
    }
}

/// The live claimed ranges of the unit at `off`, or why they cannot be
/// read (P-G3).
fn claims(
    image: &Image<'_>,
    units: &OutUnits,
    off: u64,
    ext: &[(u64, u64)],
) -> Result<Vec<Range>, String> {
    let (Some(sec), Some(u)) = (units.sec, units.units.iter().find(|u| u.off == off)) else {
        return Err(format!("no unit of the output's .debug_info at {off:#x}"));
    };
    let v = OutView { image };
    let (_, rs) = gdbdwarf::ranges(&v, sec, u)?;
    // A producer may write adjacent ranges of a unit as one entry (gdb's
    // own `gdb-add-index` does): the live ranges, merged where they touch.
    let mut live: Vec<Range> = rs.into_iter().filter(|r| inside(ext, r.lo, r.hi)).collect();
    live.sort_unstable_by_key(|r| (r.lo, r.hi));
    let mut merged: Vec<Range> = Vec::with_capacity(live.len());
    for r in live {
        match merged.last_mut() {
            Some(m) if r.lo <= m.hi => m.hi = m.hi.max(r.hi),
            _ => merged.push(r),
        }
    }
    Ok(merged)
}

/// P-G3. The unit's own ranges bound an entry in mortar's output only:
/// gdb 16's `gdb-add-index` also writes the addresses of a unit's
/// variables (a reference's area may hold more than S-G5's).
fn area(
    image: &Image<'_>,
    index: &Index<'_>,
    units: &OutUnits,
    mortar: bool,
    out: &mut Vec<Finding>,
) {
    let ext = alloc_extents(image);
    let mut cache: BTreeMap<u32, Result<Vec<Range>, String>> = BTreeMap::new();
    let mut said = 0usize;
    let mut say = |out: &mut Vec<Finding>, m: String| {
        said += 1;
        if said <= 8 {
            out.push(finding(3, m));
        }
    };
    for (i, &(lo, hi, cu)) in index.area.iter().enumerate() {
        let what = format!("address entry {i} [{lo:#x}, {hi:#x}) of unit {cu}");
        let Some(&(off, _)) = index.cus.get(cu as usize) else {
            say(
                out,
                format!("{what}: the unit is not in the CU list ({} entries)", index.cus.len()),
            );
            continue;
        };
        if lo >= hi {
            say(out, format!("{what} is empty"));
            continue;
        }
        if !inside(&ext, lo, hi) {
            say(
                out,
                format!(
                    "{what} is not inside an allocated section of the output (a tombstone, or a dead function's address)"
                ),
            );
            continue;
        }
        if !mortar {
            continue;
        }
        let c = cache.entry(cu).or_insert_with(|| claims(image, units, off, &ext));
        match c {
            Err(e) => {
                let e = e.clone();
                say(out, format!("{what}: the unit's own ranges cannot be read: {e}"));
            }
            Ok(rs) if !rs.iter().any(|r| r.lo <= lo && hi <= r.hi) => {
                say(out, format!("{what} is not inside a range the unit's root DIE states"));
            }
            Ok(_) => {}
        }
    }
    if said > 8 {
        out.push(finding(3, format!("and {} more address entries", said - 8)));
    }
}

/// P-G4 and P-G5: the slots that hold names, read.
#[allow(clippy::too_many_lines)] // the rules in order
fn table<'a>(index: &Index<'a>, out: &mut Vec<Finding>) -> Vec<Entry<'a>> {
    let t = index.slots.len();
    let mut v = Vec::new();
    if t == 0 || !t.is_power_of_two() || t > u32::MAX as usize {
        out.push(finding(4, format!("the symbol table has {t} slots, not a power of two")));
        return v;
    }
    if !index.slots.contains(&(0, 0)) {
        out.push(finding(4, "the symbol table has no empty slot: gdb's lookup of a name it does not hold never ends"));
    }
    let pool = index.pool();
    let units = index.cus.len() + index.tus;
    let mut said = (0usize, 0usize);
    let mut bad = |out: &mut Vec<Finding>, n: u8, m: String| {
        let c = if n == 4 { &mut said.0 } else { &mut said.1 };
        *c += 1;
        if *c <= 8 {
            out.push(finding(n, m));
        }
    };
    for (slot, &(no, vo)) in index.slots.iter().enumerate() {
        if (no, vo) == (0, 0) {
            continue;
        }
        let at = format!("slot {slot}");
        let name =
            pool.get(no as usize..).and_then(|r| r.iter().position(|&c| c == 0).map(|n| &r[..n]));
        let Some(name) = name else {
            bad(
                out,
                4,
                format!(
                    "{at}: name offset {no:#x} is not a NUL-terminated string in the constant pool"
                ),
            );
            continue;
        };
        let count = pool.get(vo as usize..).filter(|r| r.len() >= 4).map(|r| u32_at(r, 0));
        let values: Option<Vec<u32>> = count.and_then(|c| {
            let n = 4usize.checked_mul(c as usize)?;
            let r = pool.get(vo as usize + 4..)?.get(..n)?;
            Some(r.chunks_exact(4).map(|x| u32_at(x, 0)).collect())
        });
        let Some(values) = values else {
            bad(
                out,
                4,
                format!(
                    "{at} '{}': vector offset {vo:#x} is not a vector inside the constant pool",
                    show(name)
                ),
            );
            continue;
        };
        v.push(Entry { slot: slot as u32, name, values });
    }
    // The probe: each entry is found from its own name's hash.
    let mut seen: crate::hasher::HashMap<&[u8], u32> = crate::hasher::HashMap::default();
    for e in &v {
        if let Some(first) = seen.insert(e.name, e.slot) {
            bad(out, 4, format!("'{}' is held by slots {first} and {}", show(e.name), e.slot));
        }
        let mut reached = false;
        for s in probe(hash(e.name), t as u32) {
            let (no, vo) = index.slots[s as usize];
            if s == e.slot {
                reached = true;
                break;
            }
            if (no, vo) == (0, 0) {
                break;
            }
            let other = pool
                .get(no as usize..)
                .and_then(|r| r.iter().position(|&c| c == 0).map(|n| &r[..n]));
            if other == Some(e.name) {
                break;
            }
        }
        if !reached {
            bad(
                out,
                4,
                format!(
                    "slot {} '{}': gdb's probe for the name does not reach the slot",
                    e.slot,
                    show(e.name)
                ),
            );
        }
        if e.values.is_empty() {
            bad(out, 5, format!("'{}': an empty CU vector", show(e.name)));
        }
        for &x in &e.values {
            let (cu, res, kind) = (x & 0x00ff_ffff, (x >> 24) & 0xf, (x >> 28) & 7);
            if cu as usize >= units {
                bad(
                    out,
                    5,
                    format!("'{}': value {x:#010x} names unit {cu}, of {units}", show(e.name)),
                );
            } else if res != 0 {
                bad(
                    out,
                    5,
                    format!("'{}': value {x:#010x} has reserved bits 24-27 set", show(e.name)),
                );
            } else if kind >= 5 {
                bad(
                    out,
                    5,
                    format!("'{}': value {x:#010x} has the reserved kind {kind}", show(e.name)),
                );
            }
        }
    }
    for (n, c) in [(4u8, said.0), (5, said.1)] {
        if c > 8 {
            out.push(finding(n, format!("and {} more", c - 8)));
        }
    }
    v
}

/// A name for a message.
pub(crate) fn show(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    if s.chars().count() > 80 {
        format!("{}...", s.chars().take(80).collect::<String>())
    } else {
        s.into_owned()
    }
}

/// The sweep's name for the byte at `off` of an index (§12.3): (kind,
/// instance).
pub(crate) fn field(data: &[u8], off: u64) -> (String, String) {
    let at = |k: &str, i: String| (k.to_string(), i);
    let Ok(index) = parse(data) else { return at("unreadable", String::new()) };
    let o = index.offs;
    let off32 = u32::try_from(off).unwrap_or(u32::MAX);
    let hdr = if index.version >= 9 { 28 } else { 24 };
    if off32 < hdr {
        const NAMES: [&str; 7] = [
            "version",
            "CU list offset",
            "TU list offset",
            "address area offset",
            "symbol table offset",
            "constant pool offset",
            "constant pool offset",
        ];
        let k = if index.version >= 9 && (20..24).contains(&off32) {
            "shortcut table offset"
        } else {
            NAMES[(off32 / 4) as usize]
        };
        return at(k, String::new());
    }
    if off32 < o[1] {
        let r = off32 - o[0];
        return at(if r % 16 < 8 { "CU offset" } else { "CU length" }, format!("unit {}", r / 16));
    }
    if off32 < o[2] {
        return at("TU entry", format!("type unit {}", (off32 - o[1]) / 24));
    }
    if off32 < o[3] {
        let r = off32 - o[2];
        let k = match r % 20 {
            0..8 => "address low",
            8..16 => "address high",
            _ => "address cu",
        };
        return at(k, format!("entry {}", r / 20));
    }
    if off32 < o[4] {
        let j = (off32 - o[3]) / 8;
        let slot = index.slots.get(j as usize).copied().unwrap_or((0, 0));
        if slot == (0, 0) {
            return at("empty slot", String::new());
        }
        let name = index
            .pool()
            .get(slot.0 as usize..)
            .and_then(|b| b.split(|&c| c == 0).next())
            .map(show)
            .unwrap_or_default();
        let k = if (off32 - o[3]) % 8 < 4 { "slot name offset" } else { "slot vector offset" };
        return at(k, format!("slot {j} '{name}'"));
    }
    if off32 < o[5] {
        return at("shortcut table", String::new());
    }
    // The pool: a vector's word or a name's byte.
    let p = off32 - o[5];
    let pool = index.pool();
    for &(n, v) in index.slots.iter().filter(|s| **s != (0, 0)) {
        let count = pool
            .get(v as usize..(v as usize).saturating_add(4))
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let end = v.saturating_add(4).saturating_add(count.saturating_mul(4));
        let name = pool.get(n as usize..).and_then(|b| b.split(|&c| c == 0).next()).unwrap_or(&[]);
        if p >= v && p < end {
            let k = if p < v + 4 { "vector count" } else { "vector value" };
            return at(k, format!("'{}'", show(name)));
        }
        if p >= n && p <= n.saturating_add(name.len() as u32) {
            let k = if p == n + name.len() as u32 { "name NUL" } else { "name" };
            return at(k, format!("'{}'", show(name)));
        }
    }
    at("pool (unreferenced)", String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/synth.md S-G7: the hash and the probe (gdb's manual).
    #[test]
    fn p_g4_hash_and_probe() {
        assert_eq!(hash(b""), 0);
        assert_eq!(hash(b"a"), u32::from(b'a').wrapping_sub(113));
        assert_eq!(hash(b"Main"), hash(b"main"));
        assert_ne!(hash(b"\xc3\x89"), hash(b"\xc3\xa9"), "lower() is ASCII-only");
        let slots: Vec<u32> = probe(5, 8).collect();
        assert_eq!(slots.len(), 8);
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8, "an odd step visits every slot");
        assert_eq!(slots[0], 5);
        assert_eq!(slots[1], (5 + ((5 * 17) & 7 | 1)) & 7);
    }

    fn header(v: u32, offs: &[u32]) -> Vec<u8> {
        let mut b = v.to_le_bytes().to_vec();
        for o in offs {
            b.extend_from_slice(&o.to_le_bytes());
        }
        b
    }

    // P-G1: each way a header can be wrong is an error, never a panic.
    #[test]
    fn p_g1_headers() {
        assert!(parse(&[]).is_err());
        assert!(parse(&header(6, &[24, 24, 24, 24, 24])).is_err());
        assert!(parse(&header(7, &[24, 24, 24])).is_err());
        let mut ok = header(7, &[24, 24, 24, 24, 32]);
        ok.extend_from_slice(&[0; 8]);
        assert_eq!(parse(&ok).unwrap().slots.len(), 1);
        assert!(
            parse(&header(7, &[28, 28, 28, 28, 28])).is_err(),
            "CU list not at the header's end"
        );
        assert!(parse(&header(7, &[24, 40, 24, 24, 24])).is_err(), "out of order");
        assert!(parse(&header(7, &[24, 24, 24, 24, 99])).is_err(), "pool past the end");
        let mut odd = header(7, &[24, 24, 24, 24, 28]);
        odd.extend_from_slice(&[0; 4]);
        assert!(parse(&odd).is_err(), "half a slot");
        let mut v9 = header(9, &[28, 28, 28, 28, 36, 36]);
        v9.extend_from_slice(&[0; 8]);
        assert_eq!(parse(&v9).unwrap().version, 9);
    }
}
