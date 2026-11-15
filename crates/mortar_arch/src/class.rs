//! Demand classes (§4.3): the class × output kind × target kind table,
//! as data, and the rules applied before it (non-alloc places, TLS
//! mismatch, undefined targets).

use crate::error::Incompatible;
use crate::flags::{DemandFlags, GlobalFlags};
use crate::shapes::DynRole;
use crate::vocab::{OutputKind, TargetKind};

/// The demand class of a relocation type (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DemandClass {
    AbsWord,
    AbsNarrow,
    Pc,
    /// Low bits of an address paired with a `Pc` page.
    Lo,
    Branch,
    Got,
    /// A value relative to the GOT base.
    GotRel,
    /// The GOT base itself (x86 `GOTPC`).
    GotBaseRef,
    Size,
    TlsGd,
    TlsLd,
    TlsIe,
    TlsDesc,
    TlsLe,
    TlsDtpRel,
    None,
}

impl DemandClass {
    /// Every class, in declaration order.
    pub const ALL: [Self; 16] = [
        Self::AbsWord,
        Self::AbsNarrow,
        Self::Pc,
        Self::Lo,
        Self::Branch,
        Self::Got,
        Self::GotRel,
        Self::GotBaseRef,
        Self::Size,
        Self::TlsGd,
        Self::TlsLd,
        Self::TlsIe,
        Self::TlsDesc,
        Self::TlsLe,
        Self::TlsDtpRel,
        Self::None,
    ];

    /// A TLS class: its target must be thread-local (§4.3).
    #[must_use]
    pub const fn is_tls(self) -> bool {
        matches!(
            self,
            Self::TlsGd | Self::TlsLd | Self::TlsIe | Self::TlsDesc | Self::TlsLe | Self::TlsDtpRel
        )
    }

    /// The classes whose `Field::Data` relocations are allowed in a
    /// non-alloc section (§4.3 "Non-alloc places").
    #[must_use]
    pub const fn allowed_in_non_alloc(self) -> bool {
        matches!(
            self,
            Self::AbsWord
                | Self::AbsNarrow
                | Self::Pc
                | Self::GotRel
                | Self::TlsDtpRel
                | Self::Size
        )
    }
}

/// §4.3 "Non-alloc places": an `AbsWord` or `AbsNarrow` relocation at a
/// non-alloc place against an `STT_TLS` target writes `DtpRel(S + A)`, the
/// offset in the TLS block, not an address (GCC `-gsplit-dwarf`
/// `.debug_addr` entries, `spec/x64.md` X-53). The one exception to the
/// TLS mismatch rule; `false` at an alloc place.
#[must_use]
pub const fn writes_dtprel(class: DemandClass, place: PlaceFacts, tls: bool) -> bool {
    !place.alloc && tls && matches!(class, DemandClass::AbsWord | DemandClass::AbsNarrow)
}

/// One cell of the §4.3 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cell {
    /// `—`.
    Nothing,
    /// `R` + `TX`: a `Relative` dynamic relocation; `E` when read-only.
    RelativeTx,
    /// `D` + `TX`: a `Symbolic` one with `EXPORT`; `E` when read-only.
    SymbolicTx,
    /// `writable D, else C`.
    WritableSymbolicElseCopy,
    /// `writable D, else E`.
    WritableSymbolicElseError,
    /// `C`: `COPY` for data, `CANON_PLT` for a function.
    Copy,
    /// `I`.
    Iplt,
    /// `I`, `R` + `TX`.
    IpltRelativeTx,
    /// `E`.
    Error,
    /// `PLT`.
    Plt,
    /// `GOT`.
    Got,
    /// `GOT`, `I`.
    GotIplt,
    /// `GOT_BASE`.
    GotBase,
    /// `I`, `GOT_BASE`.
    IpltGotBase,
    /// The architecture's TLS table (a64.md §8, §10.1, §12).
    Tls,
    /// `—` if the target is defined here, else `E` (`TlsLe`, `TlsDtpRel`).
    DefinedHereElseError,
    /// The architecture spec's §7 decides (`Pc` against `Weak0` in `Pie`
    /// and `Shared`: x86-64 `E`, `spec/x64.md` X-15; AArch64 `—` with
    /// `S = P`).
    ArchWeak0,
    /// `E` with the reason `local-exec TLS in a shared object`.
    LocalExecInShared,
    /// A blank cell: the output kind never has this target kind.
    Impossible,
}

/// The target kinds that have a column, in the table's order. `Undefined`
/// has none (every class gives `undefined-symbol`).
pub const COLUMNS: [TargetKind; 8] = [
    TargetKind::Local,
    TargetKind::Abs,
    TargetKind::Preemptible,
    TargetKind::Shlib { func: false },
    TargetKind::Shlib { func: true },
    TargetKind::Ifunc,
    TargetKind::Weak0,
    TargetKind::WeakDyn,
];

const fn column(t: TargetKind) -> usize {
    match t {
        TargetKind::Local => 0,
        TargetKind::Abs => 1,
        TargetKind::Preemptible => 2,
        TargetKind::Shlib { .. } => 3,
        TargetKind::Ifunc => 4,
        TargetKind::Weak0 => 5,
        TargetKind::WeakDyn | TargetKind::Undefined => 6,
    }
}

use Cell::{
    ArchWeak0 as AW0, Copy as C, Error as E, Got as G, GotBase as GB, GotIplt as GI,
    Impossible as X, Iplt as I, IpltGotBase as IGB, IpltRelativeTx as IRT, Nothing as N,
    Plt as PLT, RelativeTx as RT, SymbolicTx as DT, WritableSymbolicElseCopy as WDC,
    WritableSymbolicElseError as WDE,
};

/// The table of §4.3 as data: per class, the row for `Static`/`Exec` and
/// the row for `Pie`/`Shared`, each over the columns `Local`, `Abs`,
/// `Preemptible`, `Shlib`, `Ifunc`, `Weak0`, `WeakDyn`. (`TlsLe` in
/// `Pie` is the first row's: [`cell`] handles it.)
pub const TABLE: [(DemandClass, [Cell; 7], [Cell; 7]); 16] = [
    (DemandClass::AbsWord, [N, N, X, WDC, I, N, WDE], [RT, N, DT, DT, IRT, N, DT]),
    (DemandClass::AbsNarrow, [N, N, X, C, I, N, E], [E, N, E, E, E, N, E]),
    (DemandClass::Pc, [N, N, X, C, I, N, E], [N, E, E, E, I, AW0, E]),
    (DemandClass::Lo, [N, N, X, C, I, N, E], [N, N, E, E, I, N, E]),
    (DemandClass::Branch, [N, N, X, PLT, I, N, PLT], [N, E, PLT, PLT, I, N, PLT]),
    (DemandClass::Got, [G, G, G, G, GI, G, G], [G, G, G, G, GI, G, G]),
    (DemandClass::GotRel, [GB, GB, E, E, IGB, E, E], [GB, E, E, E, IGB, E, E]),
    (DemandClass::GotBaseRef, [GB; 7], [GB; 7]),
    (DemandClass::Size, [N, N, E, N, N, N, E], [N, N, E, N, N, N, E]),
    (DemandClass::TlsGd, [Cell::Tls; 7], [Cell::Tls; 7]),
    (DemandClass::TlsLd, [Cell::Tls; 7], [Cell::Tls; 7]),
    (DemandClass::TlsIe, [Cell::Tls; 7], [Cell::Tls; 7]),
    (DemandClass::TlsDesc, [Cell::Tls; 7], [Cell::Tls; 7]),
    (DemandClass::TlsLe, [Cell::DefinedHereElseError; 7], [Cell::LocalExecInShared; 7]),
    (DemandClass::TlsDtpRel, [Cell::DefinedHereElseError; 7], [Cell::DefinedHereElseError; 7]),
    (DemandClass::None, [N; 7], [N; 7]),
];

/// The cell of the table for one class, output and target kind.
/// `Undefined` and `Relocatable` have no cell: [`Cell::Impossible`].
#[must_use]
pub fn cell(class: DemandClass, output: OutputKind, target: TargetKind) -> Cell {
    if target == TargetKind::Undefined || output == OutputKind::Relocatable {
        return Cell::Impossible;
    }
    // `Static` never has `Preemptible`, `Shlib` or `WeakDyn` targets; only
    // `Shared` has `Preemptible` ones (§2.1).
    let never = match output {
        OutputKind::Static => matches!(
            target,
            TargetKind::Preemptible | TargetKind::Shlib { .. } | TargetKind::WeakDyn
        ),
        OutputKind::Exec | OutputKind::Pie => target == TargetKind::Preemptible,
        OutputKind::Shared | OutputKind::Relocatable => false,
    };
    if never {
        return Cell::Impossible;
    }
    let row = &TABLE[class as usize];
    debug_assert_eq!(row.0, class, "TABLE is in DemandClass order");
    let second = match class {
        // `TlsLe`: Static, Exec, Pie | Shared.
        DemandClass::TlsLe => output == OutputKind::Shared,
        _ => output.is_pic(),
    };
    if second { row.2[column(target)] } else { row.1[column(target)] }
}

/// What the place is, for the rules that depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaceFacts {
    /// The place's section has `SHF_ALLOC`.
    pub alloc: bool,
    /// The place's section has `SHF_WRITE` (so a dynamic relocation there
    /// is not a text relocation).
    pub writable: bool,
    /// The place's section has `SHF_EXECINSTR`; `None` where the caller
    /// cannot know it (arch.md §3.4; i386 X86-13 step 1 refuses a GOT form
    /// outside code at scan when it is known, at fill otherwise).
    pub exec: Option<bool>,
}

impl PlaceFacts {
    /// A place whose `SHF_EXECINSTR` the caller does not know.
    #[must_use]
    pub const fn new(alloc: bool, writable: bool) -> Self {
        Self { alloc, writable, exec: None }
    }
}

/// The demands of one relocation by its class (§3.4, §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Demand {
    pub flags: DemandFlags,
    pub global: GlobalFlags,
    /// A dynamic relocation at the place (`R` or `D`), counted per section.
    pub dynamic: Option<DynRole>,
}

/// The outcome of the generic class rule for one relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassOutcome {
    Demand(Demand),
    /// `relocation-incompatible` with this reason.
    Incompatible(Incompatible),
    /// A TLS-model class: the architecture's table decides (§4.3).
    ArchTls,
    /// `Pc` against `Weak0` in `Pie` or `Shared`: the architecture spec's
    /// §7 decides (§4.3; x86-64 `E` with the output's `recompile` reason,
    /// AArch64 nothing, its value `S = P`).
    ArchWeak0,
    /// The target is `Undefined`: `undefined-symbol` (§3.4), unless a
    /// matched sequence consumes the relocation.
    Undefined,
    /// A combination the output kind never has: an internal error.
    Impossible,
}

const fn pic_reason(output: OutputKind) -> Incompatible {
    match output {
        OutputKind::Pie => Incompatible::RecompilePie,
        _ => Incompatible::RecompilePic,
    }
}

/// The generic rule of §3.4/§4.3 for one relocation.
///
/// In order: an `Undefined` target; a place in a non-alloc section (only
/// `None` and data fields of the allowed classes, which demand nothing but
/// `GotRel`'s `GOT_BASE`, with the TLS mismatch rule except
/// [`writes_dtprel`]'s case); a TLS class against a non-TLS symbol or the
/// reverse; then the table.
/// `data_field` is whether the type's field is `Field::Data`; `tls`
/// whether the symbol is `STT_TLS`.
#[must_use]
pub fn class_outcome(
    class: DemandClass,
    data_field: bool,
    output: OutputKind,
    target: TargetKind,
    place: PlaceFacts,
    tls: bool,
) -> ClassOutcome {
    if target == TargetKind::Undefined {
        return ClassOutcome::Undefined;
    }
    if !place.alloc {
        if class == DemandClass::None {
            return ClassOutcome::Demand(Demand::default());
        }
        if !(data_field && class.allowed_in_non_alloc()) {
            return ClassOutcome::Incompatible(Incompatible::NotInNonAlloc);
        }
        if class.is_tls() && !tls {
            return ClassOutcome::Incompatible(Incompatible::NotThreadLocal);
        }
        if !class.is_tls() && tls && !writes_dtprel(class, place, tls) {
            return ClassOutcome::Incompatible(Incompatible::ThreadLocal);
        }
        // `GotRel` writes `S + A − GOT`: the base must exist.
        let global =
            if class == DemandClass::GotRel { GlobalFlags::GOT_BASE } else { GlobalFlags::empty() };
        return ClassOutcome::Demand(Demand { global, ..Demand::default() });
    }
    if class == DemandClass::None {
        return ClassOutcome::Demand(Demand::default());
    }
    if class.is_tls() && !tls {
        return ClassOutcome::Incompatible(Incompatible::NotThreadLocal);
    }
    if !class.is_tls() && tls {
        return ClassOutcome::Incompatible(Incompatible::ThreadLocal);
    }
    let func = matches!(target, TargetKind::Shlib { func: true });
    let copy = if func { DemandFlags::CANON_PLT } else { DemandFlags::COPY };
    let d = |flags: DemandFlags, global: GlobalFlags, dynamic: Option<DynRole>| {
        ClassOutcome::Demand(Demand { flags, global, dynamic })
    };
    let none = GlobalFlags::empty();
    let read_only = ClassOutcome::Incompatible(Incompatible::ReadOnly);
    match cell(class, output, target) {
        Cell::Nothing => d(DemandFlags::empty(), none, None),
        Cell::RelativeTx if place.writable => {
            d(DemandFlags::empty(), none, Some(DynRole::Relative))
        }
        Cell::SymbolicTx | Cell::WritableSymbolicElseCopy | Cell::WritableSymbolicElseError
            if place.writable =>
        {
            d(DemandFlags::EXPORT, none, Some(DynRole::Symbolic))
        }
        Cell::IpltRelativeTx if place.writable => {
            d(DemandFlags::IPLT, none, Some(DynRole::Relative))
        }
        Cell::RelativeTx | Cell::SymbolicTx | Cell::IpltRelativeTx => read_only,
        Cell::WritableSymbolicElseCopy | Cell::Copy => d(copy, none, None),
        Cell::WritableSymbolicElseError | Cell::Error => {
            ClassOutcome::Incompatible(pic_reason(output))
        }
        Cell::Iplt => d(DemandFlags::IPLT, none, None),
        Cell::Plt => d(DemandFlags::PLT, none, None),
        Cell::Got => d(DemandFlags::GOT, none, None),
        Cell::GotIplt => d(DemandFlags::GOT | DemandFlags::IPLT, none, None),
        Cell::GotBase => d(DemandFlags::empty(), GlobalFlags::GOT_BASE, None),
        Cell::IpltGotBase => d(DemandFlags::IPLT, GlobalFlags::GOT_BASE, None),
        Cell::Tls => ClassOutcome::ArchTls,
        Cell::DefinedHereElseError if target.is_defined_here() => {
            d(DemandFlags::empty(), none, None)
        }
        Cell::DefinedHereElseError => ClassOutcome::Incompatible(pic_reason(output)),
        Cell::LocalExecInShared => ClassOutcome::Incompatible(Incompatible::LocalExecInShared),
        Cell::ArchWeak0 => ClassOutcome::ArchWeak0,
        Cell::Impossible => ClassOutcome::Impossible,
    }
}
