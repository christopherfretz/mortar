//! WP-239 probe (design experiment, not a gate): does the PER-FILE
//! producer of seam 4's `MergedPiece` (`mortar_merge::cut::placed_whole`,
//! `cut::cut`, L128 by `mortar_digest::l128`) give exactly the full link's
//! pieces and fragments, and is `(out, class, L128)` one-to-one with the
//! full link's `FragId` (finding S4-1)?
//!
//! Every test is `#[ignore]`d. Run by hand:
//!
//! ```text
//! WP239_OBJS="a.o b.o lib.a ..." cargo test -p mortar --test wp239_probe -- --ignored --nocapture
//! ```
//!
//! The objects (archive members included, each member a file) are merged
//! as one link would merge them with no `--gc-sections` and every section
//! kept: `mortar_merge::merge::merge` over every split `SHF_MERGE`
//! section (the full link's function), the output section being the
//! input section's name with the usual prefixes (`.rodata.str1.1` ->
//! `.rodata`; non-alloc sections keep their name). The test does not
//! drop COMDAT copies (a not-kept copy's pieces would be dead in a real
//! link): it compares two computations over the same section set.

use std::collections::{BTreeMap, HashMap, HashSet};

use mortar_core::par::Pool;
use mortar_core::{FileId, FragId, OutSecId, SectionId};
use mortar_elf::SectionIndex;
use mortar_elf::archive::Archive;
use mortar_elf::consts::{SHF_ALLOC, SHF_MERGE, SHF_STRINGS};
use mortar_elf::object::Object;
use mortar_merge::cut;
use mortar_merge::merge::{MergeIn, merge};

const SHF_COMPRESSED: u64 = 0x800;
const SHT_REL: u32 = 9;
const SHT_RELA: u32 = 4;

fn files() -> Vec<(String, Vec<u8>)> {
    let list = std::env::var("WP239_OBJS").unwrap_or_default();
    let mut out = Vec::new();
    for path in list.split_whitespace() {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        if bytes.starts_with(b"!<arch>\n") {
            let ar = Archive::parse(&bytes).expect("archive");
            for (k, m) in ar.members().enumerate() {
                let m = m.expect("member");
                if let Some(d) = m.data().filter(|d| d.starts_with(b"\x7fELF")) {
                    out.push((format!("{path}({k})"), d.to_vec()));
                }
            }
        } else {
            out.push((path.to_owned(), bytes));
        }
    }
    out
}

fn out_name(name: &[u8], alloc: bool) -> Vec<u8> {
    if !alloc {
        return name.to_vec();
    }
    for p in [&b".rodata"[..], b".data.rel.ro", b".data", b".text"] {
        if name == p || name.starts_with(&[p, b"."].concat()) {
            return p.to_vec();
        }
    }
    name.to_vec()
}

/// One split section: (file, out, strings, entsize, align log2, bytes).
type Split = (u32, u32, bool, u32, u8, Vec<u8>);

#[test]
#[ignore = "WP-239 design probe: needs WP239_OBJS"]
fn per_file_pieces_and_identity_equal_the_full_merge() {
    type Ident = (u32, bool, u32, u8, [u8; 16]);
    let files = files();
    assert!(!files.is_empty(), "set WP239_OBJS");
    let mut outs: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    let mut split: Vec<Split> = Vec::new();
    let mut whole = 0usize;
    for (f, (path, bytes)) in files.iter().enumerate() {
        let o = Object::parse(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let relocated: HashSet<u32> = o
            .sections()
            .filter(|s| matches!(s.sh_type(), SHT_REL | SHT_RELA))
            .filter_map(|s| s.info_section().ok().map(|i| i.0))
            .collect();
        for i in 1..o.section_count() {
            let s = o.section(SectionIndex(i)).expect("section");
            let h = *s.header();
            if h.sh_flags & SHF_MERGE == 0 || h.sh_entsize == 0 || h.sh_flags & SHF_COMPRESSED != 0
            {
                continue;
            }
            if cut::placed_whole(h.sh_flags, h.sh_addralign, h.sh_entsize, relocated.contains(&i))
                .is_some()
            {
                whole += 1;
                continue;
            }
            let name = out_name(s.name().expect("name"), h.sh_flags & SHF_ALLOC != 0);
            let n = outs.len() as u32;
            let out = *outs.entry(name).or_insert(n);
            split.push((
                f as u32,
                out,
                h.sh_flags & SHF_STRINGS != 0,
                h.sh_entsize as u32,
                cut::align_log2(h.sh_addralign),
                s.data().expect("resident").to_vec(),
            ));
        }
    }
    // The full link's merge (S-13..S-15) over every split section.
    let ins: Vec<MergeIn<'_>> = split
        .iter()
        .enumerate()
        .map(|(k, s)| MergeIn {
            section: SectionId(k as u32),
            file: FileId(s.0),
            index: k as u32,
            out: OutSecId(s.1),
            strings: s.2,
            entsize: s.3,
            align: s.4,
            bytes: &s.5,
        })
        .collect();
    let merged = merge(&Pool::new(4), &ins).expect("no problem");
    // The per-file producer: cut each section alone, identity (out, class, L128).
    let mut by_ident: HashMap<Ident, FragId> = HashMap::new();
    let mut pairs: HashSet<(u32, Ident)> = HashSet::new();
    let (mut pieces, mut offsets_differ, mut ident_conflicts) = (0usize, 0usize, 0usize);
    let mut row = 0usize;
    for (k, s) in split.iter().enumerate() {
        let mut offs = Vec::new();
        cut::cut(&s.5, s.2, s.3 as usize, &mut offs).expect("cut");
        if offs.is_empty() {
            continue;
        }
        let r = &merged.merge_of[row];
        assert_eq!(r.section, SectionId(k as u32), "merge_of row order");
        row += 1;
        let theirs =
            &merged.piece_off[r.pieces.start as usize..(r.pieces.start + r.pieces.len) as usize];
        if theirs != offs.as_slice() {
            offsets_differ += 1;
            continue;
        }
        for (j, &o) in offs.iter().enumerate() {
            let end = offs.get(j + 1).map_or(s.5.len(), |&e| e as usize);
            let ident = (s.1, s.2, s.3, s.4, mortar_digest::l128(&s.5[o as usize..end]));
            let frag = merged.piece_frag[r.pieces.start as usize + j];
            pieces += 1;
            pairs.insert((s.0, ident));
            if *by_ident.entry(ident).or_insert(frag) != frag {
                ident_conflicts += 1;
            }
        }
    }
    let frags = merged.frags.len() as usize;
    println!(
        "wp239 probe: {} files, {} split sections ({whole} placed whole), {pieces} pieces, {} (file, fragment) pairs, {frags} fragments, {} identities; offsets differ in {offsets_differ} sections; identity conflicts {ident_conflicts}",
        files.len(),
        split.len(),
        pairs.len(),
        by_ident.len()
    );
    for (name, id) in &outs {
        let n = by_ident.keys().filter(|i| i.0 == *id).count();
        let p = pairs.iter().filter(|p| p.1.0 == *id).count();
        println!("  out {} frags {n} pairs {p}", String::from_utf8_lossy(name));
    }
    assert_eq!(offsets_differ, 0, "cut::cut per section = the merge's pieces");
    assert_eq!(ident_conflicts, 0, "one (out, class, L128) names two FragIds");
    assert_eq!(by_ident.len(), frags, "(out, class, L128) is one-to-one with FragId");
}
