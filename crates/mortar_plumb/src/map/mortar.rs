//! mortar's own placement map (`spec/plumb.md` §3.2): a versioned,
//! line-oriented text format with one record per line.
//!
//! ```text
//! mortar-map 1
//! file <id> <path> [<member> <member-header-offset>]
//! place <file> <section> <addr> <size>
//! piece <file> <section> <input-offset> <addr> <size>
//! nplace <file> <section> <output-section> <offset> <size>
//! merged <file> <section> <output-section>
//! discard <file> <section> gc|comdat|script|unspecified
//! fold <file> <section> <into-file> <into-section>
//! chunk thunk|patch|synthetic|common <name> <addr>|- <size> [<target> <addend>]
//! define <name> <value>
//! output static|exec|pie|static-pie
//! interp <path>
//! runpath|rpath <paths>
//! bind lazy|now
//! relro on|off
//! arg <word>
//! needed <name>
//! copy <name> <addr> <size>
//! export <name>
//! plt <name>
//! iplt <name>
//! end
//! ```
//!
//! The records from `output` on are the dynamic records (P-D3 of
//! `spec/dyn.md`): facts of the command line, then order hints.
//!
//! A `<section>` is the section's index in its file, in decimal, or `:`
//! and the section's name as a token (a foreign map converted, §3.2).

use std::fmt::Write as _;

use super::{
    Chunk, ChunkKind, CopyHint, Defined, DiscardReason, Discarded, DynRecords, Fold, Format, Map,
    MapError, MapFile, NonAlloc, OutputKind, Placed, RunpathTag, SectionRef, dec, hex,
};

/// The first line of a version-1 map.
pub const HEADER: &str = "mortar-map 1";

/// Encodes a string as a token (§3.2): bytes from `!` to `~` other than
/// `%` stand for themselves, every other byte is `%XX` (upper-case hex),
/// and the empty string is a lone `%`.
pub fn encode(text: &str, out: &mut String) {
    if text.is_empty() {
        out.push('%');
        return;
    }
    for &b in text.as_bytes() {
        if (0x21..=0x7e).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

/// Decodes a token written by [`encode`]. `None` for a malformed escape or
/// for bytes that are not UTF-8.
pub fn decode(token: &str) -> Option<String> {
    if token == "%" {
        return Some(String::new());
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
    String::from_utf8(out).ok()
}

fn signed_hex(v: i64, out: &mut String) {
    if v < 0 {
        let _ = write!(out, "-{:x}", v.unsigned_abs());
    } else {
        let _ = write!(out, "{v:x}");
    }
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
/// files by id, then places, pieces, discards, folds, chunks and defined
/// symbols, each in the order of the map's vectors.
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
    for d in &map.defined {
        out.push_str("define ");
        encode(&d.name, &mut out);
        let _ = writeln!(out, " {:x}", d.value);
    }
    if let Some(d) = &map.dynamic {
        write_dynamic(d, &mut out);
    }
    out.push_str("end\n");
    out
}

/// The dynamic records of §3.2, in their fixed order.
fn write_dynamic(d: &DynRecords, out: &mut String) {
    let named = |out: &mut String, record: &str, name: &str| {
        out.push_str(record);
        out.push(' ');
        encode(name, out);
        out.push('\n');
    };
    if let Some(k) = d.kind {
        let _ = writeln!(out, "output {}", k.name());
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
    for a in &d.args {
        named(out, "arg", a);
    }
    for n in &d.needed {
        named(out, "needed", n);
    }
    for c in &d.copies {
        out.push_str("copy ");
        encode(&c.name, out);
        let _ = writeln!(out, " {:x} {:x}", c.addr, c.size);
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
        _ => return Err(MapError::new(1, "not a mortar map (no 'mortar-map 1' header)")),
    }
    let mut ended = false;
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
        match (t[0], t.len()) {
            ("end", 1) => ended = true,
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
            ("define", 3) => map.defined.push(Defined {
                name: decode(t[1]).ok_or_else(|| bad("name"))?,
                value: num(t[2], "value")?,
                line: n,
            }),
            ("output", 2) => {
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                if d.kind.is_some() {
                    return Err(bad("a second 'output' record"));
                }
                d.kind = Some(OutputKind::from_name(t[1]).ok_or_else(|| bad("kind"))?);
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
            ("arg" | "needed" | "export" | "plt" | "iplt", 2) => {
                let v = decode(t[1]).ok_or_else(|| bad("name"))?;
                let d = map.dynamic.get_or_insert_with(DynRecords::default);
                match t[0] {
                    "arg" => d.args.push(v),
                    "needed" => d.needed.push(v),
                    "export" => d.exports.push(v),
                    "plt" => d.plt.push(v),
                    _ => d.iplt.push(v),
                }
            }
            ("copy", 4) => {
                let c = CopyHint {
                    name: decode(t[1]).ok_or_else(|| bad("name"))?,
                    addr: num(t[2], "address")?,
                    size: num(t[3], "size")?,
                };
                map.dynamic.get_or_insert_with(DynRecords::default).copies.push(c);
            }
            ("", 1) => return Err(MapError::new(n, "empty line before 'end'")),
            (
                "end" | "file" | "place" | "piece" | "nplace" | "merged" | "discard" | "fold"
                | "chunk" | "define" | "output" | "interp" | "runpath" | "rpath" | "bind" | "relro"
                | "arg" | "needed" | "copy" | "export" | "plt" | "iplt",
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
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        m.defined.push(Defined { name: "_end".into(), value: 0x50_0000, line: 0 });
        m
    }

    fn strip_lines(mut m: Map) -> Map {
        m.placed.iter_mut().for_each(|p| p.line = 0);
        m.discarded.iter_mut().for_each(|p| p.line = 0);
        m.folds.iter_mut().for_each(|p| p.line = 0);
        m.chunks.iter_mut().for_each(|p| p.line = 0);
        m.defined.iter_mut().for_each(|p| p.line = 0);
        m
    }

    // spec/plumb.md §3.2: the writer and the reader are inverses.
    #[test]
    fn s3_2_round_trip() {
        let m = sample();
        let text = write(&m);
        assert!(text.starts_with("mortar-map 1\nfile 0 dir%20with%20space/a.o\n"), "{text}");
        assert!(text.contains("file 2 100%25%C3%A9.o\n"), "{text}");
        assert!(text.contains("chunk synthetic % - 4\n"), "{text}");
        assert!(text.contains("chunk thunk __veneer%20x 400020 c far -8\n"), "{text}");
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
            ("mortar-map 2\nend\n", 1),
            ("mortar-map 1\n", 0),
            ("mortar-map 1\nfile 1 a.o\nend\n", 2),
            ("mortar-map 1\nplace 0 1 0 0\nend\n", 2),
            ("mortar-map 1\nfile 0 a.o\nplace 0 1 0\nend\n", 3),
            ("mortar-map 1\nfile 0 a.o\nplace 0 x 0 0\nend\n", 3),
            ("mortar-map 1\nfile 0 a.o\nplace 0 1 0g 0\nend\n", 3),
            ("mortar-map 1\nfile 0 a.o\ndiscard 0 1 why\nend\n", 3),
            ("mortar-map 1\nfrob\nend\n", 2),
            ("mortar-map 1\nend\nfile 0 a.o\n", 3),
            ("mortar-map 1\n\nend\n", 2),
            ("mortar-map 1\nfile 0 a.o\nchunk thunk x 0 4 y 1z\nend\n", 3),
            ("mortar-map 1\nfile 0  a.o\nend\n", 2),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
        assert!(parse("mortar-map 1\nend\n\n").is_ok());
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
            args: vec!["-pie".into(), "--as-needed".into(), "-lc".into(), String::new()],
            needed: vec!["libc.so.6".into(), "libm.so.6".into()],
            copies: vec![CopyHint { name: "stdout".into(), addr: 0x2_0010, size: 8 }],
            exports: vec!["main".into()],
            plt: vec!["puts".into(), "memcpy@GLIBC_2.2.5".into()],
            iplt: vec!["my_ifunc".into()],
        });
        let text = write(&m);
        assert!(
            text.ends_with(
                "define _end 500000\noutput pie\ninterp /lib/ld-linux-aarch64.so.1\n\
                 runpath $ORIGIN/lib:/opt/x%20y\nbind now\nrelro off\narg -pie\n\
                 arg --as-needed\narg -lc\narg %\nneeded libc.so.6\nneeded libm.so.6\n\
                 copy stdout 20010 8\nexport main\nplt puts\nplt memcpy@GLIBC_2.2.5\n\
                 iplt my_ifunc\nend\n"
            ),
            "{text}"
        );
        assert_eq!(strip_lines(parse(&text).unwrap()), m);
        // Every record is optional.
        let only = parse("mortar-map 1\nrpath /a\nend\n").unwrap();
        assert_eq!(
            only.dynamic,
            Some(DynRecords {
                runpath: Some((RunpathTag::Rpath, "/a".into())),
                ..DynRecords::default()
            })
        );
    }

    // §3.2: malformed dynamic records.
    #[test]
    fn s3_2_malformed_dynamic_records_are_errors() {
        let cases: &[(&str, u32)] = &[
            ("mortar-map 1\noutput shared\nend\n", 2),
            ("mortar-map 1\noutput pie\noutput pie\nend\n", 3),
            ("mortar-map 1\ninterp a\ninterp b\nend\n", 3),
            ("mortar-map 1\nrunpath a\nrpath b\nend\n", 3),
            ("mortar-map 1\nbind eager\nend\n", 2),
            ("mortar-map 1\nrelro yes\nend\n", 2),
            ("mortar-map 1\nbind now\nbind now\nend\n", 3),
            ("mortar-map 1\ncopy x 10\nend\n", 2),
            ("mortar-map 1\ncopy x 10 zz\nend\n", 2),
            ("mortar-map 1\nneeded\nend\n", 2),
            ("mortar-map 1\nplt a b\nend\n", 2),
            ("mortar-map 1\nexport %4\nend\n", 2),
        ];
        for (text, line) in cases {
            let e = parse(text).unwrap_err();
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
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
