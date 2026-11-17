//! GNU ld's `-Map` file (`spec/plumb.md` §3.4).

use super::{
    Chunk, ChunkKind, DiscardReason, Discarded, FileInterner, Format, Included, IncludedBy, Map,
    MapError, Placed, SectionRef, hex, split_member,
};

/// The heading of the preamble's member list (§3.4).
const INCLUDED: &str = "Archive member included to satisfy reference by file (symbol)";
/// GNU ld prints a member in a field of this width, its reason after it;
/// a longer member name ends its line and the reason follows, indented.
const MEMBER_FIELD: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    /// "Archive member included ...", "Allocating common symbols", property notes.
    Preamble,
    /// "Discarded input sections".
    Discarded,
    /// "Memory Configuration".
    Memory,
    /// "Linker script and memory map".
    Script,
}

/// The data statements of a linker script, which GNU ld prints in the
/// input-section column (§3.4).
const DATA_STATEMENTS: [&str; 6] = ["BYTE", "SHORT", "LONG", "QUAD", "SQUAD", "FILL"];

/// The file GNU ld names for the stubs it makes (veneers, erratum patches).
const LINKER_STUBS: &str = "linker stubs";

/// Reads a GNU ld map (§3.4).
#[allow(clippy::too_many_lines)] // one arm per block and line form of §3.4
pub fn parse(text: &str) -> Result<Map, MapError> {
    let mut map = Map::new(Format::Gnu);
    let mut files = FileInterner::default();
    let mut block = Block::Preamble;
    let mut seen_script = false;
    let mut output_section = String::new();
    // A wrapped input-section line: the name and its line number.
    let mut pending: Option<(String, u32)> = None;
    // In the member list of the preamble, and a member whose reason is on
    // the next line.
    let mut in_members = false;
    let mut member: Option<(String, u32)> = None;
    for (i, line) in text.lines().enumerate() {
        let n = (i + 1) as u32;
        if block == Block::Preamble {
            if line == INCLUDED {
                in_members = true;
                continue;
            }
            if in_members {
                if let Some((m, at)) = member.take() {
                    if !line.starts_with(' ') || line.trim().is_empty() {
                        return Err(MapError::new(
                            at,
                            format!("archive member '{m}' has no reason"),
                        ));
                    }
                    map.included.push(included(&m, line.trim(), at)?);
                    continue;
                }
                if line.is_empty() {
                    continue;
                }
                if line.starts_with(' ') {
                    return Err(MapError::new(n, "a reason without an archive member"));
                }
                let split = line
                    .get(..MEMBER_FIELD)
                    .filter(|f| f.ends_with(' ') && line.len() > MEMBER_FIELD)
                    .map(|f| (f.trim_end(), &line[MEMBER_FIELD..]));
                match split {
                    // GNU ld's notes after the list (i386: "Local IFUNC
                    // function `strcpy' in libc.a(strcpy.o)") quote a
                    // name with a backquote, which no member has.
                    _ if line.contains('`') => in_members = false,
                    // A member and a reason; anything else in the field
                    // ends the list (GNU ld's notes: "discarding zero
                    // address range FDE in ...").
                    Some((m, why))
                        if !m.is_empty()
                            && let Ok(inc) = included(m, why.trim(), n) =>
                    {
                        map.included.push(inc);
                    }
                    // A long member alone (a thin archive's member is its
                    // path, without the archive's name).
                    _ if !line.contains("  ")
                        && ((line.ends_with(')') && line.contains('(')) || !line.contains(' ')) =>
                    {
                        member = Some((line.to_string(), n));
                    }
                    // Another heading ("Allocating common symbols",
                    // "Discarded input sections", ...): the list has
                    // ended, and the line is read as any other.
                    _ => in_members = false,
                }
                if in_members {
                    continue;
                }
            }
        }
        if let Some((name, at)) = pending.take() {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if !line.starts_with(' ') || tokens.len() < 2 {
                return Err(MapError::new(
                    at,
                    format!("input section '{name}' has no address line"),
                ));
            }
            entry(&mut map, &mut files, block, &output_section, &name, line, &tokens, at)?;
            continue;
        }
        match line {
            "Discarded input sections" => {
                block = Block::Discarded;
                continue;
            }
            "Memory Configuration" => {
                block = Block::Memory;
                continue;
            }
            "Linker script and memory map" => {
                block = Block::Script;
                seen_script = true;
                continue;
            }
            _ => {}
        }
        if !matches!(block, Block::Discarded | Block::Script) || line.is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes[0] != b' ' {
            // Column 0: an output section, or a command (LOAD, OUTPUT(...),
            // START GROUP, "Address of section ...").
            if block == Block::Script {
                let first = line.split_whitespace().next().unwrap_or("");
                if !matches!(
                    first,
                    "LOAD" | "START" | "END" | "Address" | "TARGET" | "INPUT" | "GROUP"
                ) && !first.starts_with("OUTPUT(")
                {
                    output_section = first.to_string();
                }
            }
            continue;
        }
        if bytes.len() < 2 || bytes[1] == b' ' || bytes[1] == b'*' {
            // Symbols, assignments, "(size before relaxing)", patterns, fill.
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if DATA_STATEMENTS.contains(&tokens[0]) {
            continue;
        }
        if tokens.len() == 1 {
            pending = Some((tokens[0].to_string(), n));
            continue;
        }
        let rest = &line[1 + tokens[0].len()..];
        let rest_tokens: Vec<&str> = rest.split_whitespace().collect();
        entry(&mut map, &mut files, block, &output_section, tokens[0], rest, &rest_tokens, n)?;
    }
    if let Some((name, at)) = pending {
        return Err(MapError::new(at, format!("input section '{name}' has no address line")));
    }
    if let Some((m, at)) = member {
        return Err(MapError::new(at, format!("archive member '{m}' has no reason")));
    }
    if !seen_script {
        return Err(MapError::new(0, "not a GNU ld map: no 'Linker script and memory map'"));
    }
    Ok(map)
}

/// A member of the preamble's list and its reason: `(--whole-archive)`,
/// `(sym)` or `file (sym)` (§3.4).
fn included(member: &str, why: &str, n: u32) -> Result<Included, MapError> {
    // A thin archive's member is named by its path (`lib/one.o`).
    let file = split_member(member);
    let bad = || MapError::new(n, format!("archive member '{member}': reason '{why}' not read"));
    let by = if why == "(--whole-archive)" {
        IncludedBy::WholeArchive
    } else if let Some(sym) = why.strip_prefix('(').and_then(|w| w.strip_suffix(')')) {
        IncludedBy::Root(sym.to_string())
    } else {
        let (f, sym) = why.rsplit_once(" (").ok_or_else(bad)?;
        let sym = sym.strip_suffix(')').ok_or_else(bad)?;
        if f.is_empty() || sym.is_empty() {
            return Err(bad());
        }
        IncludedBy::Reference { file: split_member(f), symbol: sym.to_string() }
    };
    Ok(Included { file, by, line: n })
}

/// One input-section entry: `name`, then `rest` = `0xADDR 0xSIZE FILE`.
#[allow(clippy::too_many_arguments)]
fn entry(
    map: &mut Map,
    files: &mut FileInterner,
    block: Block,
    output_section: &str,
    name: &str,
    rest: &str,
    tokens: &[&str],
    n: u32,
) -> Result<(), MapError> {
    let bad = |what: &str| MapError::new(n, format!("input section '{name}': {what}"));
    if tokens.len() < 3 || !tokens[0].starts_with("0x") || !tokens[1].starts_with("0x") {
        return Err(bad("expected '0xADDRESS 0xSIZE FILE'"));
    }
    let addr = hex(tokens[0]).ok_or_else(|| bad("bad address"))?;
    let size = hex(tokens[1]).ok_or_else(|| bad("bad size"))?;
    // The file is the rest of the line after the size: it may hold spaces.
    let after_addr = &rest[rest.find(tokens[0]).map_or(0, |p| p + tokens[0].len())..];
    let after_size = &after_addr[after_addr.find(tokens[1]).map_or(0, |p| p + tokens[1].len())..];
    let spec = after_size.trim();
    if spec == LINKER_STUBS {
        map.chunks.push(Chunk {
            kind: ChunkKind::Thunk,
            name: name.to_string(),
            addr: Some(addr),
            size,
            target: None,
            line: n,
        });
        return Ok(());
    }
    if name == "COMMON" {
        map.chunks.push(Chunk {
            kind: ChunkKind::Common,
            name: spec.to_string(),
            addr: Some(addr),
            size,
            target: None,
            line: n,
        });
        return Ok(());
    }
    let file = files.intern(map, split_member(spec));
    let section = SectionRef::Name(name.to_string());
    if block == Block::Discarded || output_section == "/DISCARD/" {
        let reason = if block == Block::Discarded {
            DiscardReason::Unspecified
        } else {
            DiscardReason::Script
        };
        map.discarded.push(Discarded { file, section, reason, line: n });
    } else {
        map.placed.push(Placed { file, section, input_offset: None, addr, size, line: n });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::MapFile;
    use super::*;

    /// Captured from GNU ld 2.44 (`gcc -static -Wl,-Map`), trimmed.
    const EXCERPT: &str = "\
Archive member included to satisfy reference by file (symbol)

/usr/lib/aarch64-linux-gnu/libc.a(libc-start.o)
                              /usr/lib/aarch64-linux-gnu/crt1.o (__libc_start_main)

Discarded input sections

 .note.gnu.property
                0x0000000000000000       0x20 /usr/lib/aarch64-linux-gnu/crt1.o
 .group         0x0000000000000000        0x8 a.o
 .text._Z1fv    0x0000000000000000       0x10 b.o

Memory Configuration

Name             Origin             Length             Attributes
*default*        0x0000000000000000 0xffffffffffffffff

Linker script and memory map

LOAD /usr/lib/aarch64-linux-gnu/crt1.o
LOAD a.o
START GROUP
LOAD /usr/lib/aarch64-linux-gnu/libc.a
END GROUP
                [!provide]                        PROVIDE (__executable_start = SEGMENT_START (\"text-segment\", 0x400000))
                0x00000000004001c8                . = (SEGMENT_START (\"text-segment\", 0x400000) + SIZEOF_HEADERS)

.rela.plt       0x00000000004001f0       0x78
 *(.rela.plt)
                0x00000000004001f0                PROVIDE (__rela_iplt_start = .)
 *(.rela.iplt)
 .rela.iplt     0x00000000004001f0       0x78 /usr/lib/aarch64-linux-gnu/crt1.o

.text           0x0000000000400300    0x57968
 *(.text.unlikely .text.*_unlikely .text.unlikely.*)
 .text.unlikely
                0x0000000000400300       0x94 /usr/lib/aarch64-linux-gnu/libc.a(abort.o)
                0x0000000000400300                abort
 *fill*         0x00000000004003a4       0x1c 1f2003d5
 .text          0x00000000004005c0       0x48 /usr/lib/aarch64-linux-gnu/crt1.o
                0x00000000004005c0                _start
 .text          0x0000000000400720       0x20 dir with space/a.o
 .text.stub     0x00000000004000c0       0x18 linker stubs
 .text          0x0000000000400740       0x10 a.o
 COMMON         0x0000000000490000       0x10 a.o
                0x0000000000490000                common_var

.rodata         0x0000000000457c80    0x1a608
 .rodata.str1.8
                0x0000000000457c88     0x67ea a.o
                                          0x6 (size before relaxing)
 LONG           0x0000000000457c80        0x4 0x0

.eh_frame       0x0000000000472288     0xd240
 .eh_frame      0x0000000000472288       0x40 /usr/lib/aarch64-linux-gnu/crt1.o
                                         0x50 (size before relaxing)

/DISCARD/
 *(.note.GNU-stack)
 .note.GNU-stack
                0x0000000000000000        0x0 a.o
LOAD linker stubs
OUTPUT(a.out elf64-littleaarch64)
";

    // spec/plumb.md §3.4: the grammar, wrapped names, the blocks, stubs,
    // COMMON, data statements, same-named sections of one file.
    #[test]
    fn s3_4_gnu_map_excerpt() {
        let m = parse(EXCERPT).unwrap();
        let crt1 = MapFile::plain("/usr/lib/aarch64-linux-gnu/crt1.o");
        assert_eq!(m.files[0], crt1);
        let names: Vec<(String, &str, u64, u64)> = m
            .placed
            .iter()
            .map(|p| {
                let SectionRef::Name(s) = &p.section else { panic!() };
                (m.files[p.file as usize].to_string(), s.as_str(), p.addr, p.size)
            })
            .collect();
        assert_eq!(
            names,
            [
                ("/usr/lib/aarch64-linux-gnu/crt1.o".to_string(), ".rela.iplt", 0x0040_01f0, 0x78),
                (
                    "/usr/lib/aarch64-linux-gnu/libc.a(abort.o)".to_string(),
                    ".text.unlikely",
                    0x0040_0300,
                    0x94
                ),
                ("/usr/lib/aarch64-linux-gnu/crt1.o".to_string(), ".text", 0x0040_05c0, 0x48),
                ("dir with space/a.o".to_string(), ".text", 0x0040_0720, 0x20),
                ("a.o".to_string(), ".text", 0x0040_0740, 0x10),
                ("a.o".to_string(), ".rodata.str1.8", 0x0045_7c88, 0x67ea),
                ("/usr/lib/aarch64-linux-gnu/crt1.o".to_string(), ".eh_frame", 0x0047_2288, 0x40),
            ]
        );
        assert_eq!(m.files.iter().filter(|f| f.member.is_some()).count(), 1);
        let discarded: Vec<_> = m.discarded.iter().map(|d| (d.file, d.reason, d.line)).collect();
        assert_eq!(discarded.len(), 4);
        assert_eq!(discarded[3].1, DiscardReason::Script);
        assert!(discarded[..3].iter().all(|d| d.1 == DiscardReason::Unspecified));
        assert_eq!(discarded[0].2, 8, "a wrapped entry has the line of its name");
        let chunks: Vec<_> = m.chunks.iter().map(|c| (c.kind, c.name.as_str())).collect();
        assert_eq!(chunks, [(ChunkKind::Thunk, ".text.stub"), (ChunkKind::Common, "a.o")]);
    }

    // §3.4: the preamble's member list (the input set, §3.8): a member in
    // its 30-column field and the reason after it, or a longer member and
    // the reason on the next line; the three forms of a reason.
    #[test]
    fn s3_4_the_preamble_s_members_and_reasons() {
        let m = parse(EXCERPT).unwrap();
        assert_eq!(
            m.included,
            [Included {
                file: MapFile::member("/usr/lib/aarch64-linux-gnu/libc.a", "libc-start.o"),
                by: IncludedBy::Reference {
                    file: MapFile::plain("/usr/lib/aarch64-linux-gnu/crt1.o"),
                    symbol: "__libc_start_main".into(),
                },
                line: 3,
            }]
        );
        let text = "\
Archive member included to satisfy reference by file (symbol)

libk.a(k.o)                   (k_fn)
libk.a(j.o)                   (--whole-archive)
libk.a(i.o)                   libk.a(k.o) (i_fn)
a very long archive name.a(member.o)
                              dir (x)/r.o (r_fn)
Local IFUNC function `strcpy' in libc.a(strcpy.o)

Allocating common symbols
Common symbol       size              file

v                   0x4               r.o

Linker script and memory map
";
        let m = parse(text).unwrap();
        let got: Vec<(String, IncludedBy, u32)> =
            m.included.iter().map(|i| (i.file.to_string(), i.by.clone(), i.line)).collect();
        assert_eq!(
            got,
            [
                ("libk.a(k.o)".into(), IncludedBy::Root("k_fn".into()), 3),
                ("libk.a(j.o)".into(), IncludedBy::WholeArchive, 4),
                (
                    "libk.a(i.o)".into(),
                    IncludedBy::Reference {
                        file: MapFile::member("libk.a", "k.o"),
                        symbol: "i_fn".into()
                    },
                    5
                ),
                (
                    "a very long archive name.a(member.o)".into(),
                    IncludedBy::Reference {
                        file: MapFile::plain("dir (x)/r.o"),
                        symbol: "r_fn".into()
                    },
                    6
                ),
            ]
        );
        let head = "Archive member included to satisfy reference by file (symbol)\n\n";
        let tail = "\nLinker script and memory map\n";
        for (bad, line) in [
            ("a long archive member name.a(m.o)\n\n", 3),
            ("                              r.o (x)\n", 3),
        ] {
            assert_eq!(parse(&format!("{head}{bad}{tail}")).unwrap_err().line, line, "{bad}");
        }
        // GNU ld's notes in the list (i386) end it.
        let note = "discarding zero address range FDE in libc.a(clone3.o)(.eh_frame).\n";
        let m = parse(&format!("{head}lib/k.o                       (x)\n{note}libk.a(j.o)                   r.o x\n{tail}")).unwrap();
        assert_eq!(
            m.included,
            [Included {
                file: MapFile::plain("lib/k.o"),
                by: IncludedBy::Root("x".into()),
                line: 3
            }]
        );
        let ended = format!("{head}a long archive member name.a(m.o)");
        assert_eq!(parse(&ended).unwrap_err().line, 3);
    }

    // §3.4, §3.7: what is refused.
    #[test]
    fn s3_4_malformed_gnu_maps_are_errors() {
        assert_eq!(parse("Archive member included\n").unwrap_err().line, 0);
        let wrapped = "Linker script and memory map\n\n.text 0x0 0x0\n .text.x\n";
        assert_eq!(parse(wrapped).unwrap_err().line, 4);
        let bad = "Linker script and memory map\n\n.text 0x0 0x0\n .text.x\n.data\n";
        assert_eq!(parse(bad).unwrap_err().line, 4);
        let nofile = "Linker script and memory map\n .text 0x10 0x4\n";
        assert_eq!(parse(nofile).unwrap_err().line, 2);
        let badnum = "Linker script and memory map\n .text 0x1g 0x4 a.o\n";
        assert_eq!(parse(badnum).unwrap_err().line, 2);
    }
}
