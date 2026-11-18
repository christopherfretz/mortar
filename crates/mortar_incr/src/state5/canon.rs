//! The canonical form of a state (`spec/incr.md` IN-78: `canon(S') ==
//! canon(C(U))`), for H-1's judge and `incr_state canon`. WP-213 (U-1):
//! the types [`Canon`], [`CanonDiff`] and the entry points [`canon`],
//! [`compare`]; filled by U-2 (WP-214).
//!
//! `canon` is the compaction's renumbering (`super::compact::normalize`:
//! gone files, tombstones, `GONE` keys, slack and append areas dropped;
//! files by rank position, keys in IN-18e's order, which names a key by
//! its first mentioner and index or its name, sections by (file,
//! position), strings re-interned in table order so equal bytes give equal
//! offsets), then each table's rows encoded, key-keyed tables sorted by
//! key (`FRAGS` by `(out, class, hash)`, each file's `FRAG_USE` rows by
//! that key), dead keys dropped (IN-71b, [`super::live_keys`], through
//! `normalize`). IN-78's exceptions:
//! `LEAVES` dropped; `META`'s binding (build id) and history fields (boot
//! id, `output-synced`, compactions) zeroed, and the `STAT` output row's
//! tuple; `FILES.round` replaced by 0 (the support property is NOT
//! checked here: H-1's, a finding of WP-214).

use mortar_core::incr::StoredLayout;

use super::{Keyed, Rec, StrFn, encode_rows};
use crate::format::{StateError, Str, kind, stat_tag};
use crate::update::testing::MemoryState;

/// A state in canonical form (IN-78: files by rank position, keys by
/// name, dead rows, slack and append areas dropped, `META`'s history
/// fields left out). Opaque: U-2 decides its contents; H-1 only compares
/// two.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Canon {
    /// The canonical rows, by table kind (U-2's encoding).
    pub tables: Vec<(u32, Vec<Vec<u8>>)>,
}

/// One difference between two canonical states: the table, the row (its
/// canonical key, printable), and what differs (H-1 prints it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonDiff {
    pub kind: u32,
    pub row: String,
    pub what: String,
}

fn rows<T: Rec>(v: &[T]) -> Vec<Vec<u8>> {
    v.iter().map(|r| encode_rows(std::slice::from_ref(r))).collect()
}

fn sorted<T: Keyed>(v: &[T], s: StrFn<'_>) -> Result<Vec<Vec<u8>>, StateError> {
    let mut k: Vec<(T::K, Vec<u8>)> = Vec::with_capacity(v.len());
    for r in v {
        k.push((
            r.key(s).map_err(super::read::to_state_error)?,
            encode_rows(std::slice::from_ref(r)),
        ));
    }
    k.sort();
    Ok(k.into_iter().map(|x| x.1).collect())
}

/// The canonical form of the state `bytes` (a whole read: tests and
/// `incr_state` only, IN-18c).
///
/// # Errors
/// The state's own `StateError`.
pub fn canon(bytes: &[u8]) -> Result<Canon, StateError> {
    let m = super::read::decode(bytes)?;
    canon_of(&m)
}

/// [`canon`] of rows in memory (a capture's, a decode's).
///
/// # Errors
/// `state-corrupt` for a reference with no image.
#[allow(clippy::too_many_lines)] // one line per table
pub fn canon_of(m: &MemoryState) -> Result<Canon, StateError> {
    let mut n = super::compact::normalize(m)?;
    n.meta.boot_id = [0; 16];
    n.meta.output_synced = false;
    n.meta.compactions = 0;
    n.meta.build_id = [0; 64];
    n.meta.build_id_len = 0;
    for f in &mut n.files {
        f.round = 0;
    }
    for r in n.stat.iter_mut().filter(|r| r.tag == stat_tag::OUTPUT) {
        *r = crate::format::StatRow {
            tag: r.tag,
            trust: r.trust,
            path: r.path,
            fstype: r.fstype,
            ..Default::default()
        };
    }
    let pool = n.strings.clone();
    let s: StrFn<'_> = &|x: Str| {
        let start = x.off as usize;
        start
            .checked_add(x.len as usize)
            .and_then(|end| pool.get(start..end))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| super::corrupt("STRINGS: a string out of range"))
    };
    let mut frag_uses = n.frag_uses.clone();
    frag_uses.sort_by_key(|(f, r)| (f.0, *r));
    let file_rows = |v: Vec<Vec<u8>>, f: &[mortar_core::FileId]| -> Vec<Vec<u8>> {
        v.into_iter()
            .zip(f)
            .map(|(mut r, f)| {
                let mut x = f.0.to_le_bytes().to_vec();
                x.append(&mut r);
                x
            })
            .collect()
    };
    macro_rules! per_file {
        ($v:expr) => {{
            let f: Vec<_> = $v.iter().map(|x| x.0).collect();
            let r: Vec<_> = $v.iter().map(|x| x.1.clone()).collect();
            file_rows(rows(&r), &f)
        }};
    }
    // DIAGS: compared by their strings' bytes.
    let diags: Vec<Vec<u8>> = n
        .diags
        .iter()
        .map(|d| {
            format!(
                "{:?}",
                (
                    (d.key.phase, d.key.unit, d.key.seq, &d.code, &d.text, &d.subjects),
                    (&d.refs, d.ref_count),
                )
            )
            .into_bytes()
        })
        .collect();
    let mut diags = diags;
    diags.sort();
    let tables = vec![
        (kind::META, rows(std::slice::from_ref(&n.meta))),
        (kind::STAT, rows(&n.stat)),
        (kind::FILES, rows(&n.files)),
        (kind::PROBES, rows(&n.probes)),
        (kind::OUTSECS, rows(&n.outsecs)),
        (kind::SYMBOLS, rows(&n.symbols)),
        (kind::SECTIONS, rows(&n.sections)),
        (kind::CHUNKS, rows(&n.chunks)),
        (kind::ORDER, rows(&n.order)),
        (kind::SEGMENTS, rows(&n.segments)),
        (kind::FRAGS, sorted(&n.frags, s)?),
        (kind::SLOTS, rows(&n.slots)),
        (kind::SIZES, rows(&n.sizes)),
        (kind::SYM_PLACE, rows(&n.sym_place)),
        (kind::DEFINED, rows(&n.defined)),
        (kind::PATCHES, rows(&n.patches)),
        (kind::REDIRECTS, rows(&n.redirects)),
        (kind::SIZED, rows(&n.sized)),
        (kind::LAYOUT, rows(std::slice::from_ref(&n.layout))),
        (kind::STRINGS, vec![n.strings.clone()]),
        (kind::MENTIONS, per_file!(n.mentions)),
        (kind::RESOLVED, rows(&n.resolved)),
        (kind::INDEX, per_file!(n.index)),
        (kind::HOLES, sorted(&n.holes, s)?),
        // normalized: `first_global` only (the ranges are the writer's)
        (kind::FILE_INDEX, rows(&n.file_index)),
        (kind::EDGES, per_file!(n.edges)),
        (kind::SYMTAB, per_file!(n.symtab)),
        (kind::GROUPS, sorted(&n.groups, s)?),
        (kind::KEY_REFS, sorted(&n.key_refs, s)?),
        (kind::COMMONS, sorted(&n.commons, s)?),
        (kind::DIAGS, diags),
        (kind::SYMTAB_KEYS, rows(&n.symtab_keys)),
        (kind::REGIONS, per_file!(n.regions)),
        (kind::NAMES, sorted(&n.names, s)?),
        (kind::COPIES, sorted(&n.copies, s)?),
        (kind::FRAG_USE, per_file!(frag_uses)),
    ];
    Ok(Canon { tables })
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// The differences of `a` against `b`, `p_u` (the update's placement as
/// its map gives it) naming the layout rows (IN-78). Empty: equal.
///
/// Table by table: equal row counts are compared row by row (the row's
/// position and both rows in hex); unequal ones as sets (each row of one
/// that the other lacks). `p_u` is not read yet: the comparison of the
/// decision rows with `P_U` is H-1's (a finding of WP-214).
#[must_use]
pub fn compare(a: &Canon, b: &Canon, p_u: &StoredLayout<'_>) -> Vec<CanonDiff> {
    let _ = p_u;
    let mut out = Vec::new();
    let empty = Vec::new();
    let kinds: std::collections::BTreeSet<u32> =
        a.tables.iter().chain(&b.tables).map(|t| t.0).collect();
    for k in kinds {
        let ra = a.tables.iter().find(|t| t.0 == k).map_or(&empty, |t| &t.1);
        let rb = b.tables.iter().find(|t| t.0 == k).map_or(&empty, |t| &t.1);
        if ra.len() == rb.len() {
            for (i, (x, y)) in ra.iter().zip(rb).enumerate() {
                if x != y {
                    out.push(CanonDiff {
                        kind: k,
                        row: format!("row {i}"),
                        what: format!("{} != {}", hex(x), hex(y)),
                    });
                }
            }
            continue;
        }
        let sa: std::collections::BTreeSet<&Vec<u8>> = ra.iter().collect();
        let sb: std::collections::BTreeSet<&Vec<u8>> = rb.iter().collect();
        for x in sa.difference(&sb) {
            out.push(CanonDiff { kind: k, row: hex(x), what: "only in the first".to_owned() });
        }
        for y in sb.difference(&sa) {
            out.push(CanonDiff { kind: k, row: hex(y), what: "only in the second".to_owned() });
        }
        if out.last().is_none_or(|d| d.kind != k) {
            out.push(CanonDiff {
                kind: k,
                row: String::new(),
                what: format!("{} rows against {} (duplicates)", ra.len(), rb.len()),
            });
        }
    }
    out
}
