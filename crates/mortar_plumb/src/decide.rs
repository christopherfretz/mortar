//! The decision records of an incremental-mode map judged against the
//! output (`spec/plumb.md` §6.8 P-I8, WP-210; `spec/image.md` I-78a):
//! `capacity`, `order`, `slot`, `redirect` and `frag`. `region` and
//! `symblock` are [`crate::symfill`]'s.
//!
//! - **capacity**: each record names an output section; `size` at most
//!   `capacity`; an allocated chunk inside its section, a non-allocated
//!   one without an address; in `order`'s order the chunks of a section
//!   ascend without overlapping, from the section's first byte to its
//!   last; a chunk's reserve (`capacity − size` after it) holds no other
//!   chunk and no other allocated section's bytes; the non-allocated
//!   chunks' sizes add up to at most the section's; every live item the
//!   map places in an allocated section lies in one of its chunks.
//! - **order**: one per output section that has chunks, a permutation of
//!   their ordinals.
//! - **slot**: in a section of its kind with room for the slot; one
//!   record per kind and key and one key per GOT slot; every GOT slot a
//!   live reference is verified through (§5.5) is its key's record; an
//!   `iplt` record is an IPLT entry of its key's resolver (§4.8), a `plt`
//!   record a PLT entry whose slot's `JUMP_SLOT` record names its key.
//! - **redirect**: its address is a `chunk thunk` or `chunk patch`
//!   record's; the site is placed and its branch reaches that address;
//!   one per site.
//! - **frag**: `L128` (image.md I-114) of the fragment's bytes at its
//!   offset (a string with its NUL character, or `entsize` bytes) is its
//!   hash; it lies in a merged chunk of its section, at its alignment, in
//!   no dead range.
//!
//! What only the linker's state could tell (named in the coverage line):
//! the ordinals' creation order, the capacities' amounts, a slot no live
//! reference uses (beyond its place and its word's key), the identity
//! order of `frag` records.

// c, d, e, f, k, o, s, w, y: a capacity, the decisions, the engine, a
// frag, a key, an order, a slot, a word, a symbol, throughout.
#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use mortar_elf::SymbolIndex;
use mortar_elf::consts::{
    EM_AARCH64, EM_X86_64, SHF_ALLOC, SHF_EXECINSTR, SHF_STRINGS, SHF_TLS, SHT_NOBITS, SHT_RELA,
    STB_LOCAL, STT_GNU_IFUNC, STT_TLS,
};

use crate::a64::Slot as GotKind;
use crate::buildid::l128;
use crate::dead::DeadSet;
use crate::image::Image;
use crate::map::{Capacity, ChunkKind, Map, Part, SectionRef, SlotKey, SlotKind};
use crate::reloc::Engine;
use crate::report::{Finding, Rule};

fn finding(msg: String) -> Finding {
    Finding::new(Rule::Incr(8), msg)
}

/// The judgement of §6.8 P-I8; the part of the coverage line it adds.
pub(crate) fn check(
    e: &Engine<'_, '_>,
    map: &Map,
    set: &DeadSet,
    out: &mut Vec<Finding>,
) -> String {
    let image = e.image;
    let d = &map.decisions;
    capacities(e, map, out);
    let (used, unused) = slots(e, map, out);
    redirects(image, map, out);
    frags(image, map, set, out);
    format!(
        "P-I8: capacity {}, order {}, slot {} ({used} by a live reference, their word or their entry, {unused} by place and key only), redirect {}, frag {}",
        d.capacities.len(),
        d.orders.len(),
        d.slots.len(),
        d.redirects.len(),
        d.frags.len()
    )
}

/// The one output section named `name`.
fn section(image: &Image<'_>, name: &str) -> Option<usize> {
    let mut it = image.sections.iter().enumerate().filter(|(_, s)| s.name == name);
    let first = it.next()?.0;
    it.next().is_none().then_some(first)
}

fn capacities(e: &Engine<'_, '_>, map: &Map, out: &mut Vec<Finding>) {
    let image = e.image;
    let d = &map.decisions;
    let mut by_sec: BTreeMap<usize, Vec<&Capacity>> = BTreeMap::new();
    for c in &d.capacities {
        let at = |m: String| {
            finding(format!("map line {}: capacity {} {}: {m}", c.line, c.out, c.ordinal))
        };
        let Some(i) = section(image, &c.out) else {
            out.push(at("names no single output section".into()));
            continue;
        };
        let sh = &image.sections[i].shdr;
        if c.size > c.capacity {
            out.push(at(format!("size {:#x} above its capacity {:#x}", c.size, c.capacity)));
        }
        let alloc = sh.sh_flags & SHF_ALLOC != 0;
        match (alloc, c.addr) {
            (true, None) => out.push(at("an allocated section's chunk without an address".into())),
            (false, Some(_)) => {
                out.push(at("a non-allocated section's chunk with an address".into()));
            }
            (true, Some(a)) => {
                let (lo, hi) = (sh.sh_addr, sh.sh_addr + sh.sh_size);
                if a < lo || a.checked_add(c.size).is_none_or(|z| z > hi) {
                    out.push(at(format!(
                        "chunk [{a:#x}, +{:#x}) outside the section [{lo:#x}, {hi:#x})",
                        c.size
                    )));
                }
            }
            (false, None) => {}
        }
        by_sec.entry(i).or_default().push(c);
    }
    // `order`: one per section with chunks, a permutation of their ordinals.
    let mut orders: BTreeMap<usize, &crate::map::Order> = BTreeMap::new();
    for o in &d.orders {
        let Some(i) = section(image, &o.out) else {
            out.push(finding(format!(
                "map line {}: order {}: names no single output section",
                o.line, o.out
            )));
            continue;
        };
        if orders.insert(i, o).is_some() {
            out.push(finding(format!("map line {}: a second order record of {}", o.line, o.out)));
        }
    }
    for (&i, cs) in &by_sec {
        let name = &image.sections[i].name;
        let mut ords: Vec<u32> = cs.iter().map(|c| c.ordinal).collect();
        ords.sort_unstable();
        if ords.windows(2).any(|w| w[0] == w[1]) {
            out.push(finding(format!("capacity: two chunks of {name} have one ordinal")));
            continue;
        }
        let Some(o) = orders.get(&i) else {
            out.push(finding(format!("capacity: {name} has chunks and no order record")));
            continue;
        };
        let mut given = o.ordinals.clone();
        given.sort_unstable();
        if given != ords {
            out.push(finding(format!(
                "map line {}: order {name} {:?} is not a permutation of its chunks' ordinals {ords:?}",
                o.line, o.ordinals
            )));
            continue;
        }
        let seq: Vec<&Capacity> = o
            .ordinals
            .iter()
            .filter_map(|n| cs.iter().copied().find(|c| c.ordinal == *n))
            .collect();
        let sh = &image.sections[i].shdr;
        if sh.sh_flags & SHF_ALLOC != 0 {
            placed_in_order(image, i, &seq, o.line, out);
        } else {
            let total: u64 = seq.iter().map(|c| c.size).sum();
            if total > sh.sh_size || (seq.len() == 1 && total != sh.sh_size) {
                out.push(finding(format!(
                    "capacity: the chunks of {name} hold {total:#x} bytes, the section {:#x}",
                    sh.sh_size
                )));
            }
        }
    }
    for o in &d.orders {
        if let Some(i) = section(image, &o.out)
            && !by_sec.contains_key(&i)
        {
            out.push(finding(format!(
                "map line {}: order {}: the section has no chunk",
                o.line, o.out
            )));
        }
    }
    live_in_chunks(e, map, &by_sec, out);
}

/// An allocated section's chunks in file order: ascending, not
/// overlapping, from the section's first byte to its last; each reserve
/// free of other chunks and other sections.
fn placed_in_order(
    image: &Image<'_>,
    i: usize,
    seq: &[&Capacity],
    line: u32,
    out: &mut Vec<Finding>,
) {
    let s = &image.sections[i];
    let (lo, hi) = (s.shdr.sh_addr, s.shdr.sh_addr + s.shdr.sh_size);
    let addrs: Vec<(u64, &Capacity)> = seq.iter().filter_map(|c| Some((c.addr?, *c))).collect();
    if addrs.len() != seq.len() || addrs.is_empty() {
        return;
    }
    if addrs[0].0 != lo {
        out.push(finding(format!(
            "map line {line}: the first chunk of {} in order is at {:#x}, the section at {lo:#x}",
            s.name, addrs[0].0
        )));
    }
    let (la, lc) = addrs[addrs.len() - 1];
    if la + lc.size != hi {
        out.push(finding(format!(
            "map line {line}: the last chunk of {} in order ends at {:#x}, the section at {hi:#x}",
            s.name,
            la + lc.size
        )));
    }
    for w in addrs.windows(2) {
        if w[0].0 + w[0].1.size > w[1].0 {
            out.push(finding(format!(
                "map line {}: chunk {} of {} [{:#x}, +{:#x}) is not before chunk {} at {:#x} (order line {line})",
                w[0].1.line, w[0].1.ordinal, s.name, w[0].0, w[0].1.size, w[1].1.ordinal, w[1].0
            )));
        }
    }
    // A TLS `NOBITS` section's addresses are offsets laid over what
    // follows it: its reserve is not judged against the others.
    let tbss = |k: usize| {
        let x = &image.sections[k].shdr;
        x.sh_type == SHT_NOBITS && x.sh_flags & SHF_TLS != 0
    };
    if tbss(i) {
        return;
    }
    for &(a, c) in &addrs {
        let (r0, r1) = (a + c.size, a + c.capacity);
        if r1 <= r0 {
            continue;
        }
        if let Some(&(b, d)) =
            addrs.iter().find(|(b, d)| d.ordinal != c.ordinal && *b < r1 && r0 < b + d.size)
        {
            out.push(finding(format!(
                "map line {}: the reserve [{r0:#x}, {r1:#x}) of chunk {} of {} holds chunk {} at {b:#x}",
                c.line, c.ordinal, s.name, d.ordinal
            )));
        }
        let other = image.sections.iter().enumerate().find(|(k, x)| {
            *k != i
                && !tbss(*k)
                && x.shdr.sh_flags & SHF_ALLOC != 0
                && x.shdr.sh_size > 0
                && x.shdr.sh_addr < r1
                && r0 < x.shdr.sh_addr + x.shdr.sh_size
        });
        if let Some((_, x)) = other {
            out.push(finding(format!(
                "map line {}: the reserve [{r0:#x}, {r1:#x}) of chunk {} of {} holds bytes of {}",
                c.line, c.ordinal, s.name, x.name
            )));
        }
    }
}

/// Every live item the map places in an allocated section with chunks
/// lies in one of them (I-78a: a chunk's `size` is what it holds). A TLS
/// `NOBITS` section's addresses are offsets laid over what follows it
/// (WP-210: `.tbss` at `.init_array`'s address): an input section of
/// `SHF_TLS` and `SHT_NOBITS` is judged against the TLS `NOBITS`
/// sections' chunks, every other item against the other sections'; an
/// item in a TLS `NOBITS` section's range whose input is not known (a
/// path the inputs hold twice) is not judged.
fn live_in_chunks(
    e: &Engine<'_, '_>,
    map: &Map,
    by_sec: &BTreeMap<usize, Vec<&Capacity>>,
    out: &mut Vec<Finding>,
) {
    let image = e.image;
    let tbss = |sh: &mortar_elf::Shdr| sh.sh_type == SHT_NOBITS && sh.sh_flags & SHF_TLS != 0;
    let spans_of = |tls: bool| {
        let mut v: Vec<(u64, u64)> = by_sec
            .iter()
            .filter(|(i, _)| {
                let sh = &image.sections[**i].shdr;
                sh.sh_flags & SHF_ALLOC != 0 && tbss(sh) == tls
            })
            .flat_map(|(_, cs)| cs.iter().filter_map(|c| Some((c.addr?, c.addr? + c.size))))
            .collect();
        v.sort_unstable();
        v
    };
    let (spans, tspans) = (spans_of(false), spans_of(true));
    if spans.is_empty() && tspans.is_empty() {
        return;
    }
    let inside = |s: &[(u64, u64)], a: u64, z: u64| {
        let k = s.partition_point(|x| x.0 <= a);
        k > 0 && s[k - 1].1 >= z
    };
    let judged = |a: u64| {
        image.alloc_section_of(a, 0).is_some_and(|(k, s)| by_sec.contains_key(&k) && !tbss(&s.shdr))
    };
    // The TLS `NOBITS` sections with chunks: `[start, end)`.
    let tranges: Vec<(u64, u64)> = by_sec
        .keys()
        .map(|&i| &image.sections[i].shdr)
        .filter(|sh| sh.sh_flags & SHF_ALLOC != 0 && tbss(sh))
        .map(|sh| (sh.sh_addr, sh.sh_addr + sh.sh_size))
        .collect();
    let in_tls = |a: u64| tranges.iter().any(|&(lo, hi)| lo <= a && a < hi);
    // Whether the placed row's input section is TLS `NOBITS`: `None`
    // when its input is not one object of the inputs.
    let tls_input = |p: &crate::map::Placed| -> Option<bool> {
        let f = map.files.get(p.file as usize)?;
        let mut it = e.objs.iter().filter(|o| o.path == f.path && o.member == f.member);
        let o = it.next()?;
        if it.next().is_some() {
            return None;
        }
        let SectionRef::Index(k) = p.section else { return None };
        Some(tbss(&o.secs.get(k as usize)?.shdr))
    };
    let mut shown = 0;
    let mut miss = |a: u64, z: u64, line: u32, out: &mut Vec<Finding>| {
        shown += 1;
        if shown <= 8 {
            out.push(finding(format!(
                "map line {line}: [{a:#x}, +{z:#x}) is in no chunk's size of its section (capacity records)"
            )));
        }
    };
    for p in map.placed.iter().filter(|p| p.size > 0) {
        let (a, z) = (p.addr, p.size);
        match tls_input(p) {
            Some(true) => {
                if in_tls(a) && !inside(&tspans, a, a + z) {
                    miss(a, z, p.line, out);
                }
            }
            Some(false) => {
                if judged(a) && !inside(&spans, a, a + z) {
                    miss(a, z, p.line, out);
                }
            }
            None => {
                if !in_tls(a) && judged(a) && !inside(&spans, a, a + z) {
                    miss(a, z, p.line, out);
                }
            }
        }
    }
    for c in map.chunks.iter().filter(|c| c.size > 0) {
        let Some(a) = c.addr else { continue };
        if judged(a) && !inside(&spans, a, a + c.size) {
            miss(a, c.size, c.line, out);
        }
    }
    if shown > 8 {
        out.push(finding(format!("capacity: {} more items in no chunk", shown - 8)));
    }
}

/// The record key of the symbol `gsym` of object `oi`.
fn key_of(
    e: &Engine<'_, '_>,
    files: &BTreeMap<(&str, Option<&str>), u32>,
    oi: usize,
    gsym: u32,
) -> Option<SlotKey> {
    let o = e.objs.get(oi)?;
    let t = o.symtab.as_ref()?;
    let s = t.get(SymbolIndex(gsym)).ok()?;
    if s.binding() == STB_LOCAL {
        let file = *files.get(&(o.path.as_str(), o.member.as_deref()))?;
        return Some(SlotKey::Local { file, index: gsym });
    }
    Some(SlotKey::Global(String::from_utf8_lossy(t.name(&s).ok()?).into_owned()))
}

fn kind_of(k: GotKind) -> SlotKind {
    match k {
        GotKind::Addr => SlotKind::Got,
        GotKind::TpRel => SlotKind::GotTprel,
        GotKind::Desc => SlotKind::TlsDesc,
        GotKind::Gd => SlotKind::TlsGd,
        GotKind::Ld => SlotKind::TlsLd,
    }
}

fn key_text(k: &SlotKey) -> String {
    match k {
        SlotKey::Global(n) => format!("'{n}'"),
        SlotKey::Local { file, index } => format!("local {index} of file {file}"),
        SlotKey::Module => "the module".into(),
    }
}

/// A record's key without its version (`name@version`): the references'
/// keys are input symbols, which carry none.
fn bare(k: &SlotKey) -> SlotKey {
    match k {
        SlotKey::Global(n) => SlotKey::Global(n.split('@').next().unwrap_or(n).to_string()),
        k => k.clone(),
    }
}

/// The slot records judged; `(records a live reference was verified
/// through, the others)`.
#[allow(clippy::too_many_lines)] // P-I8's sentences 4 to 7, one after the other
fn slots(e: &Engine<'_, '_>, map: &Map, out: &mut Vec<Finding>) -> (usize, usize) {
    let image = e.image;
    let d = &map.decisions;
    if d.slots.is_empty() {
        return (0, 0);
    }
    let word = if image.class() == mortar_elf::Class::Elf32 { 4 } else { 8 };
    let mut keys: BTreeMap<(SlotKind, String), (u64, u32)> = BTreeMap::new();
    let mut at: BTreeMap<u64, (u32, SlotKind)> = BTreeMap::new();
    let plts = crate::loader::plt_entries(image);
    let jump = jump_slots(image);
    let mut entries = 0usize;
    for s in &d.slots {
        let tag = format!("map line {}: slot {} {}", s.line, s.kind.name(), key_text(&s.key));
        let got = !matches!(s.kind, SlotKind::Plt | SlotKind::Iplt);
        let room = match s.kind {
            SlotKind::TlsGd | SlotKind::TlsDesc | SlotKind::TlsLd => 2 * word,
            _ => word,
        };
        let sec = image.alloc_section_of(s.addr, if got { room } else { 1 }).map(|(_, x)| x);
        let fits = sec.is_some_and(|x| {
            if got {
                x.name.starts_with(".got") && x.shdr.sh_type != SHT_NOBITS
            } else {
                x.shdr.sh_flags & SHF_EXECINSTR != 0
            }
        });
        if !fits {
            out.push(finding(format!(
                "{tag}: {:#x} is not {} ({})",
                s.addr,
                if got {
                    format!("{room} bytes of a GOT section")
                } else {
                    "in executable code".into()
                },
                sec.map_or("no section".into(), |x| format!("in {}", x.name))
            )));
        }
        let k = (s.kind, format!("{:?}", bare(&s.key)));
        if let Some((_, l)) = keys.insert(k, (s.addr, s.line)) {
            out.push(finding(format!("{tag}: a second record of the kind and key (line {l})")));
        }
        if got && let Some((l, kind)) = at.insert(s.addr, (s.line, s.kind)) {
            out.push(finding(format!(
                "{tag}: the slot at {:#x} is also line {l}'s ({}): one key per slot",
                s.addr,
                kind.name()
            )));
        }
        match (&s.kind, &s.key) {
            (SlotKind::Iplt, SlotKey::Global(n)) => {
                entries += 1;
                let n = n.split('@').next().unwrap_or(n);
                // A hidden global is a local of the output (demoted).
                let y = image.global(n).or_else(|| image.unique(n));
                match y.filter(|y| y.sym.sym_type() == STT_GNU_IFUNC) {
                    Some(y) if e.iplt.entries_of(y.sym.st_value).contains(&s.addr) => {}
                    Some(y) => out.push(finding(format!(
                        "{tag}: {:#x} is not an IPLT entry of the resolver {:#x} (§4.8)",
                        s.addr, y.sym.st_value
                    ))),
                    None => {
                        out.push(finding(format!(
                            "{tag}: the output's .symtab has no IFUNC '{n}'"
                        )));
                    }
                }
            }
            (SlotKind::Plt, SlotKey::Global(n)) => {
                entries += 1;
                let n = n.split('@').next().unwrap_or(n);
                let named = plts.get(&s.addr).and_then(|slot| jump.get(slot));
                if named.map(String::as_str) != Some(n) {
                    out.push(finding(format!(
                        "{tag}: {:#x} is not a PLT entry whose slot's JUMP_SLOT names '{n}' ({})",
                        s.addr,
                        named.map_or("none".into(), |m| format!("it names '{m}'"))
                    )));
                }
            }
            _ => {}
        }
    }
    // Every GOT slot a live reference is verified through is its key's
    // record (§5.5).
    let Some(used) = e.slot_use.as_ref() else { return (entries, d.slots.len() - entries) };
    let mut through: BTreeSet<(SlotKind, String)> = BTreeSet::new();
    let mut files: BTreeMap<(&str, Option<&str>), u32> = BTreeMap::new();
    for (i, f) in map.files.iter().enumerate() {
        files.entry((f.path.as_str(), f.member.as_deref())).or_insert(i as u32);
    }
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for (kind, oi, gsym, addend, valid) in used {
        if *addend != 0 {
            continue;
        }
        let kind = kind_of(*kind);
        let key = if kind == SlotKind::TlsLd {
            Some(SlotKey::Module)
        } else {
            key_of(e, &files, *oi, *gsym)
        };
        let Some(key) = key else { continue };
        let k = (kind, format!("{key:?}"));
        match keys.get(&k) {
            None => {
                missing.insert(format!("{} {}", kind.name(), key_text(&key)));
            }
            Some(&(a, line)) => {
                through.insert(k.clone());
                if let Some(v) = valid.iter().find(|&&v| v != a) {
                    out.push(finding(format!(
                        "map line {line}: slot {} {} at {a:#x}: a live reference of {} uses the slot at {v:#x} (§5.5)",
                        kind.name(),
                        key_text(&key),
                        e.objs.get(*oi).map_or(String::new(), |o| o.path.clone())
                    )));
                }
            }
        }
    }
    for m in missing.iter().take(8) {
        out.push(finding(format!(
            "slot: a live reference uses the {m} slot, and no slot record states it"
        )));
    }
    if missing.len() > 8 {
        out.push(finding(format!("slot: {} more slots used and not stated", missing.len() - 8)));
    }
    // A GOT slot no live reference is verified through (its references
    // relaxed, or none): its word holds its key's value, in an output
    // with no dynamic table (whose words a loader may still write).
    let dynamic = image.phdrs.iter().any(|p| p.p_type == mortar_elf::consts::PT_DYNAMIC);
    let mut by_word = 0usize;
    for s in &d.slots {
        let (SlotKind::Got | SlotKind::GotTprel, SlotKey::Global(n)) = (&s.kind, &s.key) else {
            continue;
        };
        if dynamic || through.contains(&(s.kind, format!("{:?}", bare(&s.key)))) {
            continue;
        }
        let n = n.split('@').next().unwrap_or(n);
        let Some(y) = image.global(n).or_else(|| image.unique(n)) else { continue };
        let Some(w) = image.value(s.addr, word as u8) else { continue };
        let want: Option<(u64, String)> = match s.kind {
            _ if y.sym.st_shndx == mortar_elf::consts::SHN_UNDEF => {
                Some((0, "0 (undefined)".into()))
            }
            SlotKind::Got if y.sym.sym_type() == STT_GNU_IFUNC => {
                let ok = e.iplt.entries_of(y.sym.st_value).contains(&w);
                (!ok)
                    .then(|| {
                        (w ^ 1, format!("an IPLT entry of the resolver {:#x}", y.sym.st_value))
                    })
                    .or(Some((w, String::new())))
            }
            SlotKind::Got if y.sym.sym_type() == STT_TLS => None,
            SlotKind::Got => Some((y.sym.st_value, format!("{:#x}", y.sym.st_value))),
            SlotKind::GotTprel if y.sym.sym_type() == STT_TLS => match e.tls {
                Ok(Some(b)) => {
                    let v = b.tprel(b.start.wrapping_add(y.sym.st_value));
                    Some((v, format!("TPREL {v:#x}")))
                }
                _ => None,
            },
            _ => None,
        };
        let Some((v, what)) = want else { continue };
        by_word += 1;
        if w != v {
            out.push(finding(format!(
                "map line {}: slot {} '{n}' at {:#x}: its word {w:#x} is not {what}",
                s.line,
                s.kind.name(),
                s.addr
            )));
        }
    }
    let judged = through.len() + by_word + entries;
    (judged, d.slots.len().saturating_sub(judged))
}

/// The `JUMP_SLOT` records of the output: slot → symbol name.
fn jump_slots(image: &Image<'_>) -> BTreeMap<u64, String> {
    let mut m = BTreeMap::new();
    let (ty, w) = match image.header.e_machine {
        EM_AARCH64 => (1026u32, 24usize),
        EM_X86_64 => (7, 24),
        _ => return m,
    };
    for (i, s) in image.sections.iter().enumerate() {
        if s.shdr.sh_type != SHT_RELA || s.shdr.sh_flags & SHF_ALLOC == 0 {
            continue;
        }
        let Some(d) = image.section_data(i) else { continue };
        let ds = s.shdr.sh_link as usize;
        let (Some(syms), Some(strs)) = (
            image.section_data(ds),
            image.sections.get(ds).and_then(|x| image.section_data(x.shdr.sh_link as usize)),
        ) else {
            continue;
        };
        for r in d.chunks_exact(w) {
            let off = u64::from_le_bytes(r[0..8].try_into().unwrap_or_default());
            let info = u64::from_le_bytes(r[8..16].try_into().unwrap_or_default());
            if (info & 0xffff_ffff) as u32 != ty {
                continue;
            }
            let si = (info >> 32) as usize;
            let Some(e) = syms.get(si * 24..si * 24 + 4) else { continue };
            let no = u32::from_le_bytes(e.try_into().unwrap_or_default()) as usize;
            let name =
                strs.get(no..).map(|t| &t[..t.iter().position(|&c| c == 0).unwrap_or(t.len())]);
            if let Some(n) = name {
                m.insert(off, String::from_utf8_lossy(n).into_owned());
            }
        }
    }
    m
}

fn redirects(image: &Image<'_>, map: &Map, out: &mut Vec<Finding>) {
    let d = &map.decisions;
    if d.redirects.is_empty() {
        return;
    }
    let patches: BTreeSet<u64> = map
        .chunks
        .iter()
        .filter(|c| matches!(c.kind, ChunkKind::Thunk | ChunkKind::Patch))
        .filter_map(|c| c.addr)
        .collect();
    let mut sites: BTreeMap<(u32, &SectionRef, u64), u32> = BTreeMap::new();
    for r in &d.redirects {
        let tag =
            format!("map line {}: redirect {} {:?} {:#x}", r.line, r.file, r.section, r.offset);
        if let Some(l) = sites.insert((r.file, &r.section, r.offset), r.line) {
            out.push(finding(format!("{tag}: a second redirect of the site (line {l})")));
        }
        if !patches.contains(&r.addr) {
            out.push(finding(format!(
                "{tag}: {:#x} is no chunk thunk or chunk patch record's address",
                r.addr
            )));
        }
        let place = map
            .placed
            .iter()
            .find(|p| p.file == r.file && p.section == r.section && p.input_offset.is_none());
        let Some(p) = place else {
            out.push(finding(format!("{tag}: the site's section is not placed")));
            continue;
        };
        if r.offset >= p.size {
            out.push(finding(format!(
                "{tag}: the offset is past the section's size {:#x}",
                p.size
            )));
            continue;
        }
        let site = p.addr + r.offset;
        let target = match image.header.e_machine {
            EM_AARCH64 => image.insn(site).and_then(|w| {
                (w & 0x7c00_0000 == 0x1400_0000).then(|| {
                    let imm = i64::from(((w & 0x03ff_ffff) << 6) as i32 >> 6) * 4;
                    site.wrapping_add_signed(imm)
                })
            }),
            EM_X86_64 => image
                .value(site, 4)
                .map(|v| site.wrapping_add(4).wrapping_add_signed(i64::from(v as u32 as i32))),
            _ => None,
        };
        if target != Some(r.addr) {
            out.push(finding(format!(
                "{tag}: the branch at {site:#x} reaches {}, not the patch at {:#x}",
                target.map_or("nothing decoded".into(), |t| format!("{t:#x}")),
                r.addr
            )));
        }
    }
}

fn frags(image: &Image<'_>, map: &Map, set: &DeadSet, out: &mut Vec<Finding>) {
    let d = &map.decisions;
    let mut merged: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for c in d.capacities.iter().filter(|c| c.part == Part::Merged) {
        merged.entry(c.out.clone()).or_default().push((c.addr.unwrap_or(0), c.size));
    }
    let mut bad = 0usize;
    for f in &d.frags {
        let tag = format!(
            "map line {}: frag {} {:x},{:x},{:x} {} {:#x}",
            f.line, f.out, f.class.0, f.class.1, f.class.2, f.hash, f.offset
        );
        let Some(i) = section(image, &f.out) else {
            out.push(finding(format!("{tag}: names no single output section")));
            continue;
        };
        let s = &image.sections[i];
        let Some(data) = image.section_data(i) else {
            out.push(finding(format!("{tag}: the section has no bytes")));
            continue;
        };
        let (flags, es, al) = f.class;
        let e = usize::try_from(es).unwrap_or(0);
        let o = usize::try_from(f.offset).unwrap_or(usize::MAX);
        let bytes = if e == 0 {
            None
        } else if flags & SHF_STRINGS != 0 {
            let mut k = o;
            loop {
                match data.get(k..k.saturating_add(e)) {
                    Some(c) if c.iter().all(|&b| b == 0) => break data.get(o..k + e),
                    Some(_) => k += e,
                    None => break None,
                }
            }
        } else {
            data.get(o..o.saturating_add(e))
        };
        let why = match bytes {
            None => Some("its bytes are not in the section (or not a whole string)".to_string()),
            Some(b) => {
                let h = l128(b).iter().fold(String::new(), |mut h, x| {
                    let _ = write!(h, "{x:02x}");
                    h
                });
                (h != f.hash).then(|| format!("L128 of its {} bytes is {h}", b.len()))
            }
        };
        let base = s.shdr.sh_addr;
        let addr = base + f.offset;
        let why = why.or_else(|| {
            (al > 1 && !addr.is_multiple_of(al))
                .then(|| format!("{addr:#x} is not aligned at {al:#x}"))
        });
        let why = why.or_else(|| {
            let len = bytes.map_or(0, |b| b.len() as u64);
            let alloc = s.shdr.sh_flags & SHF_ALLOC != 0;
            let pos = if alloc { addr } else { f.offset };
            let ok = merged.get(&f.out).is_some_and(|v| {
                v.iter().any(|&(a, z)| {
                    let start = if alloc { a } else { merged_offset(map, &f.out) };
                    start <= pos && pos + len <= start + z
                })
            });
            (!ok).then(|| "it lies in no merged chunk of the section (capacity records)".into())
        });
        let why = why.or_else(|| {
            set.in_section(i, addr).map(|r| format!("it lies in the {}", r.describe(image)))
        });
        if let Some(w) = why {
            bad += 1;
            if bad <= 8 {
                out.push(finding(format!("{tag}: {w}")));
            }
        }
    }
    if bad > 8 {
        out.push(finding(format!("frag: {} more records not judged sound", bad - 8)));
    }
}

/// The offset of the merged chunk of non-allocated section `out`: the
/// sizes of the chunks before it in `order`'s order.
fn merged_offset(map: &Map, out: &str) -> u64 {
    let d = &map.decisions;
    let Some(o) = d.orders.iter().find(|o| o.out == out) else { return 0 };
    let mut at = 0;
    for n in &o.ordinals {
        let Some(c) = d.capacities.iter().find(|c| c.out == out && c.ordinal == *n) else { break };
        if c.part == Part::Merged {
            return at;
        }
        at += c.size;
    }
    at
}
