//! AArch64 (`spec/a64.md`), behind the interface of `mortar_arch`.
//!
//! - [`forms`]: the instruction field forms of §3.2.
//! - [`table`]: the static relocation table of §6.1, as data.
//! - [`mapping`]: instruction ranges from mapping symbols (§3.4).
//! - [`erratum`]: the 843419 predicate, ADR fix and patch (§11).
//! - [`relax`]: the input-only predicates of the relaxations (§8).
//! - [`synth`]: PLT/IPLT entries, thunks, static slot contents (§9, §10).
//! - [`A64`]: the [`Arch`] implementation; [`A64::scan_checks`] are the
//!   input-only checks of arch.md §3.4.
#![forbid(unsafe_code)]

pub mod erratum;
pub mod forms;
pub mod mapping;
pub mod synth;
pub mod table;

mod apply;
pub mod relax;

use mortar_arch::shapes::EH_ENCODINGS;
use mortar_arch::{
    Arch, Branch, Code, DemandClass, DynRole, EhEncodings, Erratum, ErratumSite, Facts, Field,
    FieldError, GotBase, GotShape, Howto, OutputCx, OutputKind, PatchCx, PltChunk, PltPart,
    PltShape, PltVariant, PropMerge, Reach, RefName, Refusal, RelForm, RelocAt, Site, SlotKind,
    Status, SymNames, SynthCx, ThunkShape, TlsAbi, TlsVariant, UnitWindow, Unproved, Weak0Rule,
};
use mortar_diag::RefusalClass;
use mortar_elf::{Class, Machine};

pub use apply::{ADD_X0_X1_X0, MRS_X1_TPIDR, WEAK_TLS, scan_checks};

/// The AArch64 architecture, built with the milestone it enables and its
/// options (arch.md §3.7 `errata`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct A64 {
    /// The last milestone whose types `classify` accepts (`M1` or `M2`);
    /// `Later` and `Refused` types are refused whatever it is.
    pub max: Status,
    /// `--fix-cortex-a53-843419`.
    pub fix_843419: bool,
}

impl A64 {
    /// M1: static non-PIE executables (`docs/ROADMAP.md`).
    pub const M1: Self = Self { max: Status::M1, fix_843419: false };

    #[must_use]
    pub const fn new(max: Status, fix_843419: bool) -> Self {
        Self { max, fix_843419 }
    }

    /// Whether a type of status `s` is accepted.
    #[must_use]
    pub const fn enables(&self, s: Status) -> bool {
        match s {
            Status::M1 => true,
            Status::M2 => matches!(self.max, Status::M2),
            Status::Later | Status::Refused => false,
        }
    }
}

/// The facts of §13.3.
pub static FACTS: Facts = Facts {
    machine: Machine::AArch64,
    class: Class::Elf64,
    e_machine: 183,
    rel_form: RelForm::Rela,
    word: 8,
    osabi: 0,
    max_page: 0x1_0000,
    common_page: 0x1000,
    image_base: 0x40_0000,
    page_mask: 0xFFF,
    got_base: GotBase::GotStart,
    got_header: 1,
    got_plt_reserved: 3,
};

/// The TLS ABI (§12): variant I, a 16-byte TCB.
pub static TLS: TlsAbi = TlsAbi {
    variant: TlsVariant::I { tcb: 16 },
    get_addr: b"__tls_get_addr",
    module_base: Some(b"_TLS_MODULE_BASE_"),
};

const fn slot_words(k: SlotKind) -> u8 {
    match k {
        SlotKind::Addr | SlotKind::TpOff { .. } | SlotKind::PltGot => 1,
        SlotKind::TlsGd | SlotKind::TlsLd | SlotKind::TlsDesc => 2,
    }
}

static GOT: GotShape = GotShape { slot_words };

static PLT_PLAIN: PltShape = PltShape {
    chunks: &[PltChunk { name: ".plt", header: 32, entry: 16, align: 16 }],
    call_chunk: 0,
    lazy_chunk: 0,
    iplt_entry: 16,
};
static PLT_BTI: PltShape = PltShape {
    chunks: &[PltChunk { name: ".plt", header: 32, entry: 24, align: 16 }],
    call_chunk: 0,
    lazy_chunk: 0,
    iplt_entry: 24,
};

static THUNK: ThunkShape = ThunkShape { size: 12, align: 4 };

/// `GNU_PROPERTY_AARCH64_FEATURE_1_AND` and its BTI bit (§13.3).
pub const FEATURE_1_AND: u32 = 0xc000_0000;
pub const FEATURE_1_BTI: u32 = 1;

/// The refusal name of a type: its `R_AARCH64_*` name, or its number in
/// hex when it has none (diag.md §4.2).
fn type_name(r_type: u32) -> RefName {
    mortar_elf::reloc::aarch64::name(r_type).map_or(RefName::Hex(u64::from(r_type)), RefName::Named)
}

const fn reach(bits: u8) -> Reach {
    Reach { min: -(1 << (bits - 1)), max: (1 << (bits - 1)) - 1 }
}

impl Arch for A64 {
    fn facts(&self) -> &'static Facts {
        &FACTS
    }

    /// 0: AAELF64 defines no flags and inputs' are not checked (§13.3).
    fn e_flags(&self, _inputs: &mut dyn Iterator<Item = u32>) -> Result<u32, Refusal> {
        Ok(0)
    }

    fn classify(&self, r_type: u32, _site: Site<'_>) -> Result<Howto, Refusal> {
        let refuse = || Refusal { class: RefusalClass::RelocationType, name: type_name(r_type) };
        if table::refused(r_type) {
            return Err(refuse());
        }
        match table::howto(r_type) {
            Some(h) if self.enables(h.status) => Ok(h),
            _ => Err(refuse()),
        }
    }

    /// REL is refused for AArch64 (§1); 0.
    fn read_addend(&self, _h: &Howto, _field: &[u8]) -> i64 {
        0
    }

    fn write_addend(&self, _h: &Howto, _field: &mut [u8], _addend: i64) -> Result<(), FieldError> {
        Err(FieldError::Refused(Refusal::named(RefusalClass::SectionType, "SHT_REL")))
    }

    fn scan(
        &self,
        cx: &mortar_arch::ScanCx<'_>,
        at: &RelocAt<'_>,
        out: &mut mortar_arch::DemandSink<'_>,
    ) {
        apply::scan(cx, at, out);
    }

    fn apply(
        &self,
        cx: &mortar_arch::FillCx<'_>,
        at: &RelocAt<'_>,
        out: &mut mortar_arch::SectionOut<'_>,
    ) -> Result<(), FieldError> {
        apply::apply(self.enables(Status::M2), cx, at, out)
    }

    /// R4's members (§8.4): the lead's index is the unit; the `CALL26`
    /// owns `[o+8, o+16)`, the `nop` at `o+12` included.
    fn unit_window(
        &self,
        output: OutputKind,
        at: &RelocAt<'_>,
        names: &dyn SymNames,
    ) -> Option<UnitWindow> {
        let (start, end) = mortar_arch::seq::field_window(at.r_offset, at.howto)?;
        let mut w = UnitWindow { start, end, unit: at.index, record: at.index };
        if output.is_exec()
            && let Some((lead, o)) = relax::gd_lead(at, names)
        {
            w.unit = lead;
            if at.r_type == relax::CALL26 {
                (w.start, w.end) = (o + 8, o + 16);
            }
        }
        Some(w)
    }

    /// R4's `CALL26` in an exec output (§8.4).
    fn consumed(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames) -> bool {
        output.is_exec() && at.r_type == relax::CALL26 && relax::gd_lead(at, names).is_some()
    }

    fn got(&self) -> &'static GotShape {
        &GOT
    }

    fn plt(&self, v: PltVariant) -> &'static PltShape {
        match v {
            PltVariant::Bti | PltVariant::BtiPac => &PLT_BTI,
            _ => &PLT_PLAIN,
        }
    }

    /// BTI when the merged property has the BTI bit; `-z pac-plt` and
    /// `-z force-bti` are refused by name, later (§10.3, §13.3).
    fn plt_variant(&self, cx: &OutputCx) -> Result<PltVariant, Refusal> {
        if cx.pac_plt {
            return Err(Refusal::named(RefusalClass::Option, "-z pac-plt"));
        }
        if cx.force_bti {
            return Err(Refusal::named(RefusalClass::Option, "-z force-bti"));
        }
        Ok(if cx.feature_1_and & FEATURE_1_BTI != 0 { PltVariant::Bti } else { PltVariant::Plain })
    }

    fn write_plt(
        &self,
        v: PltVariant,
        what: PltPart,
        cx: &SynthCx,
        out: &mut [u8],
    ) -> Result<(), FieldError> {
        let bti = match v {
            PltVariant::Plain => false,
            PltVariant::Bti => true,
            _ => {
                return Err(FieldError::Refused(Refusal::named(
                    RefusalClass::Option,
                    "-z pac-plt",
                )));
            }
        };
        match what {
            PltPart::Header(0) => synth::put(out, &synth::plt_header(cx.addr, cx.got_plt, bti)?),
            PltPart::Entry { chunk: 0, .. } | PltPart::Iplt(_) => {
                let (words, n) = synth::plt_entry(cx.addr, cx.got_plt, bti)?;
                synth::put(out, &words[..n])
            }
            PltPart::Header(_) | PltPart::Entry { .. } => Err(FieldError::MissingTerm("PLT chunk")),
        }
    }

    /// `CALL26`, `JUMP26` (`s(28)`, with veneer); `CONDBR19` (`s(21)`) and
    /// `TSTBR14` (`s(16)`) without (§9). `PLT32` is data, not a branch.
    fn branch(&self, h: &Howto) -> Option<Branch> {
        match h.field {
            Field::Insn { form: forms::B26 } => Some(Branch { reach: reach(28), veneer: true }),
            Field::Insn { form: forms::BC19 } => Some(Branch { reach: reach(21), veneer: false }),
            Field::Insn { form: forms::TB14 } => Some(Branch { reach: reach(16), veneer: false }),
            _ => None,
        }
    }

    fn thunk(&self) -> Option<&'static ThunkShape> {
        Some(&THUNK)
    }

    fn write_thunk(&self, cx: &PatchCx, out: &mut [u8]) -> Result<(), FieldError> {
        synth::put(out, &synth::thunk(cx.addr, cx.target)?)
    }

    /// §9: in a BTI output, a direct target that is not `SHN_ABS` must be
    /// a proved instruction and a landing pad for `br x16`.
    fn thunk_target_ok(&self, cx: &PatchCx) -> Result<(), Refusal> {
        if !cx.bti || cx.target_abs || cx.target_word.is_some_and(synth::is_landing_pad) {
            Ok(())
        } else {
            Err(synth::BTI_LANDING_PAD)
        }
    }

    fn errata(&self) -> &'static [Erratum] {
        if self.fix_843419 { &[Erratum::E843419] } else { &[] }
    }

    fn erratum_sites(
        &self,
        e: Erratum,
        code: &Code<'_>,
        out: &mut Vec<ErratumSite>,
    ) -> Result<(), Unproved> {
        match e {
            Erratum::E843419 => erratum::sites(code, out),
            // Not in `errata()`: never asked.
            _ => Ok(()),
        }
    }

    /// The ADR fix reaches: `Page(V) − Q` in `s(21)`, with `cx.site = Q`
    /// and `cx.word` the pre-fix `W1`.
    /// `b patch` and `b F + 4`: `s(28)` (§11.2).
    fn patch_reach(&self, e: Erratum) -> Reach {
        match e {
            Erratum::E843419 => reach(28),
            // Not in `errata()`: never asked; no reach.
            _ => Reach { min: 0, max: -1 },
        }
    }

    fn erratum_in_place(&self, e: Erratum, cx: &PatchCx) -> bool {
        match e {
            Erratum::E843419 => erratum::adr_fix(cx.site, cx.word).is_some(),
            _ => false,
        }
    }

    /// The patch at `cx.addr` for the final word at `cx.target`, holding
    /// `cx.word` (`W4'`), then `b F + 4` (§11.2).
    fn write_patch(&self, e: Erratum, cx: &PatchCx, out: &mut [u8]) -> Result<(), FieldError> {
        match e {
            Erratum::E843419 => synth::put(out, &erratum::patch(cx.addr, cx.target, cx.word)?),
            _ => Err(FieldError::MissingTerm("erratum")),
        }
    }

    fn tls(&self) -> &'static TlsAbi {
        &TLS
    }

    fn eh(&self) -> &'static EhEncodings {
        &EH_ENCODINGS
    }

    fn dyn_type(&self, role: DynRole) -> Option<u32> {
        Some(match role {
            DynRole::Symbolic => 257,
            DynRole::Copy => 1024,
            DynRole::GlobDat => 1025,
            DynRole::JumpSlot => 1026,
            DynRole::Relative => 1027,
            DynRole::DtpMod => 1028,
            DynRole::DtpOff => 1029,
            DynRole::TpOff { negated: false } => 1030,
            DynRole::TlsDesc => 1031,
            DynRole::IRelative => 1032,
            DynRole::TpOff { negated: true } => return None,
        })
    }

    fn tombstone(&self, section_name: &[u8]) -> u64 {
        mortar_arch::tombstone(section_name)
    }

    /// §7, by class and field: `AW`, `AN`, `LO` (and `GT`, whose slot holds
    /// 0) take `S = 0`; `PC` and the data `PLT32` take `S = P`; the branch
    /// instructions `X = 4`. TLS classes are refused at scan.
    fn weak0(&self, h: &Howto) -> Weak0Rule {
        weak0_rule(h)
    }

    fn property_merge(&self, pr_type: u32) -> Option<PropMerge> {
        (pr_type == FEATURE_1_AND).then_some(PropMerge::And)
    }
}

/// §7 as a function of the `Howto` (see [`A64::weak0`]).
#[must_use]
pub const fn weak0_rule(h: &Howto) -> Weak0Rule {
    match (h.class, h.field) {
        (DemandClass::Pc, _) | (DemandClass::Branch, Field::Data { .. }) => Weak0Rule::SIsP,
        (DemandClass::Branch, _) => Weak0Rule::XConst(4),
        _ => Weak0Rule::SZero,
    }
}

/// An erratum site's ADR fix word, `adr xn, Page(V)`, written over `W1`
/// (§11.2), or `None` when the site needs a patch. Not in the trait as
/// arch.md §3 writes it (see `spec/landed/a64/WP-023.md`).
#[must_use]
pub const fn erratum_adr(q: u64, w1: u32) -> Option<u32> {
    erratum::adr_fix(q, w1)
}

/// The word at a redirected final word `F`: `b patch` (§11.2).
pub fn erratum_branch(f: u64, patch: u64) -> Result<u32, FieldError> {
    erratum::branch(f, patch)
}
