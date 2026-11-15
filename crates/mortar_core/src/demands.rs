//! `Demands` (`spec/model.md` §3.7): what the relocations of live sections
//! require, with no placement. Written by `mortar_arch`'s scan.
//!
//! The flags are `spec/arch.md` §4.2's, adopted by the model (M-1).

use crate::column::Column;
use crate::flags::flag_set;
use crate::id::{LocalSym, SectionId, SymKey, SymKind, SymbolId};
use crate::table::{Broken, ensure, unsorted_at};

flag_set! {
    /// What a symbol's relocations require (§3.7). The union over all
    /// relocations that mention it (§6 rule 2: bitwise or), never cleared
    /// within a link.
    pub struct DemandFlags: u16 {
        /// A GOT address slot.
        const GOT = 1 << 0;
        /// A PLT entry.
        const PLT = 1 << 1;
        /// A TLS descriptor.
        const TLS_DESC = 1 << 2;
        /// A TLS module/offset pair (general dynamic).
        const TLS_GD = 1 << 3;
        /// A thread-pointer offset slot (initial exec).
        const GOT_TPREL = 1 << 4;
        /// A copy relocation.
        const COPY = 1 << 5;
        /// A canonical PLT address.
        const CANON_PLT = 1 << 6;
        /// Export to the dynamic symbol table.
        const EXPORT = 1 << 7;
        /// An IPLT entry: a non-preemptible IFUNC, whose every address is
        /// that entry.
        const IPLT = 1 << 8;
        /// Some use forbids the GOT relaxation.
        const GOT_NORELAX = 1 << 9;
        /// Some use forbids the IE-to-LE relaxation.
        const TLSIE_NORELAX = 1 << 10;
    }
}

impl DemandFlags {
    /// The flags another file's bytes depend on (`spec/arch.md` §4.2), which
    /// the dependency index tracks per key (§7.2, M-8).
    pub const CROSS_FILE: Self =
        Self(Self::GOT_NORELAX.0 | Self::TLSIE_NORELAX.0 | Self::CANON_PLT.0 | Self::COPY.0);

    /// The flags a local symbol never demands (§3.7).
    pub const GLOBAL_ONLY: Self = Self(Self::COPY.0 | Self::CANON_PLT.0 | Self::EXPORT.0);

    /// The pairs of flags that never occur on one key, since its flags
    /// follow from one target kind (§3.7).
    pub const EXCLUSIVE: [(Self, Self); 4] = [
        (Self::IPLT, Self::PLT),
        (Self::IPLT, Self::CANON_PLT),
        (Self::IPLT, Self::COPY),
        (Self::COPY, Self::CANON_PLT),
    ];

    /// The first pair of [`Self::EXCLUSIVE`] both set here.
    #[must_use]
    pub fn conflict(self) -> Option<(Self, Self)> {
        Self::EXCLUSIVE.into_iter().find(|&(a, b)| self.contains(a) && self.contains(b))
    }
}

flag_set! {
    /// What the relocations require of the output as a whole (§3.7): the
    /// union over all of them, never cleared within a link.
    pub struct GlobalFlags: u8 {
        /// The GOT base symbol is referenced.
        const GOT_BASE = 1 << 0;
        /// The module slot of local-dynamic TLS.
        const TLS_LD = 1 << 1;
        /// `DF_STATIC_TLS`.
        const STATIC_TLS = 1 << 2;
    }
}

/// The kinds of dynamic relocation a section carries (§3.7, "by kind").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DynKind {
    /// Base-relative (`R_*_RELATIVE`, or a `.relr.dyn` entry).
    Relative,
    /// Against a symbol (absolute, `GLOB_DAT`-like).
    Symbolic,
    /// `R_*_IRELATIVE`.
    IRelative,
    /// TLS module or offset.
    Tls,
}

impl DynKind {
    /// Every kind, in index order.
    pub const ALL: [Self; 4] = [Self::Relative, Self::Symbolic, Self::IRelative, Self::Tls];
}

/// The number of dynamic relocations of each [`DynKind`] a section carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DynCounts(pub [u32; 4]);

impl DynCounts {
    #[must_use]
    pub fn get(self, kind: DynKind) -> u32 {
        self.0[kind as usize]
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0.iter().all(|&n| n == 0)
    }
}

/// The table (§3.7). Errors found on the way are emitted to the
/// collector (phase 70, §8), not stored.
#[derive(Debug, Clone, Default)]
pub struct Demands {
    /// Per global key.
    pub global: Column<SymbolId, DemandFlags>,
    /// The local symbols with a demand, sorted, none empty.
    pub local: Box<[(LocalSym, DemandFlags)]>,
    /// Per output.
    pub output: GlobalFlags,
    /// The sections that carry dynamic relocations, sorted, none empty.
    pub dyn_relocs: Box<[(SectionId, DynCounts)]>,
}

impl Demands {
    /// No demand, for `symbols` keys.
    #[must_use]
    pub fn empty(symbols: u32) -> Self {
        Self {
            global: Column::filled(symbols, DemandFlags::empty()),
            local: Box::new([]),
            output: GlobalFlags::empty(),
            dyn_relocs: Box::new([]),
        }
    }

    /// The flags of a key.
    #[must_use]
    pub fn of(&self, key: SymKey) -> DemandFlags {
        match key.kind() {
            SymKind::Global(id) => self.global.get(id).copied().unwrap_or_default(),
            SymKind::Local(sym) => self
                .local
                .binary_search_by_key(&sym, |row| row.0)
                .map_or(DemandFlags::empty(), |at| self.local[at].1),
        }
    }

    /// Every key with a flag that another file's bytes depend on
    /// ([`DemandFlags::CROSS_FILE`]), with those flags, in `SymKey` order:
    /// the flag rows of the dependency index (§7.2).
    pub fn cross_file(&self) -> impl Iterator<Item = (SymKey, DemandFlags)> + '_ {
        let globals = self.global.iter().map(|row| (SymKey::global(row.0), *row.1));
        let locals = self.local.iter().map(|row| (SymKey::local(row.0), row.1));
        globals.chain(locals).filter_map(|(key, flags)| {
            let cross = DemandFlags(flags.0 & DemandFlags::CROSS_FILE.0);
            (!cross.is_empty()).then_some((key, cross))
        })
    }

    /// Every key with a demand among `flags`, in `SymKey` order.
    pub fn keys_with(&self, flags: DemandFlags) -> impl Iterator<Item = SymKey> + '_ {
        let globals = self
            .global
            .iter()
            .filter(move |row| row.1.intersects(flags))
            .map(|row| SymKey::global(row.0));
        let locals = self
            .local
            .iter()
            .filter(move |row| row.1.intersects(flags))
            .map(|row| SymKey::local(row.0));
        globals.chain(locals)
    }

    /// The invariants of §3.7 the table shows: one row per key; the sparse
    /// lists sorted, without empty rows; a local symbol demands neither a
    /// copy relocation, a canonical PLT address nor export; no key has two
    /// flags of [`DemandFlags::EXCLUSIVE`].
    pub fn check(&self, symbols: u32) -> Result<(), Broken> {
        let t = "Demands";
        ensure(self.global.len() == symbols, t, || "global is not per key".to_owned())?;
        if let Some(at) = unsorted_at(&self.local, |row| row.0) {
            return Err(Broken { table: t, what: format!("local[{at}] out of order") });
        }
        if let Some(at) = unsorted_at(&self.dyn_relocs, |row| row.0) {
            return Err(Broken { table: t, what: format!("dyn_relocs[{at}] out of order") });
        }
        for (sym, flags) in &self.local {
            ensure(!flags.is_empty() && !flags.intersects(DemandFlags::GLOBAL_ONLY), t, || {
                format!("local ({}, {}): {flags:?}", sym.file.0, sym.index.0)
            })?;
        }
        let keys = self.global.iter().map(|row| (SymKey::global(row.0), *row.1));
        let keys = keys.chain(self.local.iter().map(|row| (SymKey::local(row.0), row.1)));
        for (key, flags) in keys {
            if let Some((a, b)) = flags.conflict() {
                return Err(Broken {
                    table: t,
                    what: format!("{key:?} demands both {a:?} and {b:?}"),
                });
            }
        }
        ensure(self.dyn_relocs.iter().all(|row| !row.1.is_empty()), t, || {
            "an empty dyn_relocs row".to_owned()
        })?;
        Ok(())
    }
}

const _: () = assert!(size_of::<DemandFlags>() == 2);
const _: () = assert!(size_of::<GlobalFlags>() == 1);
const _: () = assert!(size_of::<(LocalSym, DemandFlags)>() == 12);
