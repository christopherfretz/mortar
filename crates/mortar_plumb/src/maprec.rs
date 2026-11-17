//! The allocation records of mortar's map (`spec/plumb.md` §3.2, §3.3
//! M6-M8; WP-096): facts of the layout the linker states and the
//! verifier checks against the output instead of inferring them.
//!
//! - **M6**: a `synthetic` chunk in an allocated `NOBITS` section (the
//!   linker-made `.pad.relro`) is that whole section;
//! - **M7**: a `common` record is a common symbol of the inputs, at the
//!   output's `.symtab` value and size, aligned as its definitions ask;
//! - **M8**: a `copy` record is a `COPY` record of the output, with the
//!   size of its symbol and the alignment of the copied definition.
//!
//! These run for every map of mortar's format (version 2), whatever it
//! claims, and each record is required where its fact exists: a
//! linker-made `NOBITS` section no line names, a common symbol of the
//! inputs with no `common` record and a `COPY` record with no `copy`
//! record are findings of the same rules.

use std::collections::{BTreeMap, BTreeSet};

use mortar_elf::SymbolSection;
use mortar_elf::consts::{
    SHF_TLS, SHN_COMMON, SHN_LORESERVE, SHN_UNDEF, SHN_X86_64_LCOMMON, SHT_NOBITS, STB_GLOBAL,
    STB_LOCAL, STB_WEAK, STT_FILE, STT_SECTION, STV_HIDDEN, STV_INTERNAL,
};

use crate::image::Image;
use crate::locate::{Obj, Place};
use crate::map::{ChunkKind, Map};
use crate::report::{Finding, Rule};

fn finding(n: u8, msg: String) -> Finding {
    Finding::new(Rule::Map(n), msg)
}

/// The names the inputs give only common definitions (`SHN_COMMON`, or
/// x86-64 `SHN_X86_64_LCOMMON`, global or weak; no definition in a
/// section the map places), each with the largest alignment its common
/// definitions ask (their `st_value`, gABI "Symbol Values").
pub(crate) fn common_only(objs: &[Obj<'_>]) -> BTreeMap<String, u64> {
    let mut common: BTreeMap<String, u64> = BTreeMap::new();
    let mut defined: BTreeSet<String> = BTreeSet::new();
    for o in objs {
        let Some(t) = &o.symtab else { continue };
        for (i, sym) in t.iter() {
            if !matches!(sym.binding(), STB_GLOBAL | STB_WEAK) {
                continue;
            }
            let Ok(name) = t.name(&sym) else { continue };
            let name = String::from_utf8_lossy(name).into_owned();
            if sym.st_shndx == SHN_COMMON || sym.st_shndx == SHN_X86_64_LCOMMON {
                let a = common.entry(name).or_insert(1);
                *a = (*a).max(sym.st_value.max(1));
            } else if let Ok(SymbolSection::Section(k)) = t.section_of(i, &sym)
                && o.secs
                    .get(k.0 as usize)
                    .is_some_and(|s| matches!(s.place, Place::Whole(_) | Place::Pieces(_)))
            {
                defined.insert(name);
            }
        }
    }
    common.retain(|n, _| !defined.contains(n));
    common
}

/// The names an input entry gives hidden or internal visibility (gABI
/// "Symbol Visibility": the output symbol is `STB_LOCAL`; S-121).
fn hidden_names(objs: &[Obj<'_>]) -> BTreeSet<String> {
    let mut h = BTreeSet::new();
    for o in objs {
        let Some(t) = &o.symtab else { continue };
        for (_, sym) in t.iter() {
            if matches!(sym.binding(), STB_GLOBAL | STB_WEAK)
                && matches!(sym.visibility(), STV_HIDDEN | STV_INTERNAL)
                && let Ok(n) = t.name(&sym)
            {
                h.insert(String::from_utf8_lossy(n).into_owned());
            }
        }
    }
    h
}

/// M6 and M7 over a map of mortar's format.
/// `made_local`: the keys the line makes local by a version script or
/// `--exclude-libs` (stage 3's; WP-169).
pub(crate) fn check(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    made_local: &BTreeSet<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    nobits_chunks(image, map, &mut out);
    commons(image, objs, map, made_local, &mut out);
    out
}

/// M6: a synthetic chunk in an allocated, not thread-local `NOBITS`
/// section is that section; such a section of size above 0 that no
/// record of the map lies in (no input section, piece, common, copy or
/// chunk) is a linker-made one, and has its chunk.
fn nobits_chunks(image: &Image<'_>, map: &Map, out: &mut Vec<Finding>) {
    let starts: Vec<u64> = map
        .placed
        .iter()
        .map(|p| p.addr)
        .chain(map.commons.iter().map(|c| c.addr))
        .chain(map.dynamic.iter().flat_map(|d| &d.copies).map(|c| c.addr))
        .chain(map.chunks.iter().filter_map(|c| c.addr))
        .collect();
    for s in image.sections.iter().skip(1) {
        let h = &s.shdr;
        if !s.alloc() || h.sh_type != SHT_NOBITS || h.sh_flags & SHF_TLS != 0 || h.sh_size == 0 {
            continue;
        }
        let end = h.sh_addr.saturating_add(h.sh_size);
        if !starts.iter().any(|&a| h.sh_addr <= a && a < end) {
            out.push(
                finding(
                    6,
                    format!(
                        "the NOBITS section '{}' [{:#x}, +{:#x}) holds no record of the map: a section the linker made is stated as a chunk",
                        s.name, h.sh_addr, h.sh_size
                    ),
                )
                .at(h.sh_addr),
            );
        }
    }
    for c in &map.chunks {
        let (ChunkKind::Synthetic, Some(a)) = (c.kind, c.addr) else { continue };
        let Some(s) = image.sections.iter().skip(1).find(|s| {
            let h = &s.shdr;
            s.alloc()
                && h.sh_type == SHT_NOBITS
                && h.sh_flags & SHF_TLS == 0
                && h.sh_size > 0
                && h.sh_addr <= a
                && a < h.sh_addr.saturating_add(h.sh_size)
        }) else {
            continue;
        };
        let h = &s.shdr;
        if c.name != s.name || a != h.sh_addr || c.size != h.sh_size {
            out.push(
                finding(
                    6,
                    format!(
                        "map line {}: the chunk '{}' [{a:#x}, +{:#x}) lies in the NOBITS section '{}' [{:#x}, +{:#x}) and is not that section",
                        c.line, c.name, c.size, s.name, h.sh_addr, h.sh_size
                    ),
                )
                .at(a),
            );
        }
    }
}

/// M7: each `common` record is a common symbol of the inputs, placed
/// where the output's `.symtab` says, aligned as its definitions ask.
fn commons(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    made_local: &BTreeSet<String>,
    out: &mut Vec<Finding>,
) {
    let only = common_only(objs);
    // S-121: the keys whose output binding is `STB_LOCAL`, the common's
    // `.symtab` symbol then a local of the name (synth S-77; all three
    // references write `OBJECT LOCAL DEFAULT` for a script's: WP-150).
    let hidden = hidden_names(objs);
    let local_of = |name: &str, addr: u64| {
        let made = made_local.contains(name) || hidden.contains(name);
        let mut l = image.symbols.iter().filter(|s| {
            made && s.name == name
                && s.sym.binding() == STB_LOCAL
                && s.sym.st_shndx != SHN_UNDEF
                && !matches!(s.sym.sym_type(), STT_SECTION | STT_FILE)
        });
        let first = l.clone().next();
        // Two locals of the name (another file's static one): the one at
        // the record's address.
        if l.clone().nth(1).is_some() { l.find(|s| s.sym.st_value == addr) } else { first }
    };
    for (name, want) in &only {
        if !map.commons.iter().any(|r| &r.name == name) {
            let at = image.global(name).map_or(0, |s| s.sym.st_value);
            out.push(
                finding(
                    7,
                    format!(
                        "the common symbol '{name}' (asked aligned to {want:#x}) has no common record"
                    ),
                )
                .at(at),
            );
        }
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for r in &map.commons {
        let say = |msg: String| {
            finding(7, format!("map line {}: the common '{}' {msg}", r.line, r.name)).at(r.addr)
        };
        if !seen.insert(r.name.as_str()) {
            out.push(say("has a second record".into()));
            continue;
        }
        let Some(&want) = only.get(&r.name) else {
            out.push(say("is not a name the inputs give only common definitions".into()));
            continue;
        };
        if r.align != want {
            let mut f = say(format!(
                "is stated aligned to {:#x}; its common definitions ask {want:#x}",
                r.align
            ));
            f.expected = Some(want);
            f.found = Some(r.align);
            out.push(f);
        }
        if !r.addr.is_multiple_of(r.align) {
            out.push(say(format!(
                "at {:#x} is not a multiple of its alignment {:#x}",
                r.addr, r.align
            )));
        }
        // A stripped output: the address and size are not judged.
        if image.symbols.is_empty() {
            continue;
        }
        let Some(sym) = image.global(&r.name).or_else(|| local_of(&r.name, r.addr)).map(|s| s.sym)
        else {
            out.push(say(if made_local.contains(&r.name) || hidden.contains(&r.name) {
                "is a key the line makes local, and no symbol of the output's .symtab defines it"
                    .into()
            } else {
                "is not a global symbol of the output's .symtab".into()
            }));
            continue;
        };
        if sym.st_shndx == SHN_UNDEF || sym.st_shndx >= SHN_LORESERVE {
            out.push(say(format!(
                "is not defined in a section of the output (st_shndx {:#x})",
                sym.st_shndx
            )));
            continue;
        }
        for (what, stated, found) in
            [("address", r.addr, sym.st_value), ("size", r.size, sym.st_size)]
        {
            if stated != found {
                let mut f = say(format!(
                    "is stated at {what} {stated:#x}; the output's symbol has {found:#x}"
                ));
                f.expected = Some(found);
                f.found = Some(stated);
                out.push(f);
            }
        }
        let held = image.sections.get(usize::from(sym.st_shndx)).is_some_and(|s| {
            let h = &s.shdr;
            s.alloc()
                && h.sh_type == SHT_NOBITS
                && h.sh_addr <= r.addr
                && r.addr
                    .checked_add(r.size)
                    .is_some_and(|e| e <= h.sh_addr.saturating_add(h.sh_size))
        });
        if !held {
            out.push(say("is not inside the allocated NOBITS section its symbol names".into()));
        }
    }
}

/// A `COPY` record of the output (P-D11): its place, its symbol's name
/// and `.dynsym` size, and the copied definition's alignment when its
/// definition was found.
#[derive(Debug, Clone)]
pub(crate) struct ActualCopy {
    pub addr: u64,
    pub name: String,
    pub size: u64,
    pub align: Option<u64>,
}

/// M8: each `copy` record is a `COPY` record of the output, and each
/// `COPY` record has one. `actual` is empty for an output with no
/// dynamic section.
pub(crate) fn copies(map: &Map, actual: &[ActualCopy]) -> Vec<Finding> {
    let mut out = Vec::new();
    for a in actual {
        if !map.dynamic.iter().flat_map(|d| &d.copies).any(|c| c.addr == a.addr) {
            out.push(
                finding(
                    8,
                    format!("the COPY record of '{}' at {:#x} has no copy record", a.name, a.addr),
                )
                .at(a.addr),
            );
        }
    }
    let stated = map.dynamic.iter().flat_map(|d| &d.copies);
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    for c in stated {
        let say = |msg: String| {
            finding(8, format!("the copy record of '{}' at {:#x} {msg}", c.name, c.addr)).at(c.addr)
        };
        if !seen.insert(c.addr) {
            out.push(say("is the second record of that address".into()));
            continue;
        }
        let Some(a) = actual.iter().find(|a| a.addr == c.addr) else {
            out.push(say("names no COPY record of the output".into()));
            continue;
        };
        if a.name != c.name {
            out.push(say(format!("names '{}'; the COPY record there is '{}'s", c.name, a.name)));
        }
        if a.size != c.size {
            let mut f = say(format!(
                "is stated of size {:#x}; the COPY record's symbol has {:#x}",
                c.size, a.size
            ));
            f.expected = Some(a.size);
            f.found = Some(c.size);
            out.push(f);
        }
        let stated = c.align;
        if let Some(want) = a.align
            && stated != want
        {
            let mut f = say(format!(
                "is stated aligned to {stated:#x}; the copied definition's alignment is {want:#x} (P-D11, D-42)"
            ));
            f.expected = Some(want);
            f.found = Some(stated);
            out.push(f);
        }
        if !c.addr.is_multiple_of(stated) {
            out.push(say(format!("is not a multiple of its alignment {stated:#x}")));
        }
    }
    out
}

// ---- The shared-object line records (§3.2; WP-121) ----

/// Reads one of the shared-object line records (`symbolic`,
/// `dynamic-list`, `version-script`; §3.2) into `d`: a second `symbolic`,
/// an unknown form, a wrong number of fields or a path that is not a
/// string token is a [`MapError`] at line `n`.
pub(crate) fn read_shared(
    d: &mut crate::map::DynRecords,
    t: &[&str],
    n: u32,
) -> Result<(), crate::map::MapError> {
    use crate::map::{MapError, Symbolic};
    let bad = |what: &str| MapError::new(n, format!("malformed '{}' record: {what}", t[0]));
    if t.len() != 2 {
        return Err(bad("wrong number of fields"));
    }
    let sh = d.shared.get_or_insert_with(Default::default);
    match t[0] {
        "symbolic" => {
            let form = Symbolic::from_name(t[1]).ok_or_else(|| bad("form"))?;
            if sh.symbolic.replace(form).is_some() {
                return Err(bad("a second 'symbolic' record"));
            }
        }
        "dynamic-list" => {
            sh.dynamic_lists.push(crate::map::mortar::decode(t[1]).ok_or_else(|| bad("path"))?);
        }
        _ => {
            sh.version_scripts.push(crate::map::mortar::decode(t[1]).ok_or_else(|| bad("path"))?);
        }
    }
    Ok(())
}

/// The map-wide rules of the shared-object line records (§3.2): exactly
/// one `symbolic` record in a map whose `output` is `shared` (reported at
/// its `output` line), and none of the three in any other map (reported
/// at the first of them).
pub(crate) fn end_shared(
    d: Option<&crate::map::DynRecords>,
    output_line: u32,
    shared_line: Option<u32>,
) -> Result<(), crate::map::MapError> {
    use crate::map::{MapError, OutputKind};
    let shared = d.is_some_and(|d| d.kind == Some(OutputKind::Shared));
    let has = d.and_then(|d| d.shared.as_ref());
    match (shared, has) {
        (true, Some(s)) if s.symbolic.is_some() => Ok(()),
        (true, _) => Err(MapError::new(
            output_line,
            "an 'output shared' map without its 'symbolic' record (§3.2)",
        )),
        (false, None) => Ok(()),
        (false, Some(_)) => Err(MapError::new(
            shared_line.unwrap_or(0),
            "a shared-object line record in a map whose output is not 'shared' (§3.2)",
        )),
    }
}

/// Writes the shared-object line records in the grammar's order.
pub(crate) fn write_shared(sh: &crate::map::SharedRecords, out: &mut String) {
    if let Some(s) = sh.symbolic {
        out.push_str("symbolic ");
        out.push_str(s.name());
        out.push('\n');
    }
    for (record, paths) in
        [("dynamic-list", &sh.dynamic_lists), ("version-script", &sh.version_scripts)]
    {
        for p in paths {
            out.push_str(record);
            out.push(' ');
            crate::map::mortar::encode(p, out);
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::map::mortar::{parse, write};
    use crate::map::{OutputKind, SharedRecords, Symbolic};

    const HEAD: &str = "mortar-map 2\n";
    const TAIL: &str = "arg -shared\nend\n";

    // §3.2: the shared-object line records round-trip in a `Shared` map,
    // after `relro` and before `arg`, in line order.
    #[test]
    fn s3_2_shared_line_records_round_trip() {
        for form in Symbolic::ALL {
            let text = format!(
                "{HEAD}output shared\nrelro on\nsymbolic {}\ndynamic-list a%20b.list\n\
                 dynamic-list c.list\nversion-script v.map\n{TAIL}",
                form.name()
            );
            let m = parse(&text).unwrap();
            let d = m.dynamic.as_ref().unwrap();
            assert_eq!(d.kind, Some(OutputKind::Shared));
            assert_eq!(
                d.shared,
                Some(SharedRecords {
                    symbolic: Some(form),
                    dynamic_lists: vec!["a b.list".into(), "c.list".into()],
                    version_scripts: vec!["v.map".into()],
                })
            );
            let back = write(&m);
            assert!(
                back.contains(&format!(
                    "relro on\nsymbolic {}\ndynamic-list a%20b.list\ndynamic-list c.list\n\
                     version-script v.map\narg -shared\n",
                    form.name()
                )),
                "{back}"
            );
            assert_eq!(parse(&back).unwrap(), m);
        }
        // A map of another kind writes none of them.
        let m = parse(&format!("{HEAD}output pie\n{TAIL}")).unwrap();
        assert_eq!(m.dynamic.as_ref().unwrap().shared, None);
        assert!(!write(&m).contains("symbolic"));
    }

    // §3.2: "A second `symbolic` record, an unknown form, a wrong number
    // of fields, or any of the three in a map whose `output` is not
    // `shared` is a `MapError`"; and a `Shared` map has exactly one
    // `symbolic` record.
    #[test]
    fn s3_2_malformed_shared_line_records_are_errors() {
        let cases: &[(&str, u32)] = &[
            ("output shared\nsymbolic all\nsymbolic none\n", 4),
            ("output shared\nsymbolic some\n", 3),
            ("output shared\nsymbolic\n", 3),
            ("output shared\nsymbolic all extra\n", 3),
            ("output shared\nsymbolic all\ndynamic-list\n", 4),
            ("output shared\nsymbolic all\nversion-script a b\n", 4),
            ("output shared\nsymbolic all\nversion-script %4\n", 4),
            ("output shared\ndynamic-list a\n", 2),
            ("output pie\nsymbolic none\n", 3),
            ("output exec\ndynamic-list a\n", 3),
            ("output static-pie\nversion-script a\n", 3),
            ("version-script a\noutput shared\nsymbolic none\n", 0),
            ("symbolic none\n", 2),
        ];
        for (body, line) in cases {
            let text = format!("{HEAD}{body}{TAIL}");
            match parse(&text) {
                Ok(_) if *line == 0 => {}
                Ok(m) => panic!("{text:?} read as {m:?}"),
                Err(e) => assert_eq!(e.line, *line, "{text:?}: {e}"),
            }
        }
    }
}
