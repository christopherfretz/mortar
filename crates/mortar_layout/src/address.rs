//! Stage D (`spec/layout.md` §11, L-64, L-68 to L-78, L-118): addresses,
//! file offsets, segments and the TLS template.
//!
//! One sequential pass over `order` (O(chunks)). It is a pure function of
//! the stage C output and the island sizes, that is of the patch set
//! (L-21): nothing else enters.

use mortar_core::column::Column;
use mortar_core::id::ChunkId;
use mortar_core::layout::{
    Chunk, ChunkKind, LoadClass, Segment, SegmentRules, Tls, derive_segments,
};
use mortar_elf::consts::SHF_EXECINSTR;

use crate::chunk::Staged;
use crate::diag::{LayoutDiag, feature, internal, output_too_large};
use crate::target::{Options, Target};

/// The result of stage D.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addressed {
    pub chunks: Column<ChunkId, Chunk>,
    pub segments: Vec<Segment>,
    pub tls: Option<Tls>,
}

/// The page offsets an 843419-padded chunk may not start at (L-118).
fn bad_843419(addr: u64) -> bool {
    matches!(addr & 0xfff, 0xffc | 0x000 | 0x004 | 0x008)
}

/// `align_up` in u128, which never overflows for u64 inputs.
fn up(x: u128, a: u128) -> u128 {
    x.div_ceil(a) * a
}

/// The least `A ≥ lo` with `A ≡ o (mod p)`.
fn congruent_at_least(lo: u128, o: u128, p: u128) -> u128 {
    let r = o % p;
    let base = lo - lo % p + r;
    if base >= lo { base } else { base + p }
}

/// Stage D: places every chunk.
///
/// `island_sizes[i]` is the size of `staged.islands[i]` (L-115).
///
/// # Errors
///
/// `refused` (`feature`, `non-PIE image base 0`, L-68; a PIE's base is 0),
/// `output-too-large`
/// (L-73), and `internal-error` for a table stage C cannot have made.
#[allow(clippy::too_many_lines)]
pub fn address(
    staged: &Staged,
    island_sizes: &[u64],
    target: &Target,
    options: &Options,
    rules: &SegmentRules,
) -> Result<Addressed, LayoutDiag> {
    let mut chunks = staged.chunks.clone();
    if island_sizes.len() != staged.islands.len() {
        return Err(internal("island sizes are not per island"));
    }
    for (&i, &size) in staged.islands.iter().zip(island_sizes) {
        chunks[i].size = size;
        chunks[i].capacity = size;
    }
    let max_page = u128::from(options.max_page);
    let limit = target.address_limit();
    // The PT_LOAD runs and their alignment P (L-65, L-67).
    let order = &staged.order;
    let mut run_of = vec![usize::MAX; chunks.len() as usize];
    let mut runs: Vec<(LoadClass, u8)> = Vec::new();
    for &c in order {
        let chunk = &chunks[c];
        let Some(class) = chunk.run_class(runs.last().map(|r| r.0)) else { continue };
        match runs.last_mut() {
            Some((last, align)) if *last == class => *align = (*align).max(chunk.align),
            _ => runs.push((class, chunk.align)),
        }
        run_of[c.0 as usize] = runs.len() - 1;
    }
    let page_log = options.max_page.trailing_zeros() as u8;
    let p_of = |r: usize| -> u128 { 1u128 << runs[r].1.max(page_log).min(64) };
    // The TLS alignment T (L-76).
    let t_align = order
        .iter()
        .map(|&c| &chunks[c])
        .filter(|c| c.is_alloc() && c.is_tls())
        .map(|c| c.align)
        .max();
    let first_tls = order.iter().copied().find(|&c| chunks[c].is_alloc() && chunks[c].is_tls());
    // L-68: the base; 0 for a position-independent output (D-1).
    let p1 = if runs.is_empty() { max_page } else { p_of(0) };
    let base = if options.pie { 0 } else { u128::from(target.image_base) / p1 * p1 };
    if base == 0 && !options.pie {
        return Err(feature("non-PIE image base 0"));
    }
    // L-71: every code segment (the small one, and x86-64's `.ltext`,
    // L-36a) and the segment after it start on a fresh page.
    let is_code = |r: usize| matches!(runs[r].0, LoadClass::Rx | LoadClass::LargeRx);
    let mut cur: u128 = base; // memory cursor: end of the previous chunk
    let mut fcur: u128 = 0; // file cursor
    let mut prev_run: Option<usize> = None;
    // The current PT_LOAD's (address, offset).
    let mut seg: (u128, u128) = (base, 0);
    let too_large = |what: &str| output_too_large(what, limit);
    for &c in order {
        let chunk = &chunks[c];
        let cap = u128::from(chunk.capacity);
        if !chunk.is_alloc() {
            // L-72: non-alloc chunks after the last alloc chunk.
            let a = 1u128 << chunk.align.min(64);
            let off = if chunk.in_file() { up(fcur, a) } else { fcur };
            if chunk.in_file() {
                fcur = off + cap;
            }
            if fcur > limit || off >= limit {
                return Err(too_large("the file"));
            }
            let chunk = &mut chunks[c];
            chunk.addr = None;
            chunk.offset = off as u64;
            continue;
        }
        let run = run_of[c.0 as usize];
        if run == usize::MAX {
            return Err(internal("an alloc chunk outside every load run"));
        }
        // L-69, L-76: the effective alignment.
        let own = chunk.align;
        let eff = if Some(c) == first_tls { t_align.unwrap_or(own) } else { own };
        let a = 1u128 << eff.min(64);
        let addr;
        let new_load = prev_run != Some(run);
        if new_load {
            // L-70: a new PT_LOAD at A ≡ o (mod P).
            let p = p_of(run);
            let separate = options.separate_code && (is_code(run) || run > 0 && is_code(run - 1));
            let o = if prev_run.is_none() {
                0
            } else if separate {
                up(fcur, max_page.max(a))
            } else {
                up(fcur, a)
            };
            let lo = if prev_run.is_none() { base } else { up(cur, max_page) };
            addr = congruent_at_least(lo, o, p);
            seg = (addr, o);
            cur = addr;
            prev_run = Some(run);
        } else {
            let mut at = up(cur, a);
            // L-118: the 843419 start padding of a synthetic code chunk.
            let synthetic_code =
                matches!(chunk.kind, ChunkKind::Patches(_) | ChunkKind::Synthetic(_))
                    && chunk.flags & SHF_EXECINSTR != 0
                    && chunk.size > 0;
            if options.fix_843419 && synthetic_code {
                if a >= 0x1000 {
                    return Err(internal("an 843419-padded chunk aligned to a page"));
                }
                let mut steps = 0;
                while bad_843419(at as u64) {
                    at += a;
                    steps += 1;
                    if steps > 16 {
                        return Err(internal("843419 padding does not terminate"));
                    }
                }
            }
            addr = at;
        }
        // L-69: file content is displaced like the addresses from the
        // segment's start; NOBITS offsets are undisplaced (the file end).
        let off = if chunk.in_file() { seg.1 + (addr - seg.0) } else { fcur };
        if addr + cap > limit || off + cap > limit {
            return Err(too_large(&format!("chunk {}", c.0)));
        }
        // L-64: the padding makes the RELRO end a multiple of maxpage.
        let mut cap = cap;
        if Some(c) == staged.relro_padding {
            let size = up(addr, max_page) - addr;
            cap = size;
            let chunk = &mut chunks[c];
            chunk.size = size as u64;
            chunk.capacity = size as u64;
        }
        let chunk = &mut chunks[c];
        chunk.addr = Some(addr as u64);
        chunk.offset = off as u64;
        // L-77: .tbss takes no address space; the next chunk starts at
        // the end of the chunk before it. Every other chunk ends at
        // `addr + cap`, an empty one at its aligned address (L-69's
        // `prev_end`): the cursors never move back, or an empty chunk's
        // alignment would leave a later chunk below it and the segment's
        // file image would cover a non-alloc chunk's first bytes.
        if chunk.in_file() || !chunk.is_tls() {
            cur = cur.max(addr + cap);
        }
        if chunk.in_file() {
            fcur = fcur.max(off + cap);
        }
    }
    let segments = derive_segments(&chunks, order, rules);
    let tls = first_tls.map(|first| {
        let start = chunks[first].addr.unwrap_or(0);
        let end = order
            .iter()
            .map(|&c| &chunks[c])
            .filter(|c| c.is_alloc() && c.is_tls())
            .filter_map(|c| c.addr.map(|a| a + c.size))
            .max()
            .unwrap_or(start);
        Tls { start, size: end - start, align: t_align.unwrap_or(0) }
    });
    Ok(Addressed { chunks, segments, tls })
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: layout.md §11.2 L-70 (the least A ≥ lo with A ≡ o mod P)
    #[test]
    fn l70_congruence() {
        assert_eq!(congruent_at_least(0x23_0000, 0x2_7b80, 0x1_0000), 0x23_7b80);
        assert_eq!(congruent_at_least(0x23_8000, 0x2_7b80, 0x1_0000), 0x24_7b80);
        assert_eq!(congruent_at_least(0x40_0000, 0, 0x1_0000), 0x40_0000);
    }

    // spec: layout.md §14.2 L-118 (forbidden page offsets of an 843419-padded chunk)
    #[test]
    fn l118_page_offsets() {
        for bad in [0x40_0ffc, 0x40_1000, 0x40_1004, 0x40_1008] {
            assert!(bad_843419(bad));
        }
        for ok in [0x40_0ff8, 0x40_100c, 0x40_1010] {
            assert!(!bad_843419(ok));
        }
    }
}
