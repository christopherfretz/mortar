//! `incr_state`: format-5 state tool (WP-214, U-2; `spec/incr.md` IN-18c,
//! IN-78).
//!
//! ```text
//! incr_state convert <format-4 state> <out> <mortar version string>
//! incr_state canon <state>
//! incr_state compare <state A> <state B>
//! incr_state size <state> [<format-4 state> <mortar version string>]
//! incr_state strings <format-4 state> <mortar version string> <path suffix>...
//! ```
//!
//! `convert` writes the format-5 state of a format-4 one (the full link's
//! writer over its rows: `encode(normalize(from_format4(..)))`); `canon`
//! prints the canonical rows' digest per table; `compare` prints the
//! canonical differences (exit 1 when any); `size` prints the per-table
//! sizes and, given the format-4 state, the ratio (U-2's 1.15 gate). Every
//! command refuses a format-5 state whose updating flag is set.

use std::process::ExitCode;

use mortar_core::incr::{MemoryLayout, StoredLayout};
use mortar_incr::format;
use mortar_incr::state5::{self, compact, read, write};
use mortar_incr::update::testing::MemoryState;

fn load5(path: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    read::refuse_updating(&bytes).map_err(|e| format!("{path}: {e}"))?;
    Ok(bytes)
}

fn convert(path4: &str, version: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path4).map_err(|e| format!("{path4}: {e}"))?;
    let s = format::decode(&bytes, version.as_bytes()).map_err(|e| format!("{path4}: {e}"))?;
    let mut m = MemoryState::from_format4(&s);
    seam_rows(&mut m);
    state5::check_capture(&m).map_err(|e| format!("{path4}: {}", e.report()))?;
    let n = compact::normalize(&m).map_err(|e| format!("{path4}: {e}"))?;
    write::encode(&n).map_err(|e| format!("{path4}: {e}"))
}

/// The rows format 4 does not store but a full link's capture writes
/// (WP-215, WP-220), rebuilt from format 4's own rows so the size gate
/// counts them: `GROUPS` one row per (signature, file) of a section whose
/// anchor is a group signature; `COMMONS` one per (key, file) of a common
/// mention; `COPIES` one per global definition in such a section (format 4
/// keeps one mention per (file, key) and no `st_size`: the count is a
/// lower bound, the sizes 0).
fn seam_rows(m: &mut MemoryState) {
    use mortar_incr::update::rows::{CommonRow, CopyRow, GroupRow};
    let bytes = |x: format::Str| {
        m.strings.get(x.off as usize..(x.off + x.len) as usize).unwrap_or_default().to_vec()
    };
    let mut seen = std::collections::BTreeMap::new();
    for r in &m.sections {
        if r.anchor_kind == 1 && r.file != u32::MAX {
            seen.entry((bytes(r.anchor), r.file)).or_insert(r.anchor);
        }
    }
    let groups: Vec<GroupRow> = seen
        .into_iter()
        .map(|((_, file), signature)| GroupRow { signature, file, tomb: false })
        .collect();
    let mut commons = Vec::new();
    let mut copies = Vec::new();
    for (f, r) in &m.mentions {
        if r.kind == format::def_kind::COMMON {
            commons.push(CommonRow { key: r.symbol, file: f.0, size: 0, align: 1, tomb: false });
        }
        let in_group = m.sections.get(r.section as usize).is_some_and(|x| x.anchor_kind == 1);
        if r.kind == format::def_kind::SECTION && in_group && r.bind != 0 {
            copies.push(CopyRow {
                key: r.symbol,
                file: f.0,
                index: 0,
                size: 0,
                typ: r.typ,
                tomb: false,
            });
        }
    }
    commons.sort_by_key(|c| (c.key, c.file));
    commons.dedup_by_key(|c| (c.key, c.file));
    for (i, c) in copies.iter_mut().enumerate() {
        c.index = u32::try_from(i).unwrap_or(u32::MAX);
    }
    eprintln!(
        "seam rows from format 4: GROUPS {} (was {}), COMMONS {} (was {}), COPIES {}",
        groups.len(),
        m.groups.len(),
        commons.len(),
        m.commons.len(),
        copies.len()
    );
    m.groups = groups;
    m.commons = commons;
    m.copies = copies;
}

/// `STRINGS`' rows and capacity in a format-5 state's directory.
fn strings_entry(bytes: &[u8]) -> Result<(u64, u64), String> {
    let h = state5::Header::parse(bytes).map_err(|e| e.to_string())?;
    let end = state5::HEADER + state5::TABLES.len() * state5::ENTRY;
    let dir = state5::parse_directory(&h, bytes.get(state5::HEADER..end).ok_or("short")?)
        .map_err(|e| e.to_string())?;
    let e = &dir[state5::table_index(format::kind::STRINGS).ok_or("no STRINGS")?];
    Ok((e.rows, e.capacity))
}

/// F-11's measurement: per file whose path ends with one of `suffixes`,
/// the update of that file recompiled unchanged (its `SECTIONS` names and
/// anchors and its `GROUPS` signatures all in the update's own pool, as
/// `ChangedFiles::strings` gives them): the pool's bytes (what a patch
/// appended before F-11) and what `STRINGS` grows by now.
fn strings_growth(path4: &str, version: &str, suffixes: &[&str]) -> Result<(), String> {
    use mortar_core::FileId;
    use mortar_incr::update::rows::GroupRow;
    use mortar_incr::update::view::{FileRanges, StatePatch};
    let bytes = convert(path4, version)?;
    let m = read::decode(&bytes).map_err(|e| e.to_string())?;
    let (rows, capacity) = strings_entry(&bytes)?;
    let old = m.strings.len();
    let s = |x: format::Str| m.strings[x.off as usize..(x.off + x.len) as usize].to_vec();
    let mut out: Vec<(usize, u64, String)> = Vec::new();
    for (f, file) in m.files.iter().enumerate() {
        let path = String::from_utf8_lossy(&s(file.path)).into_owned();
        if !suffixes.iter().any(|x| path.ends_with(x)) {
            continue;
        }
        let f32 = u32::try_from(f).map_err(|e| e.to_string())?;
        let mut p = StatePatch::default();
        let fresh = |p: &mut StatePatch, b: &[u8]| {
            let off = u32::try_from(old + p.strings.len()).unwrap_or(u32::MAX);
            p.strings.extend_from_slice(b);
            format::Str { off, len: u32::try_from(b.len()).unwrap_or(u32::MAX) }
        };
        let mut sections = Vec::new();
        for r in m.sections.iter().filter(|r| r.file == f32) {
            let name = fresh(&mut p, &s(r.name));
            let anchor = if r.anchor.len == 0 { r.anchor } else { fresh(&mut p, &s(r.anchor)) };
            sections.push(format::SectionRow { name, anchor, ..r.clone() });
        }
        for g in m.groups.iter().filter(|g| g.file == f32 && !g.tomb) {
            let signature = fresh(&mut p, &s(g.signature));
            p.groups.put.push(GroupRow { signature, ..*g });
        }
        p.ranges.push(FileRanges {
            file: FileId(f32),
            first_global: m.file_index.get(f).map_or(0, |i| i.first_global),
            sections: Some(sections),
            mentions: None,
            index: None,
            edges: None,
            frag_uses: None,
            symtab: None,
            regions: None,
            gone: false,
        });
        let view = read::open_bytes(bytes.clone()).map_err(|e| e.to_string())?;
        let plan = state5::plan_patch(&view, &p).map_err(|e| e.to_string())?;
        let out_row =
            m.stat.iter().rfind(|r| r.tag == format::stat_tag::OUTPUT).cloned().unwrap_or_default();
        let grew = match write::patched_bytes(&bytes, &plan, &out_row) {
            Some(b) => strings_entry(&b)?.0 - rows,
            None => u64::MAX,
        };
        let member = String::from_utf8_lossy(&s(file.member_name)).into_owned();
        out.push((p.strings.len(), grew, member));
    }
    out.sort_by_key(|x| x.0);
    let area = capacity - rows;
    println!("files {} STRINGS rows {rows} capacity {capacity} area {area}", out.len());
    let n = out.len();
    if n == 0 {
        return Ok(());
    }
    for (label, i) in [("median", n / 2), ("p90", n * 9 / 10), ("max", n - 1)] {
        let (pool, grew, name) = &out[i];
        println!("{label} (by pool): {name} pool {pool} grew {grew}");
    }
    let mut grew: Vec<u64> = out.iter().map(|x| x.1).collect();
    grew.sort_unstable();
    println!(
        "grew median {} p90 {} max {}; compacted {}",
        grew[n / 2],
        grew[n * 9 / 10],
        grew[n - 1],
        grew.iter().filter(|&&g| g == u64::MAX).count()
    );
    Ok(())
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["convert", from, to, version] => {
            let bytes = convert(from, version)?;
            std::fs::write(to, &bytes).map_err(|e| format!("{to}: {e}"))?;
            // the round trip: the written state reads back to its own bytes
            let back = read::decode(&bytes).map_err(|e| format!("{to}: {e}"))?;
            let again = write::encode(&compact::normalize(&back).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            if again != bytes {
                return Err(format!("{to}: the round trip differs"));
            }
            println!("converted {from} -> {to}: {} bytes", bytes.len());
            Ok(ExitCode::SUCCESS)
        }
        ["canon", path] => {
            let c = state5::canon(&load5(path)?).map_err(|e| format!("{path}: {e}"))?;
            for (k, rows) in &c.tables {
                let mut all = Vec::new();
                for r in rows {
                    all.extend_from_slice(&(r.len() as u64).to_le_bytes());
                    all.extend_from_slice(r);
                }
                let h = mortar_digest::l128(&all);
                let mut hex = String::with_capacity(32);
                for b in h {
                    use std::fmt::Write as _;
                    let _ = write!(hex, "{b:02x}");
                }
                println!("{:<12} {:>10} rows {hex}", state5::name(*k), rows.len());
            }
            Ok(ExitCode::SUCCESS)
        }
        ["compare", a, b] => {
            let ca = state5::canon(&load5(a)?).map_err(|e| format!("{a}: {e}"))?;
            let cb = state5::canon(&load5(b)?).map_err(|e| format!("{b}: {e}"))?;
            let rows = MemoryLayout::default();
            let diffs = state5::compare(&ca, &cb, &StoredLayout::new(&rows));
            for d in &diffs {
                println!("{} {}: {}", state5::name(d.kind), d.row, d.what);
            }
            println!("{} differences", diffs.len());
            Ok(if diffs.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        ["size", path] => {
            print!("{}", state5::size_report(&load5(path)?).map_err(|e| format!("{path}: {e}"))?);
            Ok(ExitCode::SUCCESS)
        }
        ["size", path4, version] => {
            let four = std::fs::metadata(path4).map_err(|e| format!("{path4}: {e}"))?.len();
            let bytes = convert(path4, version)?;
            print!("{}", state5::size_report(&bytes).map_err(|e| e.to_string())?);
            #[allow(clippy::cast_precision_loss)] // a ratio for people
            let ratio = bytes.len() as f64 / four as f64;
            println!("format 4 {four} bytes, format 5 {} bytes, ratio {ratio:.3} (gate 1.15)", bytes.len());
            Ok(ExitCode::SUCCESS)
        }
        ["strings", path4, version, suffixes @ ..] => {
            strings_growth(path4, version, suffixes)?;
            Ok(ExitCode::SUCCESS)
        }
        _ => Err("usage: incr_state convert <4> <out> <version> | canon <s> | compare <a> <b> | size <s> | size <4> <version>"
            .to_owned()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("incr_state: {e}");
            ExitCode::from(2)
        }
    }
}
