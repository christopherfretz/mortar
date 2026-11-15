//! mold's `-Map` / `--print-map` output (`spec/plumb.md` §3.6).

use super::lld::{Columns, add_input, row};
use super::{FileInterner, Format, Map, MapError, dec, hex};

/// Reads a mold map (§3.6).
pub fn parse(text: &str) -> Result<Map, MapError> {
    let mut lines = text.lines().enumerate().map(|(i, l)| ((i + 1) as u32, l));
    let columns = lines
        .next()
        .and_then(|(_, h)| Columns::from_header(h, &["VMA", "Size", "Align"]))
        .ok_or_else(|| {
            MapError::new(1, "not a mold map: no 'VMA Size Align Out In Symbol' header")
        })?;
    let mut map = Map::new(Format::Mold);
    let mut files = FileInterner::default();
    for (n, line) in lines {
        if line.trim().is_empty() {
            continue;
        }
        let r = row(line, &columns, n)?;
        // Non-alloc sections and the headers have VMA 0 written as `0`.
        let addr = if r.numbers[0].starts_with("0x") || r.numbers[0] == "0" {
            hex(r.numbers[0])
        } else {
            None
        }
        .ok_or_else(|| MapError::new(n, "bad VMA"))?;
        let size = dec(r.numbers[1]).ok_or_else(|| MapError::new(n, "bad size"))?;
        dec(r.numbers[2]).ok_or_else(|| MapError::new(n, "bad alignment"))?;
        if r.column == columns.input {
            add_input(&mut map, &mut files, r.text, addr, size, n);
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
    use super::super::{MapFile, SectionRef};
    use super::*;

    /// Captured from mold 2.37 (`-Wl,-Map`), trimmed.
    const EXCERPT: &str = "               VMA       Size Align Out     In      Symbol
          0x200000         64     8 EHDR
          0x200270         32     4 .note.ABI-tag
          0x200270         32     4         /usr/lib/aarch64-linux-gnu/crt1.o:(.note.ABI-tag)
          0x200270          0     0                 $d
          0x200330      53440     8 .eh_frame
          0x2398e0         32     4         a.o:(.text._Z9shared_fni)
          0x2398e0          0     0                 shared_fn(int)
          0x20f3b0       1015    16         /usr/lib/aarch64-linux-gnu/libc.a(gconv_conf.o):(.rodata)
                 0         37     1 .comment
";

    // spec/plumb.md §3.6.
    #[test]
    fn s3_6_mold_map_excerpt() {
        let m = parse(EXCERPT).unwrap();
        let rows: Vec<(String, String, u64, u64)> = m
            .placed
            .iter()
            .map(|p| {
                let SectionRef::Name(s) = &p.section else { panic!() };
                (m.files[p.file as usize].to_string(), s.clone(), p.addr, p.size)
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "/usr/lib/aarch64-linux-gnu/crt1.o".into(),
                    ".note.ABI-tag".into(),
                    0x0020_0270,
                    32
                ),
                ("a.o".into(), ".text._Z9shared_fni".into(), 0x0023_98e0, 32),
                (
                    "/usr/lib/aarch64-linux-gnu/libc.a(gconv_conf.o)".into(),
                    ".rodata".into(),
                    0x0020_f3b0,
                    1015
                ),
            ]
        );
        assert_eq!(
            m.files[2],
            MapFile::member("/usr/lib/aarch64-linux-gnu/libc.a", "gconv_conf.o")
        );
    }

    // §3.6, §3.7.
    #[test]
    fn s3_6_malformed_mold_maps_are_errors() {
        let h = "               VMA       Size Align Out     In      Symbol\n";
        assert_eq!(parse("VMA LMA Size Align Out In Symbol\n").unwrap_err().line, 1);
        assert_eq!(parse(&format!("{h}          0x10  0x4     8 .text\n")).unwrap_err().line, 2);
        assert_eq!(parse(&format!("{h}          10  4     8 .text\n")).unwrap_err().line, 2);
    }
}
