//! The relocations of the input `.eh_frame` sections over the output
//! `.eh_frame` (`spec/synth.md` S-52, S-53): synth's chunk writes each kept
//! record's bytes (the length, the rewritten CIE pointer, the input body);
//! the image then applies, by the ordinary rules, every relocation of a
//! kept record (initial location, personality, LSDA), with `P` the field's
//! output address. A relocation in a removed record is not applied: no
//! clip of a kept record holds its field, and only the job whose clip
//! holds `r_offset` reports (I-36).

use mortar_diag::{Diagnostic, Key, Kind};

use crate::PHASE_IMAGE;
use crate::fill::Image;
use crate::reloc::{self, Clip, Scratch};

/// Applies the relocations of the kept records in `[a, b)` of the output
/// `.eh_frame` to `dst` (its bytes, `dst.len() == b − a`).
pub(crate) fn relocate(
    img: &Image<'_>,
    s: &mut Scratch,
    a: u64,
    b: u64,
    dst: &mut [u8],
    errs: &mut Vec<(Key, Diagnostic)>,
) -> Result<(), (Key, Diagnostic)> {
    let Some(e) = img.eh else {
        let what = "fill: the .eh_frame chunk without its plan".to_owned();
        return Err((Key::new(PHASE_IMAGE, 0, 0), Diagnostic::error(Kind::InternalError { what })));
    };
    if img.plan.eh.is_empty() {
        return Ok(());
    }
    let frames = e.src.frames();
    let n = e.plan.records().len();
    // The first output record ending after `a`.
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if e.plan.out_offset(mid + 1) <= a {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    for k in lo..n {
        let at = e.plan.out_offset(k);
        if at >= b {
            break;
        }
        let (row, g) = e.plan.source(k);
        let (Some(rec), Some(sec)) = (frames.records.get(g as usize), frames.sections.get(row))
        else {
            continue;
        };
        let Some(es) = img.plan.eh_section(sec.section) else { continue };
        let rs = &img.plan.relocs[es.relocs as usize];
        // The record's input bytes land at `at..at + 4 + len` (its
        // padding, S-47, holds no field).
        let (ca, cb) = (a.max(at), b.min(at + 4 + u64::from(rec.len)));
        if ca >= cb {
            continue;
        }
        let input = e.src.bytes(row);
        let off = u64::from(rec.offset);
        let clip = Clip {
            file: es.file,
            index: es.index,
            section: sec.section,
            name: b".eh_frame",
            alloc: true,
            section_size: input.len() as u64,
            sec_src: 0,
            addr: e.addr.wrapping_add(at).wrapping_sub(off),
            rs,
            a: off + (ca - at),
            b: off + (cb - at),
        };
        reloc::relocate(
            img,
            s,
            &clip,
            Some(input),
            &mut dst[(ca - a) as usize..(cb - a) as usize],
            errs,
        )
        .map_err(|d| (Key::new(PHASE_IMAGE, es.file.0, 0), d))?;
    }
    Ok(())
}
