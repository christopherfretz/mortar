//! lld's `-Map` file (`spec/plumb.md` §3.5), and the column grammar it
//! shares with mold's (§3.6).

use super::{
    Chunk, ChunkKind, FileInterner, Format, Map, MapError, Placed, SectionRef, dec, hex,
    split_input, split_member,
};

/// The columns of a table-shaped map (lld, mold).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Columns {
    /// How many numbers start a row.
    pub numbers: usize,
    pub out: usize,
    pub input: usize,
    pub symbol: usize,
}

impl Columns {
    /// Reads the header row: `names` before `Out In Symbol`.
    pub(crate) fn from_header(header: &str, names: &[&str]) -> Option<Columns> {
        let tokens: Vec<&str> = header.split_whitespace().collect();
        let mut expected: Vec<&str> = names.to_vec();
        expected.extend(["Out", "In", "Symbol"]);
        if tokens != expected {
            return None;
        }
        let col = |name: &str| header.find(&format!(" {name}")).map(|p| p + 1);
        Some(Columns {
            numbers: names.len(),
            out: col("Out")?,
            input: col("In")?,
            symbol: col("Symbol")?,
        })
    }
}

/// One row: the numbers and the column the text starts in.
pub(crate) struct Row<'a> {
    pub numbers: Vec<&'a str>,
    pub column: usize,
    pub text: &'a str,
}

pub(crate) fn row<'a>(line: &'a str, columns: &Columns, n: u32) -> Result<Row<'a>, MapError> {
    let mut numbers = Vec::with_capacity(columns.numbers);
    let mut rest = line;
    let mut offset = 0;
    for _ in 0..columns.numbers {
        let trimmed = rest.trim_start();
        offset += rest.len() - trimmed.len();
        let end = trimmed.find(' ').unwrap_or(trimmed.len());
        if end == 0 {
            return Err(MapError::new(n, "row has too few numbers"));
        }
        numbers.push(&trimmed[..end]);
        offset += end;
        rest = &trimmed[end..];
    }
    let text = rest.trim_start();
    let column = offset + (rest.len() - text.len());
    if text.is_empty() {
        return Err(MapError::new(n, "row has no name"));
    }
    Ok(Row { numbers, column, text })
}

/// What an input-column row says.
pub(crate) enum InputRow<'a> {
    /// `file:(section)`, maybe a piece (`section+0xOFF`).
    Section { file: &'a str, section: &'a str, piece: Option<u64> },
    /// Not an input section: an assignment or a command.
    Other,
}

pub(crate) fn input_row(text: &str) -> InputRow<'_> {
    let Some((file, section)) = split_input(text) else { return InputRow::Other };
    if file.is_empty() || section.is_empty() {
        return InputRow::Other;
    }
    if let Some(plus) = section.rfind("+0x")
        && let Some(off) = hex(&section[plus + 1..])
    {
        return InputRow::Section { file, section: &section[..plus], piece: Some(off) };
    }
    InputRow::Section { file, section, piece: None }
}

/// The kind of a synthetic chunk by the name lld gives it.
#[allow(clippy::case_sensitive_file_extension_comparisons)] // section names, not files
fn chunk_kind(name: &str) -> ChunkKind {
    if name == "COMMON" {
        ChunkKind::Common
    } else if name.ends_with(".thunk") {
        ChunkKind::Thunk
    } else if name.ends_with(".patch") {
        ChunkKind::Patch
    } else {
        ChunkKind::Synthetic
    }
}

/// Adds an input-column row to `map`.
pub(crate) fn add_input(
    map: &mut Map,
    files: &mut FileInterner,
    text: &str,
    addr: u64,
    size: u64,
    n: u32,
) {
    match input_row(text) {
        InputRow::Other => {}
        InputRow::Section { file: "<internal>", section, .. } => map.chunks.push(Chunk {
            kind: chunk_kind(section),
            name: section.to_string(),
            addr: Some(addr),
            size,
            target: None,
            line: n,
        }),
        // lld's `COMMON`, mold's `.common` (§3.5, §3.6): the file's common
        // symbols, which have no input section.
        InputRow::Section { file, section: "COMMON" | ".common", .. } => map.chunks.push(Chunk {
            kind: ChunkKind::Common,
            name: file.to_string(),
            addr: Some(addr),
            size,
            target: None,
            line: n,
        }),
        InputRow::Section { file, section, piece } => {
            let file = files.intern(map, split_member(file));
            map.placed.push(Placed {
                file,
                section: SectionRef::Name(section.to_string()),
                input_offset: piece,
                addr,
                size,
                line: n,
            });
        }
    }
}

/// Reads an lld map (§3.5).
pub fn parse(text: &str) -> Result<Map, MapError> {
    let mut lines = text.lines().enumerate().map(|(i, l)| ((i + 1) as u32, l));
    let columns = lines
        .next()
        .and_then(|(_, h)| Columns::from_header(h, &["VMA", "LMA", "Size", "Align"]))
        .ok_or_else(|| {
            MapError::new(1, "not an lld map: no 'VMA LMA Size Align Out In Symbol' header")
        })?;
    let mut map = Map::new(Format::Lld);
    let mut files = FileInterner::default();
    for (n, line) in lines {
        if line.trim().is_empty() {
            continue;
        }
        // An assignment row whose text lld 19 leaves empty (a `--defsym`,
        // `spec/plumb.md` §4.4b, WP-170): four numbers, size 0, no text.
        // It places nothing.
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() == columns.numbers
            && hex(words[2]) == Some(0)
            && words.iter().take(2).all(|w| hex(w).is_some())
            && dec(words[3]).is_some()
        {
            continue;
        }
        let r = row(line, &columns, n)?;
        let addr = hex(r.numbers[0]).ok_or_else(|| MapError::new(n, "bad VMA"))?;
        hex(r.numbers[1]).ok_or_else(|| MapError::new(n, "bad LMA"))?;
        let size = hex(r.numbers[2]).ok_or_else(|| MapError::new(n, "bad size"))?;
        dec(r.numbers[3]).ok_or_else(|| MapError::new(n, "bad alignment"))?;
        if r.column == columns.input {
            add_input(&mut map, &mut files, r.text, addr, size, n);
        } else if r.column == columns.out {
            super::push_block(&mut map, r.text, addr, size, n);
        } else if r.column != columns.out && r.column != columns.symbol {
            return Err(MapError::new(
                n,
                format!("text in column {} is in no column of the header", r.column + 1),
            ));
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::super::MapFile;
    use super::*;

    /// Captured from lld 22 (`-Wl,-Map`), trimmed.
    const EXCERPT: &str = "             VMA              LMA     Size Align Out     In      Symbol
          200238           200238       20     4 .note.ABI-tag
          200238           200238       20     4         /usr/lib/aarch64-linux-gnu/crt1.o:(.note.ABI-tag)
          200238           200238        0     1                 $d
          200300           200300    1a6c0    16 .rodata
          200308           200308     686a     8         <internal>:(.rodata.str1.8)
          206d3c           206d3c      118     4         /usr/lib/gcc/libgcc_eh.a(unwind-dw2.o):(.rodata)
          21aaa0           21aaa0     d0c0     8 .eh_frame
          21aaa0           21aaa0       28     1         /usr/lib/aarch64-linux-gnu/crt1.o:(.eh_frame+0x0)
          21aac8           21aac8       14     1         /usr/lib/aarch64-linux-gnu/crt1.o:(.eh_frame+0x3c)
          400000           400000       20     4 .text
          400000           400000       10     4         far.o:(.text)
          400000           400000       10     4         far.o:(.text)
          400010           400010       10     8         <internal>:(.text.thunk)
          400020           400020        8     4         <internal>:(.text.patch)
          410000           410000       10     8         a.o:(COMMON)
          410010           410010        0     1                 _end = .
               0                0       1a     1 .comment
               0                0       1a     1         <internal>:(.comment)
";

    // spec/plumb.md §3.5: columns, pieces, synthetic chunks, same-named
    // sections of one file, assignments.
    #[test]
    fn s3_5_lld_map_excerpt() {
        let m = parse(EXCERPT).unwrap();
        let rows: Vec<(String, String, Option<u64>, u64, u64)> = m
            .placed
            .iter()
            .map(|p| {
                let SectionRef::Name(s) = &p.section else { panic!() };
                (m.files[p.file as usize].to_string(), s.clone(), p.input_offset, p.addr, p.size)
            })
            .collect();
        let crt1 = "/usr/lib/aarch64-linux-gnu/crt1.o".to_string();
        assert_eq!(
            rows,
            [
                (crt1.clone(), ".note.ABI-tag".into(), None, 0x0020_0238, 0x20),
                (
                    "/usr/lib/gcc/libgcc_eh.a(unwind-dw2.o)".into(),
                    ".rodata".into(),
                    None,
                    0x0020_6d3c,
                    0x118
                ),
                (crt1.clone(), ".eh_frame".into(), Some(0), 0x0021_aaa0, 0x28),
                (crt1, ".eh_frame".into(), Some(0x3c), 0x0021_aac8, 0x14),
                ("far.o".into(), ".text".into(), None, 0x0040_0000, 0x10),
                ("far.o".into(), ".text".into(), None, 0x0040_0000, 0x10),
            ]
        );
        assert_eq!(m.files[1], MapFile::member("/usr/lib/gcc/libgcc_eh.a", "unwind-dw2.o"));
        let kinds: Vec<_> = m.chunks.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                ChunkKind::Synthetic,
                ChunkKind::Thunk,
                ChunkKind::Patch,
                ChunkKind::Common,
                ChunkKind::Synthetic
            ]
        );
    }

    // §3.5, §3.7.
    #[test]
    fn s3_5_malformed_lld_maps_are_errors() {
        assert_eq!(parse("VMA Size Align Out In Symbol\n").unwrap_err().line, 1);
        let h = "             VMA              LMA     Size Align Out     In      Symbol\n";
        assert_eq!(parse(&format!("{h}  zz 0 0 1 .text\n")).unwrap_err().line, 2);
        assert_eq!(parse(&format!("{h}  0 0 0\n")).unwrap_err().line, 2);
        assert_eq!(parse(&format!("{h}  0 0 0 1   .text\n")).unwrap_err().line, 2);
        assert_eq!(parse(&format!("{h}  0 0 0 x .text\n")).unwrap_err().line, 2);
        assert!(parse(h).unwrap().placed.is_empty());
        // A sized row without text is not an assignment.
        assert_eq!(parse(&format!("{h}  0 0 8 1 \n")).unwrap_err().line, 2);
    }

    // §4.4b (WP-170): lld 19's row of a `--defsym` (no text, size 0)
    // places nothing.
    #[test]
    fn s4_4b_lld_defsym_row_is_an_assignment() {
        let h = "             VMA              LMA     Size Align Out     In      Symbol\n";
        let m = parse(&format!("{h}           30be1            30be1        0     1 \n")).unwrap();
        assert!(m.placed.is_empty() && m.chunks.is_empty());
    }
}
