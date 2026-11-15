//! A hand-written flag-set newtype (no third-party `bitflags`): the shape
//! of every small set of named bits in the tables.

/// `flag_set! { /// doc
///     pub struct Name: u8 { /// doc
///         const A = 1 << 0; ... } }`
macro_rules! flag_set {
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

            /// Every named flag, with its name, in declaration order.
            pub const NAMED: &'static [(&'static str, Self)] = &[$((stringify!($flag), Self::$flag)),*];

            /// No flag.
            #[must_use]
            pub const fn empty() -> Self {
                Self(0)
            }

            /// The bits.
            #[must_use]
            pub const fn bits(self) -> $bits {
                self.0
            }

            /// The set with these bits, if every bit is a named flag.
            #[must_use]
            pub const fn from_bits(bits: $bits) -> Option<Self> {
                let all = 0 $(| $value)*;
                if bits & !all == 0 { Some(Self(bits)) } else { None }
            }

            #[must_use]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// Whether every flag of `other` is set.
            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            /// Whether some flag of `other` is set.
            #[must_use]
            pub const fn intersects(self, other: Self) -> bool {
                self.0 & other.0 != 0
            }

            /// The flags of either set.
            #[must_use]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            /// Sets the flags of `other`.
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

pub(crate) use flag_set;
