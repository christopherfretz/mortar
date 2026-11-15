//! `.eh_frame_hdr` (`spec/synth.md` §5): the binary-search table of FDEs.
//!
//! **Plan** ([`HdrPlan::new`], phase 80): whether the table exists (S-57)
//! and its entries (S-59): every output FDE except those whose address
//! range is 0. Two entries with one initial location are refused here,
//! where they are first known (S-59 says "when the table is written";
//! the count is fixed in phase 80 from `(section, offset)` pairs, and the
//! verdict is the same).
//!
//! **Write** ([`HdrPlan::write`], image): the header (S-58) and the pairs
//! sorted by initial location (S-61). One unit (S-4): the sort needs every
//! entry.

use std::cmp::Ordering;

use mortar_core::par::Pool;
use mortar_diag::{Collector, Diagnostic, Kind};

use crate::eh::{CodeRef, EhPlan, EhSource};
use crate::error::{Failed, SynthError, fail, internal, malformed, record_key};
use crate::out::Window;

/// S-59's refusal.
pub const OVERLAPPING: &str = ".eh_frame_hdr refers to overlapping FDEs";

/// The table's entries (S-59).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HdrPlan {
    /// Per entry: the output index of its FDE in [`EhPlan::records`].
    fdes: Box<[u32]>,
}

/// The output addresses the table needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdrAddrs {
    /// `.eh_frame_hdr`'s address.
    pub hdr: u64,
    /// `.eh_frame`'s address.
    pub eh: u64,
}

impl HdrPlan {
    /// The table for `eh`, or `None` when there is none: S-57, it exists
    /// exactly when `eh_frame_hdr` (the last of `--eh-frame-hdr` and
    /// `--no-eh-frame-hdr`) is set and some FDE is live.
    pub fn new<S>(
        src: &S,
        eh: &EhPlan,
        eh_frame_hdr: bool,
        pool: &Pool,
        diag: &Collector,
    ) -> Result<Option<Self>, Failed>
    where
        S: EhSource + ?Sized,
    {
        if !eh_frame_hdr || eh.fde_count() == 0 {
            return Ok(None);
        }
        let n = eh.records().len();
        let entries: Vec<Option<(CodeRef, u32)>> = pool.map(n, |k| {
            eh.cie_of(k)?;
            let (code, empty) = eh.code(src, k)?;
            (!empty).then_some((code, k as u32))
        });
        let mut keyed: Vec<(CodeRef, u32)> = entries.into_iter().flatten().collect();
        pool.sort_by(&mut keyed, Ord::cmp);
        // S-59: one initial location twice.
        if let Some(w) = keyed.windows(2).find(|w| w[0].0 == w[1].0) {
            let rec = eh.records()[w[1].1 as usize];
            return fail(diag, record_key(rec.section, rec.piece), malformed(OVERLAPPING));
        }
        let mut fdes: Vec<u32> = keyed.into_iter().map(|e| e.1).collect();
        fdes.sort_unstable();
        Ok(Some(Self { fdes: fdes.into() }))
    }

    /// The number of entries.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.fdes.len()
    }

    /// The size (S-58): `12 + 8 × entries`.
    #[must_use]
    pub fn size(&self) -> u64 {
        12 + 8 * self.fdes.len() as u64
    }

    /// Writes the whole table into `out` (`size()` bytes). `code_addr`
    /// gives the output address of an FDE's initial location (its code
    /// section's place plus the offset; model §4.5). An offset that does
    /// not fit `sdata4` is `relocation-out-of-range` (S-62); two entries
    /// at one address are S-59's refusal.
    pub fn write<S, F>(
        &self,
        src: &S,
        eh: &EhPlan,
        at: HdrAddrs,
        code_addr: F,
        pool: &Pool,
        out: &mut [u8],
    ) -> Result<(), SynthError>
    where
        S: EhSource + ?Sized,
        F: Fn(CodeRef) -> Option<u64> + Sync,
    {
        if out.len() as u64 != self.size() {
            return Err(internal(".eh_frame_hdr written to a buffer of the wrong size"));
        }
        let rel = |x: u64| -> Result<i32, SynthError> {
            let d = x.wrapping_sub(at.hdr) as i64;
            i32::try_from(d).map_err(|_| {
                Diagnostic::error(Kind::RelocationOutOfRange {
                    r_type: ".eh_frame_hdr entry".to_owned(),
                    value: d,
                    min: i64::from(i32::MIN),
                    max: i64::from(i32::MAX),
                    name: ".eh_frame_hdr".to_owned(),
                })
                .into()
            })
        };
        let pairs: Vec<Result<(i32, i32), SynthError>> = pool.map(self.fdes.len(), |e| {
            let k = self.fdes[e] as usize;
            let (code, _) = eh
                .code(src, k)
                .ok_or_else(|| internal::<SynthError>(".eh_frame_hdr entry is not an FDE"))?;
            let pc = code_addr(code).ok_or_else(|| {
                internal::<SynthError>(format!("section {} has no address", code.section.0))
            })?;
            Ok((rel(pc)?, rel(at.eh + eh.out_offset(k))?))
        });
        let mut table = Vec::with_capacity(pairs.len());
        for p in pairs {
            table.push(p?);
        }
        // S-61: signed offsets from the header, ascending.
        pool.sort_by(&mut table, |a, b| a.0.cmp(&b.0).then(Ordering::Equal));
        if table.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err(malformed(OVERLAPPING));
        }
        let ptr = rel(at.eh)?;
        let mut w = Window::new(0, out);
        // S-58: version 1, then the three encodings (arch.md §3.9).
        let enc = mortar_arch::shapes::EH_ENCODINGS;
        w.put(0, &[1, enc.hdr_ptr, enc.hdr_count, enc.hdr_table]);
        // `eh_frame_ptr` is pc-relative: from its own field, at offset 4.
        w.word(4, 4, u64::from(ptr.wrapping_sub(4) as u32));
        w.word(8, 4, table.len() as u64);
        for (e, (pc, fde)) in table.iter().enumerate() {
            let at = 12 + 8 * e as u64;
            w.word(at, 4, u64::from(*pc as u32));
            w.word(at + 4, 4, u64::from(*fde as u32));
        }
        Ok(())
    }
}
