//! x86-64 (`spec/x64.md`), behind the interface of `mortar_arch`.
//!
//! - [`table`]: the static relocation table of §6.1, as data.
//! - [`forms`]: the instruction forms of §8 as byte patterns (X-8).
//! - [`relax`]: the `TLSGD`/`TLSLD` sequences (§8.3, §8.4, X-30).
//! - [`synth`]: IPLT entries and static slot contents (§10).
//! - [`X64`]: the [`Arch`] implementation; [`scan_checks`] are the
//!   input-only checks of arch.md §3.4.
#![forbid(unsafe_code)]

pub mod forms;
pub mod relax;
pub mod synth;
pub mod table;

mod apply;

use std::fmt::Write as _;

use mortar_arch::shapes::EH_ENCODINGS;
use mortar_arch::{
    Arch, Branch, Code, DemandClass, DynRole, EhEncodings, Erratum, ErratumSite, Facts, Field,
    FieldError, GotBase, GotShape, Howto, OutputCx, OutputKind, PatchCx, PltChunk, PltPart,
    PltShape, PltVariant, PropMerge, Reach, Records, RefName, Refusal, RelForm, RelocAt, Site,
    SlotKind, Status, SymNames, SynthCx, ThunkShape, TlsAbi, TlsVariant, UnitWindow, Unproved,
    Weak0Rule, Window, max_window, x86_property_merge,
};
use mortar_diag::RefusalClass;
use mortar_elf::{Class, Machine};

pub use apply::{
    DTPOFF_PLACE, DYNAMIC_TLS, LD12, LD13, LD22, NON_ALLOC_TLS, TLS_TO_IE, UNRECOGNIZED_DESC,
    UNRECOGNIZED_GD, UNRECOGNIZED_LD, scan_checks,
};

/// The x86-64 architecture (M1: static and static-pie executables,
/// `docs/ROADMAP.md`). Every type of §6.1 is M1 or later, so the value
/// has no milestone switch; the M2 parts (the lazy PLT, the dynamic TLS
/// models) are refused by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct X64;

impl X64 {
    pub const M1: Self = Self;
}

/// The facts of X-36, X-49 and X-50.
pub static FACTS: Facts = Facts {
    machine: Machine::X86_64,
    class: Class::Elf64,
    e_machine: 62,
    rel_form: RelForm::Rela,
    word: 8,
    osabi: 0,
    max_page: 0x1000,
    common_page: 0x1000,
    image_base: 0x40_0000,
    // No `Page()` term on x86-64 (§3.1).
    page_mask: 0,
    got_base: GotBase::GotPltStart,
    // `.got` has no reserved word; `.got.plt` has 3 (X-36).
    got_header: 0,
    got_plt_reserved: 3,
    max_reach: MAX_REACH,
    code_fill: &[0xCC],
    code_nop: &[0x90],
    seq_leads: &relax::SEQ_LEADS,
};

/// `Facts.max_reach` (arch.md §3.1, `spec/image.md` I-111): the maximum
/// over every `Howto` window of the table and every window of the
/// recognized forms (§8.6). (4, 15): GD16's `before`, the 22-byte forms'
/// `after` (X-29).
pub const MAX_REACH: Window = {
    let mut w = relax::MEMBER_WINDOWS;
    let mut t = 0;
    while t <= table::MAX_TYPE {
        if let Some(h) = table::howto(t) {
            w = max_window(w, h.window);
        }
        t += 1;
    }
    w
};

/// The TLS ABI (§12): variant II.
pub static TLS: TlsAbi = TlsAbi {
    variant: TlsVariant::II,
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

/// The non-IBT shape: `.plt` (16-byte header and lazy entries, M2) and
/// 8-byte IPLT entries (X-37).
static PLT_LAZY: PltShape = PltShape {
    chunks: &[PltChunk { name: ".plt", header: 16, entry: 16, align: 16 }],
    call_chunk: 0,
    lazy_chunk: 0,
    iplt_entry: 8,
};
/// The IBT shape (arch.md A.8): `.plt` with the lazy entries and
/// `.plt.sec` with the call entries (M2); 16-byte IPLT entries (X-38).
static PLT_IBT: PltShape = PltShape {
    chunks: &[
        PltChunk { name: ".plt", header: 16, entry: 16, align: 16 },
        PltChunk { name: ".plt.sec", header: 0, entry: 16, align: 16 },
    ],
    call_chunk: 1,
    lazy_chunk: 0,
    iplt_entry: 16,
};

/// `GNU_PROPERTY_X86_FEATURE_1_AND` and its bits (§11).
pub const FEATURE_1_AND: u32 = 0xc000_0002;
pub const FEATURE_1_IBT: u32 = 1;
pub const FEATURE_1_SHSTK: u32 = 2;

/// The refusal name of a type: its `R_X86_64_*` name, or its number in
/// hex when it has none (diag.md §4.2).
fn type_name(r_type: u32) -> RefName {
    mortar_elf::reloc::x86_64::name(r_type).map_or(RefName::Hex(u64::from(r_type)), RefName::Named)
}

/// The window of a record whose bytes do not match the form its
/// sequence needs: its field (arch.md §3.4; X-26 refuses it).
fn field_only(at: &RelocAt<'_>) -> Option<UnitWindow> {
    let end = at.r_offset.checked_add(u64::from(at.howto.field.width()))?;
    Some(UnitWindow { start: at.r_offset, end, unit: at.index, record: at.index })
}

impl Arch for X64 {
    fn facts(&self) -> &'static Facts {
        &FACTS
    }

    /// 0: no input `e_flags` bit is defined (X-49).
    fn e_flags(&self, _inputs: &mut dyn Iterator<Item = u32>) -> Result<u32, Refusal> {
        Ok(0)
    }

    /// §6: an M1 row, its window that of the form the bytes match (§8.6);
    /// every other type refused by name (X-13, X-14, §6.3).
    fn classify(&self, r_type: u32, site: Site<'_>) -> Result<Howto, Refusal> {
        match table::howto(r_type) {
            Some(mut h) if h.status == Status::M1 => {
                if let Some(f) = forms::form(r_type, site) {
                    h.window = f.window();
                }
                Ok(h)
            }
            _ => Err(Refusal { class: RefusalClass::RelocationType, name: type_name(r_type) }),
        }
    }

    /// REL is refused for x86-64 (X-2); 0.
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
        apply::apply(cx, at, out)
    }

    /// §8.6: in an executable a consumed follower owns nothing and is a
    /// member of its lead's unit; a lead whose sequence does not hold owns
    /// its field only. Every other record owns its `Howto` window.
    fn unit_window(
        &self,
        output: OutputKind,
        at: &RelocAt<'_>,
        names: &dyn SymNames,
    ) -> Option<UnitWindow> {
        let (start, end) = mortar_arch::seq::field_window(at.r_offset, at.howto)?;
        let w = UnitWindow { start, end, unit: at.index, record: at.index };
        if !matches!(at.r_type, forms::TLSGD | forms::TLSLD) {
            if output.is_exec()
                && let Some((lead, _)) = relax::follower_of(at, names)
            {
                return Some(UnitWindow { start: at.r_offset, end: at.r_offset, unit: lead, ..w });
            }
            return Some(w);
        }
        if output.is_exec() && relax::lead_form(at, names).is_some() {
            Some(w)
        } else {
            field_only(at)
        }
    }

    /// X-25: the follower of a matched sequence, in an executable.
    fn consumed(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames) -> bool {
        output.is_exec() && relax::follower_of(at, names).is_some()
    }

    /// X-30.
    fn sequence_extent(&self, records: &dyn Records, i: u32) -> u32 {
        relax::sequence_extent(records, i)
    }

    /// X-53: `R_X86_64_GOTOFF64` at a non-alloc place needs the GOT base.
    fn nonalloc_demand_types(&self) -> &'static [u32] {
        &[25]
    }

    fn got(&self) -> &'static GotShape {
        &GOT
    }

    fn plt(&self, v: PltVariant) -> &'static PltShape {
        match v {
            PltVariant::Ibt => &PLT_IBT,
            _ => &PLT_LAZY,
        }
    }

    /// X-38, X-43: IBT when the merged `FEATURE_1_AND` has IBT (after `-z
    /// ibt`) or with `-z ibtplt`/`-z force-ibt` (`force_bti`); `-z
    /// pac-plt` is AArch64's.
    fn plt_variant(&self, cx: &OutputCx) -> Result<PltVariant, Refusal> {
        if cx.pac_plt {
            return Err(Refusal::named(RefusalClass::Option, "-z pac-plt"));
        }
        let ibt = cx.feature_1_and & FEATURE_1_IBT != 0 || cx.force_bti;
        Ok(if ibt { PltVariant::Ibt } else { PltVariant::Lazy })
    }

    /// The IPLT entries of X-37 and X-38; the lazy PLT is M2.
    fn write_plt(
        &self,
        v: PltVariant,
        what: PltPart,
        cx: &SynthCx,
        out: &mut [u8],
    ) -> Result<(), FieldError> {
        match (v, what) {
            (PltVariant::Lazy, PltPart::Iplt(_)) => {
                synth::put(out, &synth::iplt(cx.addr, cx.got_plt)?)
            }
            (PltVariant::Ibt, PltPart::Iplt(_)) => {
                synth::put(out, &synth::iplt_ibt(cx.addr, cx.got_plt)?)
            }
            (PltVariant::Lazy | PltVariant::Ibt, _) => Err(FieldError::Refused(synth::LAZY_PLT)),
            _ => Err(FieldError::MissingTerm("x86-64 PLT variant")),
        }
    }

    /// `PLT32`: `s(32)`, no veneer (X-33, arch.md A.1). `PLTOFF64` has no
    /// reach.
    fn branch(&self, h: &Howto) -> Option<Branch> {
        (h.class == DemandClass::Branch && h.field == Field::Data { bytes: 4 }).then_some(Branch {
            reach: Reach { min: -(1 << 31), max: (1 << 31) - 1 },
            veneer: false,
        })
    }

    /// X-33: no range-extension thunks.
    fn thunk(&self) -> Option<&'static ThunkShape> {
        None
    }

    fn write_thunk(&self, _cx: &PatchCx, _out: &mut [u8]) -> Result<(), FieldError> {
        Err(FieldError::MissingTerm("thunk"))
    }

    fn thunk_target_ok(&self, _cx: &PatchCx) -> Result<(), Refusal> {
        Ok(())
    }

    fn errata(&self) -> &'static [Erratum] {
        &[]
    }

    fn erratum_sites(
        &self,
        _e: Erratum,
        _code: &Code<'_>,
        _out: &mut Vec<ErratumSite>,
    ) -> Result<(), Unproved> {
        Ok(())
    }

    fn erratum_in_place(&self, _e: Erratum, _cx: &PatchCx) -> bool {
        false
    }

    fn patch(&self, _e: Erratum) -> Option<&'static ThunkShape> {
        None
    }

    fn patch_reach(&self, _e: Erratum) -> Reach {
        Reach { min: 0, max: -1 }
    }

    fn write_patch(&self, _e: Erratum, _cx: &PatchCx, _out: &mut [u8]) -> Result<(), FieldError> {
        Err(FieldError::MissingTerm("erratum"))
    }

    fn tls(&self) -> &'static TlsAbi {
        &TLS
    }

    fn eh(&self) -> &'static EhEncodings {
        &EH_ENCODINGS
    }

    /// §6.2.
    fn dyn_type(&self, role: DynRole) -> Option<u32> {
        Some(match role {
            DynRole::Symbolic => 1,
            DynRole::Copy => 5,
            DynRole::GlobDat => 6,
            DynRole::JumpSlot => 7,
            DynRole::Relative => 8,
            DynRole::DtpMod => 16,
            DynRole::DtpOff => 17,
            DynRole::TpOff { negated: false } => 18,
            DynRole::TlsDesc => 36,
            DynRole::IRelative => 37,
            DynRole::TpOff { negated: true } => return None,
        })
    }

    /// §7: `S = 0` for every class (`Pc` in `Pie` is refused at scan,
    /// X-15; TLS classes take offset 0, X-18).
    fn weak0(&self, _h: &Howto) -> Weak0Rule {
        Weak0Rule::SZero
    }

    /// X-41.
    fn property_merge(&self, pr_type: u32) -> Option<PropMerge> {
        x86_property_merge(pr_type)
    }
}

/// The input bytes around a refused sequence's place, `[P − 4, P + 19)`
/// clipped to the section, in hex: the text a diagnostic's note shows
/// for X-26 and X-28 ("refused by name with the bytes shown").
#[must_use]
pub fn shown_bytes(bytes: &[u8], r_offset: u64) -> String {
    let max = MAX_REACH;
    let start = r_offset.saturating_sub(u64::from(max.before));
    let end = r_offset.saturating_add(4 + u64::from(max.after)).min(bytes.len() as u64);
    let (Ok(a), Ok(b)) = (usize::try_from(start), usize::try_from(end)) else {
        return String::new();
    };
    let mut s = String::new();
    for (i, x) in bytes.get(a..b).unwrap_or(&[]).iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{x:02x}");
    }
    s
}
