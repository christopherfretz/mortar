//! The demand flags of §4.2: per `SymKey` and per output.
//!
//! These are the flag words `spec/arch.md` asks `spec/model.md` to adopt
//! (M-1). Until the model's `Demands` carries them (WP-020c), scan writes
//! them through [`crate::DemandSink`], which owns the mapping.

macro_rules! flag_word {
    (
        $(#[$meta:meta])*
        pub struct $name:ident: $bits:ty {
            $( $(#[$fmeta:meta])* const $flag:ident = $value:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name($bits);

        impl $name {
            $( $(#[$fmeta])* pub const $flag: Self = Self($value); )*

            /// Every named flag with its name, in declaration order.
            pub const NAMED: &'static [(&'static str, Self)] =
                &[$((stringify!($flag), Self::$flag)),*];

            #[must_use]
            pub const fn empty() -> Self {
                Self(0)
            }

            #[must_use]
            pub const fn bits(self) -> $bits {
                self.0
            }

            /// The set with these bits, if each is a named flag.
            #[must_use]
            pub const fn from_bits(bits: $bits) -> Option<Self> {
                let all = 0 $(| $value)*;
                if bits & !all == 0 { Some(Self(bits)) } else { None }
            }

            #[must_use]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            #[must_use]
            pub const fn intersects(self, other: Self) -> bool {
                self.0 & other.0 != 0
            }

            #[must_use]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            pub fn insert(&mut self, other: Self) {
                self.0 |= other.0;
            }
        }

        impl std::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, other: Self) -> Self {
                self.union(other)
            }
        }

        impl std::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, other: Self) {
                self.insert(other);
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut set = f.debug_set();
                for (name, flag) in Self::NAMED {
                    if self.contains(*flag) {
                        set.entry(&format_args!("{name}"));
                    }
                }
                set.finish()
            }
        }
    };
}

flag_word! {
    /// Per-`SymKey` demands (§4.2). A flag means "some valid layout may
    /// use this" (or, for the `*_NORELAX` flags, "some use forbids this
    /// rewrite"); it is never cleared within a link.
    pub struct DemandFlags: u16 {
        const GOT = 1 << 0;
        const PLT = 1 << 1;
        const CANON_PLT = 1 << 2;
        const COPY = 1 << 3;
        const EXPORT = 1 << 4;
        /// A non-preemptible IFUNC: an IPLT entry, its address everywhere.
        const IPLT = 1 << 5;
        const GOT_TPREL = 1 << 6;
        const TLS_GD = 1 << 7;
        const TLS_DESC = 1 << 8;
        /// Some use forbids the GOT relaxation (a64.md R5).
        const GOT_NORELAX = 1 << 9;
        /// Some use forbids IE → LE (a64.md R3).
        const TLSIE_NORELAX = 1 << 10;
    }
}

flag_word! {
    /// Per-output demands (§4.2).
    pub struct GlobalFlags: u8 {
        /// The GOT base symbol is referenced.
        const GOT_BASE = 1 << 0;
        /// The module slot of local-dynamic TLS.
        const TLS_LD = 1 << 1;
        /// `DF_STATIC_TLS`.
        const STATIC_TLS = 1 << 2;
    }
}

impl DemandFlags {
    /// The flags that change other files' bytes and so belong in the
    /// incremental dependency index (§4.2, M-8).
    pub const CROSS_FILE: Self =
        Self(Self::GOT_NORELAX.0 | Self::TLSIE_NORELAX.0 | Self::CANON_PLT.0 | Self::COPY.0);
}
