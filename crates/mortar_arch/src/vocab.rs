//! The vocabulary of `spec/arch.md` §2: output and target kinds, value
//! terms, fields, checks and the `Howto` of a relocation type.

use mortar_elf::Class;

use crate::class::DemandClass;
use crate::error::{FieldError, Range};

/// The kind of output being linked (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutputKind {
    /// `ET_EXEC` without `.dynamic`.
    Static,
    /// `ET_EXEC` with `.dynamic`.
    Exec,
    /// `ET_DYN` with `DF_1_PIE`, a static PIE included.
    Pie,
    /// An `ET_DYN` shared object.
    Shared,
    /// `-r`: no relocation is applied (§3.3).
    Relocatable,
}

impl OutputKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 5] =
        [Self::Static, Self::Exec, Self::Pie, Self::Shared, Self::Relocatable];

    /// An executable: `Static`, `Exec` or `Pie` (a64.md §8 "exec output").
    #[must_use]
    pub const fn is_exec(self) -> bool {
        matches!(self, Self::Static | Self::Exec | Self::Pie)
    }

    /// Position-independent: `Pie` or `Shared` (the second row of each
    /// class in §4.3).
    #[must_use]
    pub const fn is_pic(self) -> bool {
        matches!(self, Self::Pie | Self::Shared)
    }
}

/// What a relocation's symbol is, for this output (§2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TargetKind {
    /// Defined in this output, not preemptible, not IFUNC, not `SHN_ABS`.
    Local,
    /// Defined, `SHN_ABS`: does not move with the load address.
    Abs,
    /// Defined in this output (`Shared`), may be preempted at load time.
    Preemptible,
    /// Defined only in a shared object (`Exec`, `Pie`).
    Shlib { func: bool },
    /// A non-preemptible `STT_GNU_IFUNC` defined in this output.
    Ifunc,
    /// Undefined weak, not dynamic: value 0.
    Weak0,
    /// Undefined weak, in `.dynsym` and preemptible.
    WeakDyn,
    /// Undefined, not weak, defined nowhere and not made dynamic (§3.4).
    Undefined,
}

impl TargetKind {
    /// Every kind (`Shlib` both ways), in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Local,
        Self::Abs,
        Self::Preemptible,
        Self::Shlib { func: false },
        Self::Shlib { func: true },
        Self::Ifunc,
        Self::Weak0,
        Self::WeakDyn,
        Self::Undefined,
    ];

    /// Defined in this output (`Local`, `Abs`, `Preemptible`, `Ifunc`).
    #[must_use]
    pub const fn is_defined_here(self) -> bool {
        matches!(self, Self::Local | Self::Abs | Self::Preemptible | Self::Ifunc)
    }
}

/// The base term of a value (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    Zero,
    /// The symbol's address (its PLT/IPLT entry when canonical).
    S,
    /// The PLT (or IPLT) entry when the class demanded one, else `S`.
    L,
    /// The address of the symbol's GOT slot of that kind.
    G(SlotKind),
    /// The architecture's GOT base (§3.1 `got_base`).
    GotBase,
    /// The TP offset of `S + A` (§3.8; its sign is the variant's).
    TpRel,
    /// `S + A − tls.start`.
    DtpRel,
    /// `st_size` of the resolved definition.
    Size,
}

/// The kind of a GOT-like slot (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotKind {
    /// The symbol's address.
    Addr,
    /// The thread-pointer offset; `negated` holds its negation (i386).
    TpOff { negated: bool },
    /// A general-dynamic module/offset pair.
    TlsGd,
    /// The output's local-dynamic module pair.
    TlsLd,
    /// A TLS descriptor.
    TlsDesc,
    /// The `.got.plt` word of the symbol's PLT entry.
    PltGot,
}

/// What is subtracted from the base (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Minus {
    None,
    P,
    PageP,
    GotBase,
    PageGotBase,
}

/// `X = ±((page ? Page(B + a) : B + a) − M)` (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value {
    pub base: Base,
    pub add_a: bool,
    pub page: bool,
    pub negate: bool,
    pub minus: Minus,
}

/// The resolved terms a [`Value`] is computed from. `b` is the value of
/// the [`Base`] (for `TpRel` and `DtpRel` it already includes `S + A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Terms {
    pub b: u64,
    pub a: i64,
    pub p: u64,
    pub got: u64,
}

impl Value {
    /// A value with no addend, no page, no negation and nothing subtracted.
    #[must_use]
    pub const fn of(base: Base) -> Self {
        Self { base, add_a: false, page: false, negate: false, minus: Minus::None }
    }

    /// `X` in 64-bit two's complement (§2.2, §2.3), `Page(x) = x &
    /// !page_mask`.
    #[must_use]
    pub const fn eval(&self, t: Terms, page_mask: u64) -> i64 {
        let mut v = t.b;
        if self.add_a {
            v = v.wrapping_add(t.a as u64);
        }
        if self.page {
            v &= !page_mask;
        }
        let m = match self.minus {
            Minus::None => 0,
            Minus::P => t.p,
            Minus::PageP => t.p & !page_mask,
            Minus::GotBase => t.got,
            Minus::PageGotBase => t.got & !page_mask,
        };
        let x = v.wrapping_sub(m) as i64;
        if self.negate { x.wrapping_neg() } else { x }
    }
}

/// Where the value goes (§2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// A marker: writes nothing.
    None,
    /// 1, 2, 4 or 8 bytes, little-endian at `P`.
    Data { bytes: u8 },
    /// An architecture's instruction form (a64.md §3.2).
    Insn { form: u16 },
}

impl Field {
    /// The number of bytes the field occupies at `P`: 0 for a marker, 4
    /// for every instruction form of the three targets.
    #[must_use]
    pub const fn width(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Data { bytes } => bytes,
            Self::Insn { .. } => 4,
        }
    }
}

/// The overflow check of a field (§2.3, a64.md §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Check {
    None,
    /// `s(n)`: `−2^(n−1) ≤ X < 2^(n−1)`.
    Signed(u8),
    /// `u(n)`: `0 ≤ X < 2^n`.
    Unsigned(u8),
    /// `su(n)`: `−2^(n−1) ≤ X < 2^n`.
    Either(u8),
    /// `ms(n)`: `−2^n ≤ X < 2^n`.
    MovSigned(u8),
}

impl Check {
    /// The inclusive bounds `[min, max]` the check accepts, or `None` for
    /// `Check::None`.
    #[must_use]
    pub const fn bounds(self) -> Option<(i128, i128)> {
        // Widths are at most 64 (the tables' own); `n − 1` saturates so a
        // zero width cannot underflow.
        const fn p(n: u8) -> i128 {
            1i128 << (if n > 100 { 100 } else { n })
        }
        match self {
            Self::None => None,
            Self::Signed(n) => Some((-p(n.saturating_sub(1)), p(n.saturating_sub(1)) - 1)),
            Self::Unsigned(n) => Some((0, p(n) - 1)),
            Self::Either(n) => Some((-p(n.saturating_sub(1)), p(n) - 1)),
            Self::MovSigned(n) => Some((-p(n), p(n) - 1)),
        }
    }

    /// Whether `x` satisfies the check in an output of `class` (§2.3).
    ///
    /// ELFCLASS64: the check applies to the 64-bit two's-complement `x`.
    /// ELFCLASS32: `x` is reduced modulo 2^32 and read as a 32-bit value,
    /// unsigned for `Unsigned`, signed otherwise; `Either(32)` then accepts
    /// every value. On failure the error carries the value as the check
    /// saw it and the bounds, for the message (§7).
    pub fn verify(self, x: i64, class: Class) -> Result<(), FieldError> {
        let Some((min, max)) = self.bounds() else { return Ok(()) };
        let seen: i128 = match class {
            Class::Elf64 => i128::from(x),
            Class::Elf32 => match self {
                Self::Unsigned(_) => i128::from(x as u32),
                _ => i128::from(x as i32),
            },
        };
        if class == Class::Elf32 && self == Self::Either(32) {
            return Ok(());
        }
        if seen < min || seen > max {
            return Err(FieldError::Range(Range { value: seen, min, max }));
        }
        Ok(())
    }
}

/// `a(k)` (a64.md §3.3): `x mod k = 0` for a power of two `k`; 1 is no
/// check.
pub fn verify_align(x: i64, align: u8) -> Result<(), FieldError> {
    if align > 1 && (x as u64) & (u64::from(align) - 1) != 0 {
        return Err(FieldError::Misaligned { value: x, align });
    }
    Ok(())
}

/// Whether a type may carry a non-zero addend (§2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddendRule {
    Any,
    /// GOT and TLS-model forms whose `G` term ignores `A` (a64.md §3.1).
    MustBeZero,
}

/// REL: where and how the implicit addend is stored (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Implicit {
    pub bytes: u8,
    pub signed: bool,
    pub shift: u8,
}

/// The bytes a rewrite may touch around the field (§3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Window {
    pub before: u8,
    pub after: u8,
}

/// A type's milestone (§3.2; a64.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    M1,
    M2,
    Later,
    Refused,
}

/// What a relocation type means (§2.3). `Copy`: `classify` hands it back
/// by value and the caller keeps it per type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Howto {
    /// The ABI name, for diagnostics and refusals.
    pub name: &'static str,
    pub value: Value,
    pub field: Field,
    pub check: Check,
    /// `a(k)`: `X mod k = 0`, 1 when none.
    pub align: u8,
    pub class: DemandClass,
    pub addend: AddendRule,
    /// REL: where the addend is read (§3.3).
    pub implicit: Option<Implicit>,
    /// Bytes a rewrite may touch around the field (§3.6).
    pub window: Window,
    pub status: Status,
}

/// The input section a relocation applies to, and the place in it
/// (§3.2).
#[derive(Debug, Clone, Copy)]
pub struct Site<'a> {
    /// The bytes of the input section.
    pub bytes: &'a [u8],
    /// `r_offset`.
    pub offset: u64,
}

impl<'a> Site<'a> {
    /// The `n` bytes at the place, if they lie in the section.
    #[must_use]
    pub fn at(&self, n: usize) -> Option<&'a [u8]> {
        let start = usize::try_from(self.offset).ok()?;
        self.bytes.get(start..start.checked_add(n)?)
    }

    /// The little-endian word at `offset + delta`, if it lies in the
    /// section.
    #[must_use]
    pub fn word(&self, delta: i64) -> Option<u32> {
        let start = usize::try_from(self.offset.checked_add_signed(delta)?).ok()?;
        let b = self.bytes.get(start..start.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// `align_up(x, a)` for a power-of-two `a` (0 and 1 leave `x`).
#[must_use]
pub const fn align_up(x: u64, a: u64) -> u64 {
    if a <= 1 { x } else { x.wrapping_add(a - 1) & !(a - 1) }
}
