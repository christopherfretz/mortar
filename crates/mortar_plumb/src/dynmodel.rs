//! The verifier's own model of a dynamic link's shared-object decisions
//! (stage 3, `spec/plumb.md` P-D2; `spec/dyn.md` D-10..D-13, D-19a, D-21, D-25): the
//! loaded objects (command line and closure, input §8.6), the needed set
//! N, the used set U, `giver` and `bind`, the expected `DT_NEEDED` order,
//! imports, exports and the `WeakDyn` keys. Written from the spec, never
//! from the linker; nothing here is taken from the map.
// ELF fields and addresses read best by the specs' short names.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use mortar_elf::consts::{
    SHN_ABS, SHN_COMMON, SHN_UNDEF, STB_GLOBAL, STB_GNU_UNIQUE, STB_WEAK, STV_HIDDEN, STV_INTERNAL,
};
use mortar_elf::{SharedObject, SymbolIndex, SymbolSection, Version};

use crate::line::{Files, Item, Line};
use crate::locate::Obj;
use crate::map::OutputKind;

/// A definition in a shared object's `.dynsym`.
#[derive(Debug, Clone)]
pub struct SoDef {
    /// The version's name; `None` for an unversioned (index 1) entry.
    pub version: Option<String>,
    /// `name@@V` (or unversioned): what an unversioned reference binds.
    pub default: bool,
    pub ty: u8,
    /// `STB_GLOBAL`, `STB_WEAK` or `STB_GNU_UNIQUE` (the input set's
    /// `cfirst`, §3.8).
    pub bind: u8,
    pub other: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

impl SoDef {
    pub fn visibility(&self) -> u8 {
        self.other & 3
    }
    /// Whether a reference can bind it (D-10: hidden entries are ignored).
    pub fn visible(&self) -> bool {
        !matches!(self.visibility(), STV_HIDDEN | STV_INTERNAL)
    }
    /// Whether it satisfies a reference asking for `ver` (resolve §9.2).
    pub fn matches(&self, ver: Option<&str>) -> bool {
        match ver {
            None => self.default,
            Some(v) => self.version.as_deref() == Some(v),
        }
    }
}

/// An undefined reference of a shared object (D-13).
#[derive(Debug, Clone)]
pub struct SoRef {
    pub name: String,
    pub weak: bool,
    pub version: Option<String>,
}

/// A loaded shared object (P-D2).
#[derive(Debug, Clone)]
pub struct Dso {
    pub path: String,
    /// `DT_SONAME`, else the name as written (D-19a).
    pub soname: String,
    pub needed: Vec<String>,
    pub runpath: Option<String>,
    pub rpath: Option<String>,
    pub defs: HashMap<String, Vec<SoDef>>,
    pub refs: Vec<SoRef>,
    /// `(vaddr, memsz, flags)` of each `PT_LOAD`.
    pub loads: Vec<(u64, u64, u32)>,
    pub relro: Vec<(u64, u64)>,
    /// Version definitions in order: `(index, name, hash)`.
    pub verdefs: Vec<(u16, String, u32)>,
    pub machine: u16,
    /// A member of C (the command line); else a closure file (W).
    pub in_c: bool,
    /// The merged `as_needed` of its mentions (input §8.2).
    pub as_needed: bool,
    /// The bytes, kept for the loader model (`COPY` contents).
    pub bytes: Vec<u8>,
}

impl Dso {
    /// The visible definition of `name` at `ver`, if any.
    pub fn def(&self, name: &str, ver: Option<&str>) -> Option<&SoDef> {
        self.defs.get(name)?.iter().find(|d| d.visible() && d.matches(ver))
    }

    /// Whether the definition's address is in RELRO or non-writable
    /// memory (D-42).
    pub fn read_only_at(&self, value: u64) -> bool {
        self.relro.iter().any(|&(a, s)| a <= value && value < a + s)
            || self
                .loads
                .iter()
                .any(|&(a, s, f)| a <= value && value < a + s && f & mortar_elf::consts::PF_W == 0)
    }

    /// D-42's copy alignment of a definition at `value` in section
    /// `shndx`: min(the section's alignment, 2^ctz(value)).
    pub fn copy_align(&self, shndx: u16, value: u64) -> u64 {
        let sec = mortar_elf::FileHeader::parse(&self.bytes)
            .ok()
            .and_then(|h| mortar_elf::SectionTable::parse(&self.bytes, &h).ok())
            .and_then(|t| t.iter().nth(usize::from(shndx)).map(|s| s.header().sh_addralign))
            .unwrap_or(1)
            .max(1);
        if value == 0 { sec } else { sec.min(1u64 << value.trailing_zeros()) }
    }

    pub(crate) fn read(path: &str, bytes: Vec<u8>, in_c: bool) -> Result<Dso, String> {
        let so = SharedObject::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let dynamic = so.dynamic().map_err(|e| format!("{path}: {e}"))?;
        let mut d = Dso {
            path: path.into(),
            soname: String::new(),
            needed: Vec::new(),
            runpath: None,
            rpath: None,
            defs: HashMap::new(),
            refs: Vec::new(),
            loads: Vec::new(),
            relro: Vec::new(),
            verdefs: Vec::new(),
            machine: so.header().e_machine,
            in_c,
            as_needed: false,
            bytes: Vec::new(),
        };
        let lossy = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        if let Some(t) = &dynamic {
            d.soname = t.soname().ok().flatten().map(lossy).unwrap_or_default();
            for n in t.needed() {
                d.needed.push(lossy(n.map_err(|e| format!("{path}: {e}"))?));
            }
            d.runpath = t.runpath().ok().flatten().map(lossy);
            d.rpath = t.rpath().ok().flatten().map(lossy);
        }
        if d.soname.is_empty() {
            // input §8.3: a file without DT_SONAME is needed by its name as
            // written; two spellings are two objects (D-19a).
            d.soname = path.to_string();
        }
        for seg in so.program_headers() {
            let p = *seg.header();
            match p.p_type {
                mortar_elf::consts::PT_LOAD => d.loads.push((p.p_vaddr, p.p_memsz, p.p_flags)),
                mortar_elf::consts::PT_GNU_RELRO => d.relro.push((p.p_vaddr, p.p_memsz)),
                _ => {}
            }
        }
        let versions = so.versions().map_err(|e| format!("{path}: {e}"))?;
        if let Ok(Some(defs)) = versions.definitions() {
            for vd in defs {
                let Ok(vd) = vd else { break };
                d.verdefs.push((
                    vd.index(),
                    vd.name().map(lossy).unwrap_or_default(),
                    vd.record().vd_hash,
                ));
            }
        }
        if let Some(t) = so.symbols().map_err(|e| format!("{path}: {e}"))? {
            for (i, s) in t.iter() {
                if i.0 == 0 {
                    continue;
                }
                let name = lossy(t.name(&s).map_err(|e| format!("{path}: {e}"))?);
                let ver = versions.of(SymbolIndex(i.0)).ok().flatten();
                let (version, hidden) = match ver {
                    Some(v) => (
                        match v.version {
                            Version::Local | Version::Global => None,
                            _ => v.name().ok().flatten().map(lossy),
                        },
                        v.versym.hidden(),
                    ),
                    None => (None, false),
                };
                if s.st_shndx == SHN_UNDEF {
                    if matches!(s.binding(), STB_GLOBAL | STB_WEAK) {
                        d.refs.push(SoRef { name, weak: s.binding() == STB_WEAK, version });
                    }
                    continue;
                }
                if !matches!(s.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) {
                    continue;
                }
                if ver.is_some_and(|v| matches!(v.version, Version::Local)) {
                    continue;
                }
                // The base version definition (VER_FLG_BASE) names the
                // object: a symbol at it is unversioned.
                let base =
                    ver.is_some_and(|v| matches!(v.version, Version::Defined(vd) if vd.is_base()));
                let version = if base { None } else { version };
                d.defs.entry(name).or_default().push(SoDef {
                    default: !hidden,
                    version,
                    ty: s.sym_type(),
                    bind: s.binding(),
                    other: s.st_other,
                    shndx: s.st_shndx,
                    value: s.st_value,
                    size: s.st_size,
                });
            }
        }
        d.bytes = bytes;
        Ok(d)
    }
}

/// A key of the program: a name and the version a reference asks for.
fn split_version(name: &str) -> (&str, Option<&str>) {
    match name.find('@') {
        Some(i) => {
            let v = name[i + 1..].trim_start_matches('@');
            (&name[..i], Some(v))
        }
        None => (name, None),
    }
}

/// The program's view of one key (its relocatable objects and file 0).
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // independent facts of D-12
pub struct Key {
    /// Defined by a live relocatable object, or file 0 (`.symtab`).
    pub defined: bool,
    /// Its merged visibility is hidden or internal.
    pub hidden: bool,
    pub strong_ref: bool,
    pub weak_ref: bool,
    /// The version a reference asks for (`.symver`).
    pub version: Option<String>,
    /// The positions (line items) of the objects that reference it
    /// strongly, with their groups (D-19a).
    pub ref_pos: Vec<(usize, Option<usize>)>,
    /// Every definition by a relocatable object is in a member of an
    /// archive `--exclude-libs` names (D-110 item 1; WP-161): no export
    /// of the line gives it, and a shared object's reference to it finds
    /// nothing (D-13).
    pub excluded: bool,
    /// An executable's `--dynamic-list` lists it (resolve §9.6, D-21;
    /// WP-161): a definition of it is exported.
    pub listed: bool,
    /// A root of the line (`Line::root_names`: `-e`, `-u`,
    /// `--require-defined`, `EXTERN`; resolve §4.2, WP-163). A root is
    /// no reference for D-11 (item 2: it never makes an `--as-needed`
    /// library needed); after the needed set it is a strong reference
    /// iff a library of that set defines it (D-21 item 1: imported),
    /// else it is written nowhere.
    pub root: bool,
    /// A root a library of the needed set defines and no input references:
    /// `strong_ref` by D-21 item 1 alone (GNU ld and mortar import it; lld
    /// and mold write none, anomaly A47; WP-163).
    pub root_import: bool,
    /// Defined by file 0 alone (the linker's definition, resolve §8; D-21
    /// item 2's seven names among them).
    pub linker: bool,
}

/// D-12's stages over the line's inputs (`crate::inset`'s closure of
/// resolve §4.1): what is live at a stage, given its sources.
pub(crate) trait Staging {
    /// The live mask over the output's objects (`objs` of
    /// [`Model::build`]) at a stage whose sources are the members of C
    /// with these sonames. An object the inputs do not name is live.
    fn stage(&mut self, objs: &[Obj<'_>], sources: &BTreeSet<String>) -> Vec<bool>;
    /// Whether `first(k)` of resolve §4.1 for a shared object's reference
    /// (`name`, at `ver`) is a lazy file not live at the last stage
    /// (D-12's item-3 rule, cell 7a2).
    fn lazy_first(&self, name: &str, ver: Option<&str>) -> bool;
}

/// D-11 over one stage's view of the program's keys.
#[derive(Default)]
struct Stage {
    n0: BTreeSet<usize>,
    n: BTreeSet<usize>,
    /// Item-2 givers: the positions of the strong references they give.
    obj_reasons: HashMap<usize, Vec<(usize, Option<usize>)>>,
    /// Item-3 pairs: provider, the needers.
    dso_reasons: HashMap<usize, Vec<usize>>,
    /// The keys each item-2 giver gives (GNU ld's own neededness).
    given: HashMap<usize, Vec<String>>,
}

/// The model.
#[derive(Debug, Default)]
pub struct Model {
    pub dsos: Vec<Dso>,
    /// Mentions in order: `(dso, as_needed, item position, group)`.
    pub mentions: Vec<(usize, bool, usize, Option<usize>)>,
    /// C, in first-mention order.
    pub c: Vec<usize>,
    pub keys: BTreeMap<String, Key>,
    pub needed: BTreeSet<usize>,
    pub used: BTreeSet<usize>,
    /// The expected `DT_NEEDED` order per D-19a (GNU ld, mortar).
    pub order_d19a: Vec<usize>,
    /// The first-mention order (lld, mold).
    pub order_first: Vec<usize>,
    /// Needed objects GNU ld does not record (D-11: it decides neededness
    /// by its binding in D-19a's order): as-needed, needed by item 2 alone,
    /// and every key they give binds to an object before them, so no
    /// binding changes without them (`needed-order-dup-def` line 1).
    pub gnu_unbound: BTreeSet<usize>,
    /// The givers of keys the program references only weakly, which mold
    /// makes needed (D-11 "mold differs", E-shlib 2.2a).
    pub mold_weak: BTreeSet<usize>,
    /// The objects D-11 item 3 alone makes needed (an under-linked
    /// library's providers).
    pub item3: BTreeSet<usize>,
    /// Objects of N that D-12 keeps needed (row 2a): every key they gave
    /// at the stage that made them needed is defined by the output's own
    /// objects, a later stage's member (GNU ld and mold keep them, lld
    /// drops them, anomaly A71).
    pub kept: BTreeSet<usize>,
    /// Objects D-11 over every object of the output makes needed and D-12's
    /// stages do not: only an object no stage extracts needs them.
    pub unreached_needed: BTreeSet<usize>,
    /// The sources of D-12's last stage: the as-needed members of N the
    /// output's `DT_NEEDED` names, whose strong references are edges.
    pub sources: BTreeSet<usize>,
    /// The number of D-12 stages replayed (1 without a staging view).
    pub stages: usize,
    /// Names of a needed or used object's references: `ref_dynamic`.
    pub ref_dynamic: HashSet<String>,
    pub def_dynamic: HashSet<String>,
    pub problems: Vec<String>,
    /// `DT_NEEDED` objects no search finds: the link is outside stage 3
    /// (P-D1, [`crate::report::OUTSIDE`]).
    pub outside: Vec<String>,
    /// The line links a shared object: `bind(k)` ranges over N alone
    /// (`spec/dyn.md` D-10 as WP-137 amends it).
    pub shared: bool,
}

impl Model {
    pub fn by_soname(&self, n: &str) -> Option<usize> {
        self.dsos.iter().position(|d| d.soname == n)
    }

    /// `giver(k)` (D-11): the first member of C defining `k` visibly at
    /// its version.
    pub fn giver(&self, k: &str) -> Option<usize> {
        let key = self.keys.get(k);
        if key.is_some_and(|x| x.defined) {
            return None;
        }
        let ver = key.and_then(|x| x.version.as_deref());
        self.c.iter().copied().find(|&i| self.dsos[i].def(k, ver).is_some())
    }

    /// `reach(S)` along `DT_NEEDED` edges.
    pub fn reach(&self, s: impl IntoIterator<Item = usize>) -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut q: VecDeque<usize> = s.into_iter().collect();
        while let Some(i) = q.pop_front() {
            if !seen.insert(i) {
                continue;
            }
            for n in &self.dsos[i].needed {
                if let Some(j) = self.by_soname(n) {
                    q.push_back(j);
                }
            }
        }
        seen
    }

    /// The loader's search list (ld.so's breadth-first order, D-11): the
    /// objects named by `needed` (the image's `DT_NEEDED`, resolved by
    /// soname) and then their dependencies breadth first.
    pub fn loader_order(&self, needed: &[usize]) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        let mut q: VecDeque<usize> = needed.iter().copied().collect();
        while let Some(i) = q.pop_front() {
            if out.contains(&i) {
                continue;
            }
            out.push(i);
            for n in &self.dsos[i].needed {
                if let Some(j) = self.by_soname(n) {
                    q.push_back(j);
                }
            }
        }
        out
    }

    /// Builds the model from the line, the live relocatable objects and the
    /// output's `.symtab` definitions (file 0's). `staging`: D-12's stages
    /// over the line's inputs, and the output's `DT_NEEDED` names; without
    /// it N is D-11 over the output's objects (one stage).
    #[allow(clippy::too_many_lines)] // D-11, D-12, D-19a in order
    pub(crate) fn build(
        line: &Line,
        objs: &[Obj<'_>],
        file0: &[String],
        files: &dyn Files,
        staging: Option<(&mut dyn Staging, &[String])>,
    ) -> Model {
        let mut m = Model { shared: line.kind == Some(OutputKind::Shared), ..Model::default() };
        // C: one object per soname, in mention order. A file without a
        // soname is named as the line names it, so two spellings are two
        // members (two DT_NEEDED entries, D-19a).
        let mut by_path: HashMap<(String, String), usize> = HashMap::new();
        for (pos, item) in line.items.iter().enumerate() {
            let Item::Shared { path, named, as_needed, group } = item else { continue };
            let idx = if let Some(&i) = by_path.get(&(path.clone(), named.clone())) {
                i
            } else {
                let Some(bytes) = files.read(path) else {
                    m.problems.push(format!("{path}: cannot be read"));
                    continue;
                };
                match Dso::read(path, bytes, true) {
                    Ok(mut d) => {
                        if d.soname == *path {
                            d.soname.clone_from(named);
                        }
                        let i = if let Some(j) = m.by_soname(&d.soname) {
                            j
                        } else {
                            m.dsos.push(d);
                            m.c.push(m.dsos.len() - 1);
                            m.dsos.len() - 1
                        };
                        by_path.insert((path.clone(), named.clone()), i);
                        i
                    }
                    Err(e) => {
                        m.problems.push(e);
                        continue;
                    }
                }
            };
            m.mentions.push((idx, *as_needed, pos, *group));
        }
        for &i in &m.c {
            m.dsos[i].as_needed = m.mentions.iter().filter(|x| x.0 == i).all(|x| x.1);
        }
        // W: the closure, breadth first (input §8.6).
        let mut q: VecDeque<(usize, String)> = VecDeque::new();
        for &i in &m.c.clone() {
            for n in m.dsos[i].needed.clone() {
                q.push_back((i, n));
            }
        }
        while let Some((s, n)) = q.pop_front() {
            if m.by_soname(&n).is_some() {
                continue;
            }
            let found = m.search(line, s, &n, files);
            let Some((path, bytes)) = found else {
                m.outside.push(format!(
                    "{n} (needed by {}) is not found: the loader model needs it (dyn.md D-13)",
                    m.dsos[s].path
                ));
                continue;
            };
            match Dso::read(&path, bytes, false) {
                Ok(mut d) => {
                    if m.by_soname(&d.soname).is_some() {
                        continue;
                    }
                    if d.soname == path {
                        d.soname.clone_from(&n);
                    }
                    let needs = d.needed.clone();
                    m.dsos.push(d);
                    let j = m.dsos.len() - 1;
                    for x in needs {
                        q.push_back((j, x));
                    }
                }
                Err(e) => m.problems.push(e),
            }
        }
        // The program's keys.
        m.keys = Self::keys_of(line, objs.iter(), file0);
        // An executable's dynamic list (D-1: none in a static-pie; a
        // `Shared` line's list decides preemption, §12.5).
        if matches!(line.kind, Some(OutputKind::Exec | OutputKind::Pie))
            && !line.dynamic_lists.is_empty()
        {
            m.dynamic_list(line, files);
        }
        for u in line.root_names() {
            m.keys.entry(u.to_string()).or_default().root = true;
        }
        // D-11 at each stage of D-12 (WP-171): N only grows; a stage's
        // sources are the as-needed members of N the output's DT_NEEDED
        // names (the output is what is judged, and the need must be the
        // replay's own, so an unjustified entry licenses nothing).
        let mut acc = Stage::default();
        let mut sources: BTreeSet<usize> = BTreeSet::new();
        let mut staging = staging;
        loop {
            m.stages += 1;
            let st = match staging.as_mut() {
                Some((s, _)) => {
                    let names: BTreeSet<String> =
                        sources.iter().map(|&i| m.dsos[i].soname.clone()).collect();
                    let live = s.stage(objs, &names);
                    let view = Self::keys_of(
                        line,
                        objs.iter().zip(&live).filter(|(_, l)| **l).map(|(o, _)| o),
                        file0,
                    );
                    let s: &dyn Staging = &**s;
                    m.d11(&view, &|n, v| s.lazy_first(n, v), &acc)
                }
                None => m.d11(&m.keys, &|_, _| false, &acc),
            };
            acc.n0.extend(st.n0.iter().copied());
            acc.n.extend(st.n.iter().copied());
            for (g, v) in st.obj_reasons {
                acc.obj_reasons.entry(g).or_default().extend(v);
            }
            for (s, v) in st.dso_reasons {
                acc.dso_reasons.entry(s).or_default().extend(v);
            }
            for (g, v) in st.given {
                acc.given.entry(g).or_insert(v);
            }
            let Some((_, needed)) = staging.as_ref() else { break };
            let next: BTreeSet<usize> = acc
                .n
                .iter()
                .copied()
                .filter(|&i| m.dsos[i].as_needed && needed.iter().any(|x| *x == m.dsos[i].soname))
                .collect();
            if next == sources || m.stages > m.c.len() + 1 {
                break;
            }
            sources = next;
        }
        m.sources = sources;
        if staging.is_some() {
            let all = m.d11(&m.keys, &|_, _| false, &Stage::default());
            m.unreached_needed = all.n.difference(&acc.n).copied().collect();
        }
        let n0 = acc.n0;
        let n = acc.n;
        // Kept (row 2a): an as-needed object item 2 made needed at some
        // stage, no item-3 pair, and every key it gave is defined by the
        // output's own objects (a later stage's member, or lld's output
        // that dropped it).
        m.kept = n
            .iter()
            .copied()
            .filter(|i| {
                m.dsos[*i].as_needed
                    && !acc.dso_reasons.contains_key(i)
                    && acc.given.get(i).is_some_and(|g| {
                        !g.is_empty() && g.iter().all(|k| m.keys.get(k).is_some_and(|x| x.defined))
                    })
            })
            .collect();
        // D-21 item 1 (WP-163): a root the program does not define is
        // imported iff a library of the final needed set defines it (any
        // version or the default one: what a reference without a version
        // binds).
        let imported: Vec<String> = m
            .keys
            .iter()
            .filter(|(name, k)| {
                k.root
                    && !k.defined
                    && !k.strong_ref
                    && n.iter().any(|&i| m.dsos[i].def(name, None).is_some())
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in imported {
            if let Some(k) = m.keys.get_mut(&name) {
                k.strong_ref = true;
                k.root_import = true;
                k.ref_pos.push((0, None));
            }
        }
        m.item3 = n.difference(&n0).copied().collect();
        m.used = m.reach(n.iter().copied()).difference(&n).copied().collect();
        m.needed = n;
        // D-19a: each needed object at its keeping mention, by the reasons
        // of every stage (a kept object's stage-1 reason stays).
        let mut keep: HashMap<usize, usize> = HashMap::new();
        let obj_reasons = acc.obj_reasons;
        let dso_reasons = acc.dso_reasons;
        loop {
            let mut changed = false;
            for &(d, asn, pos, group) in &m.mentions {
                if !m.needed.contains(&d) || keep.contains_key(&d) {
                    continue;
                }
                let qualifies =
                    !asn || obj_reasons.get(&d).is_some_and(|v| {
                        v.iter().any(|&(p, g)| p < pos || (g.is_some() && g == group))
                    }) || dso_reasons
                        .get(&d)
                        .is_some_and(|v| v.iter().any(|x| keep.get(x).is_some_and(|&kp| kp < pos)));
                if qualifies {
                    keep.insert(d, pos);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for &d in &m.needed {
            if !keep.contains_key(&d)
                && let Some(first) = m.mentions.iter().find(|x| x.0 == d)
            {
                keep.insert(d, first.2);
            }
        }
        let mut order: Vec<usize> = m.needed.iter().copied().collect();
        order.sort_by_key(|d| (keep.get(d).copied().unwrap_or(usize::MAX), *d));
        m.order_d19a = order;
        m.order_first = m.c.iter().copied().filter(|d| m.needed.contains(d)).collect();
        // GNU ld's and mold's own neededness, where D-11 records theirs
        // (a reference output is held to its own form).
        let mut gnu_unbound = BTreeSet::new();
        for &s in &n0 {
            if !m.dsos[s].as_needed || dso_reasons.contains_key(&s) {
                continue;
            }
            let at = m.order_d19a.iter().position(|&d| d == s).unwrap_or(m.order_d19a.len());
            let before = &m.order_d19a[..at];
            let bound_before = |k: &String| {
                let ver = m.keys.get(k).and_then(|x| x.version.as_deref());
                before.iter().any(|&d| m.dsos[d].def(k, ver).is_some())
            };
            if acc.given.get(&s).is_none_or(|g| g.iter().all(bound_before)) {
                gnu_unbound.insert(s);
            }
        }
        m.gnu_unbound = gnu_unbound;
        let mut mold_weak = BTreeSet::new();
        for (name, k) in &m.keys {
            if k.weak_ref
                && !k.strong_ref
                && !k.defined
                && let Some(g) = m.giver(name)
                && !m.needed.contains(&g)
            {
                mold_weak.insert(g);
            }
        }
        m.mold_weak = mold_weak;
        // ref_dynamic / def_dynamic over N ∪ U (D-12).
        for &i in m.needed.iter().chain(m.used.iter()) {
            for r in &m.dsos[i].refs {
                m.ref_dynamic.insert(r.name.clone());
            }
            for (name, defs) in &m.dsos[i].defs {
                if defs.iter().any(SoDef::visible) {
                    m.def_dynamic.insert(name.clone());
                }
            }
        }
        m
    }

    /// The program's keys from `objs` and file 0's definitions.
    fn keys_of<'o, 'a: 'o>(
        line: &Line,
        objs: impl Iterator<Item = &'o Obj<'a>>,
        file0: &[String],
    ) -> BTreeMap<String, Key> {
        let mut keys: BTreeMap<String, Key> = BTreeMap::new();
        let wrap = crate::wrap::Wrap::new(line.wrap.iter().cloned());
        // Keys defined in an excluded archive member, and elsewhere
        // (D-110 item 1, WP-161).
        let mut in_excluded: HashSet<String> = HashSet::new();
        let mut in_kept: HashSet<String> = HashSet::new();
        // resolve §6.2 (WP-178): a non-local mention defined in a dropped
        // COMDAT copy (the verifier's own keeper, `fate`, in the line's
        // order) is an undefined
        // mention of its key with its binding: no definition.
        let objs: Vec<&Obj<'a>> = objs.collect();
        let dropped = crate::fate::comdat_dropped_ranked(&objs, |o| {
            o.member.is_none().then(|| line.position_of(&o.path)).flatten()
        });
        for (o, dropped) in objs.into_iter().zip(&dropped) {
            let Some(t) = o.symtab else { continue };
            let excluded = o.member.is_some() && line.exclude_libs.excludes(&o.path);
            let pos = line.position_of(&o.path);
            let group = pos.and_then(|p| match &line.items[p] {
                Item::Relocatable { group, .. } => *group,
                Item::Shared { .. } => None,
            });
            for (i, s) in t.iter() {
                if i.0 == 0 || !matches!(s.binding(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) {
                    continue;
                }
                let Ok(raw) = t.name(&s) else { continue };
                let full = String::from_utf8_lossy(raw).into_owned();
                let (name, ver) = split_version(&full);
                let demoted = t.section_of(i, &s).is_ok_and(|w| match w {
                    SymbolSection::Section(k) => dropped.get(k.0 as usize).copied().unwrap_or(false),
                    _ => false,
                });
                let defined = s.st_shndx != SHN_UNDEF && !demoted;
                // resolve §7.1 (WP-170): an undefined, unversioned mention
                // is its `--wrap` key's reference.
                let name = if s.st_shndx != SHN_UNDEF || ver.is_some() {
                    name.into()
                } else {
                    wrap.key(name)
                };
                let name: &str = &name;
                let k = keys.entry(name.to_string()).or_default();
                if matches!(s.st_other & 3, STV_HIDDEN | STV_INTERNAL) {
                    k.hidden = true;
                }
                if defined {
                    // A versioned definition `foo@@V` defines `foo`.
                    let _ = (SHN_ABS, SHN_COMMON);
                    k.defined = true;
                    if excluded {
                        in_excluded.insert(name.to_string());
                    } else {
                        in_kept.insert(name.to_string());
                    }
                } else {
                    if let Some(v) = ver {
                        k.version = Some(v.to_string());
                    }
                    if s.binding() == STB_WEAK {
                        k.weak_ref = true;
                    } else {
                        k.strong_ref = true;
                        k.ref_pos.push((pos.unwrap_or(0), group));
                    }
                }
            }
        }
        for n in &in_excluded {
            if !in_kept.contains(n)
                && let Some(k) = keys.get_mut(n)
            {
                k.excluded = true;
            }
        }
        // file 0's definitions (before the dynamic list: D-21 item 2's
        // seven names may be listed).
        for n in file0 {
            let k = keys.entry(n.clone()).or_default();
            k.linker = !k.defined;
            k.defined = true;
        }
        keys
    }

    /// `giver(k)` over a stage's `keys`.
    fn giver_in(&self, keys: &BTreeMap<String, Key>, k: &str) -> Option<usize> {
        let key = keys.get(k);
        if key.is_some_and(|x| x.defined) {
            return None;
        }
        let ver = key.and_then(|x| x.version.as_deref());
        self.c.iter().copied().find(|&i| self.dsos[i].def(k, ver).is_some())
    }

    /// D-11 over a stage's `keys`, with every object of `earlier` (the
    /// earlier stages) kept in N. `lazy_first`: D-12's item-3 rule (a
    /// reference whose edge targets a lazy file that is not live gives no
    /// reason: the next stage extracts that file).
    fn d11(
        &self,
        keys: &BTreeMap<String, Key>,
        lazy_first: &dyn Fn(&str, Option<&str>) -> bool,
        earlier: &Stage,
    ) -> Stage {
        let mut st = Stage::default();
        let strong: Vec<&String> =
            keys.iter().filter(|(_, k)| k.strong_ref && !k.defined).map(|(n, _)| n).collect();
        let mut n0: BTreeSet<usize> =
            self.c.iter().copied().filter(|&i| !self.dsos[i].as_needed).collect();
        for k in &strong {
            if let Some(g) = self.giver_in(keys, k) {
                n0.insert(g);
                st.obj_reasons.entry(g).or_default().extend(keys[*k].ref_pos.iter().copied());
                st.given.entry(g).or_default().push((*k).clone());
            }
        }
        let reach_n0 = self.reach(n0.iter().chain(&earlier.n0).copied());
        // The provider of `x`'s reference `r` that item 3 counts.
        let provider = |x: usize, r: &SoRef, rx: &BTreeSet<usize>| {
            if r.weak || keys.get(&r.name).is_some_and(|k| k.defined) {
                return None;
            }
            let s = self
                .c
                .iter()
                .copied()
                .find(|&i| self.dsos[i].def(&r.name, r.version.as_deref()).is_some())?;
            (s != x
                && !reach_n0.contains(&s)
                && !rx.contains(&s)
                && !lazy_first(&r.name, r.version.as_deref()))
            .then_some(s)
        };
        let mut n: BTreeSet<usize> = n0.iter().chain(&earlier.n).copied().collect();
        loop {
            let mut add = BTreeSet::new();
            for &x in &n {
                let rx = self.reach([x]);
                for r in &self.dsos[x].refs {
                    if let Some(s) = provider(x, r, &rx)
                        && !n.contains(&s)
                    {
                        add.insert(s);
                    }
                }
            }
            if add.is_empty() {
                break;
            }
            n.extend(add);
        }
        // D-19a counts a shared object's reference only as D-11 item 3
        // does: a provider outside reach(N0) and reach({x}).
        for &x in &n {
            let rx = self.reach([x]);
            for r in &self.dsos[x].refs {
                if let Some(s) = provider(x, r, &rx) {
                    st.dso_reasons.entry(s).or_default().push(x);
                }
            }
        }
        st.n0 = n0;
        st.n = n;
        st
    }

    /// Marks the defined keys an executable's `--dynamic-list` files list
    /// (every file one list; resolve §9.6, D-112's grammar). A file the
    /// verifier cannot read or parse is P-D2's; a form D-112 refuses
    /// (`extern "C++"`) puts the link outside stage 3 (P-D1).
    fn dynamic_list(&mut self, line: &Line, files: &dyn Files) {
        let mut texts = Vec::new();
        for p in &line.dynamic_lists {
            let Some(b) = files.read(p) else {
                self.problems.push(format!("{p}: cannot be read (a --dynamic-list file)"));
                return;
            };
            texts.push((p.clone(), String::from_utf8_lossy(&b).into_owned()));
        }
        match crate::shared::DynList::parse(&texts) {
            Ok(list) => {
                for (name, k) in &mut self.keys {
                    k.listed = k.defined && list.lists(name);
                }
            }
            Err(crate::shared::ScriptError::Syntax(e)) => {
                self.problems.push(format!("a dynamic list the verifier cannot read: {e}"));
            }
            Err(crate::shared::ScriptError::Refused(e)) => {
                self.outside.push(format!("{e} (dyn.md D-112)"));
            }
        }
    }

    /// The search of input §8.6 for edge `n` of object `s`.
    fn search(
        &self,
        line: &Line,
        s: usize,
        n: &str,
        files: &dyn Files,
    ) -> Option<(String, Vec<u8>)> {
        if n.contains('/') {
            return files.read(n).map(|b| (n.to_string(), b));
        }
        let sp = &self.dsos[s].path;
        let origin = sp.rsplit_once('/').map_or(".", |(d, _)| d).to_string();
        let mut dirs: Vec<String> = line.rpath_link.clone();
        dirs.extend(line.rpath.iter().cloned());
        let rp = self.dsos[s].runpath.clone().or_else(|| self.dsos[s].rpath.clone());
        if let Some(rp) = rp {
            for d in rp.split(':').filter(|d| !d.is_empty()) {
                dirs.push(d.replace("${ORIGIN}", &origin).replace("$ORIGIN", &origin));
            }
        }
        dirs.push(origin);
        dirs.extend(line.lib_dirs.iter().cloned());
        let machine = self.dsos[s].machine;
        for d in dirs.iter().filter(|d| !d.is_empty()) {
            let p = format!("{d}/{n}");
            if let Some(b) = files.read(&p) {
                // §4.4: a candidate of another machine is skipped.
                if b.len() > 20 && u16::from_le_bytes([b[18], b[19]]) == machine {
                    return Some((p, b));
                }
            }
        }
        None
    }

    /// `bind(k)` over a loader order: the first object in `order` (members
    /// of C only, D-12) with a visible definition at `ver`. In `Shared`
    /// members of N only (D-10, WP-137): a weak-only key whose every
    /// definition lies in an object that is not needed stays unbound.
    pub fn bind_in(&self, order: &[usize], k: &str, ver: Option<&str>) -> Option<(usize, &SoDef)> {
        order
            .iter()
            .copied()
            .filter(|&i| self.dsos[i].in_c && (!self.shared || self.needed.contains(&i)))
            .find_map(|i| self.dsos[i].def(k, ver).map(|d| (i, d)))
    }

    /// The loader's lookup (ld.so): every object in `order`, closure files
    /// included (at run time the loader does not know the command line).
    pub fn lookup(&self, order: &[usize], k: &str, ver: Option<&str>) -> Option<(usize, &SoDef)> {
        order.iter().copied().find_map(|i| {
            let d = &self.dsos[i];
            let def = match ver {
                Some(_) => d.def(k, ver),
                // An unversioned lookup takes the default version, or an
                // unversioned definition (glibc `check_match`).
                None => d.defs.get(k)?.iter().find(|x| x.visible() && x.default),
            };
            def.map(|x| (i, x))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_split_from_the_name() {
        assert_eq!(split_version("memcpy@GLIBC_2.2.5"), ("memcpy", Some("GLIBC_2.2.5")));
        assert_eq!(split_version("f@@V2"), ("f", Some("V2")));
        assert_eq!(split_version("puts"), ("puts", None));
    }
}
