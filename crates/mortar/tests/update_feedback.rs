//! WP-215's soundness test of the update's closure check (`spec/incr.md`
//! IN-71, WP-215 §Q.2; rulings on WP-215): a few thousand small generated
//! links with COMDAT copies of mismatched symbol sets, an edit each,
//! resolved by `mortar_resolve` itself (no output). Whenever the FULL link
//! of the new inputs iterates (resolve.md §6.2), the update fell back.
//! The model the update is run on (`mortar_incr`'s
//! `tests/support/resolve_model.rs`) is checked against `mortar_resolve`
//! on every link first: the same live files, iteration and definers.

#[allow(dead_code, clippy::all)]
mod model {
    include!("../../mortar_incr/tests/support/resolve_model.rs");

    use mortar_core::par::Pool;
    use mortar_elf::consts::{GRP_COMDAT, SHF_ALLOC, SHF_EXECINSTR, SHT_PROGBITS};
    use mortar_elf::{Machine, SectionIndex, SymbolSection};
    use mortar_resolve::options::{Entry, Named, Options};
    use mortar_resolve::view::{FileInfo, FileKind, Groups, Mention, SectionInfo, View};
    use mortar_resolve::{Def, Resolved};

    /// A `mortar_resolve` view of a model link: files in load order (by
    /// rank), each member in an archive of its own.
    struct Real {
        files: Vec<RFile>,
        names: Vec<String>,
        sigs: Vec<String>,
        /// Model file of each view file.
        of: Vec<u32>,
    }

    struct RFile {
        info: FileInfo,
        mentions: Vec<Mention>,
        groups: Groups,
        sections: Vec<(Vec<u8>, u32, u64)>,
    }

    impl Real {
        fn key(&mut self, name: &str) -> mortar_core::SymbolId {
            let p = self.names.iter().position(|n| n == name).unwrap_or_else(|| {
                self.names.push(name.to_owned());
                self.names.len() - 1
            });
            mortar_core::SymbolId(p as u32)
        }

        fn new(link: &Link) -> Self {
            let mut r = Self { files: Vec::new(), names: Vec::new(), sigs: Vec::new(), of: Vec::new() };
            let mut order: Vec<(u64, u32)> = Vec::new();
            for (i, f) in link.files.iter().enumerate().skip(1) {
                if let Some(f) = f {
                    order.push((f.rank, i as u32));
                }
            }
            order.sort_unstable();
            r.files.push(RFile {
                info: FileInfo {
                    kind: FileKind::Internal,
                    archive: mortar_core::OptId::NONE,
                    group: None,
                    sections: 0,
                    source: 0,
                },
                mentions: Vec::new(),
                groups: Groups::default(),
                sections: Vec::new(),
            });
            r.of.push(0);
            let mut archives = 0;
            for (_, f) in order {
                let lf = link.file(f).unwrap();
                let id = r.files.len() as u32;
                let archive = if lf.member {
                    archives += 1;
                    mortar_core::OptId::some(mortar_core::ArchiveId(archives - 1))
                } else {
                    mortar_core::OptId::NONE
                };
                let mut sections: Vec<(Vec<u8>, u32, u64)> = vec![
                    (Vec::new(), 0, 0),
                    (b".text".to_vec(), SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR),
                ];
                let mut groups = Groups::default();
                let mut sec_of: BTreeMap<String, u32> = BTreeMap::new();
                for s in lf.sections().into_iter().skip(1) {
                    let text = sections.len() as u32;
                    sections.push((
                        format!(".text.{s}").into_bytes(),
                        SHT_PROGBITS,
                        SHF_ALLOC | SHF_EXECINSTR | mortar_elf::consts::SHF_GROUP,
                    ));
                    let g = sections.len() as u32;
                    sections.push((b".group".to_vec(), mortar_elf::consts::SHT_GROUP, 0));
                    let sig = match r.sigs.iter().position(|x| *x == s) {
                        Some(p) => p,
                        None => {
                            r.sigs.push(s.clone());
                            r.sigs.len() - 1
                        }
                    };
                    groups.push(
                        SectionIndex(g),
                        GRP_COMDAT,
                        mortar_core::ComdatId(sig as u32),
                        &[text],
                    );
                    sec_of.insert(s, text);
                }
                let mut mentions = Vec::new();
                for (i, m) in lf.ments.iter().enumerate() {
                    let place = match m.kind {
                        def_kind::UNDEFINED => SymbolSection::Undefined,
                        def_kind::COMMON => SymbolSection::Common,
                        def_kind::ABSOLUTE => SymbolSection::Absolute,
                        _ if m.sig.is_empty() => SymbolSection::Section(SectionIndex(1)),
                        _ => SymbolSection::Section(SectionIndex(sec_of[&m.sig])),
                    };
                    let (place, large) = mortar_resolve::view::place_of(Machine::AArch64, place);
                    let key = r.key(&m.key);
                    mentions.push(Mention {
                        index: mortar_elf::SymbolIndex(i as u32 + 1),
                        key,
                        also: mortar_core::OptId::NONE,
                        binding: m.bind,
                        sym_type: m.typ,
                        visibility: m.vis,
                        other: 0,
                        place,
                        value: m.value,
                        size: m.size,
                        large,
                    });
                }
                r.files.push(RFile {
                    info: FileInfo {
                        kind: if lf.member { FileKind::LazyMember } else { FileKind::Object },
                        archive,
                        group: None,
                        sections: sections.len() as u32,
                        source: id,
                    },
                    mentions,
                    groups,
                    sections,
                });
                r.of.push(f);
            }
            for k in &link.roots {
                r.key(k);
            }
            r
        }

        fn resolve(&mut self, link: &Link) -> Resolved {
            let undefined: Vec<Named> = link
                .roots
                .iter()
                .enumerate()
                .map(|(i, k)| Named {
                    key: mortar_core::OptId::some(self.key(k)),
                    name: k.as_bytes().into(),
                    arg: i as u32 + 1,
                })
                .collect();
            let opts = Options { entry: Entry::None, undefined, ..Options::default() };
            mortar_resolve::resolve(&*self, &opts, &Pool::sequential())
        }
    }

    impl View for Real {
        fn machine(&self) -> Machine {
            Machine::AArch64
        }
        fn files(&self) -> u32 {
            self.files.len() as u32
        }
        fn keys(&self) -> u32 {
            self.names.len() as u32
        }
        fn comdats(&self) -> u32 {
            self.sigs.len() as u32
        }
        fn file(&self, file: FileId) -> FileInfo {
            self.files[file.0 as usize].info
        }
        fn mentions(&self, file: FileId, out: &mut Vec<Mention>) {
            out.extend_from_slice(&self.files[file.0 as usize].mentions);
        }
        fn mention(&self, file: FileId, index: mortar_elf::SymbolIndex) -> Option<Mention> {
            self.files[file.0 as usize].mentions.iter().find(|m| m.index == index).copied()
        }
        fn groups(&self, file: FileId, out: &mut Groups) {
            let g = &self.files[file.0 as usize].groups;
            for info in &g.groups {
                out.push(info.section, info.flags, info.signature, g.members_of(info));
            }
        }
        fn section(&self, file: FileId, index: SectionIndex) -> Option<SectionInfo<'_>> {
            let s = self.files[file.0 as usize].sections.get(index.0 as usize)?;
            Some(SectionInfo {
                name: &s.0,
                sh_type: s.1,
                flags: s.2,
                entsize: 0,
                addralign: if s.1 == mortar_elf::consts::SHT_GROUP { 4 } else { 8 },
                size: if s.1 == mortar_elf::consts::SHT_GROUP { 8 } else { 16 },
                link: 0,
                info: 0,
            })
        }
        fn key_name(&self, key: mortar_core::SymbolId) -> (&[u8], Option<&[u8]>) {
            (self.names[key.0 as usize].as_bytes(), None)
        }
        fn find_key(&self, name: &[u8]) -> Option<mortar_core::SymbolId> {
            self.names.iter().position(|n| n.as_bytes() == name).map(|p| mortar_core::SymbolId(p as u32))
        }
        fn shared(&self, _: FileId) -> Option<mortar_resolve::view::SharedInfo> {
            None
        }
        fn needed(&self, _: FileId, _: &mut Vec<Option<FileId>>) {}
        fn version(&self, _: FileId, _: mortar_elf::SymbolIndex) -> u16 {
            0
        }
    }

    /// `mortar_resolve` on `link`: whether it iterated; the model agrees
    /// with it (live files, iteration, definers and their classes).
    fn real(link: &Link, what: &str) -> bool {
        let mut v = Real::new(link);
        let r = v.resolve(link);
        let full = link.full();
        let live: BTreeSet<u32> = r
            .file_live
            .iter()
            .enumerate()
            .filter(|(i, l)| *i != 0 && l.is_live())
            .map(|(i, _)| v.of[i])
            .collect();
        let want: BTreeSet<u32> = full.live.keys().copied().filter(|&f| f != 0).collect();
        assert_eq!(r.iterations > 1, full.iterated, "{what}: iteration");
        // The model does not iterate (the update falls back instead): its
        // live set and rows are the first iteration's.
        if full.iterated {
            return true;
        }
        assert_eq!(live, want, "{what}: live files, mortar_resolve vs the model");
        if full.clash.is_empty() {
            for (k, row) in &full.rows {
                let Some(id) = v.find_key(k.as_bytes()) else { continue };
                let def = r.def[id.0 as usize];
                let got = match def {
                    Def::Strong(e) => (1, Some(v.of[e.file.0 as usize])),
                    Def::Common(e) => (3, Some(v.of[e.file.0 as usize])),
                    Def::Weak(e) => (2, Some(v.of[e.file.0 as usize])),
                    Def::Undefined { weak } => (if weak { 5 } else { 4 }, None),
                    other => panic!("{what}: {k}: {other:?}"),
                };
                let want = match row.kind {
                    def_kind::COMMON => (3, full.by.get(k).copied()),
                    def_kind::UNDEFINED => (if row.bind == WEAK { 5 } else { 4 }, None),
                    _ => (if row.bind == WEAK { 2 } else { 1 }, full.by.get(k).copied()),
                };
                assert_eq!(got, want, "{what}: the definition of {k}");
            }
        }
        r.iterations > 1
    }

    // spec: incr.md IN-71 (WP-215 rulings: `feedback_check_is_sound_random`;
    // condition 1(b) of seam 3's review): whenever the full link of the
    // new inputs iterates, the update falls back.
    #[test]
    fn feedback_check_is_sound_random() {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        let total = 3000;
        let (mut iterated, mut caught, mut ok) = (0, 0, 0);
        for i in 0..total {
            let keys = 2 + rng.below(4);
            let mut old = Link::new();
            let n = 2 + rng.below(6);
            for j in 0..n {
                let member = rng.below(2) == 0;
                let ments = random_resolve_ments(&mut rng, keys);
                old.add((j + 1) * R, member, ments);
            }
            if rng.below(3) == 0 {
                old.roots.push(format!("k{}", rng.below(keys)));
            }
            let mut new = old.clone();
            for _ in 0..1 + rng.below(2) {
                random_edit(&mut rng, &mut new, n, keys);
            }
            let what = format!("link {i}: old {old:#?}\nnew {new:#?}");
            if real(&old, &what) {
                continue;
            }
            let iterates = real(&new, &what);
            let j = join(&old, &new);
            if iterates {
                iterated += 1;
                assert!(j.result.is_err(), "the full link iterates, the update did not fall back: {what}");
                caught += usize::from(matches!(j.result, Err(IncrReason::ClosureIterated(_))));
            } else if j.result.is_ok() {
                ok += 1;
            }
        }
        eprintln!("feedback_check_is_sound_random: {iterated} new links iterated ({caught} closure-iterated), {ok} updated");
        assert!(iterated >= 20, "only {iterated} iterating links: the series does not test the check");
    }
}
