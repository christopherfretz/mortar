//! Kind 4's note (`spec/odr.md` O-19, §9.1; WP-145): the first difference
//! between two layout-detail entries, rendered for a developer. Kind 4
//! never decides a finding: the record check of kind 3 has decided that
//! two classes' layouts differ; this module only says where.

use mortar_meta::Unit;
use mortar_meta::details::{Entry, Item, item};

/// One side of the comparison: the unit (for its strings), the entry and
/// the file as a note writes it.
pub(crate) struct Side<'u, 'l> {
    pub(crate) unit: &'u Unit<'l>,
    pub(crate) entry: Entry<'l>,
    pub(crate) file: &'u str,
}

/// An item with its strings resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resolved<'l> {
    kind: u32,
    name: &'l [u8],
    ty: &'l [u8],
    value: u64,
    width: u32,
    flags: u32,
}

impl<'l> Resolved<'l> {
    fn new(unit: &Unit<'l>, it: Item) -> Self {
        Self {
            kind: it.kind(),
            name: unit.string(it.name),
            ty: unit.string(it.ty),
            value: it.value,
            width: it.width,
            flags: it.flags & item::KNOWN_FLAGS,
        }
    }

    /// §9.1's identity: kind and name (member, slot, enumerator), kind
    /// and type (base), kind alone (vptr, underlying type).
    fn identity(&self) -> (u32, &'l [u8]) {
        match self.kind {
            item::MEMBER | item::SLOT | item::ENUMERATOR => (self.kind, self.name),
            item::BASE => (self.kind, self.ty),
            _ => (self.kind, b""),
        }
    }

    /// §9.1's `D` as (kind words, the rest): `D'` is the rest.
    fn text(&self) -> (&'static str, String) {
        let lossy = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let bitfield = self.flags & item::BITFIELD != 0;
        let offset = || offset_text(self.value, bitfield);
        match self.kind {
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
                let v = if self.value >> 63 == 0 {
                    self.value.to_string()
                } else {
                    format!("{:#x}", self.value)
                };
                ("enumerator ", format!("'{}' = {v}", lossy(self.name)))
            }
            _ => ("underlying type ", lossy(self.ty)),
        }
    }

    fn whole(&self) -> String {
        let (k, rest) = self.text();
        format!("{k}{rest}")
    }
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

fn resolved<'l>(side: &Side<'_, 'l>) -> Vec<Resolved<'l>> {
    side.entry.items().map(|it| Resolved::new(side.unit, it)).collect()
}

/// §9.1's note on the first difference of `a`'s and `b`'s entries.
pub(crate) fn first_difference(a: &Side<'_, '_>, b: &Side<'_, '_>) -> String {
    let (xa, xb) = (resolved(a), resolved(b));
    let (fa, fb) = (a.file, b.file);
    let Some(i) = (0..xa.len().max(xb.len())).find(|&i| xa.get(i) != xb.get(i)) else {
        return "the layout details are equal: the difference is in something kind 4 does not \
                describe"
            .to_owned();
    };
    let only = |items: &[Resolved<'_>], file: &str| {
        let rest = items.len() - i - 1;
        let more = match rest {
            0 => String::new(),
            1 => ", and 1 more item".to_owned(),
            n => format!(", and {n} more items"),
        };
        format!("{} in {file} only{more}", items[i].whole())
    };
    let (Some(ia), Some(ib)) = (xa.get(i), xb.get(i)) else {
        return if i < xa.len() { only(&xa, fa) } else { only(&xb, fb) };
    };
    if ia.identity() == ib.identity() {
        return format!("{} in {fa}; {} in {fb}", ia.whole(), ib.text().1);
    }
    let a_in_b = xb.iter().any(|x| x.identity() == ia.identity());
    let b_in_a = xa.iter().any(|x| x.identity() == ib.identity());
    match (a_in_b, b_in_a) {
        (false, true) => format!("{} in {fa} only", ia.whole()),
        (true, false) => format!("{} in {fb} only", ib.whole()),
        (false, false) => format!("{} in {fa}; {} in {fb}", ia.whole(), ib.whole()),
        (true, true) => format!(
            "the items stand in a different order: {} in {fa}; {} in {fb}",
            ia.whole(),
            ib.whole()
        ),
    }
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
        )
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

    // spec: T-O-19 (vptr and vtable slots: a virtual function added)
    #[test]
    fn vptr_and_slots() {
        let a = [vptr(0), member("x", "i", 64), slot(0, "_ZN1S1fEv"), slot(1, "_ZN1S1gEv")];
        let b = [vptr(0), member("x", "i", 64), slot(0, "_ZN1S1fEv")];
        both(
            &a,
            &b,
            "vtable slot 1: _ZN1S1gEv in a.o only",
            "vtable slot 1: _ZN1S1gEv in b.o only",
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
            "vtable slot 0: _ZN1S1fEv in a.o; vtable slot 0: _ZN1T1fEv in b.o",
            "vtable slot 0: _ZN1T1fEv in a.o; vtable slot 0: _ZN1S1fEv in b.o",
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
        both(
            &[underlying("i"), enumerator("A", u64::MAX)],
            &[underlying("i"), enumerator("A", 1)],
            "enumerator 'A' = 0xffffffffffffffff in a.o; 'A' = 1 in b.o",
            "enumerator 'A' = 1 in a.o; 'A' = 0xffffffffffffffff in b.o",
        );
        both(
            &[underlying("i"), enumerator("A", 0)],
            &[underlying("i"), enumerator("A", 0), enumerator("C", 1), enumerator("D", 2)],
            "enumerator 'C' = 1 in b.o only, and 1 more item",
            "enumerator 'C' = 1 in a.o only, and 1 more item",
        );
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
