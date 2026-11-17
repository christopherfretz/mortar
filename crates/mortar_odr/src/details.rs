//! Kind 4's note (`spec/odr.md` O-19, §9.1; WP-145): the first difference
//! between two layout-detail entries, rendered for a developer. Kind 4
//! never decides a finding: the record check of kind 3 has decided that
//! two classes' layouts differ; this module only says where.

use std::fmt::Write as _;

use mortar_meta::Unit;
use mortar_meta::details::{Entry, Item, item, nesting, signed_underlying};

/// One side of the comparison: the unit (for its strings), the entry and
/// the file as a note writes it.
pub(crate) struct Side<'u, 'l> {
    pub(crate) unit: &'u Unit<'l>,
    pub(crate) entry: Entry<'l>,
    pub(crate) file: &'u str,
}

/// An item with its strings resolved, and the offsets of the unnamed
/// members it is nested in (O-19's `width`), the innermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Resolved<'l> {
    kind: u32,
    name: &'l [u8],
    ty: &'l [u8],
    value: u64,
    width: u32,
    flags: u32,
    outer: Vec<u64>,
}

impl<'l> Resolved<'l> {
    fn new(unit: &Unit<'l>, it: Item, outer: Vec<u64>) -> Self {
        Self {
            kind: it.kind(),
            name: unit.string(it.name),
            ty: unit.string(it.ty),
            // O-19 (WP-204): an underlying type's value holds two bits;
            // the others are reserved and ignored.
            value: if it.kind() == item::UNDERLYING {
                it.value & (item::SCOPED | item::FIXED)
            } else {
                it.value
            },
            width: it.width,
            flags: it.flags & item::KNOWN_FLAGS,
            outer,
        }
    }

    /// §9.1's identity: kind and name (member, slot, enumerator), kind
    /// and type (base), kind alone (vptr, underlying type); a nested
    /// item's identity includes the unnamed members around it (WP-151).
    fn identity(&self) -> (u32, &'l [u8], &[u64]) {
        match self.kind {
            item::MEMBER | item::SLOT | item::ENUMERATOR => (self.kind, self.name, &self.outer),
            item::BASE => (self.kind, self.ty, &self.outer),
            _ => (self.kind, b"", &self.outer),
        }
    }

    /// §9.1's `D` as (kind words, the rest): `D'` is the rest. `signed`:
    /// whether the entry's enumeration is signed (O-19), when known.
    fn text(&self, signed: Option<bool>) -> (&'static str, String) {
        let lossy = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let bitfield = self.flags & item::BITFIELD != 0;
        let offset = || offset_text(self.value, bitfield);
        let (kind, mut rest) = match self.kind {
            item::MEMBER => {
                let name = if self.name.is_empty() {
                    "unnamed member".to_owned()
                } else {
                    format!("'{}'", lossy(self.name))
                };
                let ty = if self.ty.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", lossy(self.ty))
                };
                let field = if bitfield {
                    format!(", bit-field of {} bits", self.width)
                } else if self.name.is_empty() && self.ty.is_empty() && self.width > 0 {
                    // O-19's count of the nested class's items (WP-151).
                    let s = if self.width == 1 { "" } else { "s" };
                    format!(" with {} nested item{s}", self.width)
                } else {
                    String::new()
                };
                let kind = if self.name.is_empty() { "" } else { "member " };
                (kind, format!("{name}{ty} at offset {}{field}", offset()))
            }
            item::BASE => {
                let virt = if self.flags & item::VIRTUAL != 0 { " (virtual)" } else { "" };
                ("base ", format!("{} at offset {}{virt}", lossy(self.ty), offset()))
            }
            item::VPTR => ("vptr ", format!("at offset {}", offset())),
            item::SLOT => ("vtable slot ", format!("{}: {}", self.value, lossy(self.name))),
            item::ENUMERATOR => {
                // O-19: the value is sign-extended when the underlying
                // type is signed (WP-151: printed in signed decimal then).
                #[allow(clippy::cast_possible_wrap, reason = "O-19's two's complement")]
                let v = match signed {
                    Some(true) => (self.value as i64).to_string(),
                    Some(false) => self.value.to_string(),
                    None if self.value >> 63 == 0 => self.value.to_string(),
                    None => format!("{:#x}", self.value),
                };
                ("enumerator ", format!("'{}' = {v}", lossy(self.name)))
            }
            _ => ("underlying type ", format!("{}{}", lossy(self.ty), enum_bits(self.value, false))),
        };
        for at in &self.outer {
            let _ = write!(rest, " of the unnamed member at offset {}", offset_text(*at, false));
        }
        (kind, rest)
    }

    fn whole(&self, signed: Option<bool>) -> String {
        let (k, rest) = self.text(signed);
        format!("{k}{rest}")
    }
}

/// An underlying-type item's bits as words (O-19, WP-204): nothing for 0
/// unless `explicit` (both sides of a difference in the bits alone, where
/// 0 says "unscoped, not fixed").
fn enum_bits(value: u64, explicit: bool) -> String {
    if value == 0 && !explicit {
        return String::new();
    }
    let scoped = if value & item::SCOPED != 0 { "scoped" } else { "unscoped" };
    let fixed = if value & item::FIXED != 0 { "fixed" } else { "not fixed" };
    format!(" ({scoped}, {fixed})")
}

/// An offset in bits as a developer reads it (§9.1): bytes, without a
/// unit, when the item is not a bit-field and the bits are a multiple of
/// 8; the bits followed by `bits` otherwise.
fn offset_text(bits: u64, bitfield: bool) -> String {
    if !bitfield && bits.is_multiple_of(8) {
        (bits / 8).to_string()
    } else {
        format!("{bits} bits")
    }
}

/// A side's items of known kinds, each with the unnamed members around it
/// (counted over every item, unknown kinds included: O-19's `width`
/// counts the items that follow), and whether its enumeration is signed.
fn resolved<'l>(side: &Side<'_, 'l>) -> (Vec<Resolved<'l>>, Option<bool>) {
    let all: Vec<Item> = side.entry.all_items().collect();
    let nest = nesting(&all);
    let items: Vec<Resolved<'l>> = all
        .iter()
        .zip(nest)
        .filter(|(it, _)| it.known())
        .map(|(it, outer)| {
            Resolved::new(side.unit, *it, outer.iter().map(|&at| all[at].value).collect())
        })
        .collect();
    let char_signed = side.unit.summary_value(b"abi.char").and_then(|(v, _)| match v {
        b"signed" => Some(true),
        b"unsigned" => Some(false),
        _ => None,
    });
    let signed = items
        .iter()
        .find(|x| x.kind == item::UNDERLYING)
        .and_then(|x| signed_underlying(x.ty, char_signed));
    (items, signed)
}

/// The note that follows a first difference at a vtable slot (WP-151:
/// kiln numbers a slot by its entry in the whole vtable group, so the
/// first virtual function of a class is slot 1, not 0).
const SLOT_HELP: &str = "vtable slots are numbered over the whole vtable group from 0, its \
                         offset-to-top, vcall and vbase entries counted and its RTTI entries not \
                         (spec/odr.md O-18, O-19)";

/// §9.1's notes on the first difference of `a`'s and `b`'s entries: the
/// difference, then [`SLOT_HELP`] when it is at a vtable slot.
/// `enumeration`: both rows are enumerations (kind 3's `ENUM`).
pub(crate) fn first_difference(
    a: &Side<'_, '_>,
    b: &Side<'_, '_>,
    enumeration: bool,
) -> Vec<String> {
    let ((xa, sa), (xb, sb)) = (resolved(a), resolved(b));
    let (fa, fb) = (a.file, b.file);
    let Some(i) = (0..xa.len().max(xb.len())).find(|&i| xa.get(i) != xb.get(i)) else {
        let note = if enumeration {
            // O-18 hashes whether the underlying type is fixed and whether
            // the enumeration is scoped; kind 4 says it from kiln's `.d5`
            // on (the underlying type's value, WP-204), so equal details
            // come from an older producer.
            "the layout details are equal: the enumerations differ in something kind 4 does not \
             describe (whether the underlying type is fixed, or whether the enumeration is \
             scoped)"
        } else {
            "the layout details are equal: the difference is in something kind 4 does not \
             describe"
        };
        return vec![note.to_owned()];
    };
    let slot = [xa.get(i), xb.get(i)].into_iter().flatten().any(|x| x.kind == item::SLOT);
    let only = |items: &[Resolved<'_>], signed: Option<bool>, file: &str| {
        let rest = items.len() - i - 1;
        let more = match rest {
            0 => String::new(),
            1 => ", and 1 more item".to_owned(),
            n => format!(", and {n} more items"),
        };
        format!("{} in {file} only{more}", items[i].whole(signed))
    };
    let note = match (xa.get(i), xb.get(i)) {
        (Some(_), None) => only(&xa, sa, fa),
        (None, _) => only(&xb, sb, fb),
        (Some(ia), Some(ib))
            if ia.kind == item::UNDERLYING && ib.kind == item::UNDERLYING && ia.ty == ib.ty =>
        {
            // The bits alone differ (WP-204): both sides in words.
            let ty = String::from_utf8_lossy(ia.ty);
            format!(
                "underlying type {ty}{} in {fa}; {ty}{} in {fb}",
                enum_bits(ia.value, true),
                enum_bits(ib.value, true)
            )
        }
        (Some(ia), Some(ib)) if ia.identity() == ib.identity() => {
            format!("{} in {fa}; {} in {fb}", ia.whole(sa), ib.text(sb).1)
        }
        (Some(ia), Some(ib)) => {
            let a_in_b = xb.iter().any(|x| x.identity() == ia.identity());
            let b_in_a = xa.iter().any(|x| x.identity() == ib.identity());
            match (a_in_b, b_in_a) {
                (false, true) => format!("{} in {fa} only", ia.whole(sa)),
                (true, false) => format!("{} in {fb} only", ib.whole(sb)),
                (false, false) => format!("{} in {fa}; {} in {fb}", ia.whole(sa), ib.whole(sb)),
                (true, true) => format!(
                    "the items stand in a different order: {} in {fa}; {} in {fb}",
                    ia.whole(sa),
                    ib.whole(sb)
                ),
            }
        }
    };
    let mut out = vec![note];
    if slot {
        out.push(SLOT_HELP.to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    //! T-O-19 (`spec/odr.md` §12.6): a kind 4 entry renders the first
    //! difference, per item kind, both directions.

    use mortar_meta::write::{Detail, Type};
    use mortar_meta::{SectionAttrs, UnitBuilder, producer, read_section};

    use super::*;

    fn member(name: &str, ty: &str, bits: u64) -> Detail {
        Detail {
            name: name.into(),
            ty: ty.into(),
            value: bits,
            flags: item::MEMBER,
            ..Detail::default()
        }
    }
    fn bitfield(name: &str, ty: &str, bits: u64, width: u32) -> Detail {
        Detail { width, flags: item::MEMBER | item::BITFIELD, ..member(name, ty, bits) }
    }
    fn base(ty: &str, bits: u64, virt: bool) -> Detail {
        let flags = if virt { item::BASE | item::VIRTUAL } else { item::BASE };
        Detail { ty: ty.into(), value: bits, flags, ..Detail::default() }
    }
    fn vptr(bits: u64) -> Detail {
        Detail { value: bits, flags: item::VPTR, ..Detail::default() }
    }
    fn slot(index: u64, name: &str) -> Detail {
        Detail { name: name.into(), value: index, flags: item::SLOT, ..Detail::default() }
    }
    fn enumerator(name: &str, value: u64) -> Detail {
        Detail { name: name.into(), value, flags: item::ENUMERATOR, ..Detail::default() }
    }
    fn underlying(ty: &str) -> Detail {
        Detail { ty: ty.into(), flags: item::UNDERLYING, ..Detail::default() }
    }

    /// One unit's bytes holding `items` as the entry of `1S`.
    fn unit_bytes(items: Vec<Detail>) -> Vec<u8> {
        let mut b = UnitBuilder::new(producer::TEST);
        // Another row first, so that the entries' offsets differ by unit.
        let mut other = Type::named(b"1A");
        other.items = Some(vec![member("q", "c", 0)]);
        let mut t = Type::named(b"1S");
        t.items = Some(items);
        b.types.extend([other, t]);
        b.finish().unwrap()
    }

    /// The note for entries `a` (a.o) and `b` (b.o).
    fn note(a: Vec<Detail>, b: Vec<Detail>) -> String {
        fn entry<'l>(u: &Unit<'l>) -> Entry<'l> {
            let r = u.types().iter().find(|r| u.string(r.name) == b"1S").unwrap();
            u.detail_entry(r.details).unwrap().unwrap()
        }
        let (ba, bb) = (unit_bytes(a), unit_bytes(b));
        let (ua, _) = read_section(SectionAttrs::conforming(1), &ba);
        let (ub, _) = read_section(SectionAttrs::conforming(1), &bb);
        let (ea, eb) = (entry(&ua[0]), entry(&ub[0]));
        first_difference(
            &Side { unit: &ua[0], entry: ea, file: "a.o" },
            &Side { unit: &ub[0], entry: eb, file: "b.o" },
            false,
        )
        .join("\n")
    }

    /// Both directions: `a` against `b` gives `ab`, `b` against `a` gives
    /// `ba`.
    fn both(a: &[Detail], b: &[Detail], ab: &str, ba: &str) {
        assert_eq!(note(a.to_vec(), b.to_vec()), ab);
        assert_eq!(note(b.to_vec(), a.to_vec()), ba);
    }

    const EQUAL: &str =
        "the layout details are equal: the difference is in something kind 4 does not describe";

    // spec: odr.md O-19, §9.1, T-O-19 (member: a type changed; one added)
    #[test]
    fn members() {
        let a = [member("x", "i", 0), member("y", "i", 32)];
        let b = [member("x", "i", 0), member("y", "l", 64)];
        both(
            &a,
            &b,
            "member 'y' (i) at offset 4 in a.o; 'y' (l) at offset 8 in b.o",
            "member 'y' (l) at offset 8 in a.o; 'y' (i) at offset 4 in b.o",
        );
        let k4 = [member("a", "i", 0), member("extra", "i", 32), member("b", "i", 64)];
        let k4b = [member("a", "i", 0), member("b", "i", 32)];
        both(
            &k4,
            &k4b,
            "member 'extra' (i) at offset 4 in a.o only",
            "member 'extra' (i) at offset 4 in b.o only",
        );
        assert_eq!(note(a.to_vec(), a.to_vec()), EQUAL);
    }

    // spec: §9.1 (bit-fields, and the offset rule)
    #[test]
    fn bit_fields() {
        let a = [member("x", "c", 0), bitfield("y", "i", 70, 3)];
        let b = [member("x", "c", 0), member("y", "l", 64)];
        both(
            &a,
            &b,
            "member 'y' (i) at offset 70 bits, bit-field of 3 bits in a.o; 'y' (l) at offset 8 in b.o",
            "member 'y' (l) at offset 8 in a.o; 'y' (i) at offset 70 bits, bit-field of 3 bits in b.o",
        );
        // A bit-field at a byte boundary is still in bits; widths differ.
        both(
            &[bitfield("f", "j", 8, 3)],
            &[bitfield("f", "j", 8, 4)],
            "member 'f' (j) at offset 8 bits, bit-field of 3 bits in a.o; 'f' (j) at offset 8 bits, bit-field of 4 bits in b.o",
            "member 'f' (j) at offset 8 bits, bit-field of 4 bits in a.o; 'f' (j) at offset 8 bits, bit-field of 3 bits in b.o",
        );
        // An unnamed member, no type string.
        both(
            &[member("", "", 12)],
            &[member("", "", 16)],
            "unnamed member at offset 12 bits in a.o; unnamed member at offset 2 in b.o",
            "unnamed member at offset 2 in a.o; unnamed member at offset 12 bits in b.o",
        );
    }

    // spec: T-O-19 (base: a different base; virtuality)
    #[test]
    fn bases() {
        both(
            &[base("1B", 0, false), member("x", "i", 64)],
            &[base("1C", 0, false), member("x", "i", 64)],
            "base 1B at offset 0 in a.o; base 1C at offset 0 in b.o",
            "base 1C at offset 0 in a.o; base 1B at offset 0 in b.o",
        );
        both(
            &[base("1B", 0, true)],
            &[base("1B", 0, false)],
            "base 1B at offset 0 (virtual) in a.o; 1B at offset 0 in b.o",
            "base 1B at offset 0 in a.o; 1B at offset 0 (virtual) in b.o",
        );
    }

    /// The help note after a difference at a vtable slot (WP-151).
    fn with_help(note: &str) -> String {
        format!("{note}\n{SLOT_HELP}")
    }

    // spec: T-O-19 (vptr and vtable slots: a virtual function added; the
    // slot's numbering explained, WP-151)
    #[test]
    fn vptr_and_slots() {
        let a = [vptr(0), member("x", "i", 64), slot(0, "_ZN1S1fEv"), slot(1, "_ZN1S1gEv")];
        let b = [vptr(0), member("x", "i", 64), slot(0, "_ZN1S1fEv")];
        both(
            &a,
            &b,
            &with_help("vtable slot 1: _ZN1S1gEv in a.o only"),
            &with_help("vtable slot 1: _ZN1S1gEv in b.o only"),
        );
        // A class that became polymorphic: the vptr first.
        both(
            &[vptr(0), member("x", "i", 64), slot(0, "_ZN1S1fEv")],
            &[member("x", "i", 0)],
            "vptr at offset 0 in a.o only",
            "vptr at offset 0 in b.o only",
        );
        both(
            &[vptr(0), slot(0, "_ZN1S1fEv")],
            &[vptr(0), slot(0, "_ZN1T1fEv")],
            &with_help("vtable slot 0: _ZN1S1fEv in a.o; vtable slot 0: _ZN1T1fEv in b.o"),
            &with_help("vtable slot 0: _ZN1T1fEv in a.o; vtable slot 0: _ZN1S1fEv in b.o"),
        );
        both(
            &[vptr(0)],
            &[vptr(64)],
            "vptr at offset 0 in a.o; at offset 8 in b.o",
            "vptr at offset 8 in a.o; at offset 0 in b.o",
        );
    }

    // spec: T-O-19 (enumerators and the underlying type)
    #[test]
    fn enumerations() {
        both(
            &[underlying("j"), enumerator("A", 0), enumerator("B", 1)],
            &[underlying("j"), enumerator("A", 0), enumerator("B", 2)],
            "enumerator 'B' = 1 in a.o; 'B' = 2 in b.o",
            "enumerator 'B' = 2 in a.o; 'B' = 1 in b.o",
        );
        both(
            &[underlying("i"), enumerator("A", u64::MAX)],
            &[underlying("x"), enumerator("A", u64::MAX)],
            "underlying type i in a.o; x in b.o",
            "underlying type x in a.o; i in b.o",
        );
        // O-19's value is sign-extended for a signed underlying type: in
        // signed decimal then (WP-151); unsigned in unsigned decimal; an
        // underlying type whose signedness is unknown (`w`, or `c` without
        // `abi.char`), or none, as before: hex when bit 63 is set.
        both(
            &[underlying("i"), enumerator("A", u64::MAX)],
            &[underlying("i"), enumerator("A", 1)],
            "enumerator 'A' = -1 in a.o; 'A' = 1 in b.o",
            "enumerator 'A' = 1 in a.o; 'A' = -1 in b.o",
        );
        both(
            &[underlying("x"), enumerator("N0", (-2i64).cast_unsigned())],
            &[underlying("x"), enumerator("N0", (-3i64).cast_unsigned())],
            "enumerator 'N0' = -2 in a.o; 'N0' = -3 in b.o",
            "enumerator 'N0' = -3 in a.o; 'N0' = -2 in b.o",
        );
        both(
            &[underlying("y"), enumerator("A", u64::MAX)],
            &[underlying("y"), enumerator("A", 1)],
            "enumerator 'A' = 18446744073709551615 in a.o; 'A' = 1 in b.o",
            "enumerator 'A' = 1 in a.o; 'A' = 18446744073709551615 in b.o",
        );
        for unknown in [&[underlying("w")][..], &[underlying("c")], &[]] {
            let mut a = unknown.to_vec();
            a.push(enumerator("A", u64::MAX));
            let mut b = unknown.to_vec();
            b.push(enumerator("A", 1));
            both(
                &a,
                &b,
                "enumerator 'A' = 0xffffffffffffffff in a.o; 'A' = 1 in b.o",
                "enumerator 'A' = 1 in a.o; 'A' = 0xffffffffffffffff in b.o",
            );
        }
        both(
            &[underlying("i"), enumerator("A", 0)],
            &[underlying("i"), enumerator("A", 0), enumerator("C", 1), enumerator("D", 2)],
            "enumerator 'C' = 1 in b.o only, and 1 more item",
            "enumerator 'C' = 1 in a.o only, and 1 more item",
        );
    }

    fn underlying_bits(ty: &str, value: u64) -> Detail {
        Detail { value, ..underlying(ty) }
    }

    // spec: odr.md O-19 (WP-204: kiln `.d5`'s underlying-type bits, scoped
    // and fixed, rendered; a difference in the bits alone in words on both
    // sides; the reserved bits above them ignored)
    #[test]
    fn enumeration_bits() {
        let (scoped, fixed) = (item::SCOPED, item::FIXED);
        both(
            &[underlying_bits("i", fixed), enumerator("A", 0)],
            &[underlying_bits("i", scoped | fixed), enumerator("A", 0)],
            "underlying type i (unscoped, fixed) in a.o; i (scoped, fixed) in b.o",
            "underlying type i (scoped, fixed) in a.o; i (unscoped, fixed) in b.o",
        );
        both(
            &[underlying_bits("i", 0), enumerator("A", 0)],
            &[underlying_bits("i", fixed), enumerator("A", 0)],
            "underlying type i (unscoped, not fixed) in a.o; i (unscoped, fixed) in b.o",
            "underlying type i (unscoped, fixed) in a.o; i (unscoped, not fixed) in b.o",
        );
        // Another type: the bits only where they are set.
        both(
            &[underlying_bits("j", 0)],
            &[underlying_bits("i", fixed)],
            "underlying type j in a.o; i (unscoped, fixed) in b.o",
            "underlying type i (unscoped, fixed) in a.o; j in b.o",
        );
        // Reserved bits: ignored, so these entries are equal.
        let a = [underlying_bits("i", fixed | 0x40), enumerator("A", 0)];
        let b = [underlying_bits("i", fixed), enumerator("A", 0)];
        assert_eq!(note(a.to_vec(), b.to_vec()), EQUAL);
    }

    /// An unnamed class-type member whose `width` counts `n` nested items.
    fn unnamed(bits: u64, n: u32) -> Detail {
        Detail { width: n, ..member("", "", bits) }
    }

    // spec: odr.md O-19 (`width` of an unnamed class-type member, ruling
    // of 2026-11-17), §9.1 (WP-151: a nested item is named "of the
    // unnamed member at offset N"; 0 reads as before)
    #[test]
    fn nested_unnamed_members() {
        // `union { int i; float f; }; int k;` against `{ int i; long l; }`.
        let a = [unnamed(0, 2), member("i", "i", 0), member("f", "f", 0), member("k", "i", 64)];
        let b = [unnamed(0, 2), member("i", "i", 0), member("l", "l", 0), member("k", "i", 64)];
        both(
            &a,
            &b,
            "member 'f' (f) at offset 0 of the unnamed member at offset 0 in a.o; member 'l' (l) \
             at offset 0 of the unnamed member at offset 0 in b.o",
            "member 'l' (l) at offset 0 of the unnamed member at offset 0 in a.o; member 'f' (f) \
             at offset 0 of the unnamed member at offset 0 in b.o",
        );
        // An unnamed member followed by a named one is not one with two
        // members: the count tells them apart.
        both(
            &[member("x", "i", 0), unnamed(32, 1), member("p", "i", 0), member("q", "i", 64)],
            &[member("x", "i", 0), unnamed(32, 2), member("p", "i", 0), member("q", "i", 32)],
            "unnamed member at offset 4 with 1 nested item in a.o; unnamed member at offset 4 \
             with 2 nested items in b.o",
            "unnamed member at offset 4 with 2 nested items in a.o; unnamed member at offset 4 \
             with 1 nested item in b.o",
        );
        // Two deep: the innermost first; a count past its parent's items
        // ends with the parent's.
        both(
            &[unnamed(64, 3), unnamed(32, 1), member("z", "c", 8), member("w", "i", 0)],
            &[unnamed(64, 3), unnamed(32, 1), member("z", "s", 8), member("w", "i", 0)],
            "member 'z' (c) at offset 1 of the unnamed member at offset 4 of the unnamed member \
             at offset 8 in a.o; 'z' (s) at offset 1 of the unnamed member at offset 4 of the \
             unnamed member at offset 8 in b.o",
            "member 'z' (s) at offset 1 of the unnamed member at offset 4 of the unnamed member \
             at offset 8 in a.o; 'z' (c) at offset 1 of the unnamed member at offset 4 of the \
             unnamed member at offset 8 in b.o",
        );
        both(
            &[unnamed(0, 1), unnamed(0, 9), member("a", "i", 0), member("b", "i", 0)],
            &[unnamed(0, 1), unnamed(0, 9), member("a", "i", 0), member("b", "l", 0)],
            "member 'b' (i) at offset 0 in a.o; 'b' (l) at offset 0 in b.o",
            "member 'b' (l) at offset 0 in a.o; 'b' (i) at offset 0 in b.o",
        );
        // `width` 0: every following item is the class's own (as before).
        both(
            &[member("", "", 0), member("f", "f", 0)],
            &[member("", "", 0), member("l", "l", 0)],
            "member 'f' (f) at offset 0 in a.o; member 'l' (l) at offset 0 in b.o",
            "member 'l' (l) at offset 0 in a.o; member 'f' (f) at offset 0 in b.o",
        );
        // A nested member and the class's own of one name are two
        // identities.
        both(
            &[unnamed(0, 1), member("m", "i", 0)],
            &[unnamed(0, 0), member("m", "i", 0)],
            "unnamed member at offset 0 with 1 nested item in a.o; unnamed member at offset 0 in \
             b.o",
            "unnamed member at offset 0 in a.o; unnamed member at offset 0 with 1 nested item in \
             b.o",
        );
        // An unknown item kind inside the count is counted, not shown.
        let future = Detail { name: b"later".to_vec(), value: 3, flags: 9, ..Detail::default() };
        both(
            &[unnamed(0, 2), future.clone(), member("f", "f", 0), member("k", "i", 32)],
            &[unnamed(0, 2), future, member("l", "l", 0), member("k", "i", 32)],
            "member 'f' (f) at offset 0 of the unnamed member at offset 0 in a.o; member 'l' (l) \
             at offset 0 of the unnamed member at offset 0 in b.o",
            "member 'l' (l) at offset 0 of the unnamed member at offset 0 in a.o; member 'f' (f) \
             at offset 0 of the unnamed member at offset 0 in b.o",
        );
    }

    // spec: §9.1 (WP-151: two enumerations whose items are equal differ in
    // something kind 4 does not describe: a fixed underlying type, or
    // scoped against unscoped)
    #[test]
    fn equal_enumerations() {
        fn entry<'l>(u: &Unit<'l>) -> Entry<'l> {
            let r = u.types().iter().find(|r| u.string(r.name) == b"1S").unwrap();
            u.detail_entry(r.details).unwrap().unwrap()
        }
        let e = vec![underlying("j"), enumerator("H0", 0), enumerator("H1", 1)];
        let (ba, bb) = (unit_bytes(e.clone()), unit_bytes(e));
        let (ua, _) = read_section(SectionAttrs::conforming(1), &ba);
        let (ub, _) = read_section(SectionAttrs::conforming(1), &bb);
        let a = Side { unit: &ua[0], entry: entry(&ua[0]), file: "a.o" };
        let b = Side { unit: &ub[0], entry: entry(&ub[0]), file: "b.o" };
        assert_eq!(
            first_difference(&a, &b, true),
            ["the layout details are equal: the enumerations differ in something kind 4 does not \
              describe (whether the underlying type is fixed, or whether the enumeration is \
              scoped)"]
        );
        assert_eq!(first_difference(&a, &b, false), [EQUAL]);
    }

    // spec: §9.1 (replaced, reordered, and unknown item kinds)
    #[test]
    fn order_replacement_and_unknown_kinds() {
        both(
            &[member("x", "i", 0), member("y", "i", 32)],
            &[member("y", "i", 0), member("x", "i", 32)],
            "the items stand in a different order: member 'x' (i) at offset 0 in a.o; member 'y' (i) at offset 0 in b.o",
            "the items stand in a different order: member 'y' (i) at offset 0 in a.o; member 'x' (i) at offset 0 in b.o",
        );
        both(
            &[member("x", "i", 0)],
            &[member("z", "i", 0)],
            "member 'x' (i) at offset 0 in a.o; member 'z' (i) at offset 0 in b.o",
            "member 'z' (i) at offset 0 in a.o; member 'x' (i) at offset 0 in b.o",
        );
        // An item of an unknown kind is left out of the comparison (O-19).
        let future = Detail { name: b"later".to_vec(), value: 3, flags: 9, ..Detail::default() };
        assert_eq!(note(vec![member("x", "i", 0), future], vec![member("x", "i", 0)]), EQUAL);
        // Unknown flag bits do not make two items differ.
        let odd = Detail { flags: item::MEMBER | 0x100, ..member("x", "i", 0) };
        assert_eq!(note(vec![odd], vec![member("x", "i", 0)]), EQUAL);
        assert_eq!(note(Vec::new(), Vec::new()), EQUAL);
    }
}
