//! The architecture interface (`spec/arch.md`): the [`Arch`] trait, the
//! vocabulary it is written in, and the parts every architecture shares.
//!
//! - [`vocab`]: output and target kinds, value terms, fields, checks and
//!   the [`Howto`] of a relocation type (§2), with the overflow check of
//!   §2.3 (the ELFCLASS32 modulo-2^32 rule included).
//! - [`class`]: demand classes and the class × output × target table of
//!   §4.3, as data.
//! - [`flags`]: the demand flag words of §4.2.
//! - [`shapes`]: facts, GOT/PLT/thunk/erratum/TLS descriptions, dynamic
//!   relocation roles (§3.1, §3.7 to §3.13).
//! - [`cx`]: the views scan and fill see and the sink scan writes (§3.4,
//!   §3.6).
//! - [`seq`]: windows, overlaps, sequences (§3.4, §3.5) and the
//!   instruction proof of rule 1 (§3.6).
//! - [`need`]: the thunk and erratum needs of one round of the
//!   range-extension fixed point (§6 step 1).
//! - [`diag`]: scan and fill errors as diagnostics, with their places
//!   and keys (§3.4, §7).
//! - [`error`]: refusals by name, field errors, incompatibility reasons
//!   and the texts of §10.
//! - [`dead`]: references to targets in sections that are not live, and
//!   the tombstone (§3.4, §3.11).
#![forbid(unsafe_code)]

pub mod class;
pub mod cx;
pub mod dead;
pub mod diag;
pub mod error;
pub mod flags;
pub mod need;
pub mod seq;
pub mod shapes;
pub mod vocab;

pub use class::{ClassOutcome, Demand, DemandClass, PlaceFacts, class_outcome, writes_dtprel};
pub use cx::{
    DemandOut, DemandSink, EventClass, FillCx, FillTables, OutputCx, PatchCx, Records, Reloc,
    RelocAt, ScanCx, ScanError, ScanTables, SectionOut, SymFacts, SymNames, SymValue, SynthCx,
};
pub use dead::{DeadRef, dead_reference, is_debug_name, tombstone};
pub use error::{FieldError, Incompatible, Range, RefName, Refusal};
pub use flags::{DemandFlags, GlobalFlags};
pub use seq::{Code, ErratumSite, Proof, ProofRun, UnitWindow, Unproved};
pub use shapes::{
    Branch, DynRole, EhEncodings, Erratum, Facts, GotBase, GotShape, IpltWord, PltChunk, PltPart,
    PltShape, PltVariant, PropMerge, Reach, RelForm, ThunkShape, TlsAbi, TlsBlock, TlsVariant,
    Weak0Rule, max_window, x86_property_merge,
};
pub use vocab::{
    AddendRule, Base, Check, Field, Howto, Implicit, Minus, OutputKind, Site, SlotKind, Status,
    TargetKind, Terms, Value, Window,
};

/// What one architecture tells the linker (§3). An architecture is a
/// value, `&'static dyn Arch`, chosen from `Inputs.target`.
///
/// All methods are pure: no interior mutability, no I/O, no allocation
/// except into the `out` vectors given.
///
/// One change from §3 as written: `erratum_sites` takes the instruction
/// proof of the section's bytes ([`Code`]) and can refuse, because §3.7
/// says the sites are found "given its address and the instruction/data
/// ranges" and a64.md §11.1 refuses a candidate without proof, and the
/// signature of §3 had room for neither (recorded in
/// `spec/landed/a64/WP-023.md`).
pub trait Arch: Sync + 'static {
    /// §3.1.
    fn facts(&self) -> &'static Facts;
    /// The output's `e_flags` from the live inputs' (§3.1).
    fn e_flags(&self, inputs: &mut dyn Iterator<Item = u32>) -> Result<u32, Refusal>;

    /// §3.2: a total function of the type and the section bytes.
    fn classify(&self, r_type: u32, site: Site<'_>) -> Result<Howto, Refusal>;
    /// §3.3, REL only.
    fn read_addend(&self, h: &Howto, field: &[u8]) -> i64;
    /// §3.3, REL only.
    fn write_addend(&self, h: &Howto, field: &mut [u8], addend: i64) -> Result<(), FieldError>;

    /// §3.4.
    fn scan(&self, cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>);
    /// §3.6.
    fn apply(
        &self,
        cx: &FillCx<'_>,
        at: &RelocAt<'_>,
        out: &mut SectionOut<'_>,
    ) -> Result<(), FieldError>;
    /// §3.4 "Overlap", §3.5: the window of the unit the relocation belongs
    /// to. By default its field window, the record its own unit; a member
    /// of a matched sequence has the window of the words it owns and the
    /// lead's index as its unit. `None` when the window wraps.
    fn unit_window(
        &self,
        output: OutputKind,
        at: &RelocAt<'_>,
        names: &dyn SymNames,
    ) -> Option<UnitWindow> {
        let _ = (output, names);
        let (start, end) = seq::field_window(at.r_offset, at.howto)?;
        Some(UnitWindow { start, end, unit: at.index, record: at.index })
    }
    /// §3.4 "Relocations consumed by a sequence make no reference", §6
    /// step 1: a consumed relocation needs no symbol, is no branch site and
    /// has no redirect. The decision is input-only (a kind-1 rule).
    fn consumed(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames) -> bool {
        let _ = (output, at, names);
        false
    }
    /// §3.5: the number of records from `i` that a sequence recognized at
    /// `i` may consume, 1 for a record that starts no sequence (and for an
    /// `i` past the end). It reads only the records. The scan driver does
    /// not split a section's records between blocks inside an extent
    /// (`spec/image.md` §4.1).
    fn sequence_extent(&self, records: &dyn Records, i: u32) -> u32 {
        let _ = (records, i);
        1
    }
    /// `spec/image.md` I-14: the relocation types that raise a demand
    /// from a non-alloc section (x86-64 `GOTOFF64` needs the GOT base,
    /// x86.md X-53). The scan visits deferred non-alloc records of these
    /// types only, for their demands; their checks are fill's. Empty by
    /// default: no non-alloc record of the architecture raises a demand.
    fn nonalloc_demand_types(&self) -> &'static [u32] {
        &[]
    }

    /// §3.7.
    fn got(&self) -> &'static GotShape;
    fn plt(&self, v: PltVariant) -> &'static PltShape;
    fn plt_variant(&self, cx: &OutputCx) -> Result<PltVariant, Refusal>;
    fn write_plt(
        &self,
        v: PltVariant,
        what: PltPart,
        cx: &SynthCx,
        out: &mut [u8],
    ) -> Result<(), FieldError>;
    /// §3.7, §6: which types are branches, their reach, whether a veneer
    /// may be inserted.
    fn branch(&self, h: &Howto) -> Option<Branch>;
    fn thunk(&self) -> Option<&'static ThunkShape>;
    fn write_thunk(&self, cx: &PatchCx, out: &mut [u8]) -> Result<(), FieldError>;
    fn thunk_target_ok(&self, cx: &PatchCx) -> Result<(), Refusal>;
    /// The errata the options enable (§3.7). The architecture value is
    /// built with its options.
    fn errata(&self) -> &'static [Erratum];
    /// The sites of erratum `e` in one whole output code section, in
    /// address order, or the candidate that has no proof.
    fn erratum_sites(
        &self,
        e: Erratum,
        code: &Code<'_>,
        out: &mut Vec<ErratumSite>,
    ) -> Result<(), Unproved>;
    fn erratum_in_place(&self, e: Erratum, cx: &PatchCx) -> bool;
    /// The size and alignment of an erratum patch (§3.7), as `thunk()`
    /// gives a thunk's; `None` for an erratum the architecture does not
    /// fix.
    fn patch(&self, e: Erratum) -> Option<&'static ThunkShape>;
    /// §6 step 1: the reach of each of the two branches between an
    /// erratum site's final word and its patch (`b patch` from `F`, and the
    /// patch's branch back to `F + 4`).
    fn patch_reach(&self, e: Erratum) -> Reach;
    fn write_patch(&self, e: Erratum, cx: &PatchCx, out: &mut [u8]) -> Result<(), FieldError>;

    /// §3.8.
    fn tls(&self) -> &'static TlsAbi;
    /// §3.9.
    fn eh(&self) -> &'static EhEncodings;
    /// §3.10.
    fn dyn_type(&self, role: DynRole) -> Option<u32>;
    /// §3.11: the value written, truncated to the field, for a relocation
    /// in a non-alloc section with a debug name against a dead target:
    /// [`tombstone`] of the name and `Facts.class`.
    fn tombstone(&self, section_name: &[u8]) -> u64 {
        tombstone(section_name, self.facts().class)
    }
    /// §3.12.
    fn weak0(&self, h: &Howto) -> Weak0Rule;
    /// §3.13.
    fn property_merge(&self, pr_type: u32) -> Option<PropMerge>;
}
