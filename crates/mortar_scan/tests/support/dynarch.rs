//! A test architecture for the dynamic scan (WP-046): AArch64 (`A64`,
//! M2 types allowed) with `spec/dyn.md` D-25's AArch64 column as its
//! `weak_use` and D-49's lazy target, the two hooks the AArch64 package
//! (WP-047) fills. Everything else is `A64`'s.

use mortar_a64::A64;
use mortar_arch::seq::{Code, ErratumSite, Unproved};
use mortar_arch::{
    Arch, Branch, DemandSink, DynRole, DynTag, EhEncodings, Erratum, Facts, FieldError, FillCx,
    GotShape, Howto, LazyTarget, OutputCx, OutputKind, PatchCx, PltPart, PltShape, PltVariant,
    PropMerge, Reach, Records, Refusal, RelocAt, ScanCx, SectionOut, Site, Status, SymNames,
    SynthCx, ThunkShape, TlsAbi, UnitWindow, Weak0Rule, WeakClass,
};

/// AArch64 with D-25's table and D-49's lazy target.
#[derive(Debug, Clone, Copy)]
pub struct DynA64(pub A64);

impl Default for DynA64 {
    fn default() -> Self {
        Self(A64::new(Status::M2, false))
    }
}

/// D-25's AArch64 column (a64 §7.1): the class of a use of an undefined
/// weak key.
#[must_use]
pub fn a64_weak_use(r_type: u32, writable: bool) -> Option<WeakClass> {
    Some(match r_type {
        0 => return None,
        // GOTPCREL32, GOT_LD_PREL19, LD64_GOTOFF_LO15, ADR_GOT_PAGE,
        // LD64_GOT_LO12_NC, LD64_GOTPAGE_LO15.
        315 | 309 | 310 | 311 | 312 | 313 => WeakClass::Got,
        257 if writable => WeakClass::AbsWord,
        // CALL26, JUMP26, PLT32.
        282 | 283 | 314 => WeakClass::Branch,
        _ => WeakClass::Unfillable,
    })
}

impl Arch for DynA64 {
    fn facts(&self) -> &'static Facts {
        self.0.facts()
    }
    fn e_flags(&self, inputs: &mut dyn Iterator<Item = u32>) -> Result<u32, Refusal> {
        self.0.e_flags(inputs)
    }
    fn classify(&self, r_type: u32, site: Site<'_>) -> Result<Howto, Refusal> {
        self.0.classify(r_type, site)
    }
    fn read_addend(&self, h: &Howto, field: &[u8]) -> i64 {
        self.0.read_addend(h, field)
    }
    fn write_addend(&self, h: &Howto, field: &mut [u8], addend: i64) -> Result<(), FieldError> {
        self.0.write_addend(h, field, addend)
    }
    fn scan(&self, cx: &ScanCx<'_>, at: &RelocAt<'_>, out: &mut DemandSink<'_>) {
        self.0.scan(cx, at, out);
    }
    fn apply(
        &self,
        cx: &FillCx<'_>,
        at: &RelocAt<'_>,
        out: &mut SectionOut<'_>,
    ) -> Result<(), FieldError> {
        self.0.apply(cx, at, out)
    }
    fn unit_window(
        &self,
        output: OutputKind,
        at: &RelocAt<'_>,
        names: &dyn SymNames,
    ) -> Option<UnitWindow> {
        self.0.unit_window(output, at, names)
    }
    fn consumed(&self, output: OutputKind, at: &RelocAt<'_>, names: &dyn SymNames) -> bool {
        self.0.consumed(output, at, names)
    }
    fn sequence_extent(&self, records: &dyn Records, i: u32) -> u32 {
        self.0.sequence_extent(records, i)
    }
    fn nonalloc_demand_types(&self) -> &'static [u32] {
        self.0.nonalloc_demand_types()
    }
    fn got(&self) -> &'static GotShape {
        self.0.got()
    }
    fn plt(&self, v: PltVariant) -> &'static PltShape {
        self.0.plt(v)
    }
    fn plt_variant(&self, cx: &OutputCx) -> Result<PltVariant, Refusal> {
        self.0.plt_variant(cx)
    }
    fn write_plt(
        &self,
        v: PltVariant,
        what: PltPart,
        cx: &SynthCx,
        out: &mut [u8],
    ) -> Result<(), FieldError> {
        self.0.write_plt(v, what, cx, out)
    }
    fn branch(&self, h: &Howto) -> Option<Branch> {
        self.0.branch(h)
    }
    fn thunk(&self) -> Option<&'static ThunkShape> {
        self.0.thunk()
    }
    fn write_thunk(&self, cx: &PatchCx, out: &mut [u8]) -> Result<(), FieldError> {
        self.0.write_thunk(cx, out)
    }
    fn thunk_target_ok(&self, cx: &PatchCx) -> Result<(), Refusal> {
        self.0.thunk_target_ok(cx)
    }
    fn errata(&self) -> &'static [Erratum] {
        self.0.errata()
    }
    fn erratum_sites(
        &self,
        e: Erratum,
        code: &Code<'_>,
        out: &mut Vec<ErratumSite>,
    ) -> Result<(), Unproved> {
        self.0.erratum_sites(e, code, out)
    }
    fn erratum_in_place(&self, e: Erratum, cx: &PatchCx) -> bool {
        self.0.erratum_in_place(e, cx)
    }
    fn patch(&self, e: Erratum) -> Option<&'static ThunkShape> {
        self.0.patch(e)
    }
    fn patch_reach(&self, e: Erratum) -> Reach {
        self.0.patch_reach(e)
    }
    fn write_patch(&self, e: Erratum, cx: &PatchCx, out: &mut [u8]) -> Result<(), FieldError> {
        self.0.write_patch(e, cx, out)
    }
    fn tls(&self) -> &'static TlsAbi {
        self.0.tls()
    }
    fn eh(&self) -> &'static EhEncodings {
        self.0.eh()
    }
    fn dyn_type(&self, role: DynRole) -> Option<u32> {
        self.0.dyn_type(role)
    }
    fn lazy_target(&self, _: PltVariant) -> Option<LazyTarget> {
        // D-49: AArch64's JUMP_SLOT words hold PLT0.
        Some(LazyTarget::Plt0)
    }
    fn dyn_tags(&self, cx: &OutputCx) -> &'static [DynTag] {
        self.0.dyn_tags(cx)
    }
    fn weak_use(
        &self,
        r_type: u32,
        _place: &[u8],
        _offset: u64,
        writable: bool,
    ) -> Option<WeakClass> {
        a64_weak_use(r_type, writable)
    }
    fn tombstone(&self, section_name: &[u8]) -> u64 {
        self.0.tombstone(section_name)
    }
    fn weak0(&self, h: &Howto) -> Weak0Rule {
        self.0.weak0(h)
    }
    fn property_merge(&self, pr_type: u32) -> Option<PropMerge> {
        self.0.property_merge(pr_type)
    }
}
