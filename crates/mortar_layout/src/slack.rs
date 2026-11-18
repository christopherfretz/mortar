//! The slack policy of an incremental-mode link (`spec/incr.md` IN-30,
//! IN-33, IN-34a, IN-50; `spec/layout.md` L-141 to L-143): every constant
//! in one place, and the classification of output sections.
//!
//! A plain link has no slack: capacity = size everywhere, by the same code
//! paths with [`Slack::NONE`] (L-87, IN-33's last sentence).

use mortar_elf::consts::{SHF_ALLOC, SHF_EXECINSTR, SHF_TLS};

/// A region's minimum slack (IN-33: `max(64 B, size/8)`).
pub const REGION_MIN: u64 = 64;
/// A region's slack divisor.
pub const REGION_DIV: u64 = 8;
/// An allocated output section's minimum tail (`max(4 KiB, size/16)`).
pub const TAIL_MIN: u64 = 4096;
/// An allocated output section's tail divisor.
pub const TAIL_DIV: u64 = 16;
/// A `PT_LOAD`'s minimum reserve (`max(64 KiB, filesz/8)`, rounded up
/// to the maximum page size).
pub const LOAD_MIN: u64 = 64 * 1024;
/// A `PT_LOAD`'s reserve divisor.
pub const LOAD_DIV: u64 = 8;
/// A packed (L-142) section's capacity divisor (`size/8` past the chunk).
pub const PACKED_DIV: u64 = 8;
/// A debug section's tail divisor (`size/32`).
pub const DEBUG_TAIL_DIV: u64 = 32;
/// The smallest debug size class (`at least 32 B`).
pub const CLASS_MIN: u64 = 32;
/// The neutral abbreviation table's hole (IN-51b): the one table of its
/// four abbreviations (37 bytes, `mortar_image::neutral::ABBREV_TABLE`)
/// and the 0 code that ends it (a table at the end of `.debug_abbrev`
/// needs its own: readelf "section not zero terminated"). In every
/// incremental-mode output with debug information, whatever the inputs,
/// so an update never grows it.
pub const ABBREV_TABLE: u64 = 38;
/// A file's `.eh_frame` range: the smallest pad past its records (IN-34a:
/// a padding CIE is 16 bytes at least).
pub const EH_PAD_MIN: u64 = 16;
/// A file's `.eh_frame` range: the pad divisor (`ceil4(size/8)`).
pub const EH_PAD_DIV: u64 = 8;

/// Whether a link lays out slack, and how much. [`Slack::NONE`] in a
/// plain link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Slack {
    /// IN-30/IN-33's capacities (an incremental-mode link).
    pub on: bool,
}

impl Slack {
    /// A plain link: capacity = size.
    pub const NONE: Self = Self { on: false };
    /// IN-33's policy.
    pub const INCREMENTAL: Self = Self { on: true };

    /// A region's slack past its `size` (IN-33: capacity `size +
    /// max(64, size/8)` rounded up to the region's alignment `2^align`).
    #[must_use]
    pub fn region(self, size: u64, align: u8) -> u64 {
        if !self.on {
            return 0;
        }
        let a = 1u64 << align.min(63);
        let cap = size.saturating_add(REGION_MIN.max(size / REGION_DIV));
        cap.checked_next_multiple_of(a).unwrap_or(cap) - size
    }

    /// An allocated hole-tolerant output section's tail (`max(4 KiB,
    /// size/16)`).
    #[must_use]
    pub fn tail(self, size: u64) -> u64 {
        if self.on { TAIL_MIN.max(size / TAIL_DIV) } else { 0 }
    }

    /// A packed section's capacity past its chunk (`size/8`).
    #[must_use]
    pub fn packed(self, size: u64) -> u64 {
        if self.on { size / PACKED_DIV } else { 0 }
    }

    /// A file's `.eh_frame` range past its records' `size` bytes (IN-34a):
    /// `max(16, ceil4(size/8))`, so capacity = `size + eh_pad(size)`; 0
    /// without slack. The ONE rule of the delta layout's whole rewrite
    /// (WP-243) and of the incremental-mode full link's per-file pads.
    /// `size` is a multiple of 4 (records are), so the capacity is too.
    #[must_use]
    pub fn eh_pad(self, size: u64) -> u64 {
        if !self.on {
            return 0;
        }
        let p = size.div_ceil(EH_PAD_DIV);
        EH_PAD_MIN.max(p.checked_next_multiple_of(4).unwrap_or(p))
    }

    /// A debug section's tail (`size/32`).
    #[must_use]
    pub fn debug_tail(self, size: u64) -> u64 {
        if self.on { size / DEBUG_TAIL_DIV } else { 0 }
    }

    /// A `PT_LOAD`'s reserve: the gap to the next, in file and address
    /// space (`max(64 KiB, filesz/8)` rounded up to `max_page`).
    #[must_use]
    pub fn load(self, filesz: u64, max_page: u64) -> u64 {
        if !self.on {
            return 0;
        }
        let r = LOAD_MIN.max(filesz / LOAD_DIV);
        r.checked_next_multiple_of(max_page.max(1)).unwrap_or(r)
    }

    /// A debug contribution's allotment beyond its `size` (IN-50, IN-51b):
    /// the smallest class `c = m·2^k` (`m` in 8..=15, at least 32) with
    /// `c − size` either 0 or at least `form.min` and a multiple of
    /// `form.granule`. 0 for an empty contribution or without slack.
    #[must_use]
    pub fn allotment(self, size: u64, form: Neutral) -> u64 {
        if !self.on || size == 0 {
            return 0;
        }
        let mut c = class_at_least(size);
        loop {
            let pad = c - size;
            if pad == 0 || (pad >= form.min && pad.is_multiple_of(form.granule)) {
                return pad;
            }
            match class_at_least(c + 1) {
                n if n > c => c = n,
                _ => return 0,
            }
        }
    }
}

/// The smallest size class at least `n` (`m·2^k`, `m` in 8..=15, at
/// least [`CLASS_MIN`]); `n` itself past the classes' range.
#[must_use]
pub fn class_at_least(n: u64) -> u64 {
    if n <= CLASS_MIN {
        return CLASS_MIN;
    }
    let k = (64 - n.leading_zeros()).saturating_sub(4);
    let step = 1u64 << k;
    n.checked_next_multiple_of(step).unwrap_or(n)
}

/// A debug section's neutral form (IN-51b): the smallest neutral unit and
/// the granule of its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neutral {
    pub min: u64,
    pub granule: u64,
}

/// The neutral form of a debug output section that takes allotments, or
/// `None` for one that takes none (IN-51b: `.debug_loclists` and
/// `.debug_frame` take only appends and `.debug_addr` is never split;
/// `.debug_aranges` is rewritten whole; the merged `.debug_str` and
/// `.debug_line_str` take only their tail; any other section has no
/// neutral form). The minimum is the largest of the forms by version
/// (`.debug_info` 15 for DWARF 5, `.debug_line` 32 for version 5), so an
/// allotment fits whichever form the image writes.
#[must_use]
pub fn neutral(name: &[u8]) -> Option<Neutral> {
    let (min, granule) = match name {
        b".debug_info" => (15, 1),
        b".debug_abbrev" => (1, 1),
        b".debug_line" => (32, 1),
        b".debug_rnglists" => (12, 1),
        b".debug_str_offsets" => (8, 4),
        b".debug_ranges" => (16, 1),
        _ => return None,
    };
    Some(Neutral { min, granule })
}

/// Whether an output section is a debug section (IN-50's tail rule).
#[must_use]
pub fn is_debug(name: &[u8], flags: u64) -> bool {
    flags & SHF_ALLOC == 0 && name.starts_with(b".debug_")
}

/// How an allocated output section takes slack (layout.md L-142, L-143).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// L-143: regions with holes, and a tail.
    Tolerant,
    /// L-142 (and every allocated section L-143 does not name): one
    /// chunk's capacity of `size/8`, no holes.
    Packed,
    /// No slack: the TLS template (IN-34's TLS row; L-143's "holes only
    /// at the end" is not taken in v1), and notes, which a `PT_NOTE`
    /// reads as one sequence of whole notes (L-141: it covers their sizes
    /// only, and capacity between two notes would be inside it; S-114
    /// recomputes them, `property-changed`).
    None,
}

/// The class of an allocated output section by name and flags.
#[must_use]
pub fn class(name: &[u8], flags: u64) -> Class {
    if flags & SHF_TLS != 0 || name.starts_with(b".note") {
        return Class::None;
    }
    match name {
        b".init" | b".fini" => Class::Packed,
        b".text" | b".rodata" | b".data" | b".data.rel.ro" | b".bss" | b".gcc_except_table" => {
            Class::Tolerant
        }
        _ if flags & SHF_EXECINSTR != 0 && name.starts_with(b".text") => Class::Tolerant,
        _ => Class::Packed,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn eh_pad_is_max_16_ceil4_eighth() {
        let s = Slack::INCREMENTAL;
        assert_eq!(Slack::NONE.eh_pad(1000), 0);
        assert_eq!(s.eh_pad(0), 16);
        assert_eq!(s.eh_pad(24), 16);
        assert_eq!(s.eh_pad(128), 16);
        assert_eq!(s.eh_pad(132), 20);
        assert_eq!(s.eh_pad(136), 20);
        assert_eq!(s.eh_pad(160), 20);
        assert_eq!(s.eh_pad(164), 24);
        for size in (0..4096).step_by(4) {
            let p = s.eh_pad(size);
            assert!(
                p >= 16
                    && p.is_multiple_of(4)
                    && p >= size.div_ceil(8)
                    && p < size.div_ceil(8) + 4 + 16
            );
        }
    }

    use super::*;

    // spec: incr.md §5.6 IN-33 (region: size + max(64, size/8), rounded up)
    #[test]
    fn in33_region() {
        let s = Slack::INCREMENTAL;
        assert_eq!(s.region(10, 0), 64);
        assert_eq!(s.region(10, 4), 70);
        assert_eq!(s.region(1024, 3), 128);
        assert_eq!(s.region(1000, 4), 1136 - 1000);
        assert_eq!(Slack::NONE.region(1000, 4), 0);
    }

    // spec: incr.md §5.6 IN-33 (tails, packed capacity, PT_LOAD reserve)
    #[test]
    fn in33_reserves() {
        let s = Slack::INCREMENTAL;
        assert_eq!(s.tail(100), 4096);
        assert_eq!(s.tail(1 << 20), 1 << 16);
        assert_eq!(s.packed(800), 100);
        assert_eq!(s.debug_tail(3200), 100);
        assert_eq!(s.load(100, 0x1_0000), 0x1_0000);
        assert_eq!(s.load(0x10_0001, 0x1000), 0x2_0000);
        assert_eq!(s.load(0x10_8001, 0x1000), 0x2_1000);
        for f in [Slack::tail, Slack::packed, Slack::debug_tail] {
            assert_eq!(f(Slack::NONE, 12345), 0);
        }
    }

    // spec: incr.md §6 IN-50 (size classes m·2^k, m in 8..15, at least 32)
    #[test]
    fn in50_classes() {
        assert_eq!(class_at_least(1), 32);
        assert_eq!(class_at_least(32), 32);
        assert_eq!(class_at_least(33), 36);
        assert_eq!(class_at_least(100), 104);
        assert_eq!(class_at_least(4096), 4096);
        assert_eq!(class_at_least(4097), 4608);
        for n in 33..5000u64 {
            let c = class_at_least(n);
            let m = c >> c.trailing_zeros();
            assert!(c >= n && m < 16, "{n} -> {c}");
        }
    }

    // spec: incr.md §6 IN-51b (allotment: 0 or at least the minimum, by granule)
    #[test]
    fn in51b_allotments() {
        let s = Slack::INCREMENTAL;
        let info = neutral(b".debug_info").unwrap();
        assert_eq!(s.allotment(32, info), 0);
        // 32 − 20 = 12 and 34 − 20 = 14 are below 15: 36.
        assert_eq!(s.allotment(20, info), 16);
        let so = neutral(b".debug_str_offsets").unwrap();
        for n in (8..2000u64).step_by(4) {
            let a = s.allotment(n, so);
            assert!(a == 0 || (a >= 8 && a.is_multiple_of(4)), "{n}: {a}");
        }
        for n in 1..3000u64 {
            let a = s.allotment(n, info);
            assert!(a == 0 || a >= 15, "{n}: {a}");
        }
        assert_eq!(Slack::NONE.allotment(20, info), 0);
        assert!(neutral(b".debug_loclists").is_none());
        assert!(neutral(b".debug_addr").is_none());
    }

    // spec: layout.md §18 L-142, L-143 (the classes of output sections)
    #[test]
    fn l143_classes() {
        assert_eq!(class(b".text", SHF_EXECINSTR), Class::Tolerant);
        assert_eq!(class(b".init", SHF_EXECINSTR), Class::Packed);
        assert_eq!(class(b".init_array", 0), Class::Packed);
        assert_eq!(class(b"my_sec", 0), Class::Packed);
        assert_eq!(class(b".tdata", SHF_TLS), Class::None);
        assert_eq!(class(b".bss", 0), Class::Tolerant);
        assert_eq!(class(b".note.gnu.property", 0), Class::None);
    }
}
