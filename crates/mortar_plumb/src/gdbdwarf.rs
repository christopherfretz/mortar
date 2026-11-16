//! The DWARF that `.gdb_index` describes, read for its rules
//! (`spec/plumb.md` §6.10; `spec/synth.md` S-G2, S-G3a, S-G5): the unit
//! headers of a `.debug_info`, a unit's root DIE, and the address ranges
//! the root DIE states. One reader serves two views: the output's
//! relocated bytes (every value as gdb reads it, P-G2, P-G3) and an
//! input object's bytes read through its relocations (S-G5's terms, a
//! section and an offset, P-G7, P-G8). Every read is bounded by its
//! section: what cannot be read is an `Err` naming it, never a panic.

#![allow(clippy::many_single_char_names)] // byte readers: r, p, b, v, w, s, t

/// A little-endian cursor over one section's bytes.
pub(crate) struct Rd<'a> {
    pub b: &'a [u8],
    pub p: usize,
}

impl<'a> Rd<'a> {
    pub fn new(b: &'a [u8], p: usize) -> Self {
        Rd { b, p }
    }

    pub fn n(&mut self, w: usize) -> Result<u64, String> {
        let end = self.p.checked_add(w).filter(|&e| e <= self.b.len() && w <= 8);
        let Some(end) = end else {
            return Err(format!("a read of {w} bytes at {:#x} past the end", self.p));
        };
        let v = self.b[self.p..end].iter().rev().fold(0u64, |a, &c| a << 8 | u64::from(c));
        self.p = end;
        Ok(v)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        self.n(1).map(|v| v as u8)
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        self.n(2).map(|v| v as u16)
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        self.n(4).map(|v| v as u32)
    }

    pub fn skip(&mut self, n: u64) -> Result<(), String> {
        let end = usize::try_from(n)
            .ok()
            .and_then(|n| self.p.checked_add(n))
            .filter(|&e| e <= self.b.len());
        let Some(end) = end else {
            return Err(format!("a skip of {n:#x} bytes at {:#x} past the end", self.p));
        };
        self.p = end;
        Ok(())
    }

    pub fn uleb(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let c = self.u8()?;
            if shift < 64 {
                v |= u64::from(c & 0x7f) << shift;
            }
            shift = shift.saturating_add(7);
            if c & 0x80 == 0 {
                return Ok(v);
            }
        }
    }

    pub fn sleb(&mut self) -> Result<i64, String> {
        let mut v = 0i64;
        let mut shift = 0u32;
        loop {
            let c = self.u8()?;
            if shift < 64 {
                v |= i64::from(c & 0x7f) << shift;
            }
            shift = shift.saturating_add(7);
            if c & 0x80 == 0 {
                if shift < 64 && c & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Ok(v);
            }
        }
    }

    /// A NUL-terminated string; the bytes without the NUL.
    pub fn cstr(&mut self) -> Result<&'a [u8], String> {
        let rest = self.b.get(self.p..).unwrap_or(&[]);
        let Some(n) = rest.iter().position(|&c| c == 0) else {
            return Err(format!("a string at {:#x} without its NUL", self.p));
        };
        let s = &rest[..n];
        self.p += n + 1;
        Ok(s)
    }
}

/// `DW_UT_type`, `DW_UT_split_type`: type units (S-G2).
pub(crate) const UT_TYPE: u8 = 2;
pub(crate) const UT_SPLIT_TYPE: u8 = 6;
const UT_COMPILE: u8 = 1;
const UT_PARTIAL: u8 = 3;
pub(crate) const UT_SKELETON: u8 = 4;
const UT_SPLIT_COMPILE: u8 = 5;

/// A unit header of a `.debug_info` (S-G2).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Unit {
    /// The unit's offset in its section.
    pub off: u64,
    /// Its length, the 4-byte length field included.
    pub len: u64,
    pub version: u16,
    /// The DWARF 5 unit type; 0 before DWARF 5.
    pub ut: u8,
    pub addr_size: u8,
    /// The offset of the `debug_abbrev_offset` field in the section.
    pub abbrev_field: u64,
    /// The offset of the root DIE in the section.
    pub die: u64,
}

impl Unit {
    pub fn is_type_unit(&self) -> bool {
        self.version == 5 && matches!(self.ut, UT_TYPE | UT_SPLIT_TYPE)
    }
}

/// The units of a `.debug_info`'s bytes, from offset 0 to the end: the
/// units read, and the first that cannot be (its offset and why).
pub(crate) fn units(b: &[u8]) -> (Vec<Unit>, Option<(u64, String)>) {
    let mut v = Vec::new();
    let mut p = 0usize;
    while p < b.len() {
        match unit_at(b, p) {
            Ok(u) => {
                p = (u.off + u.len) as usize;
                v.push(u);
            }
            Err(e) => return (v, Some((p as u64, e))),
        }
    }
    (v, None)
}

fn unit_at(b: &[u8], p: usize) -> Result<Unit, String> {
    let mut r = Rd::new(b, p);
    let len = r.u32()?;
    if len == 0xffff_ffff {
        return Err("a DWARF64 unit".into());
    }
    let end = p.checked_add(4).and_then(|e| e.checked_add(len as usize));
    if end.is_none_or(|e| e > b.len()) {
        return Err(format!("unit_length {len:#x} runs past the section's end"));
    }
    let version = r.u16()?;
    let (ut, addr_size, abbrev_field);
    match version {
        2..=4 => {
            ut = 0;
            abbrev_field = r.p as u64;
            r.u32()?;
            addr_size = r.u8()?;
        }
        5 => {
            ut = r.u8()?;
            addr_size = r.u8()?;
            abbrev_field = r.p as u64;
            r.u32()?;
            match ut {
                UT_SKELETON | UT_SPLIT_COMPILE => r.skip(8)?,
                UT_TYPE | UT_SPLIT_TYPE => r.skip(12)?,
                _ => {}
            }
        }
        _ => return Err(format!("version {version}")),
    }
    let u = Unit {
        off: p as u64,
        len: 4 + u64::from(len),
        version,
        ut,
        addr_size,
        abbrev_field,
        die: r.p as u64,
    };
    if u.die >= u.off + u.len {
        return Err("a unit header longer than its unit".into());
    }
    Ok(u)
}

/// An abbreviation: tag, children, `(attribute, form, implicit const)`.
#[derive(Debug, Clone)]
pub(crate) struct Abbrev {
    pub tag: u64,
    pub children: bool,
    pub attrs: Vec<(u64, u64, i64)>,
}

/// The abbreviation `code` of the table at `off` of `b`.
pub(crate) fn abbrev(b: &[u8], off: u64, code: u64) -> Result<Abbrev, String> {
    let mut r = Rd::new(b, usize::try_from(off).map_err(|_| "an abbreviation offset".to_string())?);
    if r.p > b.len() {
        return Err(format!("abbreviation table offset {off:#x} past .debug_abbrev's end"));
    }
    loop {
        let c = r.uleb()?;
        if c == 0 {
            return Err(format!("abbreviation {code} not in the table at {off:#x}"));
        }
        let tag = r.uleb()?;
        let children = r.u8()? != 0;
        let mut attrs = Vec::new();
        loop {
            let a = r.uleb()?;
            let f = r.uleb()?;
            if a == 0 && f == 0 {
                break;
            }
            let k = if f == FORM_IMPLICIT_CONST { r.sleb()? } else { 0 };
            if c == code {
                attrs.push((a, f, k));
            }
        }
        if c == code {
            return Ok(Abbrev { tag, children, attrs });
        }
    }
}

const FORM_ADDR: u64 = 0x01;
const FORM_DATA2: u64 = 0x05;
const FORM_DATA4: u64 = 0x06;
const FORM_DATA8: u64 = 0x07;
const FORM_DATA1: u64 = 0x0b;
const FORM_UDATA: u64 = 0x0f;
const FORM_INDIRECT: u64 = 0x16;
const FORM_SEC_OFFSET: u64 = 0x17;
const FORM_ADDRX: u64 = 0x1b;
const FORM_IMPLICIT_CONST: u64 = 0x21;
const FORM_RNGLISTX: u64 = 0x23;
const FORM_ADDRX1: u64 = 0x29;
const FORM_ADDRX4: u64 = 0x2c;
const FORM_GNU_ADDR_INDEX: u64 = 0x1f01;

/// An attribute's value as read: where its field is (for a relocation),
/// or the number it holds.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Val {
    /// `DW_FORM_addr`: the field at this offset of `.debug_info`.
    Addr(u64),
    /// A constant (`data1`-`data8`, `udata`, `implicit_const`), with the
    /// field's offset and width (`data4`/`data8` are offsets before
    /// DWARF 4).
    Data {
        at: u64,
        w: u8,
        v: u64,
    },
    /// `DW_FORM_sec_offset`: the 4-byte field at this offset.
    SecOff(u64),
    /// An index into `.debug_addr` (`addrx*`, `GNU_addr_index`).
    Addrx(u64),
    /// `DW_FORM_rnglistx`.
    Rnglistx(u64),
    Other,
}

/// The root DIE of a unit.
#[derive(Debug, Clone)]
pub(crate) struct Root {
    pub tag: u64,
    pub children: bool,
    pub attrs: Vec<(u64, Val)>,
}

impl Root {
    pub fn get(&self, a: u64) -> Option<Val> {
        self.attrs.iter().find(|x| x.0 == a).map(|x| x.1)
    }
}

/// Reads one attribute value of `form` (S-G5's form list).
fn value(r: &mut Rd<'_>, mut form: u64, k: i64, u: &Unit) -> Result<Val, String> {
    let asz = u64::from(u.addr_size);
    let mut hops = 0;
    while form == FORM_INDIRECT {
        form = r.uleb()?;
        hops += 1;
        if hops > 4 {
            return Err("DW_FORM_indirect loops".into());
        }
    }
    let at = r.p as u64;
    let v = match form {
        FORM_ADDR => {
            r.skip(asz)?;
            Val::Addr(at)
        }
        FORM_DATA1 | FORM_DATA2 | FORM_DATA4 | FORM_DATA8 => {
            let w = match form {
                FORM_DATA1 => 1,
                FORM_DATA2 => 2,
                FORM_DATA4 => 4,
                _ => 8,
            };
            Val::Data { at, w, v: r.n(w as usize)? }
        }
        FORM_UDATA => Val::Data { at, w: 0, v: r.uleb()? },
        FORM_IMPLICIT_CONST => Val::Data { at, w: 0, v: k as u64 },
        FORM_SEC_OFFSET => {
            r.skip(4)?;
            Val::SecOff(at)
        }
        FORM_ADDRX | FORM_GNU_ADDR_INDEX => Val::Addrx(r.uleb()?),
        FORM_ADDRX1..=FORM_ADDRX4 => Val::Addrx(r.n((form - FORM_ADDRX1 + 1) as usize)?),
        FORM_RNGLISTX => Val::Rnglistx(r.uleb()?),
        // Skipped by size.
        0x03 => {
            let n = r.u16()?;
            r.skip(u64::from(n))?;
            Val::Other
        }
        0x04 => {
            let n = r.u32()?;
            r.skip(u64::from(n))?;
            Val::Other
        }
        0x08 => {
            r.cstr()?;
            Val::Other
        }
        0x09 | 0x18 => {
            let n = r.uleb()?;
            r.skip(n)?;
            Val::Other
        }
        0x0a => {
            let n = r.u8()?;
            r.skip(u64::from(n))?;
            Val::Other
        }
        0x0c | 0x11 | 0x25 => {
            r.skip(1)?;
            Val::Other
        }
        0x0d => {
            r.sleb()?;
            Val::Other
        }
        0x10 => {
            r.skip(if u.version == 2 { asz } else { 4 })?;
            Val::Other
        }
        0x12 | 0x26 => {
            r.skip(2)?;
            Val::Other
        }
        0x27 => {
            r.skip(3)?;
            Val::Other
        }
        0x0e | 0x13 | 0x1c | 0x1d | 0x1f | 0x28 | 0x1f20 | 0x1f21 => {
            r.skip(4)?;
            Val::Other
        }
        0x14 | 0x20 | 0x24 => {
            r.skip(8)?;
            Val::Other
        }
        0x1e => {
            r.skip(16)?;
            Val::Other
        }
        0x15 | 0x1a | 0x22 | 0x1f02 => {
            r.uleb()?;
            Val::Other
        }
        0x19 => Val::Other,
        _ => return Err(format!("form {form:#x} in the root DIE")),
    };
    Ok(v)
}

/// Reads unit `u`'s root DIE from `info` with the abbreviation table
/// `abbrevs` at `abbrev_off`.
pub(crate) fn root(info: &[u8], u: &Unit, abbrevs: &[u8], abbrev_off: u64) -> Result<Root, String> {
    let end = (u.off + u.len) as usize;
    let mut r = Rd::new(info.get(..end).unwrap_or(info), u.die as usize);
    let code = r.uleb()?;
    if code == 0 {
        return Err("the root DIE is a null entry".into());
    }
    let a = abbrev(abbrevs, abbrev_off, code)?;
    let mut attrs = Vec::with_capacity(a.attrs.len());
    for &(at, form, k) in &a.attrs {
        attrs.push((at, value(&mut r, form, k, u)?));
    }
    Ok(Root { tag: a.tag, children: a.children, attrs })
}

pub(crate) const TAG_COMPILE_UNIT: u64 = 0x11;
pub(crate) const TAG_PARTIAL_UNIT: u64 = 0x3c;
const AT_LOW_PC: u64 = 0x11;
const AT_HIGH_PC: u64 = 0x12;
const AT_RANGES: u64 = 0x55;
const AT_ADDR_BASE: u64 = 0x73;
const AT_RNGLISTS_BASE: u64 = 0x74;
pub(crate) const AT_DWO_NAME: u64 = 0x76;
pub(crate) const AT_GNU_DWO_NAME: u64 = 0x2130;
const AT_GNU_ADDR_BASE: u64 = 0x2133;

/// What a unit is for S-G2: a compile unit, or why not.
pub(crate) fn kind(u: &Unit, r: &Root) -> Result<(), String> {
    if u.version == 5 {
        match u.ut {
            UT_COMPILE | UT_SKELETON => {}
            UT_TYPE | UT_SPLIT_TYPE => return Err("a type unit".into()),
            UT_PARTIAL => return Err("a partial unit".into()),
            t => return Err(format!("unit type {t}")),
        }
    }
    match r.tag {
        TAG_COMPILE_UNIT => Ok(()),
        TAG_PARTIAL_UNIT => Err("a partial unit (DW_TAG_partial_unit)".into()),
        t if u.version < 5 => Err(format!("a root DIE of tag {t:#x}")),
        _ => Ok(()),
    }
}

/// S-G3a: a skeleton, a root DIE naming a `.dwo`, or one with children.
pub(crate) fn debug_bearing(u: &Unit, r: &Root) -> bool {
    (u.version == 5 && u.ut == UT_SKELETON)
        || r.get(AT_DWO_NAME).is_some()
        || r.get(AT_GNU_DWO_NAME).is_some()
        || r.children
}

/// An address or a section offset as a view gives it: the section it is
/// relative to (an input section index, through a relocation) or none,
/// and the number.
pub(crate) type Addr = (Option<usize>, u64);

/// How the reader sees a file's sections (the output's, or one input's).
pub(crate) trait View {
    /// The bytes of section `s`.
    fn bytes(&self, s: usize) -> &[u8];
    /// The value of the `w`-byte field at `off` of section `s`: through
    /// the relocation there in an input (its symbol's section and
    /// `st_value + addend`), the number itself otherwise.
    fn field(&self, s: usize, off: u64, w: u8) -> Result<Addr, String>;
    /// The section named `name` a 4-byte section offset at `off` of `s`
    /// points into, and the offset in it.
    fn offset_into(&self, s: usize, off: u64, name: &str) -> Result<(usize, u64), String>;
    /// Whether this is an input's view (S-G5's rule 3 applies).
    fn input(&self) -> bool;
}

/// A range `[lo, hi)` relative to a section (or none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Range {
    pub sec: Option<usize>,
    pub lo: u64,
    pub hi: u64,
}

/// `a + b`, at most one of them relative to a section.
fn add(a: Addr, b: Addr) -> Result<Addr, String> {
    match (a.0, b.0) {
        (Some(_), Some(_)) => Err("an address relative to two sections".into()),
        (s, None) | (None, s) => Ok((s, a.1.wrapping_add(b.1))),
    }
}

fn range<V: View>(v: &V, lo: Addr, hi: Addr) -> Result<Range, String> {
    if v.input() {
        if lo.0 != hi.0 {
            return Err("a range whose ends are in two sections".into());
        }
        if hi.1 < lo.1 {
            return Err("a range of negative length".into());
        }
    }
    Ok(Range { sec: lo.0, lo: lo.1, hi: hi.1 })
}

/// The ranges unit `u` of section `info` states in its root DIE (S-G5),
/// in the order its DIE and range list state them.
#[allow(clippy::too_many_lines)] // one arm per entry kind
pub(crate) fn ranges<V: View>(v: &V, info: usize, u: &Unit) -> Result<(Root, Vec<Range>), String> {
    let (asec, aoff) = v.offset_into(info, u.abbrev_field, ".debug_abbrev")?;
    let root = root(v.bytes(info), u, v.bytes(asec), aoff)?;
    let asz = u.addr_size;
    let addr_base = match root.get(AT_ADDR_BASE).or_else(|| root.get(AT_GNU_ADDR_BASE)) {
        Some(Val::SecOff(at) | Val::Data { at, w: 4, .. }) => {
            Some(v.offset_into(info, at, ".debug_addr")?)
        }
        Some(_) => return Err("DW_AT_addr_base of another form".into()),
        None => None,
    };
    let addrx = |i: u64| -> Result<Addr, String> {
        let Some((s, base)) = addr_base else {
            return Err("an address index without DW_AT_addr_base".into());
        };
        let at = i.checked_mul(u64::from(asz)).and_then(|o| o.checked_add(base));
        let Some(at) = at else { return Err("an address index out of range".into()) };
        v.field(s, at, asz)
    };
    let addr_of = |val: Val| -> Result<Addr, String> {
        match val {
            Val::Addr(at) => v.field(info, at, asz),
            Val::Addrx(i) => addrx(i),
            _ => Err("an address attribute of another form".into()),
        }
    };
    let low = match root.get(AT_LOW_PC) {
        Some(x) => Some(addr_of(x)?),
        None => None,
    };
    let mut out = Vec::new();
    if let Some(rv) = root.get(AT_RANGES) {
        let base0 = low.unwrap_or((None, 0));
        if u.version >= 5 {
            let (rs, off) = match rv {
                Val::SecOff(at) => v.offset_into(info, at, ".debug_rnglists")?,
                Val::Rnglistx(i) => {
                    let Some(Val::SecOff(at) | Val::Data { at, w: 4, .. }) =
                        root.get(AT_RNGLISTS_BASE)
                    else {
                        return Err("DW_FORM_rnglistx without DW_AT_rnglists_base".into());
                    };
                    let (rs, base) = v.offset_into(info, at, ".debug_rnglists")?;
                    let slot = i.checked_mul(4).and_then(|o| o.checked_add(base));
                    let Some(slot) = slot else {
                        return Err("a range list index out of range".into());
                    };
                    let mut r = Rd::new(v.bytes(rs), usize::try_from(slot).unwrap_or(usize::MAX));
                    let rel = r.u32()?;
                    (rs, base.wrapping_add(u64::from(rel)))
                }
                _ => return Err("DW_AT_ranges of another form".into()),
            };
            let b = v.bytes(rs);
            let mut r = Rd::new(b, usize::try_from(off).unwrap_or(usize::MAX));
            if r.p > b.len() {
                return Err(format!("range list offset {off:#x} past .debug_rnglists's end"));
            }
            let mut base = base0;
            loop {
                let code = r.u8()?;
                match code {
                    0 => break,
                    1 => base = addrx(r.uleb()?)?,
                    2 => {
                        let a = addrx(r.uleb()?)?;
                        let e = addrx(r.uleb()?)?;
                        out.push(range(v, a, e)?);
                    }
                    3 => {
                        let a = addrx(r.uleb()?)?;
                        let n = r.uleb()?;
                        out.push(range(v, a, (a.0, a.1.wrapping_add(n)))?);
                    }
                    4 => {
                        let x = r.uleb()?;
                        let y = r.uleb()?;
                        out.push(range(v, add(base, (None, x))?, add(base, (None, y))?)?);
                    }
                    5 => {
                        let at = r.p as u64;
                        r.skip(u64::from(asz))?;
                        base = v.field(rs, at, asz)?;
                    }
                    6 => {
                        let at = r.p as u64;
                        r.skip(2 * u64::from(asz))?;
                        let a = v.field(rs, at, asz)?;
                        let e = v.field(rs, at + u64::from(asz), asz)?;
                        out.push(range(v, a, e)?);
                    }
                    7 => {
                        let at = r.p as u64;
                        r.skip(u64::from(asz))?;
                        let a = v.field(rs, at, asz)?;
                        let n = r.uleb()?;
                        out.push(range(v, a, (a.0, a.1.wrapping_add(n)))?);
                    }
                    c => return Err(format!("range list entry code {c}")),
                }
            }
        } else {
            let (Val::SecOff(at) | Val::Data { at, w: 4, .. }) = rv else {
                return Err("DW_AT_ranges of another form".into());
            };
            let (rs, off) = v.offset_into(info, at, ".debug_ranges")?;
            let b = v.bytes(rs);
            let w = u64::from(asz);
            let ones = if asz >= 8 { u64::MAX } else { (1u64 << (8 * u32::from(asz))) - 1 };
            let mut p = off;
            let mut base = base0;
            loop {
                if p.checked_add(2 * w).is_none_or(|e| e > b.len() as u64) {
                    return Err(format!("a .debug_ranges list at {off:#x} without its end"));
                }
                let a = v.field(rs, p, asz)?;
                let e = v.field(rs, p + w, asz)?;
                p += 2 * w;
                if a == (None, 0) && e == (None, 0) {
                    break;
                }
                if a == (None, ones) {
                    base = e;
                    continue;
                }
                out.push(range(v, add(base, a)?, add(base, e)?)?);
            }
        }
    } else if let (Some(lo), Some(hv)) = (low, root.get(AT_HIGH_PC)) {
        let hi = match hv {
            Val::Data { v: n, .. } => (lo.0, lo.1.wrapping_add(n)),
            x => addr_of(x)?,
        };
        out.push(range(v, lo, hi)?);
    }
    Ok((root, out))
}
