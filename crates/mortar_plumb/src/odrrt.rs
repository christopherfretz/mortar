//! Stage 4 with the load-time ODR runtime (`spec/plumb.md` §13.1, §13.2
//! P-M2's runtime note and P-M3's "exactly this code"; WP-197; the
//! request is `spec/odr.md` O-59).
//!
//! The runtime is an input mortar adds itself under the name
//! [`PATH`] (`spec/cli.md` C-ODR-RT): no file of the line. The verifier
//! does not take its bytes from the linker or the caller: it holds its own
//! copy of the six objects of `runtime/odr/obj/` and serves the one of the
//! output's target and kind, chosen from the output's header
//! ([`expected_id`]), as an input by that name ([`Overlay`],
//! [`OverlayFiles`]). Every rule that re-derives output bytes from an
//! input then judges the runtime as any object.

use std::collections::BTreeSet;

use mortar_elf::consts::{
    EM_386, EM_AARCH64, EM_X86_64, ET_DYN, ET_REL, PT_INTERP, SHF_ALLOC, SHN_UNDEF, SHT_INIT_ARRAY,
    SHT_PREINIT_ARRAY, STB_LOCAL, STB_WEAK,
};

use crate::Source;
use crate::dynout::DynOut;
use crate::image::Image;
use crate::line::Files;
use crate::locate::{Obj, Place};
use crate::map::{Format, Map};
use crate::opts::{OdrRuntime, Options};
use crate::report::{Finding, MetaCoverage, Rule};

/// The runtime input's name in the map, the diagnostics and the tables
/// (`spec/cli.md` C-ODR-RT).
pub(crate) const PATH: &str = "<mortar:odr-runtime>";

/// `NT_MASON_ODR_RT` (`spec/odr.md` O-52).
pub(crate) const NT_ODR_RT: u32 = 0x4d4f_4403;

/// The six objects by runtime id (`spec/odr.md` O-52: 0 AArch64 `so`, 1
/// AArch64 `exe`, 2 x86-64 `so`, 3 x86-64 `exe`, 4 i386 `so`, 5 i386
/// `exe`): the verifier's own copy, never the linker's.
pub(crate) const OBJECTS: [(&str, &[u8]); 6] = [
    (
        "mason_odr_rt-aarch64-so.o",
        include_bytes!("../../../runtime/odr/obj/mason_odr_rt-aarch64-so.o"),
    ),
    (
        "mason_odr_rt-aarch64-exe.o",
        include_bytes!("../../../runtime/odr/obj/mason_odr_rt-aarch64-exe.o"),
    ),
    (
        "mason_odr_rt-x86_64-so.o",
        include_bytes!("../../../runtime/odr/obj/mason_odr_rt-x86_64-so.o"),
    ),
    (
        "mason_odr_rt-x86_64-exe.o",
        include_bytes!("../../../runtime/odr/obj/mason_odr_rt-x86_64-exe.o"),
    ),
    ("mason_odr_rt-i386-so.o", include_bytes!("../../../runtime/odr/obj/mason_odr_rt-i386-so.o")),
    ("mason_odr_rt-i386-exe.o", include_bytes!("../../../runtime/odr/obj/mason_odr_rt-i386-exe.o")),
];

/// The names the runtime takes from the link itself, not from a library
/// (`spec/odr.md` O-53): defined by the linker.
const FROM_LINKER: [&str; 2] = ["__ehdr_start", "_GLOBAL_OFFSET_TABLE_"];

/// The runtime id of the output (§13.1): its machine (AArch64 0, x86-64 2,
/// i386 4) plus 1 for an executable (a `PT_INTERP`, PIE or not), 0 for a
/// shared object (`ET_DYN` without one). `None`: no runtime can be in it
/// (another machine, or a static executable).
pub(crate) fn expected_id(image: &Image<'_>) -> Option<u32> {
    let base = match image.header.e_machine {
        EM_AARCH64 => 0,
        EM_X86_64 => 2,
        EM_386 => 4,
        _ => return None,
    };
    if image.phdrs.iter().any(|p| p.p_type == PT_INTERP) {
        Some(base + 1)
    } else if image.header.e_type == ET_DYN {
        Some(base)
    } else {
        None
    }
}

/// The note's mode for the option (`spec/odr.md` O-52): 1 `report`, 2
/// `fatal`; `None` for `off` (no note) and a value mortar refuses.
pub(crate) fn mode(o: OdrRuntime) -> Option<u32> {
    match o {
        OdrRuntime::Report => Some(1),
        OdrRuntime::Fatal => Some(2),
        OdrRuntime::Off | OdrRuntime::Other => None,
    }
}

/// Whether the line asks for the runtime (`report` or `fatal`).
pub(crate) fn on(o: &Options) -> bool {
    mode(o.odr_runtime).is_some()
}

/// The bytes the verifier serves as [`PATH`] for this link, when it
/// serves any: a mortar map whose line asks for the runtime and that
/// names the input, in an output whose kind has a runtime. Otherwise the
/// map's [`PATH`] is a file like any other (I1: not among the inputs).
pub(crate) fn embedded(output: &[u8], map: &Map) -> Option<&'static [u8]> {
    if !serves(map) {
        return None;
    }
    let image = Image::read(output).ok()?;
    let id = expected_id(&image)?;
    Some(OBJECTS[id as usize].1)
}

/// Whether the verifier reads the runtime as an input of this link: a
/// mortar map whose line asks for it and that names [`PATH`] (§13.1).
pub(crate) fn serves(map: &Map) -> bool {
    map.format == Format::Mortar
        && map.files.iter().any(|f| f.path == PATH && f.member.is_none())
        && crate::opts::of(map).is_some_and(|o| on(&o))
}

/// A source with the runtime's bytes as one more file, [`PATH`], which
/// hides any file of the caller's by that name.
pub(crate) struct Overlay<'a> {
    pub inner: &'a dyn Source,
    pub bytes: &'static [u8],
}

impl Source for Overlay<'_> {
    fn count(&self) -> usize {
        self.inner.count() + 1
    }
    fn path(&self, i: usize) -> &str {
        if i == self.inner.count() { PATH } else { self.inner.path(i) }
    }
    fn len(&self, i: usize) -> u64 {
        if i == self.inner.count() { self.bytes.len() as u64 } else { self.inner.len(i) }
    }
    fn read_at(&self, i: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        if i != self.inner.count() {
            return self.inner.read_at(i, offset, buf);
        }
        let got = usize::try_from(offset)
            .ok()
            .and_then(|at| self.bytes.get(at..at.checked_add(buf.len())?));
        match got {
            Some(b) => {
                buf.copy_from_slice(b);
                Ok(())
            }
            None => {
                Err(format!("{PATH}: {} bytes at offset {offset:#x} are past its end", buf.len()))
            }
        }
    }
}

/// The caller's files with the runtime's bytes as [`PATH`] (the line's
/// readers: the input set, stage 3's model).
pub(crate) struct OverlayFiles<'a> {
    pub inner: &'a dyn Files,
    pub bytes: &'static [u8],
}

impl Files for OverlayFiles<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if path == PATH { Some(self.bytes.to_vec()) } else { self.inner.read(path) }
    }
    fn len(&self, path: &str) -> Option<u64> {
        if path == PATH { Some(self.bytes.len() as u64) } else { self.inner.len(path) }
    }
    fn head(&self, path: &str, n: usize) -> Option<Vec<u8>> {
        if path == PATH {
            Some(self.bytes[..n.min(self.bytes.len())].to_vec())
        } else {
            self.inner.head(path, n)
        }
    }
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        if path != PATH {
            return self.inner.read_at(path, offset, buf);
        }
        let got = usize::try_from(offset)
            .ok()
            .and_then(|at| self.bytes.get(at..at.checked_add(buf.len())?));
        let b = got.ok_or_else(|| format!("{PATH}: past its end"))?;
        buf.copy_from_slice(b);
        Ok(())
    }
}

/// The line's words with the runtime after every input (`spec/cli.md`
/// C-ODR-RT: "an ordinary input after every input of the line"), for the
/// readers that derive the link's files from its words.
pub(crate) fn words_with_runtime(words: &[String]) -> Vec<String> {
    let mut w = words.to_vec();
    w.push(PATH.to_string());
    w
}

// ---------------------------------------------------------------------------
// P-M2: the runtime's note
// ---------------------------------------------------------------------------

/// P-M2's runtime note (§13.2): its descriptor's four fields, against the
/// line's mode (when the options are known) and the output's runtime id.
pub(crate) fn note(
    desc: &[u8],
    at: u64,
    image: &Image<'_>,
    mode: Option<u32>,
    f: &mut Vec<Finding>,
) {
    let m = |s: String| Finding::new(Rule::Meta(2), s).at(at);
    if desc.len() != 16 {
        f.push(m(format!(
            "runtime: the note's descriptor is {} bytes; O-52's is 16 ({{version, mode, flags, runtime id}})",
            desc.len()
        )));
        return;
    }
    let w = |i: usize| u32::from_le_bytes([desc[i], desc[i + 1], desc[i + 2], desc[i + 3]]);
    let (version, got_mode, flags, id) = (w(0), w(4), w(8), w(12));
    if version != 1 {
        f.push(m(format!("runtime: version {version}; O-52's is 1")));
    }
    match mode {
        Some(want) if got_mode != want => f.push(m(format!(
            "runtime: mode {got_mode}; the line's --odr-runtime gives {want} (O-52: 1 report, 2 fatal)"
        ))),
        None if !(1..=2).contains(&got_mode) => f.push(m(format!(
            "runtime: mode {got_mode}; O-52's are 1 (report) and 2 (fatal)"
        ))),
        _ => {}
    }
    if flags != 0 {
        f.push(m(format!("runtime: flags {flags:#x}; O-52's are 0")));
    }
    match expected_id(image) {
        Some(want) if id != want => f.push(m(format!(
            "runtime: runtime id {id}; the output ({}, {}) takes {want} ({})",
            machine(image),
            if want % 2 == 1 { "an executable" } else { "a shared object" },
            OBJECTS[want as usize].0
        ))),
        None => f.push(m(format!(
            "runtime: runtime id {id} in an output no runtime is linked into (machine {}, no PT_INTERP, e_type {})",
            image.header.e_machine, image.header.e_type
        ))),
        _ => {}
    }
}

fn machine(image: &Image<'_>) -> &'static str {
    match image.header.e_machine {
        EM_AARCH64 => "AArch64",
        EM_X86_64 => "x86-64",
        EM_386 => "i386",
        _ => "another machine",
    }
}

// ---------------------------------------------------------------------------
// P-M3: exactly this code
// ---------------------------------------------------------------------------

/// The priority of an input init-array section by its name (§13.2: the
/// order the reference linkers and mortar give, WP-198 §2.2): `.init_array.N`
/// is N; `.init_array`, `.ctors*` and any other name sort after every
/// numbered one.
fn priority(name: &str) -> u64 {
    name.strip_prefix(".init_array.").and_then(|n| n.parse::<u64>().ok()).unwrap_or(u64::MAX)
}

/// The runtime's object among the map's files: the file named [`PATH`].
pub(crate) fn runtime_obj<'o, 'a>(objs: &'o [Obj<'a>]) -> Option<(usize, &'o Obj<'a>)> {
    objs.iter().enumerate().find(|(_, o)| o.path == PATH && o.member.is_none())
}

/// P-M3 with the runtime (§13.2): the input rule's runtime half, the
/// entry, what the runtime exports, its imports, and its sections kept.
/// `rt`: the line asks for the runtime (options known).
#[allow(clippy::too_many_lines)]
pub(crate) fn pm3(
    image: &Image<'_>,
    objs: &[Obj<'_>],
    map: &Map,
    rt: bool,
    cov: &mut MetaCoverage,
    f: &mut Vec<Finding>,
) {
    let found = runtime_obj(objs);
    let m3 = |s: String| Finding::new(Rule::Meta(3), s);
    if !rt {
        return;
    }
    let Some((ri, ro)) = found else {
        f.push(m3(format!(
            "input: the line asks for the runtime (--odr-runtime) and the map names no '{PATH}' (spec/cli.md C-ODR-RT)"
        )));
        return;
    };
    cov.runtime_outputs += 1;
    let exe = image.phdrs.iter().any(|p| p.p_type == PT_INTERP);
    // Its file is the map's last but the archive members (C-ODR-RT: after
    // every input of the line, before the members are extracted).
    let rank =
        |o: &Obj<'_>| map.files.iter().position(|m| m.path == o.path && m.member == o.member);
    let r_rank = rank(ro);
    for o in objs {
        if o.member.is_none()
            && o.path != PATH
            && rank(o) > r_rank
            && o.elf.header().e_type == ET_REL
        {
            f.push(m3(format!(
                "input: {} follows '{PATH}' in the map's file order; the runtime is the line's last input (spec/cli.md C-ODR-RT)",
                o.label()
            )));
        }
    }
    // Its sections are kept: none of its allocated sections with bytes is
    // collected (`--gc-sections`: its entry is a start-up root).
    let eh = image.sections.iter().any(|s| s.name == ".eh_frame");
    for (si, s) in ro.secs.iter().enumerate() {
        if s.shdr.sh_flags & SHF_ALLOC == 0 || s.shdr.sh_size == 0 {
            continue;
        }
        if ro.no_place(si, eh) {
            f.push(m3(format!(
                "kept: the runtime's {} (section {si}) is not in the output; nothing of the runtime is collected (spec/live.md E14 Q5: its entry is a start-up root)",
                s.name
            )));
        }
    }
    // The entry.
    let want = if exe { SHT_PREINIT_ARRAY } else { SHT_INIT_ARRAY };
    let entries: Vec<(usize, u64)> = ro
        .secs
        .iter()
        .enumerate()
        .filter(|(_, s)| s.shdr.sh_type == want && s.shdr.sh_size > 0)
        .filter_map(|(si, s)| match s.place {
            Place::Whole(a) => Some((si, a)),
            _ => None,
        })
        .collect();
    let kind = if exe { ".preinit_array" } else { ".init_array" };
    if entries.len() != 1 {
        f.push(m3(format!(
            "entry: the runtime's {kind} entry is placed {} times; O-54a's entry is one {kind} word",
            entries.len()
        )));
    }
    for &(si, at) in &entries {
        cov.runtime_entries += 1;
        let rsec = &ro.secs[si];
        let Some(out) = image.sections.iter().find(|s| {
            s.shdr.sh_type == want
                && s.shdr.sh_addr <= at
                && at < s.shdr.sh_addr.saturating_add(s.shdr.sh_size)
        }) else {
            f.push(
                m3(format!(
                    "entry: the runtime's {} is placed at {at:#x}, in no output section of type {kind}",
                    rsec.name
                ))
                .at(at),
            );
            continue;
        };
        let (lo, hi) = (out.shdr.sh_addr, out.shdr.sh_addr.saturating_add(out.shdr.sh_size));
        let rprio = priority(&rsec.name);
        // Every other input section of the type in that output section:
        // before the entry exactly when the order gives it (§13.2).
        for (oi, o) in objs.iter().enumerate() {
            for s in &o.secs {
                if oi == ri && std::ptr::eq(s, rsec) {
                    continue;
                }
                if s.shdr.sh_type != want || s.shdr.sh_size == 0 {
                    continue;
                }
                let Place::Whole(a) = s.place else { continue };
                if a < lo || a >= hi {
                    continue;
                }
                let before = if exe {
                    rank(o) < r_rank
                } else {
                    let p = priority(&s.name);
                    p < rprio || (p == rprio && rank(o) < r_rank)
                };
                if before != (a < at) {
                    f.push(
                        m3(format!(
                            "entry: {}'s {} at {a:#x} is {} the runtime's entry at {at:#x}; the order (WP-198 §2.2: {}) puts it {}",
                            o.label(),
                            s.name,
                            if a < at { "before" } else { "after" },
                            if exe {
                                "input order"
                            } else {
                                "priority, then input order; the runtime is the line's last input"
                            },
                            if before { "before" } else { "after" }
                        ))
                        .at(a),
                    );
                }
            }
        }
    }
    // It exports nothing (O-53: it defines and exports no symbol): no
    // `.dynsym` definition and no DT_INIT / DT_FINI inside its sections.
    let ranges: Vec<(u64, u64, &str)> = ro
        .secs
        .iter()
        .filter(|s| s.shdr.sh_flags & SHF_ALLOC != 0 && s.shdr.sh_size > 0)
        .filter_map(|s| match s.place {
            Place::Whole(a) => Some((a, a.saturating_add(s.shdr.sh_size), s.name.as_str())),
            _ => None,
        })
        .collect();
    let inside = |v: u64| ranges.iter().find(|(a, e, _)| *a <= v && v < *e).map(|r| r.2);
    let dout = DynOut::read(image);
    if let Some(d) = &dout {
        for s in d.syms.iter().skip(1) {
            if s.st_shndx != SHN_UNDEF
                && let Some(n) = inside(s.st_value)
            {
                f.push(
                    m3(format!(
                        "exports: .dynsym defines '{}' at {:#x}, inside the runtime's {n}; the runtime exports no symbol (O-53)",
                        s.name, s.st_value
                    ))
                    .at(s.st_value),
                );
            }
        }
        for (tag, name) in [(12u64, "DT_INIT"), (13, "DT_FINI")] {
            if let Some(v) = d.get(tag)
                && let Some(n) = inside(v)
            {
                f.push(
                    m3(format!("exports: {name} is {v:#x}, inside the runtime's {n} (O-54a: its entry is the init array's)"))
                        .at(v),
                );
            }
        }
    }
    // Its imports (O-53: libc's and the linker's): each is defined in the
    // output by an input, or imported through `.dynsym`.
    let Some(t) = ro.symtab else { return };
    let mut defined: BTreeSet<&str> = BTreeSet::new();
    for s in &image.symbols {
        if s.sym.st_shndx != SHN_UNDEF && s.sym.binding() != STB_LOCAL {
            defined.insert(s.name.as_str());
        }
    }
    let mut imported: BTreeSet<&str> = BTreeSet::new();
    if let Some(d) = &dout {
        for s in d.syms.iter().skip(1) {
            if s.st_shndx == SHN_UNDEF {
                imported.insert(s.name.as_str());
            } else {
                defined.insert(s.name.as_str());
            }
        }
    }
    let input_defined = |name: &[u8]| {
        objs.iter().enumerate().any(|(oi, o)| {
            oi != ri
                && o.symtab.is_some_and(|ot| {
                    ot.iter().any(|(_, s)| {
                        s.st_shndx != SHN_UNDEF
                            && s.binding() != STB_LOCAL
                            && ot.name(&s).is_ok_and(|n| n == name)
                    })
                })
        })
    };
    for (_, s) in t.iter() {
        if s.st_shndx != SHN_UNDEF || s.binding() == STB_LOCAL || s.binding() == STB_WEAK {
            continue;
        }
        let Ok(n) = t.name(&s) else { continue };
        let name = String::from_utf8_lossy(n);
        if n.is_empty() || FROM_LINKER.contains(&name.as_ref()) {
            continue;
        }
        cov.runtime_imports += 1;
        if !imported.contains(name.as_ref())
            && !defined.contains(name.as_ref())
            && !input_defined(n)
        {
            f.push(m3(format!(
                "imports: the runtime's '{name}' is neither imported (.dynsym) nor defined by an input of the output (O-53: it needs it from libc)"
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // §13.1: the verifier's copy of each object is the one `runtime/odr/
    // PINS` records (O-53b): its SHA-256 and size, in runtime id order.
    #[test]
    fn s13_1_the_embedded_objects_are_the_pinned_ones() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../runtime/odr/PINS");
        let pins = std::fs::read_to_string(path).unwrap();
        for (name, bytes) in OBJECTS {
            let line = pins
                .lines()
                .find(|l| l.starts_with(&format!("object {name} ")))
                .unwrap_or_else(|| panic!("PINS has no object {name}"));
            let w: Vec<&str> = line.split(' ').collect();
            let hex = crate::buildid::sha256(bytes).iter().fold(String::new(), |mut h, b| {
                use std::fmt::Write as _;
                let _ = write!(h, "{b:02x}");
                h
            });
            assert_eq!(w[2], hex, "{name}");
            assert_eq!(w[3], bytes.len().to_string(), "{name}");
        }
    }

    // §13.1: the runtime id of each object is its machine and kind; an
    // executable's object has a `.preinit_array` entry, a shared
    // object's an `.init_array.00000` one (O-54a).
    #[test]
    fn s13_1_the_ids_name_machine_and_kind() {
        for (id, (name, bytes)) in OBJECTS.iter().enumerate() {
            let o = mortar_elf::Object::parse(bytes).unwrap();
            let want = [EM_AARCH64, EM_X86_64, EM_386][id / 2];
            assert_eq!(o.header().e_machine, want, "{name}");
            assert_eq!(name.ends_with("-exe.o"), id % 2 == 1, "{name}");
        }
        assert_eq!(mode(OdrRuntime::Report), Some(1));
        assert_eq!(mode(OdrRuntime::Fatal), Some(2));
        assert_eq!(mode(OdrRuntime::Off), None);
        assert_eq!(priority(".init_array.00000"), 0);
        assert_eq!(priority(".init_array.101"), 101);
        assert_eq!(priority(".init_array"), u64::MAX);
        assert_eq!(priority(".ctors"), u64::MAX);
    }
}
