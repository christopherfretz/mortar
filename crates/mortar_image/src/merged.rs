//! Merged chunks (I-38, `spec/synth.md` S-27): the live fragments of a
//! `Merged` chunk, each written at its offset from its representative
//! (the first occurrence, `Split` §3.5), found per clip by merge's
//! `Order::frags_in` (`O(range + log n)`); the bytes between fragments are
//! zero. Fragments contiguous both in the chunk and in one input section
//! are copied with one read.

use mortar_diag::{Diagnostic, Key, Kind};

use crate::PHASE_IMAGE;
use crate::fill::Image;
use crate::plan::MergedUnit;
use crate::source;

fn internal(what: String) -> (Key, Diagnostic) {
    (Key::new(PHASE_IMAGE, 0, 0), Diagnostic::error(Kind::InternalError { what }))
}

/// Fills the clip `[a, b)` of merged unit `mu` into `dst`.
pub(crate) fn fill(
    img: &Image<'_>,
    mu: &MergedUnit,
    a: u64,
    b: u64,
    dst: &mut [u8],
) -> Result<(), (Key, Diagnostic)> {
    let Some(m) = img.merged else {
        return Err(internal("fill: a merged chunk without the fragment order".into()));
    };
    if mu.chunk as usize >= m.order.chunks.len() {
        return Err(internal(format!("fill: no merged chunk {} (model §4.4)", mu.chunk)));
    }
    let plan = img.plan;
    dst.fill(0);
    let frags = m.order.frags_in(m.split, mu.chunk as usize, a..b);
    let mut k = 0;
    while let Some(&f) = frags.get(k) {
        let row = m.split.frags[f];
        let off = m.order.map.frag_off[f];
        let (mut end, mut send) =
            (off + u64::from(row.size), u64::from(row.offset) + u64::from(row.size));
        k += 1;
        while let Some(&g) = frags.get(k) {
            let r2 = m.split.frags[g];
            if r2.section != row.section
                || m.order.map.frag_off[g] != end
                || u64::from(r2.offset) != send
            {
                break;
            }
            end += u64::from(r2.size);
            send += u64::from(r2.size);
            k += 1;
        }
        let (s0, e0) = (off.max(a), end.min(b));
        if s0 >= e0 {
            continue;
        }
        let Some(src) = plan.merge_source(row.section) else {
            return Err(internal(format!(
                "fill: merge section {} has no source in the plan",
                row.section.0
            )));
        };
        let from = u64::from(row.offset) + (s0 - off);
        if from + (e0 - s0) > src.size {
            return Err(internal(format!(
                "fill: a fragment outside merge section {}",
                row.section.0
            )));
        }
        let range = src.src + from..src.src + from + (e0 - s0);
        source::read(img.source, src.file, range, &mut dst[(s0 - a) as usize..(e0 - a) as usize])
            .map_err(|e| {
            (Key::new(PHASE_IMAGE, src.file.0, 0), source::copy_error(e, &plan.path(src.file)))
        })?;
    }
    Ok(())
}
