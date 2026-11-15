//! References from sections in the output into sections that are not
//! (§6.1, I-45..I-48, I-112).
//!
//! Stand-in: request R-5 moves the table into `mortar_arch::tombstone`
//! (WP-023b). `mortar_arch::tombstone` on main is the old rule (1 for
//! `.debug_ranges`/`.debug_loc`, 0 otherwise); the image writes this
//! table's value itself and never lets `apply` write one.

/// What a referring section writes for a dead target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// The section has a GNU ld debug name (I-45): the tombstone of I-47.
    Debug(Tomb),
    /// `.gcc_except_table`, alloc or not (I-46): 0.
    Zero,
}

/// I-47's value classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tomb {
    /// `.debug_ranges`, `.debug_loc`: 1.
    One,
    /// `.debug_aranges`: all ones, and the tuple's length 0 (I-112).
    Aranges,
    /// `.debug_frame`: all ones in both classes.
    Ones,
    /// Every other debug name: all ones in ELFCLASS64, 0 in ELFCLASS32.
    ClassOnes,
}

/// I-45's names (`spec/resolve.md` §6.3).
#[must_use]
pub fn is_debug_name(name: &[u8]) -> bool {
    const PREFIXES: [&[u8]; 6] =
        [b".debug", b".zdebug", b".line", b".stab", b".gnu.linkonce.wi.", b".gnu.debuglto_.debug_"];
    name == b".gnu_debuglink" || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The rule of a referring section, `None` when a dead target is not the
/// image's to write (an alloc section other than `.gcc_except_table`; any
/// other non-alloc section, which the scan refused, I-46).
#[must_use]
pub fn rule(name: &[u8]) -> Option<Rule> {
    if name == b".gcc_except_table" {
        return Some(Rule::Zero);
    }
    if !is_debug_name(name) {
        return None;
    }
    Some(Rule::Debug(match name {
        b".debug_ranges" | b".debug_loc" => Tomb::One,
        b".debug_aranges" => Tomb::Aranges,
        b".debug_frame" => Tomb::Ones,
        _ => Tomb::ClassOnes,
    }))
}

/// `tombstone(section, width, class)` (I-47), truncated to the field
/// (I-48): the bytes to write, little-endian, `width` of them.
#[must_use]
pub fn value(rule: Rule, width: u8, is_64: bool) -> u64 {
    let ones = if width >= 8 { u64::MAX } else { (1u64 << (8 * u32::from(width))) - 1 };
    match rule {
        Rule::Zero => 0,
        Rule::Debug(Tomb::One) => 1 & ones,
        Rule::Debug(Tomb::Aranges | Tomb::Ones) => ones,
        Rule::Debug(Tomb::ClassOnes) => {
            if is_64 {
                ones
            } else {
                0
            }
        }
    }
}
