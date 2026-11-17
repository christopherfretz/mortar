//! Kind 4, layout details (`spec/odr.md` O-19): the typed view of one
//! entry of a kind 4 block.
//!
//! The block's body (after its 16-byte header) is a sequence of entries,
//! each at an offset that is a multiple of 8 from the body's start; a kind
//! 3 row's `details` is its entry's offset. An entry is `{u32 count; u32
//! reserved}` and `count` items of 24 bytes `{u32 name; u32 type; u64
//! value; u32 width; u32 flags}`.
//!
//! Nothing of kind 4 is validated when a unit is opened: an entry is read
//! and validated when a diagnostic asks for it ([`crate::Unit::detail_entry`];
//! MT-42, O-19: kind 4 is read only to render a finding kind 3 made). The
//! reader is lenient where O-19 leaves a writer free (O-19's last
//! paragraph, WP-145): entries may share bytes, overlap, leave gaps and
//! stand in any order; `count` 0 is an entry; `count` is bounded by the
//! block alone; a set `reserved`, an item kind above 5 and the flag bits
//! above 5 are ignored (an unknown item is skipped whole, its strings
//! unchecked).

use core::fmt;

use crate::read::{string_at, u32_at, u64_at};

/// An entry's header: `count` and `reserved`.
pub const ENTRY_HEADER: u32 = 8;
/// An item's size.
pub const ITEM_SIZE: u32 = 24;

/// Item kinds (bits 0-3 of `flags`) and the flag bits (O-19).
pub mod item {
    /// A non-static data member: `name`, `type`, `value` its offset in
    /// bits, `width` and [`BITFIELD`] for a bit-field.
    pub const MEMBER: u32 = 0;
    /// A base: `type` its `<type>` encoding, `value` its offset in bits,
    /// [`VIRTUAL`].
    pub const BASE: u32 = 1;
    /// The vptr: `value` its offset in bits.
    pub const VPTR: u32 = 2;
    /// A vtable slot: `name` the final overrider's declaration, `value`
    /// the slot's index over the whole vtable group.
    pub const SLOT: u32 = 3;
    /// An enumerator: `name`, `value`.
    pub const ENUMERATOR: u32 = 4;
    /// An enumeration's underlying type: `type`, and in `value` (from kiln's
    /// `kiln-odr-1.d5`; 0 before) [`SCOPED`] and [`FIXED`], the bits O-18
    /// already hashes; a reader ignores the others (WP-204).
    pub const UNDERLYING: u32 = 5;
    /// An underlying-type item's `value`: the enumeration is scoped.
    pub const SCOPED: u64 = 1 << 0;
    /// An underlying-type item's `value`: the underlying type is fixed
    /// (written, or implied by `enum class`).
    pub const FIXED: u64 = 1 << 1;
    /// The bits of `flags` that hold the kind.
    pub const KIND_MASK: u32 = 0xf;
    pub const BITFIELD: u32 = 1 << 4;
    pub const VIRTUAL: u32 = 1 << 5;
    /// Every bit O-19 assigns; a reader ignores the others.
    pub const KNOWN_FLAGS: u32 = KIND_MASK | BITFIELD | VIRTUAL;
}

/// Whether an enumeration whose underlying type is the `<builtin-type>`
/// encoding `ty` is signed (O-19's enumerator `value` is sign-extended
/// when it is, WP-151): `Some(true)` for `a` (signed char), `s`, `i`,
/// `l`, `x` and `n` (`__int128`); `Some(false)` for `h`, `t`, `j`, `m`,
/// `y`, `o`, `b`, `Ds`, `Di` and `Du`; `c` (plain `char`) by `char_signed`
/// (kind 1's `abi.char`, MT-26); `None` for anything else (`w`, whose
/// signedness is the target's, and any other string).
#[must_use]
pub fn signed_underlying(ty: &[u8], char_signed: Option<bool>) -> Option<bool> {
    match ty {
        b"a" | b"s" | b"i" | b"l" | b"x" | b"n" => Some(true),
        b"h" | b"t" | b"j" | b"m" | b"y" | b"o" | b"b" | b"Ds" | b"Di" | b"Du" => Some(false),
        b"c" => char_signed,
        _ => None,
    }
}

/// An unnamed class-type member's item (O-19, ruling of 2026-11-17): a
/// member item with `name` and `type` 0 that is not a bit-field. Its
/// `width` is the number of items after it that describe the nested
/// class, counted through every depth; 0 (kiln before the ruling) means
/// unknown, and a reader reads every following item as the class's own.
#[must_use]
pub const fn nested_count(it: &Item) -> u32 {
    if it.kind() == item::MEMBER && it.name == 0 && it.ty == 0 && !it.bitfield() {
        it.width
    } else {
        0
    }
}

/// For each item of `items` (an entry's items in order, every kind
/// included), the indices of the unnamed members it is nested in, the
/// innermost first (O-19's `width`). A count that runs past the items of
/// the member it is itself nested in (or past the entry) ends there: the
/// rendering never reads past an entry, and such a count describes no
/// other item.
#[must_use]
pub fn nesting(items: &[Item]) -> Vec<Vec<usize>> {
    let mut out = Vec::with_capacity(items.len());
    // (index of the unnamed member, end of its items, exclusive).
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        while open.last().is_some_and(|&(_, end)| end <= i) {
            open.pop();
        }
        out.push(open.iter().rev().map(|&(at, _)| at).collect());
        let n = nested_count(it) as usize;
        if n > 0 {
            let limit = open.last().map_or(items.len(), |&(_, end)| end);
            open.push((i, (i + 1).saturating_add(n).min(limit)));
        }
    }
    out
}

/// One item as the block holds it: `name` and `ty` are string offsets in
/// the unit's table (MT-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub name: u32,
    pub ty: u32,
    pub value: u64,
    pub width: u32,
    pub flags: u32,
}

impl Item {
    fn decode(b: &[u8]) -> Self {
        Self {
            name: u32_at(b, 0),
            ty: u32_at(b, 4),
            value: u64_at(b, 8),
            width: u32_at(b, 16),
            flags: u32_at(b, 20),
        }
    }

    /// The item kind (bits 0-3 of `flags`).
    #[must_use]
    pub const fn kind(&self) -> u32 {
        self.flags & item::KIND_MASK
    }

    /// Whether this reader knows the item's kind (0-5).
    #[must_use]
    pub const fn known(&self) -> bool {
        self.kind() <= item::UNDERLYING
    }

    #[must_use]
    pub const fn bitfield(&self) -> bool {
        self.flags & item::BITFIELD != 0
    }

    #[must_use]
    pub const fn is_virtual(&self) -> bool {
        self.flags & item::VIRTUAL != 0
    }
}

/// One validated entry: every item is within the block, and every string
/// of an item of a known kind is in the table.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// The entry's offset in the body.
    pub offset: u32,
    pub count: u32,
    /// Ignored by a reader (O-19); kept for the dumper.
    pub reserved: u32,
    items: &'a [u8],
}

impl<'a> Entry<'a> {
    /// Every item, known kinds or not, in entry order.
    pub fn all_items(&self) -> impl ExactSizeIterator<Item = Item> + 'a {
        let items = self.items;
        (0..self.count as usize).map(move |i| {
            let at = i * ITEM_SIZE as usize;
            Item::decode(items.get(at..at + ITEM_SIZE as usize).unwrap_or_default())
        })
    }

    /// The items of the kinds this reader knows, in entry order (O-19: a
    /// reader ignores an item kind it does not know).
    pub fn items(&self) -> impl Iterator<Item = Item> + 'a {
        self.all_items().filter(Item::known)
    }

    /// The entry's size in the body: its header and its items.
    #[must_use]
    pub const fn size(&self) -> u64 {
        ENTRY_HEADER as u64 + self.count as u64 * ITEM_SIZE as u64
    }
}

/// Why the bytes at an entry's offset are not an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The unit has no kind 4 block.
    NoBlock,
    /// The offset is not a multiple of 8.
    Misaligned,
    /// The entry's 8-byte header runs past the block (`len`: the body's
    /// size).
    Header { len: u32 },
    /// `count` items run past the block.
    Items { count: u32, len: u32 },
    /// Item `item` (from 0) holds a string offset past the table, or a
    /// string without NUL.
    String { item: u32, offset: u32 },
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBlock => f.write_str("the unit has no kind 4 block"),
            Self::Misaligned => f.write_str("not a multiple of 8"),
            Self::Header { len } => write!(f, "its header runs past the block's {len} bytes"),
            Self::Items { count, len } => {
                write!(f, "its {count} items run past the block's {len} bytes")
            }
            Self::String { item, offset } => {
                write!(f, "item {item}: string offset {offset} is past the table or has no NUL")
            }
        }
    }
}

/// The entry at `offset` of the kind 4 body `body`, its strings in
/// `strtab` (the unit's table).
///
/// # Errors
/// [`Fault`]: never a panic, whatever the bytes.
pub fn entry<'a>(body: &'a [u8], strtab: &[u8], offset: u32) -> Result<Entry<'a>, Fault> {
    if !offset.is_multiple_of(8) {
        return Err(Fault::Misaligned);
    }
    let len = u32::try_from(body.len()).unwrap_or(u32::MAX);
    let start = offset as usize;
    let Some(header) = body.get(start..).and_then(|b| b.get(..ENTRY_HEADER as usize)) else {
        return Err(Fault::Header { len });
    };
    let count = u32_at(header, 0);
    let reserved = u32_at(header, 4);
    // u64: `count * 24` cannot overflow, and the sum stays below 2^40.
    let end = u64::from(offset) + u64::from(ENTRY_HEADER) + u64::from(count) * u64::from(ITEM_SIZE);
    if end > body.len() as u64 {
        return Err(Fault::Items { count, len });
    }
    let items = body.get(start + ENTRY_HEADER as usize..end as usize).unwrap_or_default();
    let e = Entry { offset, count, reserved, items };
    for (i, it) in e.all_items().enumerate() {
        if !it.known() {
            continue;
        }
        for s in [it.name, it.ty] {
            if string_at(strtab, s).is_none() {
                return Err(Fault::String { item: i as u32, offset: s });
            }
        }
    }
    Ok(e)
}
