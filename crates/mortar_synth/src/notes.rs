//! Notes (`spec/synth.md` §8) and `.comment`'s string (§9).
//!
//! - `.note.gnu.build-id` ([`BuildIdPlan`]): the size in phase 80 (S-91);
//!   the header and, for `0xHEX`, the descriptor. The hash itself is the
//!   image phase's sealing step (S-92).
//! - `.note.gnu.property` ([`PropertyPlan`]): the merge of every live
//!   object's properties (S-94 to S-96, S-126), decided once in phase 80
//!   (S-97), and the output note.
//! - Other notes: [`check_note_alignment`] (S-99).
//! - `.comment`: [`COMMENT`] (S-100), a piece `mortar_merge` adds.

use std::collections::BTreeMap;

use mortar_arch::PropMerge;
use mortar_core::SectionId;
use mortar_core::plan::BuildId;
use mortar_diag::{Collector, Diagnostic, Kind, RefusalClass};
use mortar_elf::Class;
use mortar_elf::consts::NT_GNU_BUILD_ID;
use mortar_elf::note::Notes;

use crate::error::{Failed, SynthError, fail, feature, key, malformed};
use crate::out::{Window, align_up};

/// `.comment`'s piece of the internal file (S-100): `mortar
/// MAJOR.MINOR.PATCH`, NUL-terminated.
pub const COMMENT: &str = concat!(
    "mortar ",
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR"),
    ".",
    env!("CARGO_PKG_VERSION_PATCH"),
    "\0"
);

// ---- S-91, S-92: the build id ------------------------------------------

/// The note of I-70's `invalid-option-value`.
pub const HEX_NOTE: &str = "expected an even number of hexadecimal digits";
/// The refused name of `--build-id=uuid` in M1 (S-91: M2).
pub const UUID: &str = "--build-id=uuid";

/// The build-id note's shape: its descriptor size and, for `0xHEX`, the
/// bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdPlan {
    /// The descriptor size `n`.
    n: u32,
    /// The given bytes (`0xHEX`); `None` when the sealing step hashes.
    bytes: Option<Box<[u8]>>,
}

/// I-70's `HEX`: one or more pairs of hexadecimal digits, either case,
/// after a case-sensitive `0x`.
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let digits = text.strip_prefix("0x")?;
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok())
        .collect()
}

impl BuildIdPlan {
    /// The plan for `--build-id=STYLE` (`plan.shaping.build_id`); `None`
    /// for no note (S-91: `none`, or no option). The descriptor sizes
    /// (S-91, `spec/image.md` I-69): bare `--build-id` and `sha1` 20,
    /// `fast` 8, `sha256` 32, `0xHEX` its bytes. `md5` is refused
    /// (`option value`, note `use sha1`); `uuid` is M2 and refused in M1.
    /// A `0xHEX` with an odd number of digits, none, or a non-hex digit
    /// is `invalid-option-value` (I-70).
    pub fn new(style: Option<&BuildId>, diag: &Collector) -> Result<Option<Self>, Failed> {
        let Some(style) = style else { return Ok(None) };
        let (n, bytes) = match style {
            BuildId::Default | BuildId::Sha1 => (20, None),
            BuildId::Fast => (8, None),
            BuildId::Sha256 => (32, None),
            BuildId::Uuid => {
                return fail(diag, key(0), Diagnostic::refused(RefusalClass::OptionValue, UUID));
            }
            BuildId::Md5 => {
                return fail(
                    diag,
                    key(0),
                    Diagnostic::refused(RefusalClass::OptionValue, "--build-id=md5")
                        .note("use sha1"),
                );
            }
            BuildId::Hex(text) => match hex_bytes(text) {
                Some(b) if u32::try_from(b.len()).is_ok() => (b.len() as u32, Some(b.into())),
                _ => {
                    return fail(
                        diag,
                        key(0),
                        Diagnostic::error(Kind::InvalidOptionValue {
                            option: "--build-id".to_owned(),
                            value: text.clone(),
                        })
                        .note(HEX_NOTE),
                    );
                }
            },
        };
        Ok(Some(Self { n, bytes }))
    }

    /// The descriptor size `n`.
    #[must_use]
    pub fn desc_size(&self) -> u32 {
        self.n
    }

    /// The section's size (S-91 as ruled): `16 + n` rounded up to a
    /// multiple of 4.
    #[must_use]
    pub fn size(&self) -> u64 {
        16 + align_up(u64::from(self.n), 4)
    }

    /// Whether the sealing step computes the descriptor (every style but
    /// `0xHEX`).
    #[must_use]
    pub fn hashed(&self) -> bool {
        self.bytes.is_none()
    }

    /// The descriptor's range in the section, for the sealing step.
    #[must_use]
    pub fn desc_range(&self) -> std::ops::Range<u64> {
        16..16 + u64::from(self.n)
    }

    /// Writes the note: header, `GNU\0`, the descriptor (the hex bytes,
    /// or zeros for the sealing step to overwrite) and zero padding.
    pub fn write(&self, out: &mut Window<'_>) {
        out.word(0, 4, 4);
        out.word(4, 4, u64::from(self.n));
        out.word(8, 4, u64::from(NT_GNU_BUILD_ID));
        out.put(12, b"GNU\0");
        match &self.bytes {
            Some(b) => out.put(16, b),
            None => out.zero(16, u64::from(self.n)),
        }
        out.zero(16 + u64::from(self.n), self.size() - 16 - u64::from(self.n));
    }
}

// ---- S-94 to S-96, S-126: .note.gnu.property ---------------------------

/// `GNU_PROPERTY_STACK_SIZE`.
pub const STACK_SIZE: u32 = 1;
/// `GNU_PROPERTY_NO_COPY_ON_PROTECTED`.
pub const NO_COPY_ON_PROTECTED: u32 = 2;
/// `GNU_PROPERTY_MEMORY_SEAL`.
pub const MEMORY_SEAL: u32 = 3;

/// S-95's refusal of disagreeing `EqualOrAbsent` properties.
pub const PAUTH_DIFFERS: &str = "inputs with different pointer authentication ABIs";

/// The merge rule of one property type (S-95).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// One of the architecture's or the generic ranges' rules.
    Merge(PropMerge),
    /// `STACK_SIZE`: the largest value over the inputs that have it.
    Max,
    /// `NO_COPY_ON_PROTECTED`: present if any input has it.
    Present,
    /// `MEMORY_SEAL`: a request of the output (`-z memory-seal`); input
    /// instances are dropped.
    Option,
    /// Any other type: dropped with `unknown-property`.
    Unknown,
}

/// S-95's rule for `pr_type`; `processor` is `arch.property_merge` for
/// the range 0xc0000000..=0xdfffffff.
#[must_use]
pub fn rule(pr_type: u32, processor: &dyn Fn(u32) -> Option<PropMerge>) -> Rule {
    match pr_type {
        STACK_SIZE => Rule::Max,
        NO_COPY_ON_PROTECTED => Rule::Present,
        MEMORY_SEAL => Rule::Option,
        0xb000_0000..=0xb000_7fff => Rule::Merge(PropMerge::And),
        0xb000_8000..=0xb000_ffff => Rule::Merge(PropMerge::Or),
        0xc000_0000..=0xdfff_ffff => processor(pr_type).map_or(Rule::Unknown, Rule::Merge),
        _ => Rule::Unknown,
    }
}

/// One live relocatable object's `.note.gnu.property` sections: every
/// live object takes part, those without a note too (S-95: every
/// property absent).
#[derive(Debug, Clone, Copy)]
pub struct ObjectNotes<'a> {
    /// The object as diagnostics name it (`a.o`, `libx.a(y.o)`).
    pub path: &'a str,
    /// Each section's bytes, its `sh_addralign` and its id.
    pub sections: &'a [(SectionId, &'a [u8], u64)],
}

/// One property's value over a set of inputs.
type Value = Vec<u8>;

/// The output properties (S-96), ascending by type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PropertyPlan {
    props: Box<[(u32, Box<[u8]>)]>,
    word: u64,
}

fn words(data: &[u8]) -> Option<u64> {
    match data.len() {
        4 => Some(u64::from(u32::from_le_bytes(data.try_into().ok()?))),
        8 => Some(u64::from_le_bytes(data.try_into().ok()?)),
        _ => None,
    }
}

/// S-95 over `values`, one per input (`None`: the input lacks it), of a
/// type with `rule`. `Err(())` is a disagreement under `EqualOrAbsent`.
fn combine(rule: Rule, values: &[Option<&[u8]>]) -> Result<Option<Value>, ()> {
    let present = || values.iter().flatten().copied();
    let as32 = |v: &[u8]| v.try_into().map_or(0, u32::from_le_bytes);
    Ok(match rule {
        Rule::Merge(PropMerge::And) => {
            let all = values.iter().map(|v| v.map_or(0, as32)).fold(u32::MAX, |a, b| a & b);
            let all = if values.is_empty() { 0 } else { all };
            (all != 0).then(|| all.to_le_bytes().to_vec())
        }
        Rule::Merge(PropMerge::Or) => {
            let mut any = present().peekable();
            if any.peek().is_none() {
                return Ok(None);
            }
            Some(any.map(as32).fold(0, |a, b| a | b).to_le_bytes().to_vec())
        }
        Rule::Merge(PropMerge::OrAll) => {
            if values.is_empty() || values.iter().any(Option::is_none) {
                return Ok(None);
            }
            Some(present().map(as32).fold(0, |a, b| a | b).to_le_bytes().to_vec())
        }
        Rule::Merge(PropMerge::EqualOrAbsent) => {
            let mut it = present();
            let Some(first) = it.next() else { return Ok(None) };
            if it.any(|v| v != first) {
                return Err(());
            }
            Some(first.to_vec())
        }
        Rule::Max => {
            let mut best: Option<&[u8]> = None;
            for v in present() {
                if best.is_none_or(|b| words(v) > words(b)) {
                    best = Some(v);
                }
            }
            best.map(<[u8]>::to_vec)
        }
        Rule::Present => present().next().map(|_| Vec::new()),
        Rule::Option | Rule::Unknown => None,
    })
}

/// The properties of one note, sorted by type (S-94), or the diagnostic.
fn note_props(
    desc: mortar_elf::note::Properties<'_>,
    word: usize,
    rule_of: &dyn Fn(u32) -> Rule,
) -> Result<Vec<(u32, Vec<u8>)>, SynthError> {
    let short = || malformed("program property is too short");
    let mut props = Vec::new();
    for p in desc {
        let p = p.map_err(|_| short())?;
        let ok = match rule_of(p.pr_type) {
            Rule::Merge(PropMerge::EqualOrAbsent) | Rule::Unknown | Rule::Option => true,
            Rule::Merge(_) => p.data.len() == 4,
            Rule::Max => p.data.len() == word,
            Rule::Present => p.data.is_empty(),
        };
        if !ok {
            return Err(short());
        }
        props.push((p.pr_type, p.data.to_vec()));
    }
    props.sort_by_key(|p| p.0);
    Ok(props)
}

/// S-95 over `inputs`, each a map of its properties. A type repeated in
/// one input's list is combined first by the same rule.
fn merge_level(
    inputs: &[BTreeMap<u32, Value>],
    rule_of: &dyn Fn(u32) -> Rule,
) -> Result<BTreeMap<u32, Value>, u32> {
    let types: std::collections::BTreeSet<u32> =
        inputs.iter().flat_map(|m| m.keys().copied()).collect();
    let mut out = BTreeMap::new();
    for t in types {
        let rule = rule_of(t);
        let values: Vec<Option<&[u8]>> =
            inputs.iter().map(|m| m.get(&t).map(Vec::as_slice)).collect();
        match combine(rule, &values) {
            Ok(Some(v)) => {
                out.insert(t, v);
            }
            Ok(None) => {}
            Err(()) => return Err(t),
        }
    }
    Ok(out)
}

/// A note's properties as a map; a type given twice in one note is
/// combined by its rule, as two notes would be.
fn note_map(
    props: Vec<(u32, Vec<u8>)>,
    rule_of: &dyn Fn(u32) -> Rule,
) -> Result<BTreeMap<u32, Value>, u32> {
    let mut parts: Vec<BTreeMap<u32, Value>> = vec![BTreeMap::new()];
    for (t, v) in props {
        if let Some(m) = parts.iter_mut().find(|m| !m.contains_key(&t)) {
            m.insert(t, v);
        } else {
            parts.push(BTreeMap::from([(t, v)]));
        }
    }
    if parts.len() == 1 {
        return Ok(parts.pop().unwrap_or_default());
    }
    // Only the repeated types are combined; the others pass unchanged.
    let mut out = parts[0].clone();
    for t in parts[1..].iter().flat_map(|m| m.keys().copied()).collect::<Vec<_>>() {
        let values: Vec<Option<&[u8]>> = parts
            .iter()
            .filter(|m| m.contains_key(&t))
            .map(|m| m.get(&t).map(Vec::as_slice))
            .collect();
        match combine(rule_of(t), &values) {
            Ok(Some(v)) => {
                out.insert(t, v);
            }
            Ok(None) => {
                out.remove(&t);
            }
            Err(()) => return Err(t),
        }
    }
    Ok(out)
}

impl PropertyPlan {
    /// S-94 to S-96 and S-126 over every live relocatable object.
    /// `processor` is `arch.property_merge`; `memory_seal` is `-z
    /// memory-seal` (R-C3). Unknown types are dropped with one
    /// `unknown-property` warning per object and type; disagreeing
    /// `EqualOrAbsent` properties are refused.
    ///
    /// Within an object, its notes combine by each property's own rule
    /// as if each note were an object (S-126, the supervisor's ruling on
    /// Q12); across objects as S-95.
    pub fn new(
        objects: &[ObjectNotes<'_>],
        class: Class,
        processor: &dyn Fn(u32) -> Option<PropMerge>,
        memory_seal: bool,
        diag: &Collector,
    ) -> Result<Self, Failed> {
        let word = class.word_size();
        let rule_of = |t: u32| rule(t, processor);
        let mut per_object = Vec::with_capacity(objects.len());
        let mut failed = false;
        for obj in objects {
            let okey = key(obj.sections.first().map_or(0, |s| u64::from(s.0.0)));
            let mut notes: Vec<BTreeMap<u32, Value>> = Vec::new();
            let mut unknown = std::collections::BTreeSet::new();
            for &(id, bytes, align) in obj.sections {
                let mut bad = None;
                for note in Notes::new(bytes, align as usize, class, 0) {
                    let Ok(note) = note else {
                        bad = Some(malformed("program property is too short"));
                        break;
                    };
                    let Some(props) = note.gnu_properties() else { continue };
                    match note_props(props, word, &rule_of) {
                        Ok(props) => {
                            for p in &props {
                                if rule_of(p.0) == Rule::Unknown {
                                    unknown.insert(p.0);
                                }
                            }
                            if let Ok(m) = note_map(props, &rule_of) {
                                notes.push(m);
                            } else {
                                bad = Some(feature(PAUTH_DIFFERS));
                                break;
                            }
                        }
                        Err(d) => {
                            bad = Some(d.into_diagnostic());
                            break;
                        }
                    }
                }
                if let Some(d) = bad {
                    diag.emit(
                        key(u64::from(id.0)),
                        d.at(mortar_diag::Place::section(
                            mortar_diag::FileRef::new(obj.path),
                            ".note.gnu.property",
                            0,
                        )),
                    );
                    failed = true;
                }
            }
            for t in unknown {
                diag.emit(
                    okey,
                    Diagnostic::warning(Kind::UnknownProperty {
                        path: obj.path.to_owned(),
                        pr_type: t,
                    }),
                );
            }
            // S-126: the object's notes, as objects. An object with no
            // property note has every property absent.
            let combined = if notes.is_empty() {
                BTreeMap::new()
            } else {
                merge_level(&notes, &rule_of).unwrap_or_else(|_| {
                    diag.emit(okey, feature(PAUTH_DIFFERS).at(mortar_diag::FileRef::new(obj.path)));
                    failed = true;
                    BTreeMap::new()
                })
            };
            per_object.push(combined);
        }
        if failed {
            return Err(Failed);
        }
        let Ok(mut merged) = merge_level(&per_object, &rule_of) else {
            return fail(diag, key(0), feature(PAUTH_DIFFERS));
        };
        // An `And` type that no input has is absent (merge_level saw no
        // input with it); one every input lacks cannot appear.
        if memory_seal {
            merged.insert(MEMORY_SEAL, Vec::new());
        }
        Ok(Self {
            props: merged.into_iter().map(|(t, v)| (t, v.into_boxed_slice())).collect(),
            word: word as u64,
        })
    }

    /// Whether the section exists (S-96: some property survives).
    #[must_use]
    pub fn exists(&self) -> bool {
        !self.props.is_empty()
    }

    /// The surviving properties, ascending by type.
    #[must_use]
    pub fn properties(&self) -> &[(u32, Box<[u8]>)] {
        &self.props
    }

    /// A 32-bit property's value (0 when absent): `FEATURE_1_AND` for
    /// the PLT variant (S-97).
    #[must_use]
    pub fn value_u32(&self, pr_type: u32) -> u32 {
        self.props
            .iter()
            .find(|p| p.0 == pr_type)
            .and_then(|p| <[u8; 4]>::try_from(&p.1[..]).ok())
            .map_or(0, u32::from_le_bytes)
    }

    fn desc_size(&self) -> u64 {
        self.props.iter().map(|p| 8 + align_up(p.1.len() as u64, self.word)).sum()
    }

    /// The section's size (S-108): `16 + Σ(8 + align_up(datasz, a))`; 0
    /// when it does not exist.
    #[must_use]
    pub fn size(&self) -> u64 {
        if self.exists() { 16 + self.desc_size() } else { 0 }
    }

    /// Writes the one note (S-96).
    pub fn write(&self, out: &mut Window<'_>) {
        out.word(0, 4, 4);
        out.word(4, 4, self.desc_size());
        out.word(8, 4, u64::from(mortar_elf::consts::NT_GNU_PROPERTY_TYPE_0));
        out.put(12, b"GNU\0");
        let mut at = 16;
        for (t, v) in &*self.props {
            out.word(at, 4, u64::from(*t));
            out.word(at + 4, 4, v.len() as u64);
            out.put(at + 8, v);
            let padded = align_up(v.len() as u64, self.word);
            out.zero(at + 8 + v.len() as u64, padded - v.len() as u64);
            at += 8 + padded;
        }
    }
}

// ---- S-99 --------------------------------------------------------------

/// S-99: the live input sections of one output note section must share
/// one alignment. `inputs` are `(section, sh_addralign)`.
pub fn check_note_alignment(
    name: &str,
    inputs: &[(SectionId, u64)],
    diag: &Collector,
) -> Result<(), Failed> {
    match inputs.iter().find(|i| i.1 != inputs[0].1) {
        Some(&(id, _)) => fail(
            diag,
            key(u64::from(id.0)),
            feature("notes of different alignment in one output section")
                .note(format!("in {name}")),
        ),
        None => Ok(()),
    }
}

/// `.note.mason` (`spec/odr.md` O-50 to O-52; WP-075): the note's bytes as
/// `mortar_meta::table::note` encoded them; the content is planned before
/// layout and does not depend on addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteMasonPlan {
    bytes: Box<[u8]>,
}

impl NoteMasonPlan {
    /// The plan for the note `bytes`.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes: bytes.into() }
    }

    /// The note's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The size.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// `.mason.meta` kept for tools (`spec/meta.md` MT-32; WP-076): the live
/// inputs' units, concatenated by `mortar_odr::kept_units`; the content is
/// planned before layout and does not depend on addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasonMetaPlan {
    bytes: Box<[u8]>,
}

impl MasonMetaPlan {
    /// The plan for the section's `bytes`.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes: bytes.into() }
    }

    /// The section's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The size.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}
