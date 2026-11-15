//! i386 (`spec/x86.md`), behind the interface of `mortar_arch`.
//!
//! - [`table`]: the static relocation table of §6.1, as data.
//! - [`forms`]: the instruction forms of §8 and §9 as byte patterns, and
//!   the `GOT32` operand reading of X86-13.
//! - [`relax`]: the `TLS_GD`/`TLS_LDM` sequences (§9, X86-31, X86-41).
//! - [`synth`]: IPLT entries, static slot and `.got.plt` contents (§11).
//! - [`X86`]: the [`Arch`] implementation; [`scan_checks`] are the
//!   input-only checks of arch.md §3.4.
//!
//! Relocation sections are `SHT_REL` (X86-2): every addend is the field's
//! input bytes (X86-4), read where a rule needs it (X86-5) and never from
//! bytes a relocation has written.
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
    FieldError, GotBase, GotShape, Howto, Minus, OutputCx, OutputKind, PatchCx, PltChunk, PltPart,
    PltShape, PltVariant, PropMerge, Reach, Records, RefName, Refusal, RelForm, RelocAt, Site,
    SlotKind, Status, SymNames, SynthCx, ThunkShape, TlsAbi, TlsVariant, UnitWindow, Unproved,
    Weak0Rule, Window, max_window, x86_property_merge,
};
use mortar_diag::RefusalClass;
use mortar_elf::{Class, Machine};

pub use apply::{
    DTPOFF_PLACE, DYNAMIC_TLS, GOT_PLACE, IE_TEXT_RELOCATION, NON_ALLOC_TLS, TLS_TO_IE,
    UNRECOGNIZED_DESC, UNRECOGNIZED_GD, UNRECOGNIZED_LD, ifunc_pie, scan_checks,
};

/// The i386 architecture (M1: static and static-pie executables, §1).
/// Every type of §6.1 is M1 or later, so the value has no milestone
/// switch; the M2 parts (the lazy PLT, the dynamic TLS models) are
/// refused by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct X86;

impl X86 {
    pub const M1: Self = Self;
}

/// The facts of X86-47, X86-64 and X86-65.
pub static FACTS: Facts = Facts {
    machine: Machine::I386,
    class: Class::Elf32,
    e_machine: 3,
    rel_form: RelForm::Rel,
    word: 4,
    osabi: 0,
    max_page: 0x1000,
    common_page: 0x1000,
    image_base: 0x0804_8000,
    // No `Page()` term on i386 (§3.1).
    page_mask: 0,
    got_base: GotBase::GotPltStart,
    // `.got` has no reserved word; `.got.plt` has 3 (X86-47).
    got_header: 0,
    got_plt_reserved: 3,
    iplt_word: mortar_arch::IpltWord::Resolver,
    max_reach: MAX_REACH,
    code_fill: &[0xCC],
    code_nop: &[0x90],
    seq_leads: &relax::SEQ_LEADS,
};

/// `Facts.max_reach` (arch.md §3.1, X86-40): the maximum over every
/// `Howto` window of the table, every form's window and every read
/// window. (3, 6): D1's `before`, D2/D3/L2's `after`.
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

/// The TLS ABI (§13): variant II, `___tls_get_addr` (three underscores,
/// the `%eax`-argument entry point of X86-31).
pub static TLS: TlsAbi = TlsAbi {
    variant: TlsVariant::II,
    get_addr: b"___tls_get_addr",
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
/// 8-byte IPLT entries (X86-48).
static PLT_PLAIN: PltShape = PltShape {
    chunks: &[PltChunk { name: ".plt", header: 16, entry: 16, align: 16 }],
    call_chunk: 0,
    lazy_chunk: 0,
    iplt_entry: 8,
};
/// The IBT shape: `.plt` with the lazy entries and `.plt.sec` with the
/// call entries (M2, as x64.md's); 16-byte IPLT entries (X86-49).
static PLT_IBT: PltShape = PltShape {
    chunks: &[
        PltChunk { name: ".plt", header: 16, entry: 16, align: 16 },
        PltChunk { name: ".plt.sec", header: 0, entry: 16, align: 16 },
    ],
    call_chunk: 1,
    lazy_chunk: 0,
    iplt_entry: 16,
};

/// `GNU_PROPERTY_X86_FEATURE_1_AND` and its bits (§12).
pub const FEATURE_1_AND: u32 = 0xc000_0002;
pub const FEATURE_1_IBT: u32 = 1;
pub const FEATURE_1_SHSTK: u32 = 2;

/// The refusal of an input `e_flags` other than 0 (X86-1).
pub const E_FLAGS: Refusal =
    Refusal::named(RefusalClass::Feature, "an i386 input with e_flags other than 0");

/// The refusal name of a type: its `R_386_*` name, or its number in hex
/// when it has none (diag.md §4.2).
fn type_name(r_type: u32) -> RefName {
    mortar_elf::reloc::i386::name(r_type).map_or(RefName::Hex(u64::from(r_type)), RefName::Named)
}

impl Arch for X86 {
    fn facts(&self) -> &'static Facts {
        &FACTS
    }

    /// X86-1, X86-64: every input's `e_flags` is 0, and so is the output's.
    fn e_flags(&self, inputs: &mut dyn Iterator<Item = u32>) -> Result<u32, Refusal> {
        for f in inputs {
            if f != 0 {
                return Err(E_FLAGS);
            }
        }
        Ok(0)
    }

    /// §6: an M1 row with its read window (X86-40) and, for `GOT32` and
    /// `GOT32X`, the operand reading of X86-13 (`G + A` without a base);
    /// every other type refused by name (X86-14, X86-20, §6.3).
    fn classify(&self, r_type: u32, site: Site<'_>) -> Result<Howto, Refusal> {
        match table::howto(r_type) {
            Some(mut h) if h.status == Status::M1 => {
                h.window = forms::read_window(r_type, site);
                if matches!(r_type, forms::GOT32 | forms::GOT32X)
                    && forms::got_operand(site) == Some(forms::Operand::NoBase)
                {
                    h.value.minus = Minus::None;
                }
                Ok(h)
            }
            _ => Err(Refusal { class: RefusalClass::RelocationType, name: type_name(r_type) }),
        }
    }

    /// X86-4: the field's bytes, little-endian, sign-extended.
    fn read_addend(&self, h: &Howto, field: &[u8]) -> i64 {
        let n = usize::from(h.implicit.map_or(0, |i| i.bytes));
        forms::implicit(field.get(..n).unwrap_or(&[]))
    }

    /// X86-7 (`-r`, later): `A` written back into the field, checked
    /// against its width (signed or unsigned, `su(n)`).
    fn write_addend(&self, h: &Howto, field: &mut [u8], addend: i64) -> Result<(), FieldError> {
        let Some(imp) = h.implicit else { return Ok(()) };
        let n = imp.bytes;
        mortar_arch::Check::Either(n * 8).verify(addend, Class::Elf64)?;
        let bytes = addend.to_le_bytes();
        let dst =
            field.get_mut(..usize::from(n)).ok_or(FieldError::Outside { offset: 0, width: n })?;
        dst.copy_from_slice(&bytes[..usize::from(n)]);
        Ok(())
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

    /// X86-41: in an executable a consumed partner owns nothing and is a
    /// member of its lead's unit (`lead_of`); a lead whose sequence does
    /// not hold owns its read window `[P − 3, P + 4)`. Every other record
    /// owns its `Howto` window, its read window (X86-40).
    fn unit_window(
        &self,
        output: OutputKind,
        at: &RelocAt<'_>,
        names: &dyn SymNames,
    ) -> Option<UnitWindow> {
        let (start, end) = mortar_arch::seq::field_window(at.r_offset, at.howto)?;
        let w = UnitWindow { start, end, unit: at.index, record: at.index };
        if !matches!(at.r_type, forms::TLS_GD | forms::TLS_LDM) {
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
            let start = at.r_offset.saturating_sub(3);
            let end = at.r_offset.checked_add(4)?.min(at.site.bytes.len() as u64).max(at.r_offset);
            Some(UnitWindow { start, end, ..w })
        }
    }

    /// X86-31: the partner of a matched sequence, in an executable.
    fn consumed(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames) -> bool {
        output.is_exec() && relax::follower_of(at, names).is_some()
    }

    /// X86-41.
    fn sequence_extent(&self, records: &dyn Records, i: u32) -> u32 {
        relax::sequence_extent(records, i)
    }

    fn got(&self) -> &'static GotShape {
        &GOT
    }

    fn plt(&self, v: PltVariant) -> &'static PltShape {
        match v {
            PltVariant::IbtAbs | PltVariant::IbtPic => &PLT_IBT,
            _ => &PLT_PLAIN,
        }
    }

    /// X86-48, X86-49, X86-55: the absolute entries in `static`, the
    /// `%ebx` entries in `static-pie` (and the M2 `pie`/`shared`); IBT when
    /// the merged `FEATURE_1_AND` has IBT (after `-z ibt`) or with `-z
    /// force-ibt` (`force_bti`); `-z pac-plt` is AArch64's.
    fn plt_variant(&self, cx: &OutputCx) -> Result<PltVariant, Refusal> {
        if cx.pac_plt {
            return Err(Refusal::named(RefusalClass::Option, "-z pac-plt"));
        }
        let ibt = cx.feature_1_and & FEATURE_1_IBT != 0 || cx.force_bti;
        let pic = cx.output.is_pic();
        Ok(match (ibt, pic) {
            (false, false) => PltVariant::Abs,
            (false, true) => PltVariant::Pic,
            (true, false) => PltVariant::IbtAbs,
            (true, true) => PltVariant::IbtPic,
        })
    }

    /// The IPLT entries of X86-48 and X86-49; the lazy PLT is M2.
    fn write_plt(
        &self,
        v: PltVariant,
        what: PltPart,
        cx: &SynthCx,
        out: &mut [u8],
    ) -> Result<(), FieldError> {
        let got = Some(cx.got_base);
        match (v, what) {
            (PltVariant::Abs, PltPart::Iplt(_)) => synth::put(out, &synth::iplt(cx.got_plt, None)),
            (PltVariant::Pic, PltPart::Iplt(_)) => synth::put(out, &synth::iplt(cx.got_plt, got)),
            (PltVariant::IbtAbs, PltPart::Iplt(_)) => {
                synth::put(out, &synth::iplt_ibt(cx.got_plt, None))
            }
            (PltVariant::IbtPic, PltPart::Iplt(_)) => {
                synth::put(out, &synth::iplt_ibt(cx.got_plt, got))
            }
            (PltVariant::Abs | PltVariant::Pic | PltVariant::IbtAbs | PltVariant::IbtPic, _) => {
                Err(FieldError::Refused(synth::LAZY_PLT))
            }
            _ => Err(FieldError::MissingTerm("i386 PLT variant")),
        }
    }

    /// `PLT32`: `mod(32)`, no veneer (X86-43): every branch reaches every
    /// address, so the reach is every 32-bit displacement.
    fn branch(&self, h: &Howto) -> Option<Branch> {
        (h.class == DemandClass::Branch && h.field == Field::Data { bytes: 4 }).then_some(Branch {
            reach: Reach { min: -(1 << 32), max: (1 << 32) - 1 },
            veneer: false,
        })
    }

    /// X86-43: no range-extension thunks.
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

    /// X86-67: the pointer encodings are the shared table's.
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
            DynRole::Relative => synth::RELATIVE,
            DynRole::TpOff { negated: false } => 14,
            DynRole::DtpMod => 35,
            DynRole::DtpOff => 36,
            DynRole::TpOff { negated: true } => 37,
            DynRole::TlsDesc => 41,
            DynRole::IRelative => synth::IRELATIVE,
        })
    }

    /// §7: `S = 0` for every class (`Pc` and `GotRel` in `Pie` are refused
    /// at scan, X86-21, X86-22; TLS classes take offset 0, X86-23).
    fn weak0(&self, _h: &Howto) -> Weak0Rule {
        Weak0Rule::SZero
    }

    /// X86-53.
    fn property_merge(&self, pr_type: u32) -> Option<PropMerge> {
        x86_property_merge(pr_type)
    }
}

/// The input bytes around a refused place, `[P − 3, P + 10)` (the
/// `max_reach` around a 4-byte field) clipped to the section, in hex: the
/// text a diagnostic's note shows for X86-13 and X86-36 ("refused by name
/// with the bytes shown").
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
