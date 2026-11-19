//! WP-255d2c: a CHANGED file's section facts from their ONE producers
//! (`mortar::update::file_split::changed_facts`: resolve's per-object body
//! with the update's keepers, `assign::fate` with the state's `OUTSECS`),
//! and `split_of` on the changed file with THOSE facts.
//!
//! `changed_facts_and_split_of_an_edited_file`: `m1.o` of `lib2.a` edited
//! into C++ with string literals (a merge section), a COMDAT group kept
//! (`wp255d_inl3`, only here) and one dropped (`wp255d_inl`, kept by the
//! earlier member `m8.o`), and `.eh_frame`; opened through the OLD state
//! with `Ids::Update`'s new `FILES` row. The keepers are the full link's of
//! the new inputs (D2-6's producer is checked against the model's full
//! link in `mortar_incr`'s `update_resolve`). Each fact equals the whole
//! front's `Link::section` content for that section (kind, kept, output
//! section by NAME: the update's id is the state's `OUTSECS` row); the
//! split equals the whole front's pieces, sections placed whole,
//! `.eh_frame` sections, records and relocations for that file.
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;
#[path = "support/update.rs"]
mod update;

use std::collections::BTreeMap;

use mortar::front::Front;
use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar::update::file_split::{SectionFact, changed_facts, split_of};
use mortar_core::OutSecId;
use mortar_core::id::{FileId, SectionId, SymbolId};
use mortar_core::identity::Anchor;
use mortar_core::resolution::SectionKind;
use mortar_elf::SectionIndex;
use mortar_incr::update::rows::{FileRow, FileTable};
use mortar_incr::update::view::StateView;
use mortar_merge::eh::{Code, Kind, Record, Target};
use mortar_merge::phase::ArchRelocs;
use update::{first_free, lines, member, pool, same_file, state, whole};

/// The edited `m1.o` (C++): strings (`.rodata.str1.1`, a merge section),
/// `wp255d_inl` (a COMDAT copy `m8.o` keeps: dropped here), `wp255d_inl3`
/// (kept here), `.eh_frame`.
const M1_SPLIT: &str = "extern \"C\" { int wp255d_m1_data = 1; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl3(int x) { return x * 7 + wp255d_m1_data; }\n\
    extern \"C\" const char *wp255d_str(int i) { static const char *const t[] = \
    { \"wp255d2c-alpha\", \"wp255d2c-beta\", \"wp255d2c-gamma\" }; return t[i % 3]; }\n\
    extern \"C\" int wp255d_m1(int x) { return wp255d_inl(x) + wp255d_inl3(x) + *wp255d_str(x); }\n";
/// The earlier member with the same `wp255d_inl` (same tokens: one ODR
/// definition): it keeps the group.
const M8: &str = "extern \"C\" { extern int wp255d_m1_data; }\n\
    inline __attribute__((noinline)) int wp255d_inl(int x) { return x * 3 + wp255d_m1_data; }\n\
    extern \"C\" int wp255d_m8(int x) { return wp255d_inl(x) + 8; }\n";

/// The state's file of the front's `file` (same path and member name).
fn state_file(f: &Front<'_>, view: &StateView<'_>, file: FileId) -> Option<FileId> {
    let x = &f.inputs.files[file];
    let member = x.member.map_or(&b""[..], |m| m.name);
    let files = u32::try_from(view.count(mortar_incr::format::kind::FILES)).expect("FILES");
    (1..files).map(FileId).find(|&id| {
        let r = view.file(id).expect("FILES");
        view.string(r.path).expect("path") == x.path
            && view.string(r.member_name).expect("member") == member
    })
}

/// A section by (state file, index): the update's ids of the changed file
/// (`first + k`), the stored ones of the others, the front's.
type At = (u32, u32);

#[derive(Debug, Clone, PartialEq, Eq)]
enum T {
    Section(At, u64),
    Absolute(u64),
    Undefined(Vec<u8>, bool),
    Elsewhere(Vec<u8>),
}

struct Ids2<'a, 'f, 's> {
    f: &'a Front<'f>,
    view: &'a StateView<'s>,
    file: FileId,
    first: u32,
}

impl Ids2<'_, '_, '_> {
    fn update_at(&self, s: SectionId) -> At {
        if s.0 >= self.first {
            return (self.file.0, s.0 - self.first);
        }
        let row = self.view.section(s).expect("SECTIONS");
        let first = self.view.file_index(FileId(row.file)).expect("FILE_INDEX").ranges
            [FileTable::Sections as usize]
            .first;
        (row.file, s.0 - first)
    }

    fn front_at(&self, s: SectionId) -> At {
        let (file, i) = self.f.resolution.sh_index(s).expect("a front section");
        (state_file(self.f, self.view, file).expect("the file in the state").0, i.0)
    }

    fn name(&self, id: SymbolId, update: bool) -> Vec<u8> {
        if update {
            let r = self.view.symbol(id).expect("SYMBOLS");
            self.view.string(r.name).expect("name")
        } else {
            self.f.inputs.symbols[id].name.to_vec()
        }
    }

    fn target(&self, t: Target, update: bool) -> T {
        match t {
            Target::Section { section, value } => {
                let at = if update { self.update_at(section) } else { self.front_at(section) };
                T::Section(at, value)
            }
            Target::Absolute(v) => T::Absolute(v),
            Target::Undefined { symbol, weak } => T::Undefined(self.name(symbol, update), weak),
            Target::Elsewhere(id) => T::Elsewhere(self.name(id, update)),
        }
    }

    fn record(&self, r: &Record, update: bool) -> (Record, Option<At>) {
        let mut r = *r;
        let mut at = None;
        match &mut r.kind {
            Kind::Cie(c) => c.class = 0,
            Kind::Fde(fde) => {
                if let Code::Section { section, .. } = &mut fde.code {
                    at = Some(if update {
                        self.update_at(*section)
                    } else {
                        self.front_at(*section)
                    });
                    *section = SectionId(0);
                }
            }
        }
        (r, at)
    }
}

#[test]
fn changed_facts_and_split_of_an_edited_file() {
    for line in lines(&[("m1s", "c++", M1_SPLIT), ("m8", "c++", M8)]) {
        let args = line.args(&["wp255d_m8", "wp255d_m2", "wp255d_m1"]);
        line.archive(&["m8.o", "m1.o", "m2.o"]);
        let old = state(&line, &args, "old");
        std::fs::copy(line.dir.join("m1s.o"), line.dir.join("m1.o")).expect("the edit");
        line.archive(&["m8.o", "m1.o", "m2.o"]);
        let new = state(&line, &args, "new");
        let store = support::store();
        let f = whole(&line, &args, &store);
        let file = member(&old, "m1.o");
        let captured = new.file(member(&new, "m1.o")).expect("FILES");
        let stored = old.file(file).expect("FILES");
        let row = FileRow { base: captured.base, len: captured.len, hash: captured.hash, ..stored };
        let first = first_free(&old);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff =
            front_of(&mut cache, &old, file, Ids::Update { first_section: SectionId(first), row })
                .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let nf = same_file(&f, &old, file);
        // The keepers: the full link's of the new inputs, in the state's
        // file ids, of every signature the file's sections name.
        let mut keepers: Vec<(Vec<u8>, Option<FileId>)> = Vec::new();
        for s in &ff.sections {
            let Anchor::Comdat(sig) = s.anchor else { continue };
            if keepers.iter().any(|k| k.0 == sig) {
                continue;
            }
            let id = (0..f.inputs.comdats.len())
                .map(mortar_core::ComdatId)
                .find(|&c| f.inputs.comdats[c].name == sig)
                .expect("the signature in the new front");
            let k = f.resolution.comdat[id].get().map(|k| state_file(&f, &old, k).expect("keeper"));
            keepers.push((sig.to_vec(), k));
        }
        keepers.sort();
        let facts = changed_facts(ff, &old, &f.plan, &keepers)
            .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let what = |s: &str| format!("{}: m1.o: {s}", line.label);
        // Each fact against the whole front's `Link::section` content.
        let outsecs = old.outsecs().expect("OUTSECS");
        let state_out = |o: OutSecId| old.string(outsecs[o.0 as usize].name).expect("name");
        let (mut dropped, mut kept_comdat, mut same_ids) = (0, 0, true);
        assert_eq!(facts.len() as u32, ff.object.section_count(), "{}", what("one fact per index"));
        for (i, fact) in facts.iter().enumerate() {
            let id = f.resolution.section_id(nf, SectionIndex(i as u32)).expect("a front id");
            let r = f.resolution.sections[id];
            let want_out = f.assignment.out(id);
            assert_eq!(
                (fact.kind, fact.kept, fact.reloc),
                (r.kind, r.is_kept(), r.reloc),
                "{}",
                what(&format!("section {i}"))
            );
            assert_eq!(
                fact.out.map(state_out),
                want_out.map(|o| f.assignment.outsecs[o].name.to_vec()),
                "{}",
                what(&format!("section {i}: output section"))
            );
            same_ids &= fact.out == want_out;
            if matches!(ff.sections[i].anchor, Anchor::Comdat(_)) {
                if fact.kept { kept_comdat += 1 } else { dropped += 1 }
            }
        }
        let null =
            SectionFact { kind: SectionKind::Null, kept: false, out: None, reloc: SectionIndex(0) };
        let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
        // F-9: the changed file's globals answer from the update's
        // resolution (U4-U5 over the new link's capture).
        let capture = mortar_incr::state5::read::decode(
            &std::fs::read(line.dir.join("new.state5")).expect("the new state"),
        )
        .expect("the new state decodes");
        assert_eq!(nf, file, "{}", what("the same FILES row in both links"));
        let (cf, delta) =
            mortar::update::changed::u5(&old, &[file], &[], &f, &capture, &mut |_| {})
                .unwrap_or_else(|e| panic!("{}", what(&format!("U4-U5: {}", e.report()))))
                .expect("m1.o changed");
        assert_eq!(cf.files[0].first_section, SectionId(first), "{}", what("the ids"));
        let got = split_of(ff, &old, &by_index, &ArchRelocs(f.arch.get()), &f.pool, Some(&delta))
            .unwrap_or_else(|e| panic!("{}", what(&format!("{e:?}"))));
        let x = Ids2 { f: &f, view: &old, file, first };
        let range = f.resolution.section_base[nf];
        let mine = |s: SectionId| s.0 >= range.start.0 && s.0 < range.start.0 + range.len();
        let split = &f.merge.split;
        let mut want = Vec::new();
        for row in split.merge_of.iter().filter(|r| mine(r.section)) {
            for p in row.pieces.slots() {
                let frag = split.frags[split.piece_frag[p]];
                let out =
                    f.assignment.out(row.section).map(|o| f.assignment.outsecs[o].name.to_vec());
                want.push((
                    x.front_at(row.section),
                    u64::from(split.piece_off[p]),
                    u64::from(frag.size),
                    out,
                ));
            }
        }
        let have: Vec<_> = got
            .pieces
            .iter()
            .map(|p| (x.update_at(p.section), p.offset, p.size, Some(state_out(p.out))))
            .collect();
        assert_eq!(have, want, "{}", what("pieces"));
        let whole_want: Vec<_> = f
            .merge
            .whole
            .iter()
            .filter(|(s, _)| mine(*s))
            .map(|&(s, w)| (x.front_at(s), w))
            .collect();
        let whole_got: Vec<_> = got.whole.iter().map(|&(s, w)| (x.update_at(s), w)).collect();
        assert_eq!(whole_got, whole_want, "{}", what("placed whole"));
        let eh = &f.merge.eh;
        let rows: Vec<usize> =
            (0..eh.sections.len()).filter(|&i| mine(eh.sections[i].section)).collect();
        assert_eq!(got.eh.sections.len(), rows.len(), "{}", what("eh sections"));
        let mut records = 0;
        for (k, &i) in rows.iter().enumerate() {
            let (w, g) = (&eh.sections[i], &got.eh.sections[k]);
            assert_eq!(
                (x.update_at(g.section), g.size, g.terminator),
                (x.front_at(w.section), w.size, w.terminator),
                "{}",
                what("eh section")
            );
            let wr: Vec<_> =
                eh.records[w.records.slots()].iter().map(|r| x.record(r, false)).collect();
            let gr: Vec<_> =
                got.eh.records[g.records.slots()].iter().map(|r| x.record(r, true)).collect();
            assert_eq!(gr, wr, "{}", what("eh records"));
            let rel = |r: &mortar_merge::eh::EhReloc, u: bool| {
                (r.offset, r.r_type, r.addend, r.width, x.target(r.target, u))
            };
            let wl: Vec<_> = f.merge.eh_relocs[i].iter().map(|r| rel(r, false)).collect();
            let gl: Vec<_> = got.eh_relocs[k].iter().map(|r| rel(r, true)).collect();
            assert_eq!(gl, wl, "{}", what("eh relocations"));
            records += gr.len();
        }
        let seen: BTreeMap<&str, usize> = [
            ("sections", facts.len()),
            ("comdat dropped", dropped),
            ("comdat kept", kept_comdat),
            ("pieces", have.len()),
            ("whole", whole_got.len()),
            ("eh sections", rows.len()),
            ("eh records", records),
        ]
        .into_iter()
        .collect();
        eprintln!(
            "{}: m1.o edited: {seen:?}; output section ids equal the full link's: {same_ids}",
            line.label
        );
        assert!(dropped > 0 && kept_comdat > 0, "{}", what("a kept and a dropped COMDAT copy"));
        assert!(!have.is_empty() && records > 0, "{}", what("pieces and .eh_frame records"));
    }
}
