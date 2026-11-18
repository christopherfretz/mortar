//! mortar's own placement map (`spec/plumb.md` §3.2): a versioned,
//! line-oriented text format with one record per line.
//!
//! ```text
//! mortar-map 2
//! file <id> <path> [<member> <member-header-offset>]
//! place <file> <section> <addr> <size>
//! piece <file> <section> <input-offset> <addr> <size>
//! nplace <file> <section> <output-section> <offset> <size>
//! merged <file> <section> <output-section>
//! discard <file> <section> gc|comdat|script|unspecified
//! fold <file> <section> <into-file> <into-section>
//! chunk thunk|patch|synthetic|common <name> <addr>|- <size> [<target> <addend>]
//! common <name> <addr> <size> <align> [<file>]
//! define <name> <value>
//! incremental
//! hole <output-section> <offset> <size>
//! dead <output-section> <offset> <size> neutral|contribution|strings|cie
//! region <file> <out> <cat> <addr> <capacity>
//! capacity <out> <ordinal> <part> <addr>|- <size> <capacity>
//! order <out> <n>...
//! slot <kind> <key> <addr>
//! slot <kind> <file> <index> <addr>
//! slot tls-ld <addr>
//! redirect <file> <section> <offset> <addr>
//! frag <out> <class> <hash> <offset>
//! symblock <file>|- <first> <count> <capacity> <strtab-off> <strtab-cap>
//! output static|exec|pie|static-pie|relocatable
//! rplace <file> <section> <out> <offset> <size>
//! rframe <file> <section> <input-offset> <output-offset>|-
//! rgroup <out> <file> <section>
//! rcommon <name> <out> <offset> <size> <align>
//! interp <path>
//! runpath|rpath <paths>
//! bind lazy|now
//! relro on|off
//! arg <word>
//! needed <name>
//! copy <name> <addr> <size> <align>
//! export <name>
//! plt <name>
//! iplt <name>
//! end
//! ```
//!
//! The records from `output` on are the dynamic records (P-D3 of
//! `spec/dyn.md`): facts of the command line, then order hints. A
//! `NOBITS` synthetic chunk, `common` and the `<align>` of `copy` are the
//! allocation records (WP-096), judged by M6-M8 (§3.3).
//!
//! A `<section>` is the section's index in its file, in decimal, or `:`
//! and the section's name as a token (a foreign map converted, §3.2).
//!
//! The records from `region` to `symblock` are the decision records
//! (WP-212, `spec/image.md` I-78a; read by WP-222): only in a map that
//! says `incremental`, after its `incremental`, `hole` and `dead`
//! records, before the dynamic records, in this group order.

use std::fmt::Write as _;

use super::{
    Capacity, Chunk, ChunkKind, CommonAlloc, CopyHint, DeadKind, DeadRange, Defined, DiscardReason,
    Discarded, DynRecords, Fold, Format, Frag, Map, MapError, MapFile, NonAlloc, Order, OutputKind,
    Part, Placed, RCommon, RFrame, RGroup, RPlace, Redirect, Region, RelRecords, RunpathTag,
    SectionRef, Slot, SlotKey, SlotKind, SymBlock, dec, hex,
};

/// The decision records' words, in their group order (§3.2).
const DECISION_WORDS: [&str; 7] =
    ["region", "capacity", "order", "slot", "redirect", "frag", "symblock"];

/// The records that come before the decision records (§3.2's writer
/// order): a map's placement and allocation records and the incremental
/// records.
const BEFORE_DECISIONS: [&str; 13] = [
    "file",
    "place",
    "piece",
    "nplace",
    "merged",
    "discard",
    "fold",
    "chunk",
    "common",
    "define",
    "incremental",
    "hole",
    "dead",
];

/// The dynamic records and the line records, which follow the decision
/// records (§3.2).
const AFTER_DECISIONS: [&str; 15] = [
    "output",
    "interp",
    "runpath",
    "rpath",
    "bind",
    "relro",
    "symbolic",
    "dynamic-list",
    "version-script",
    "arg",
    "needed",
    "copy",
    "export",
    "plt",
    "iplt",
];

/// The first line of a version-2 map.
pub const HEADER: &str = "mortar-map 2";

/// Encodes a string as a token (§3.2): bytes from `!` to `~` other than
/// `%` stand for themselves, every other byte is `%XX` (upper-case hex),
/// and the empty string is a lone `%`.
pub fn encode(text: &str, out: &mut String) {
    encode_bytes(text.as_bytes(), out);
}

/// Decodes a token written by [`encode`]. `None` for a malformed escape or
/// for bytes that are not UTF-8.
pub fn decode(token: &str) -> Option<String> {
    String::from_utf8(decode_bytes(token)?).ok()
}

/// Encodes bytes as a token (§3.2): an `arg` word, which need not be
/// UTF-8.
pub fn encode_bytes(bytes: &[u8], out: &mut String) {
    if bytes.is_empty() {
        out.push('%');
        return;
    }
    for &b in bytes {
        if (0x21..=0x7e).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

/// Decodes a token to its bytes: an `arg` word (§3.2), which need not be
/// UTF-8. `None` for a malformed escape.
pub fn decode_bytes(token: &str) -> Option<Vec<u8>> {
    if token == "%" {
        return Some(Vec::new());
    }
    let bytes = token.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            out.push(digit(hi)? << 4 | digit(lo)?);
            i += 3;
        } else if (0x21..=0x7e).contains(&b) {
            out.push(b);
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

fn signed_hex(v: i64, out: &mut String) {
    if v < 0 {
        let _ = write!(out, "-{:x}", v.unsigned_abs());
    } else {
        let _ = write!(out, "{v:x}");
    }
}

/// A decimal number of the decision records (ordinals, entry numbers).
fn decimal32(t: &str) -> Option<u32> {
    dec(t).and_then(|v| u32::try_from(v).ok())
}

fn parse_signed_hex(t: &str) -> Option<i64> {
    if let Some(rest) = t.strip_prefix('-') {
        let m = hex(rest)?;
        if m > 1u64 << 63 {
            return None;
        }
        Some((m as i64).wrapping_neg())
    } else {
        let v = hex(t)?;
        i64::try_from(v).ok()
    }
}

/// Writes `map` in mortar's format (§3.2). The record order is fixed:
/// files by id, then places, pieces, the non-alloc records, discards,
/// folds, chunks, commons, defined symbols, `incremental`, holes and dead ranges,
/// each in the order of the map's vectors, then the dynamic records.
#[allow(clippy::too_many_lines)] // one block per record, in the fixed order
pub fn write(map: &Map) -> String {
    let mut out = String::with_capacity(64 * (map.placed.len() + map.files.len()) + 32);
    out.push_str(HEADER);
    out.push('\n');
    let idx = |s: &SectionRef| match s {
        SectionRef::Index(i) => i.to_string(),
        SectionRef::Name(n) => {
            let mut t = String::from(":");
            encode(n, &mut t);
            t
        }
    };
    for (i, f) in map.files.iter().enumerate() {
        let _ = write!(out, "file {i} ");
        encode(&f.path, &mut out);
        if let Some(m) = &f.member {
            out.push(' ');
            encode(m, &mut out);
            let _ = write!(out, " {:x}", f.member_offset.unwrap_or(0));
        }
        out.push('\n');
    }
    for p in &map.placed {
        match p.input_offset {
            None => {
                let _ =
                    writeln!(out, "place {} {} {:x} {:x}", p.file, idx(&p.section), p.addr, p.size);
            }
            Some(o) => {
                let _ = writeln!(
                    out,
                    "piece {} {} {o:x} {:x} {:x}",
                    p.file,
                    idx(&p.section),
                    p.addr,
                    p.size
                );
            }
        }
    }
    let rel = map.rel.as_ref();
    for r in rel.iter().flat_map(|r| &r.places) {
        let _ = writeln!(
            out,
            "rplace {} {} {} {:x} {:x}",
            r.file,
            idx(&r.section),
            r.out,
            r.offset,
            r.size
        );
    }
    for r in &map.nonalloc {
        let mut o = String::new();
        encode(&r.out, &mut o);
        match r.offset {
            Some(off) => {
                let _ =
                    writeln!(out, "nplace {} {} {o} {off:x} {:x}", r.file, idx(&r.section), r.size);
            }
            None => {
                let _ = writeln!(out, "merged {} {} {o}", r.file, idx(&r.section));
            }
        }
    }
    for r in rel.iter().flat_map(|r| &r.frames) {
        let _ = write!(out, "rframe {} {} {:x} ", r.file, idx(&r.section), r.input_offset);
        match r.output_offset {
            Some(o) => {
                let _ = writeln!(out, "{o:x}");
            }
            None => out.push_str("-\n"),
        }
    }
    for r in rel.iter().flat_map(|r| &r.groups) {
        let _ = writeln!(out, "rgroup {} {} {}", r.out, r.file, idx(&r.section));
    }
    for d in &map.discarded {
        let _ = writeln!(out, "discard {} {} {}", d.file, idx(&d.section), d.reason.name());
    }
    for f in &map.folds {
        let _ = writeln!(
            out,
            "fold {} {} {} {}",
            f.file,
            idx(&f.section),
            f.into_file,
            idx(&f.into_section)
        );
    }
    for c in &map.chunks {
        let _ = write!(out, "chunk {} ", c.kind.name());
        encode(&c.name, &mut out);
        match c.addr {
            Some(a) => {
                let _ = write!(out, " {a:x}");
            }
            None => out.push_str(" -"),
        }
        let _ = write!(out, " {:x}", c.size);
        if let Some((t, a)) = &c.target {
            out.push(' ');
            encode(t, &mut out);
            out.push(' ');
            signed_hex(*a, &mut out);
        }
        out.push('\n');
    }
    for c in &map.commons {
        out.push_str("common ");
        encode(&c.name, &mut out);
        let _ = write!(out, " {:x} {:x} {:x}", c.addr, c.size, c.align);
        if let Some(f) = c.file {
            let _ = write!(out, " {f}");
        }
        out.push('\n');
    }
    for c in rel.iter().flat_map(|r| &r.commons) {
        out.push_str("rcommon ");
        encode(&c.name, &mut out);
        let _ = writeln!(out, " {} {:x} {:x} {:x}", c.out, c.offset, c.size, c.align);
    }
    for d in &map.defined {
        out.push_str("define ");
        encode(&d.name, &mut out);
        let _ = writeln!(out, " {:x}", d.value);
    }
    if map.incremental {
        out.push_str("incremental\n");
    }
    for d in &map.dead {
        out.push_str(if d.kind.is_some() { "dead " } else { "hole " });
        encode(&d.section, &mut out);
        let _ = write!(out, " {:x} {:x}", d.offset, d.size);
        if let Some(k) = d.kind {
            out.push(' ');
            out.push_str(k.name());
        }
        out.push('\n');
    }
    write_decisions(map, &mut out, &idx);
    write_line(map, &mut out);
    out.push_str("end\n");
    out
}

/// The decision records of §3.2, in their group order, each group in the
/// order of its vector.
fn write_decisions(map: &Map, out: &mut String, idx: &dyn Fn(&SectionRef) -> String) {
    let d = &map.decisions;
    let tok = |s: &str| {
        let mut t = String::new();
        encode(s, &mut t);
        t
    };
    for r in &d.regions {
        let _ = writeln!(
            out,
            "region {} {} {} {:x} {:x}",
            r.file,
            tok(&r.out),
            r.cat,
            r.addr,
            r.capacity
        );
    }
    for c in &d.capacities {
        let addr = c.addr.map_or_else(|| "-".to_string(), |a| format!("{a:x}"));
        let _ = writeln!(
            out,
            "capacity {} {} {} {addr} {:x} {:x}",
            tok(&c.out),
            c.ordinal,
            c.part.token(),
            c.size,
            c.capacity
        );
    }
    for o in &d.orders {
        let _ = write!(out, "order {}", tok(&o.out));
        for n in &o.ordinals {
            let _ = write!(out, " {n}");
        }
        out.push('\n');
    }
    for s in &d.slots {
        let _ = match &s.key {
            SlotKey::Global(k) => {
                writeln!(out, "slot {} {} {:x}", s.kind.name(), tok(k), s.addr)
            }
            SlotKey::Local { file, index } => {
                writeln!(out, "slot {} {file} {index} {:x}", s.kind.name(), s.addr)
            }
            SlotKey::Module => writeln!(out, "slot {} {:x}", s.kind.name(), s.addr),
        };
    }
    for r in &d.redirects {
        let _ =
            writeln!(out, "redirect {} {} {:x} {:x}", r.file, idx(&r.section), r.offset, r.addr);
    }
    for f in &d.frags {
        let (a, b, c) = f.class;
        let _ = writeln!(out, "frag {} {a:x},{b:x},{c:x} {} {:x}", tok(&f.out), f.hash, f.offset);
    }
    for b in &d.symblocks {
        let file = b.file.map_or_else(|| "-".to_string(), |f| f.to_string());
        let _ = writeln!(
            out,
            "symblock {file} {} {} {} {:x} {:x}",
            b.first, b.count, b.capacity, b.strtab_off, b.strtab_cap
        );
    }
}

/// The dynamic records and the `arg` records of §3.2, in their fixed
/// order: a static map's `arg` records stand where a dynamic one's do.
fn write_line(map: &Map, out: &mut String) {
    let empty = DynRecords::default();
    let d = map.dynamic.as_ref().unwrap_or(&empty);
    let named = |out: &mut String, record: &str, name: &str| {
        out.push_str(record);
        out.push(' ');
        encode(name, out);
        out.push('\n');
    };
    if let Some(k) = d.kind {
        let _ = writeln!(out, "output {}", k.name());
    } else if map.rel.is_some() {
        out.push_str("output relocatable\n");
    }
    if let Some(i) = &d.interp {
        named(out, "interp", i);
    }
    if let Some((tag, paths)) = &d.runpath {
        let record = match tag {
            RunpathTag::Runpath => "runpath",
            RunpathTag::Rpath => "rpath",
        };
        named(out, record, paths);
    }
    if let Some(now) = d.now {
        out.push_str(if now { "bind now\n" } else { "bind lazy\n" });
    }
    if let Some(relro) = d.relro {
        out.push_str(if relro { "relro on\n" } else { "relro off\n" });
    }
    if let Some(sh) = &d.shared {
        crate::maprec::write_shared(sh, out);
    }
    for a in &map.args {
        out.push_str("arg ");
        encode_bytes(a, out);
        out.push('\n');
    }
    for n in &d.needed {
        named(out, "needed", n);
    }
    for c in &d.copies {
        out.push_str("copy ");
        encode(&c.name, out);
        let _ = writeln!(out, " {:x} {:x} {:x}", c.addr, c.size, c.align);
    }
    for e in &d.exports {
        named(out, "export", e);
    }
    for p in &d.plt {
        named(out, "plt", p);
    }
    for p in &d.iplt {
        named(out, "iplt", p);
    }
}

/// Reads a map in mortar's format (§3.2).
#[allow(clippy::too_many_lines)] // one arm per record
pub fn parse(text: &str) -> Result<Map, MapError> {
    let mut map = Map::new(Format::Mortar);
    let mut lines = text.split('\n').enumerate().map(|(i, l)| ((i + 1) as u32, l));
    match lines.next() {
        Some((_, HEADER)) => {}
        Some((_, l)) if l.starts_with("mortar-map ") => {
            return Err(MapError::new(1, format!("unsupported mortar map version '{}'", &l[11..])));
        }
        _ => return Err(MapError::new(1, "not a mortar map (no 'mortar-map 2' header)")),
    }
    let mut ended = false;
    // The first record a relocatable map refuses, and the first `-r`
    // record (§14.2).
    let mut not_at_r: Option<(String, u32)> = None;
    let mut r_record: Option<(String, u32)> = None;
    let mut output_line = 0u32;
    let mut shared_line: Option<u32> = None;
    // §3.2's place of the decision records: the first dynamic record's
    // line, the first decision record's, and the last group seen.
    let mut first_after: Option<(String, u32)> = None;
    let mut first_decision: Option<(String, u32)> = None;
    let mut decision_group = 0usize;
    let last = text.split('\n').count() as u32;
    for (n, line) in lines {
        // The text after the last newline: empty in a whole map.
        if n == last && line.is_empty() {
            break;
        }
        if ended {
            if line.is_empty() {
                continue;
            }
            return Err(MapError::new(n, "text after 'end'"));
        }
        let t: Vec<&str> = line.split(' ').collect();
        let bad = |what: &str| MapError::new(n, format!("malformed '{}' record: {what}", t[0]));
        let file_id = |s: &str| -> Result<u32, MapError> {
            let v = dec(s).ok_or_else(|| bad("file id"))?;
            if v >= map.files.len() as u64 {
                return Err(bad("file id names no earlier 'file' record"));
            }
            Ok(v as u32)
        };
        let section = |s: &str| -> Result<SectionRef, MapError> {
            if let Some(name) = s.strip_prefix(':') {
                return decode(name).map(SectionRef::Name).ok_or_else(|| bad("section name"));
            }
            let v =
                dec(s).filter(|&v| u32::try_from(v).is_ok()).ok_or_else(|| bad("section index"))?;
            Ok(SectionRef::Index(v as u32))
        };
        let num = |s: &str, what: &str| hex(s).ok_or_else(|| bad(what));
        let align = |s: &str| {
            hex(s).filter(|a| a.is_power_of_two()).ok_or_else(|| bad("alignment (a power of two)"))
        };
        if not_at_r.is_none()
            && matches!(
                t[0],
                "place"
                    | "piece"
                    | "nplace"
                    | "merged"
                    | "fold"
                    | "chunk"
                    | "common"
                    | "define"
                    | "incremental"
                    | "hole"
                    | "dead"
                    | "interp"
                    | "runpath"
                    | "rpath"
                    | "bind"
                    | "relro"
                    | "needed"
                    | "copy"
                    | "export"
                    | "plt"
                    | "iplt"
                    | "symbolic"
                    | "dynamic-list"
                    | "version-script"
                    | "region"
                    | "capacity"
                    | "order"
                    | "slot"
                    | "redirect"
                    | "frag"
                    | "symblock"
            )
        {
            not_at_r = Some((t[0].to_string(), n));
        }
        // §3.2: the decision records follow the incremental records and
        // precede the dynamic ones, in their group order.
        if let Some(g) = DECISION_WORDS.iter().position(|w| *w == t[0]) {
            if let Some((w, at)) = &first_after {
                return Err(MapError::new(
                    n,
                    format!(
                        "'{}' after '{w}' (line {at}): the decision records come before the dynamic records",
                        t[0]
                    ),
                ));
            }
            if g < decision_group {
                return Err(MapError::new(
                    n,
                    format!(
                        "'{}' after '{}': the decision records are in region, capacity, order, slot, redirect, frag, symblock order",
                        t[0], DECISION_WORDS[decision_group]
                    ),
                ));
            }
            decision_group = g;
            first_decision.get_or_insert((t[0].to_string(), n));
        } else if BEFORE_DECISIONS.contains(&t[0]) {
            if let Some((w, at)) = &first_decision {
                return Err(MapError::new(
                    n,
                    format!(
                        "'{}' after '{w}' (line {at}): the decision records follow the placement, allocation and incremental records",
                        t[0]
                    ),
                ));
            }
        } else if first_after.is_none() && AFTER_DECISIONS.contains(&t[0]) {
            first_after = Some((t[0].to_string(), n));
        }
        if r_record.is_none() && matches!(t[0], "rplace" | "rframe" | "rgroup" | "rcommon") {
            r_record = Some((t[0].to_string(), n));
        }
        match (t[0], t.len()) {
            ("end", 1) => ended = true,
            ("output", 2) if t[1] == "relocatable" => {
                if map.rel.as_ref().is_some_and(|r| r.line != 0)
                    || map.dynamic.as_ref().is_some_and(|d| d.kind.is_some())
                {
                    return Err(bad("a second 'output' record"));
                }
                map.rel.get_or_insert_with(RelRecords::default).line = n;
            }
            ("rplace", 6) => {
                let r = RPlace {
                    file: file_id(t[1])?,
                    section: section(t[2])?,
                    out: dec(t[3])
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| bad("output section index"))?,
                    offset: num(t[4], "offset")?,
                    size: num(t[5], "size")?,
                    line: n,
                };
                map.rel.get_or_insert_with(RelRecords::default).places.push(r);
            }
            ("rframe", 5) => {
                let r = RFrame {
                    file: file_id(t[1])?,
                    section: section(t[2])?,
                    input_offset: num(t[3], "input offset")?,
                    output_offset: if t[4] == "-" {
                        None
                    } else {
                        Some(num(t[4], "output offset")?)
                    },
                    line: n,
                };
                map.rel.get_or_insert_with(RelRecords::default).frames.push(r);
            }
            ("rgroup", 4) => {
                let r = RGroup {
                    out: dec(t[1])
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| bad("output section index"))?,
                    file: file_id(t[2])?,
                    section: section(t[3])?,
                    line: n,
                };
                map.rel.get_or_insert_with(RelRecords::default).groups.push(r);
            }
            ("rcommon", 6) => {
                let r = RCommon {
                    name: decode(t[1]).ok_or_else(|| bad("name"))?,
                    out: dec(t[2])
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| bad("output section index"))?,
                    offset: num(t[3], "offset")?,
                    size: num(t[4], "size")?,
                    align: align(t[5])?,
                    line: n,
                };
                map.rel.get_or_insert_with(RelRecords::default).commons.push(r);
            }
            ("file", 3 | 5) => {
                let id = dec(t[1]).ok_or_else(|| bad("id"))?;
                if id != map.files.len() as u64 {
                    return Err(bad("file ids must count up from 0"));
                }
                let path = decode(t[2]).ok_or_else(|| bad("path"))?;
                let mut f = MapFile::plain(path);
                if t.len() == 5 {
                    f.member = Some(decode(t[3]).ok_or_else(|| bad("member"))?);
                    f.member_offset = Some(num(t[4], "member offset")?);
                }
                map.files.push(f);
            }
            ("place", 5) => map.placed.push(Placed {
                file: file_id(t[1])?,
                section: section(t[2])?,
                input_offset: None,
                addr: num(t[3], "address")?,
                size: num(t[4], "size")?,
                line: n,
            }),
            ("piece", 6) => map.placed.push(Placed {
                file: file_id(t[1])?,
                section: section(t[2])?,
                input_offset: Some(num(t[3], "input offset")?),
                addr: num(t[4], "address")?,
                size: num(t[5], "size")?,
                line: n,
            }),
            ("nplace", 6) => map.nonalloc.push(NonAlloc {
                file: file_id(t[1])?,
                section: section(t[2])?,
                out: decode(t[3]).ok_or_else(|| bad("output section"))?,
                offset: Some(num(t[4], "offset")?),
                size: num(t[5], "size")?,
                line: n,
            }),
            ("merged", 4) => map.nonalloc.push(NonAlloc {
                file: file_id(t[1])?,
                section: section(t[2])?,
                out: decode(t[3]).ok_or_else(|| bad("output section"))?,
                offset: None,
                size: 0,
                line: n,
            }),
            ("discard", 4) => map.discarded.push(Discarded {
                file: file_id(t[1])?,
                section: section(t[2])?,
                reason: DiscardReason::from_name(t[3]).ok_or_else(|| bad("reason"))?,
                line: n,
            }),
            ("fold", 5) => map.folds.push(Fold {
                file: file_id(t[1])?,
                section: section(t[2])?,
                into_file: file_id(t[3])?,
                into_section: section(t[4])?,
                line: n,
            }),
            ("chunk", 5 | 7) => map.chunks.push(Chunk {
                kind: ChunkKind::from_name(t[1]).ok_or_else(|| bad("kind"))?,
                name: decode(t[2]).ok_or_else(|| bad("name"))?,
                addr: if t[3] == "-" { None } else { Some(num(t[3], "address")?) },
                size: num(t[4], "size")?,
                target: if t.len() == 7 {
                    Some((
                        decode(t[5]).ok_or_else(|| bad("target"))?,
                        parse_signed_hex(t[6]).ok_or_else(|| bad("addend"))?,
                    ))
                } else {
                    None
                },
                line: n,
            }),
            ("common", 5 | 6) => map.commons.push(CommonAlloc {
                name: decode(t[1]).ok_or_else(|| bad("name"))?,
                addr: num(t[2], "address")?,
                size: num(t[3], "size")?,
                align: align(t[4])?,
                file: if t.len() == 6 { Some(file_id(t[5])?) } else { None },
                line: n,
            }),
            ("region", 6) => map.decisions.regions.push(Region {
                file: file_id(t[1])?,
                out: decode(t[2]).ok_or_else(|| bad("output section"))?,
                cat: decimal32(t[3]).ok_or_else(|| bad("category"))?,
                addr: num(t[4], "address")?,
                capacity: num(t[5], "capacity")?,
                line: n,
            }),
            ("capacity", 7) => map.decisions.capacities.push(Capacity {
                out: decode(t[1]).ok_or_else(|| bad("output section"))?,
                ordinal: decimal32(t[2]).ok_or_else(|| bad("ordinal"))?,
                part: Part::from_token(t[3]).ok_or_else(|| bad("part"))?,
                addr: if t[4] == "-" { None } else { Some(num(t[4], "address")?) },
                size: num(t[5], "size")?,
                capacity: num(t[6], "capacity")?,
                line: n,
            }),
            ("order", k) if k >= 3 => map.decisions.orders.push(Order {
                out: decode(t[1]).ok_or_else(|| bad("output section"))?,
                ordinals: t[2..]
                    .iter()
                    .map(|s| decimal32(s).ok_or_else(|| bad("ordinal")))
                    .collect::<Result<_, _>>()?,
                line: n,
            }),
            ("slot", 3..=5) => {
                let kind = SlotKind::from_name(t[1]).ok_or_else(|| bad("kind"))?;
                let key = match t.len() {
                    3 if kind == SlotKind::TlsLd => SlotKey::Module,
                    3 => return Err(bad("wrong number of fields (only 'tls-ld' has no key)")),
                    4 => SlotKey::Global(decode(t[2]).ok_or_else(|| bad("key"))?),
                    _ => SlotKey::Local {
                        file: file_id(t[2])?,
                        index: decimal32(t[3]).ok_or_else(|| bad("symbol index"))?,
                    },
                };
                let addr = num(t[t.len() - 1], "address")?;
                map.decisions.slots.push(Slot { kind, key, addr, line: n });
            }
            ("redirect", 5) => map.decisions.redirects.push(Redirect {
                file: file_id(t[1])?,
                section: section(t[2])?,
                offset: num(t[3], "offset")?,
                addr: num(t[4], "address")?,
                line: n,
            }),
            ("frag", 5) => {
                let class: Vec<u64> = t[2].split(',').filter_map(hex).collect();
                let (3, 3) = (class.len(), t[2].split(',').count()) else {
                    return Err(bad("class (three hex numbers joined by ',')"));
                };
                let h = t[3];
                if h.len() != 32
                    || !h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(bad("hash (exactly 32 lower-case hex digits)"));
                }
                map.decisions.frags.push(Frag {
                    out: decode(t[1]).ok_or_else(|| bad("output section"))?,
                    class: (class[0], class[1], class[2]),
                    hash: h.to_string(),
                    offset: num(t[4], "offset")?,
                    line: n,
                });
            }
            ("symblock", 7) => map.decisions.symblocks.push(SymBlock {
                file: if t[1] == "-" { None } else { Some(file_id(t[1])?) },
                first: dec(t[2]).ok_or_else(|| bad("first entry"))?,
                count: dec(t[3]).ok_or_else(|| bad("count"))?,
                capacity: dec(t[4]).ok_or_else(|| bad("capacity"))?,
                strtab_off: num(t[5], "strtab offset")?,
                strtab_cap: num(t[6], "strtab capacity")?,
                line: n,
            }),
            ("define", 3) => map.defined.push(Defined {
                name: decode(t[1]).ok_or_else(|| bad("name"))?,
                value: num(t[2], "value")?,
                line: n,
            }),
            ("incremental", 1) => {
                if map.incremental {
                    return Err(bad("a second 'incremental' record"));
                }
                map.incremental = true;
            }
            ("hole", 4) | ("dead", 5) => map.dead.push(DeadRange {
                section: decode(t[1]).ok_or_else(|| bad("output section"))?,
                offset: num(t[2], "offset")?,
                size: num(t[3], "size")?,
                kind: if t[0] == "dead" {
                    Some(DeadKind::from_name(t[4]).ok_or_else(|| bad("kind"))?)
                } else {
                    None
                },
                line: n,
            }),
            ("output", 2) => {
                if map.rel.as_ref().is_some_and(|r| r.line != 0) {
                    return Err(bad("a second 'output' record"));
                }
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                if d.kind.is_some() {
                    return Err(bad("a second 'output' record"));
                }
                d.kind = Some(OutputKind::from_name(t[1]).ok_or_else(|| bad("kind"))?);
                output_line = n;
            }
            ("symbolic" | "dynamic-list" | "version-script", _) => {
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                crate::maprec::read_shared(d, &t, n)?;
                shared_line.get_or_insert(n);
            }
            ("interp", 2) => {
                let v = decode(t[1]).ok_or_else(|| bad("path"))?;
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                if d.interp.replace(v).is_some() {
                    return Err(bad("a second 'interp' record"));
                }
            }
            ("runpath" | "rpath", 2) => {
                let v = decode(t[1]).ok_or_else(|| bad("paths"))?;
                let tag = if t[0] == "runpath" { RunpathTag::Runpath } else { RunpathTag::Rpath };
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                if d.runpath.replace((tag, v)).is_some() {
                    return Err(bad("a second 'runpath' or 'rpath' record"));
                }
            }
            ("bind", 2) => {
                let v = match t[1] {
                    "now" => true,
                    "lazy" => false,
                    _ => return Err(bad("mode")),
                };
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                if d.now.replace(v).is_some() {
                    return Err(bad("a second 'bind' record"));
                }
            }
            ("relro", 2) => {
                let v = match t[1] {
                    "on" => true,
                    "off" => false,
                    _ => return Err(bad("state")),
                };
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                if d.relro.replace(v).is_some() {
                    return Err(bad("a second 'relro' record"));
                }
            }
            ("arg", 2) => {
                // A word is bytes (`spec/cli.md` §2.1): not held to UTF-8.
                map.args.push(decode_bytes(t[1]).ok_or_else(|| bad("word"))?);
            }
            ("needed" | "export" | "plt" | "iplt", 2) => {
                let v = decode(t[1]).ok_or_else(|| bad("name"))?;
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                match t[0] {
                    "needed" => d.needed.push(v),
                    "export" => d.exports.push(v),
                    "plt" => d.plt.push(v),
                    _ => d.iplt.push(v),
                }
            }
            ("copy", 5) => {
                let c = CopyHint {
                    name: decode(t[1]).ok_or_else(|| bad("name"))?,
                    addr: num(t[2], "address")?,
                    size: num(t[3], "size")?,
                    align: align(t[4])?,
                };
                map.dynamic.get_or_insert_with(DynRecords::default).copies.push(c);
            }
            ("", 1) => return Err(MapError::new(n, "empty line before 'end'")),
            (
                "end" | "file" | "place" | "piece" | "nplace" | "merged" | "discard" | "fold"
                | "chunk" | "common" | "define" | "incremental" | "hole" | "dead" | "output"
                | "rplace" | "rframe" | "rgroup" | "rcommon" | "interp" | "runpath" | "rpath"
                | "bind" | "relro" | "arg" | "needed" | "copy" | "export" | "plt" | "iplt"
                | "region" | "capacity" | "order" | "slot" | "redirect" | "frag" | "symblock",
                _,
            ) => {
                return Err(bad("wrong number of fields"));
            }
            (other, _) => {
                let mut shown = String::new();
                encode(other, &mut shown);
                return Err(MapError::new(n, format!("unknown record '{shown}'")));
            }
        }
    }
    if !ended {
        return Err(MapError::new(0, "truncated mortar map: no 'end' record"));
    }
    crate::maprec::end_shared(map.dynamic.as_ref(), output_line, shared_line)?;
    // §3.2: the decision records and `common`'s `<file>` are an
    // incremental-mode map's alone.
    if let Some((w, at)) = &first_decision {
        if !map.incremental {
            return Err(MapError::new(*at, format!("'{w}' in a map without 'incremental'")));
        }
    }
    for c in &map.commons {
        if c.file.is_some() && !map.incremental {
            return Err(MapError::new(
                c.line,
                "malformed 'common' record: a <file> field in a map without 'incremental'",
            ));
        }
        // Required in an incremental map; until mortar's writer states
        // it (U-6, with the decision records), only one with decision
        // records is held to it (WP-222, for the supervisor's ruling).
        if c.file.is_none() && !map.decisions.is_empty() {
            return Err(MapError::new(
                c.line,
                "malformed 'common' record: no <file> field in an incremental map",
            ));
        }
    }
    // §14.2: nothing at -r has an address; the -r records need the kind.
    // The -r records alone (`rplace` ...) create `map.rel`; the kind is
    // `output relocatable`, whose line is never 0.
    let relocatable = map.rel.as_ref().is_some_and(|r| r.line != 0);
    match (relocatable, not_at_r, r_record) {
        (true, Some((record, at)), _) => {
            return Err(MapError::new(at, format!("'{record}' in a relocatable map")));
        }
        (false, _, Some((record, at))) => {
            return Err(MapError::new(
                at,
                format!("'{record}' in a map without 'output relocatable'"),
            ));
        }
        _ => {}
    }
    // §3.2a: every mortar map states its line; a link has at least one
    // word (none is `no-input-files`). A map without one (written before
    // WP-102) is refused rather than judged without its options.
    if map.args.is_empty() {
        return Err(MapError::new(
            0,
            "no 'arg' record: a mortar map states its link's line (spec/plumb.md §3.2a)",
        ));
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader, for the tests of the other records: a text with no
    /// `arg` record is read with one added before `end`, and the result
    /// has none (§3.2a's requirement is `s3_7_malformed_maps_are_errors`').
    fn parse(text: &str) -> Result<Map, MapError> {
        match super::parse(text) {
            Err(e) if e.message.contains("no 'arg' record") => {
                let at = text.rfind("end\n").unwrap_or(text.len());
                let with = format!("{}arg a.o\n{}", &text[..at], &text[at..]);
                let mut m = super::parse(&with)?;
                m.args.clear();
                Ok(m)
            }
            r => r,
        }
    }

    fn sample() -> Map {
        let mut m = Map::new(Format::Mortar);
        m.files.push(MapFile::plain("dir with space/a.o"));
        m.files.push(MapFile {
            path: "/usr/lib/libc.a".into(),
            member: Some("ioputs.o".into()),
            member_offset: Some(0x1a2b),
        });
        m.files.push(MapFile::plain("100%é.o"));
        m.placed.push(Placed {
            file: 0,
            section: SectionRef::Index(1),
            input_offset: None,
            addr: 0x40_0000,
            size: 0x20,
            line: 0,
        });
        m.placed.push(Placed {
            file: 1,
            section: SectionRef::Index(7),
            input_offset: Some(0x14),
            addr: 0x41_0000,
            size: 0x18,
            line: 0,
        });
        m.placed.push(Placed {
            file: 2,
            section: SectionRef::Name(".text.a b:c".into()),
            input_offset: None,
            addr: 0x42_0000,
            size: 0x4,
            line: 0,
        });
        m.discarded.push(Discarded {
            file: 1,
            section: SectionRef::Index(3),
            reason: DiscardReason::Comdat,
            line: 0,
        });
        m.folds.push(Fold {
            file: 2,
            section: SectionRef::Index(4),
            into_file: 0,
            into_section: SectionRef::Index(1),
            line: 0,
        });
        m.chunks.push(Chunk {
            kind: ChunkKind::Thunk,
            name: "__veneer x".into(),
            addr: Some(0x40_0020),
            size: 12,
            target: Some(("far".into(), -8)),
            line: 0,
        });
        m.chunks.push(Chunk {
            kind: ChunkKind::Synthetic,
            name: String::new(),
            addr: None,
            size: 4,
            target: None,
            line: 0,
        });
        m.commons.push(CommonAlloc {
            name: "com mon".into(),
            addr: 0x50_0040,
            size: 0x30,
            align: 0x40,
            file: None,
            line: 0,
        });
        m.defined.push(Defined { name: "_end".into(), value: 0x50_0000, line: 0 });
        m
    }

    fn strip_lines(mut m: Map) -> Map {
        m.placed.iter_mut().for_each(|p| p.line = 0);
        m.discarded.iter_mut().for_each(|p| p.line = 0);
        m.folds.iter_mut().for_each(|p| p.line = 0);
        m.chunks.iter_mut().for_each(|p| p.line = 0);
        m.defined.iter_mut().for_each(|p| p.line = 0);
        m.commons.iter_mut().for_each(|p| p.line = 0);
        m.dead.iter_mut().for_each(|p| p.line = 0);
        m
    }

    // spec/plumb.md §3.2: the writer and the reader are inverses.
    #[test]
    fn s3_2_round_trip() {
        let m = sample();
        let text = write(&m);
        assert!(text.starts_with("mortar-map 2\nfile 0 dir%20with%20space/a.o\n"), "{text}");
        assert!(text.contains("file 2 100%25%C3%A9.o\n"), "{text}");
        assert!(text.contains("chunk synthetic % - 4\n"), "{text}");
        assert!(text.contains("chunk thunk __veneer%20x 400020 c far -8\n"), "{text}");
        assert!(
            text.contains("chunk synthetic % - 4\ncommon com%20mon 500040 30 40\ndefine "),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(strip_lines(back), m);
    }

    // §3.2: strings.
    #[test]
    fn s3_2_tokens_encode_every_byte() {
        for s in ["", "%", "a b", "\u{7f}\t\n", "é", "%%41"] {
            let mut t = String::new();
            encode(s, &mut t);
            assert!(!t.contains(' ') && !t.contains('\n'));
            assert_eq!(decode(&t).as_deref(), Some(s), "{t}");
        }
        assert_eq!(decode("%4"), None);
        assert_eq!(decode("%zz"), None);
        assert_eq!(decode("%41%"), None);
        assert_eq!(decode("a\u{e9}"), None);
        assert_eq!(decode("%FF"), None, "not UTF-8");
    }

    // §3.2: what is refused, by line.
    #[test]
    fn s3_2_malformed_maps_are_errors() {
        let cases: &[(&str, u32)] = &[
            ("", 1),
            ("mortar-map 3\nend\n", 1),
            ("mortar-map 1\nend\n", 1),
            ("mortar-map 2\n", 0),
            ("mortar-map 2\nfile 1 a.o\nend\n", 2),
            ("mortar-map 2\nplace 0 1 0 0\nend\n", 2),
            ("mortar-map 2\nfile 0 a.o\nplace 0 1 0\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\nplace 0 x 0 0\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\nplace 0 1 0g 0\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\ndiscard 0 1 why\nend\n", 3),
            ("mortar-map 2\nfrob\nend\n", 2),
            ("mortar-map 2\nend\nfile 0 a.o\n", 3),
            ("mortar-map 2\n\nend\n", 2),
            ("mortar-map 2\nfile 0 a.o\nchunk thunk x 0 4 y 1z\nend\n", 3),
            ("mortar-map 2\nfile 0  a.o\nend\n", 2),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
        assert!(parse("mortar-map 2\narg a.o\nend\n\n").is_ok());
        // §3.2a: a map without its line (written before WP-102) is refused.
        let e = super::parse("mortar-map 2\nend\n").unwrap_err();
        assert!(e.message.contains("no 'arg' record"), "{e}");
    }

    // §3.2 (P-D3 of spec/dyn.md): the dynamic records round-trip, in their
    // fixed order, and a static map writes none.
    #[test]
    fn s3_2_dynamic_records_round_trip() {
        let mut m = sample();
        assert!(!write(&m).contains("output "));
        m.dynamic = Some(DynRecords {
            kind: Some(OutputKind::Pie),
            interp: Some("/lib/ld-linux-aarch64.so.1".into()),
            runpath: Some((RunpathTag::Runpath, "$ORIGIN/lib:/opt/x y".into())),
            now: Some(true),
            relro: Some(false),
            needed: vec!["libc.so.6".into(), "libm.so.6".into()],
            copies: vec![
                CopyHint { name: "stdout".into(), addr: 0x2_0010, size: 8, align: 8 },
                CopyHint { name: "environ".into(), addr: 0x2_0018, size: 8, align: 1 },
            ],
            exports: vec!["main".into()],
            plt: vec!["puts".into(), "memcpy@GLIBC_2.2.5".into()],
            iplt: vec!["my_ifunc".into()],
            shared: None,
        });
        m.args = vec![b"-pie".to_vec(), b"--as-needed".to_vec(), b"-lc".to_vec(), Vec::new()];
        let text = write(&m);
        assert!(
            text.ends_with(
                "define _end 500000\noutput pie\ninterp /lib/ld-linux-aarch64.so.1\n\
                 runpath $ORIGIN/lib:/opt/x%20y\nbind now\nrelro off\narg -pie\n\
                 arg --as-needed\narg -lc\narg %\nneeded libc.so.6\nneeded libm.so.6\n\
                 copy stdout 20010 8 8\ncopy environ 20018 8 1\nexport main\nplt puts\nplt memcpy@GLIBC_2.2.5\n\
                 iplt my_ifunc\nend\n"
            ),
            "{text}"
        );
        assert_eq!(strip_lines(parse(&text).unwrap()), m);
        // Every record is optional.
        let only = parse("mortar-map 2\nrpath /a\nend\n").unwrap();
        assert_eq!(
            only.dynamic,
            Some(DynRecords {
                runpath: Some((RunpathTag::Rpath, "/a".into())),
                ..DynRecords::default()
            })
        );
    }

    // §3.2, §3.2a: a static map's `arg` records stand where a dynamic
    // one's do (after the defines and the incremental records, before
    // `end`) and make no dynamic record; a word is bytes: a space, a
    // newline, `%`, the empty word and bytes that are not UTF-8 all
    // round-trip.
    #[test]
    fn s3_2_arg_records_in_every_map() {
        let mut m = sample();
        m.incremental = true;
        m.args = vec![
            b"-o".to_vec(),
            b"dir with space/out".to_vec(),
            b"line\nbreak.o".to_vec(),
            b"\xff\xfe-latin1.o".to_vec(),
            b"100%".to_vec(),
            Vec::new(),
            "é.o".as_bytes().to_vec(),
        ];
        let text = write(&m);
        assert!(
            text.ends_with(
                "define _end 500000\nincremental\narg -o\narg dir%20with%20space/out\n\
                 arg line%0Abreak.o\narg %FF%FE-latin1.o\narg 100%25\narg %\narg %C3%A9.o\nend\n"
            ),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.dynamic, None, "arg records alone are no dynamic record");
        assert_eq!(strip_lines(back), m);
        // A malformed escape is still an error; bytes that are not UTF-8
        // are an error everywhere but in a word.
        assert_eq!(parse("mortar-map 2\narg %F\nend\n").unwrap_err().line, 2);
        assert_eq!(parse("mortar-map 2\narg a b\nend\n").unwrap_err().line, 2);
        assert_eq!(parse("mortar-map 2\nneeded %FF\nend\n").unwrap_err().line, 2);
        assert_eq!(parse("mortar-map 2\narg %FF\nend\n").unwrap().args, vec![vec![0xff]]);
    }

    // §3.2: malformed dynamic records.
    #[test]
    fn s3_2_malformed_dynamic_records_are_errors() {
        let cases: &[(&str, u32)] = &[
            ("mortar-map 2\noutput shared\nend\n", 2),
            ("mortar-map 2\noutput pie\noutput pie\nend\n", 3),
            ("mortar-map 2\ninterp a\ninterp b\nend\n", 3),
            ("mortar-map 2\nrunpath a\nrpath b\nend\n", 3),
            ("mortar-map 2\nbind eager\nend\n", 2),
            ("mortar-map 2\nrelro yes\nend\n", 2),
            ("mortar-map 2\nbind now\nbind now\nend\n", 3),
            ("mortar-map 2\ncopy x 10\nend\n", 2),
            ("mortar-map 2\ncopy x 10 zz\nend\n", 2),
            ("mortar-map 2\ncopy x 10 8\nend\n", 2),
            ("mortar-map 2\nneeded\nend\n", 2),
            ("mortar-map 2\nplt a b\nend\n", 2),
            ("mortar-map 2\nexport %4\nend\n", 2),
            ("mortar-map 2\ncopy x 10 8 3\nend\n", 2),
            ("mortar-map 2\ncopy x 10 8 0\nend\n", 2),
            ("mortar-map 2\ncopy x 10 8 4 4\nend\n", 2),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
    }

    // §3.2 (spec/incr.md IN-60): the incremental records round-trip, after
    // the defines and before the dynamic records; a map without them
    // writes none and claims no coverage.
    #[test]
    fn s3_2_dead_records_round_trip() {
        let mut m = sample();
        let plain = write(&m);
        assert!(!plain.contains("hole ") && !plain.contains("incremental"));
        assert!(!m.claims_coverage());
        m.incremental = true;
        m.dead.push(DeadRange {
            section: ".text".into(),
            offset: 0x40,
            size: 0x10,
            kind: None,
            line: 0,
        });
        for (i, k) in DeadKind::ALL.iter().enumerate() {
            m.dead.push(DeadRange {
                section: ".debug info".into(),
                offset: i as u64 * 0x20,
                size: 0x20,
                kind: Some(*k),
                line: 0,
            });
        }
        let text = write(&m);
        assert!(
            text.ends_with(
                "define _end 500000\nincremental\nhole .text 40 10\n\
                 dead .debug%20info 0 20 neutral\ndead .debug%20info 20 20 contribution\n\
                 dead .debug%20info 40 20 strings\ndead .debug%20info 60 20 cie\nend\n"
            ),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert!(back.claims_coverage());
        assert_eq!(strip_lines(back), m);
        // A hole line alone claims coverage; `incremental` alone too.
        assert!(parse("mortar-map 2\nhole .data 0 8\nend\n").unwrap().claims_coverage());
        assert!(parse("mortar-map 2\nincremental\nend\n").unwrap().claims_coverage());
    }

    // §3.2: malformed incremental records, by line.
    #[test]
    fn s3_2_malformed_dead_records_are_errors() {
        let cases: &[(&str, u32)] = &[
            ("mortar-map 2\nhole .text 0\nend\n", 2),
            ("mortar-map 2\nhole .text 0 8 neutral\nend\n", 2),
            ("mortar-map 2\ndead .text 0 8\nend\n", 2),
            ("mortar-map 2\ndead .text 0 8 rotten\nend\n", 2),
            ("mortar-map 2\ndead .text 0 8 Neutral\nend\n", 2),
            ("mortar-map 2\nhole .text -1 8\nend\n", 2),
            ("mortar-map 2\nhole .text 0 10000000000000000\nend\n", 2),
            ("mortar-map 2\nhole %4 0 8\nend\n", 2),
            ("mortar-map 2\nincremental yes\nend\n", 2),
            ("mortar-map 2\nincremental\nincremental\nend\n", 3),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
        // Lines the reader cannot judge (overlapping, out of a section,
        // empty) are read: the verifier judges them against the output
        // (P-I2, §6.8).
        let m = parse("mortar-map 2\nhole .text 0 0\nhole .text 0 8\nhole .text 4 8\nhole .nowhere ffffffffffffffff ffffffffffffffff\nend\n").unwrap();
        assert_eq!(m.dead.len(), 4);
    }

    // §14.2 (WP-185): the -r records, in any order with `output
    // relocatable`, written back, and malformed by line.
    #[test]
    fn s14_2_relocatable_records() {
        for text in [
            "mortar-map 2\nfile 0 a.o\noutput relocatable\nrplace 0 1 2 10 8\nrframe 0 4 0 -\nrframe 0 4 18 20\nrgroup 3 0 5\nrcommon c%20d 7 0 4 8\nend\n",
            "mortar-map 2\nfile 0 a.o\nrplace 0 1 2 10 8\nrframe 0 4 0 -\nrframe 0 4 18 20\nrgroup 3 0 5\nrcommon c%20d 7 0 4 8\noutput relocatable\nend\n",
        ] {
            let m = parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            let r = m.rel.as_ref().expect("relocatable");
            assert_ne!(r.line, 0);
            assert_eq!(r.places.len(), 1);
            assert_eq!((r.places[0].out, r.places[0].offset, r.places[0].size), (2, 0x10, 8));
            assert_eq!(r.frames.len(), 2);
            assert_eq!(r.frames[0].output_offset, None);
            assert_eq!((r.frames[1].input_offset, r.frames[1].output_offset), (0x18, Some(0x20)));
            assert_eq!(r.groups.len(), 1);
            assert_eq!(r.groups[0].out, 3);
            assert_eq!(r.commons.len(), 1);
            assert_eq!(
                (r.commons[0].name.as_str(), r.commons[0].out, r.commons[0].size),
                ("c d", 7, 4)
            );
        }
        let cases: &[(&str, u32)] = &[
            ("mortar-map 2\nfile 0 a.o\nrplace 0 1 2 10 8\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\noutput relocatable\nrplace 0 1 2 10\nend\n", 4),
            ("mortar-map 2\nfile 0 a.o\noutput relocatable\nrplace 0 1 x 10 8\nend\n", 4),
            ("mortar-map 2\nfile 0 a.o\noutput relocatable\nrframe 0 4 0 +\nend\n", 4),
            ("mortar-map 2\nfile 0 a.o\noutput relocatable\nrgroup 3 0\nend\n", 4),
            ("mortar-map 2\nfile 0 a.o\noutput relocatable\nrcommon c 7 0 4 3\nend\n", 4),
            (
                "mortar-map 2\nfile 0 a.o\nrplace 0 1 2 10 8\noutput relocatable\noutput relocatable\nend\n",
                5,
            ),
            (
                "mortar-map 2\nfile 0 a.o\nrplace 0 1 2 10 8\noutput relocatable\noutput exec\nend\n",
                5,
            ),
            ("mortar-map 2\nfile 0 a.o\noutput exec\noutput relocatable\nend\n", 4),
            ("mortar-map 2\nfile 0 a.o\noutput relocatable\nhole .text 0 8\nend\n", 4),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
    }

    // §3.2 (WP-096): the allocation records, malformed, by line.
    #[test]
    fn s3_2_malformed_allocation_records_are_errors() {
        let cases: &[(&str, u32)] = &[
            ("mortar-map 2\ncommon x 10 8\nend\n", 2),
            ("mortar-map 2\ncommon x 10 8 8 8\nend\n", 2),
            ("mortar-map 2\ncommon x 10 8 c\nend\n", 2),
            ("mortar-map 2\ncommon x 10 8 0\nend\n", 2),
            ("mortar-map 2\ncommon x 1g 8 8\nend\n", 2),
            ("mortar-map 2\ncommon %4 10 8 8\nend\n", 2),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
        let m = parse("mortar-map 2\ncommon x 10 8 1\nend\n").unwrap();
        assert_eq!(m.commons.len(), 1);
        assert_eq!(m.commons[0].align, 1);
    }

    /// An incremental map with one decision record of each form and a
    /// common with its `<file>` (§3.2, WP-222).
    fn with_decisions() -> Map {
        let mut m = sample();
        m.incremental = true;
        m.commons[0].file = Some(1);
        let d = &mut m.decisions;
        d.regions.push(Region {
            file: 0,
            out: ".text x".into(),
            cat: 3,
            addr: 0x40_1000,
            capacity: 0x200,
            line: 0,
        });
        for (i, part) in
            [Part::Body(2), Part::Island(0), Part::Merged, Part::Synthetic].into_iter().enumerate()
        {
            d.capacities.push(Capacity {
                out: ".text".into(),
                ordinal: i as u32,
                part,
                addr: if i == 3 { None } else { Some(0x40_0000 + i as u64 * 0x10) },
                size: 0x8,
                capacity: 0x10,
                line: 0,
            });
        }
        d.orders.push(Order { out: ".text".into(), ordinals: vec![1, 0, 3, 2], line: 0 });
        d.slots.push(Slot {
            kind: SlotKind::Got,
            key: SlotKey::Global("g h".into()),
            addr: 0x60_0000,
            line: 0,
        });
        d.slots.push(Slot {
            kind: SlotKind::TlsGd,
            key: SlotKey::Local { file: 2, index: 17 },
            addr: 0x60_0008,
            line: 0,
        });
        d.slots.push(Slot {
            kind: SlotKind::TlsLd,
            key: SlotKey::Module,
            addr: 0x60_0018,
            line: 0,
        });
        d.redirects.push(Redirect {
            file: 1,
            section: SectionRef::Index(4),
            offset: 0x1c,
            addr: 0x40_0020,
            line: 0,
        });
        d.frags.push(Frag {
            out: ".rodata".into(),
            class: (1, 0x30, 1),
            hash: "0123456789abcdef0123456789abcdef".into(),
            offset: 0x44,
            line: 0,
        });
        d.symblocks.push(SymBlock {
            file: None,
            first: 1,
            count: 3,
            capacity: 8,
            strtab_off: 1,
            strtab_cap: 0x40,
            line: 0,
        });
        d.symblocks.push(SymBlock {
            file: Some(0),
            first: 9,
            count: 2,
            capacity: 4,
            strtab_off: 0x41,
            strtab_cap: 0x20,
            line: 0,
        });
        m
    }

    fn strip_decision_lines(mut m: Map) -> Map {
        let d = &mut m.decisions;
        d.regions.iter_mut().for_each(|r| r.line = 0);
        d.capacities.iter_mut().for_each(|r| r.line = 0);
        d.orders.iter_mut().for_each(|r| r.line = 0);
        d.slots.iter_mut().for_each(|r| r.line = 0);
        d.redirects.iter_mut().for_each(|r| r.line = 0);
        d.frags.iter_mut().for_each(|r| r.line = 0);
        d.symblocks.iter_mut().for_each(|r| r.line = 0);
        strip_lines(m)
    }

    // §3.2 (WP-222): the decision records round-trip, after the
    // incremental records and before the dynamic ones, in group order;
    // `common`'s `<file>` too.
    #[test]
    fn s3_2_decision_records_round_trip() {
        let m = with_decisions();
        assert_eq!(m.decisions.len(), 13);
        let text = write(&m);
        assert!(text.contains("common com%20mon 500040 30 40 1\n"), "{text}");
        assert!(
            text.contains(
                "incremental\nregion 0 .text%20x 3 401000 200\n\
                 capacity .text 0 b2 400000 8 10\ncapacity .text 1 p0 400010 8 10\n\
                 capacity .text 2 m 400020 8 10\ncapacity .text 3 s - 8 10\n\
                 order .text 1 0 3 2\n\
                 slot got g%20h 600000\nslot tls-gd 2 17 600008\nslot tls-ld 600018\n\
                 redirect 1 4 1c 400020\n\
                 frag .rodata 1,30,1 0123456789abcdef0123456789abcdef 44\n\
                 symblock - 1 3 8 1 40\nsymblock 0 9 2 4 41 20\nend\n"
            ),
            "{text}"
        );
        let back = parse(&text).unwrap();
        assert_eq!(back.decisions.regions[0].line, 15, "lines are kept");
        assert_eq!(strip_decision_lines(back), m);
    }

    // §3.2 (WP-222): malformed decision records and misplaced ones, by
    // line; `common`'s `<file>` outside an incremental map.
    #[test]
    fn s3_2_malformed_decision_records_are_errors() {
        let head = "mortar-map 2\nfile 0 a.o\nincremental\n";
        let cases: &[(&str, u32)] = &[
            // field counts
            ("region 0 .text 0 1000", 4),
            ("region 0 .text 0 1000 10 10", 4),
            ("capacity .text 0 b0 1000 8", 4),
            ("order .text", 4),
            ("slot got", 4),
            ("slot got 1000", 4),
            ("slot got 0 1 2 1000", 4),
            ("redirect 0 1 2", 4),
            ("frag .r 1,2,3 0123456789abcdef0123456789abcdef", 4),
            ("symblock - 1 2 3 4", 4),
            // numbers
            ("region 0 .text x 1000 10", 4),
            ("region 0 .text 0 1g 10", 4),
            ("capacity .text 1a b0 1000 8 8", 4),
            ("capacity .text 0 b0 - 8 zz", 4),
            ("order .text 1 x", 4),
            ("slot tls-gd 0 x 1000", 4),
            ("redirect 0 1 2 q", 4),
            ("frag .r 1,2 0123456789abcdef0123456789abcdef 0", 4),
            ("frag .r 1,2,3,4 0123456789abcdef0123456789abcdef 0", 4),
            ("frag .r 1,,3 0123456789abcdef0123456789abcdef 0", 4),
            ("symblock - 1 2 3 4 x", 4),
            ("symblock - -1 2 3 4 5", 4),
            // parts and kinds
            ("capacity .text 0 q0 1000 8 8", 4),
            ("capacity .text 0 b 1000 8 8", 4),
            ("capacity .text 0 mx 1000 8 8", 4),
            ("slot gotx g 1000", 4),
            ("slot GOT g 1000", 4),
            // the hash
            ("frag .r 1,2,3 0123456789abcdef0123456789abcde 0", 4),
            ("frag .r 1,2,3 0123456789abcdef0123456789abcdef0 0", 4),
            ("frag .r 1,2,3 0123456789ABCDEF0123456789abcdef 0", 4),
            // files not earlier
            ("region 1 .text 0 1000 10", 4),
            ("slot got 1 3 1000", 4),
            ("redirect 1 1 2 1000", 4),
            ("symblock 1 1 2 3 4 5", 4),
        ];
        for (rec, line) in cases {
            let text = format!("{head}{rec}\nend\n");
            let e = parse(&text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
        // Place: not without `incremental`; not after a dynamic record;
        // no incremental record after one; group order.
        let placed: &[(&str, u32)] = &[
            ("mortar-map 2\nfile 0 a.o\nregion 0 .t 0 1000 10\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\nincremental\nbind now\nregion 0 .t 0 1000 10\nend\n", 5),
            ("mortar-map 2\nfile 0 a.o\nincremental\nregion 0 .t 0 1000 10\nhole .t 0 8\nend\n", 5),
            (
                "mortar-map 2\nfile 0 a.o\nincremental\nslot tls-ld 1000\nregion 0 .t 0 1000 10\nend\n",
                5,
            ),
            ("mortar-map 2\nfile 0 a.o\nincremental\nsymblock - 1 1 1 1 1\norder .t 0\nend\n", 5),
            ("mortar-map 2\nfile 0 a.o\nincremental\norder .t 0\nfile 1 b.o\nend\n", 5),
            // `common <file>`: a plain map has none; a map with decision
            // records requires it.
            ("mortar-map 2\nfile 0 a.o\ncommon x 10 8 8 0\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\ncommon x 10 8 8 1\nincremental\nend\n", 3),
            ("mortar-map 2\nfile 0 a.o\ncommon x 10 8 8\nincremental\norder .t 0\nend\n", 3),
        ];
        for (text, line) in placed {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
        // Each group may repeat; groups may be absent; `tls-ld` keyed
        // forms are not the module slot.
        let m = parse(&format!(
            "{head}region 0 .t 0 1000 10\nregion 0 .t 0 1000 10\nslot got 0 1 8\n\
             slot tls-ld 0 2 10\nslot plt p 20\nsymblock - 0 0 0 0 0\nend\n"
        ))
        .unwrap();
        assert_eq!(m.decisions.len(), 6);
        assert_eq!(m.decisions.slots[1].key, SlotKey::Local { file: 0, index: 2 });
        // Today's incremental map: `common` without `<file>`, no
        // decision records (transitional, WP-222).
        let m = parse("mortar-map 2\nfile 0 a.o\ncommon x 10 8 8\nincremental\nend\n").unwrap();
        assert_eq!(m.commons[0].file, None);
        let m = parse("mortar-map 2\nfile 0 a.o\ncommon x 10 8 8 0\nincremental\nend\n").unwrap();
        assert_eq!(m.commons[0].file, Some(0));
    }

    // §3.7: every truncation of a valid map is an error or a map, never a
    // panic; a truncation that removes 'end' is an error.
    #[test]
    fn s3_7_truncations_are_answers() {
        let text = write(&sample());
        for cut in 0..text.len() {
            if !text.is_char_boundary(cut) {
                continue;
            }
            let r = parse(&text[..cut]);
            if cut < text.len() - 1 {
                assert!(r.is_err(), "cut {cut}");
            }
        }
    }
}
