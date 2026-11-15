//! What phase 60 reports (`spec/synth.md` §1 "Diagnostics", `spec/diag.md`):
//! a [`Problem`] names the place (file, section, offset), the key of
//! `spec/model.md` §8 (phase 60, unit `FileId`, seq `section index × 2^32 +
//! piece`) and a [`What`], which a test compares and [`Problem::diagnostic`]
//! turns into the `malformed-input` or `refused` diagnostic with the
//! spec's wording.

use mortar_core::FileId;
use mortar_diag::{Diagnostic, FileRef, Key, Kind, Place, RefusalClass};

/// The phase ordinal of split and merge (`spec/model.md` §8).
pub const PHASE: u16 = 60;

/// What is wrong, by rule. The text of each is [`What::message`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum What {
    /// S-7: `malformed-input`.
    NotMultipleOfEntsize,
    /// S-8: `malformed-input`.
    Unterminated,
    /// S-12 (the table's shape, `spec/model.md` §8): the `Split` columns of
    /// one link hold at most 2^32 - 1 pieces; `feature`.
    TooManyPiecesInLink,
    /// A split section of 4 GiB or more: piece offsets are `u32`
    /// (`spec/model.md` §3.5); `feature`.
    SectionTooLarge,
    /// S-18, S-50 rule 5: `malformed-input`.
    OutsideSection,
    /// S-21: `feature`.
    GotSectionAddend,
    /// S-34: `feature`.
    Dwarf64,
    /// S-36: `malformed-input`.
    CieVersion(u8),
    /// S-36: `malformed-input`; the augmentation string as written.
    Augmentation(String),
    /// S-36: augmentation data (or another CIE or FDE field) runs past its
    /// record; `malformed-input`.
    CorruptedRecord,
    /// S-37: `feature`.
    Encoding(u8),
    /// S-38: `feature`.
    FdeWithoutReloc,
    /// S-38: the initial location's relocation is not of class `Pc` or
    /// `AbsWord`, or does not fit the field its encoding gives
    /// (`relocation-type`); the relocation's name.
    InitialLocationReloc(String),
    /// Two relocations (neither `R_*_NONE`) at one place of `.eh_frame`;
    /// `feature`.
    TwoRelocs,
    /// S-39: `malformed-input`.
    PastEnd,
    /// S-39: `malformed-input`.
    BadCieReference,
    /// S-39: `malformed-input`.
    AfterTerminator,
    /// S-39: `malformed-input`.
    RelocNotInPiece,
    /// S-50 rule 4: `malformed-input`.
    IntoRemovedRecord,
    /// The relocations or symbols of a split section could not be read
    /// (`mortar_elf`'s statement, or a refusal of the architecture's
    /// `classify`): `malformed-input` with this text.
    Malformed(String),
    /// A refusal of the architecture's `classify` (`spec/arch.md` §3.2).
    Refused(RefusalClass, String),
}

impl What {
    /// The refusal class, for the `refused` ones.
    #[must_use]
    pub fn refusal_class(&self) -> Option<RefusalClass> {
        match self {
            Self::TooManyPiecesInLink
            | Self::SectionTooLarge
            | Self::GotSectionAddend
            | Self::Dwarf64
            | Self::Encoding(_)
            | Self::FdeWithoutReloc
            | Self::TwoRelocs => Some(RefusalClass::Feature),
            Self::InitialLocationReloc(_) => Some(RefusalClass::RelocationType),
            Self::Refused(class, _) => Some(*class),
            _ => None,
        }
    }

    /// The `{what}` of `malformed-input`, or the name of the refused
    /// construct (`spec/synth.md` §3, §4; lld's wording where lld has one).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NotMultipleOfEntsize => "section size is not a multiple of sh_entsize".into(),
            Self::Unterminated => "string is not null terminated".into(),
            Self::TooManyPiecesInLink => "more than 2^32 - 1 split pieces in a link".into(),
            Self::SectionTooLarge => "a mergeable or .eh_frame section of 4 GiB or more".into(),
            Self::OutsideSection => {
                "offset is outside the section; refer to it through a named symbol".into()
            }
            Self::GotSectionAddend => {
                "GOT reference to a section symbol plus addend in a mergeable section".into()
            }
            Self::Dwarf64 => "64-bit DWARF .eh_frame record".into(),
            Self::CieVersion(v) => format!("FDE version 1 or 3 expected, but got {v}"),
            Self::Augmentation(s) => format!("unknown .eh_frame augmentation string: {s}"),
            Self::CorruptedRecord => "corrupted .eh_frame record: a field runs past its end".into(),
            Self::Encoding(e) => format!(".eh_frame pointer encoding {e:#04x}"),
            Self::FdeWithoutReloc => "FDE without a relocation for its initial location".into(),
            Self::InitialLocationReloc(name) => name.clone(),
            Self::TwoRelocs => "two relocations at one place of .eh_frame".into(),
            Self::PastEnd => "CIE/FDE ends past the end of the section".into(),
            Self::BadCieReference => "invalid CIE reference".into(),
            Self::AfterTerminator => "data after the .eh_frame terminator".into(),
            Self::RelocNotInPiece => "relocation is not in any piece".into(),
            Self::IntoRemovedRecord => "reference into a removed .eh_frame record".into(),
            Self::Malformed(s) | Self::Refused(_, s) => s.clone(),
        }
    }
}

/// One problem of phase 60, at a byte of an input section.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Problem {
    /// The file (the unit of the key).
    pub file: FileId,
    /// The section's index in its file.
    pub section: u32,
    /// The piece's ordinal among the section's pieces (records, for
    /// `.eh_frame`) where it was found; 0 when none applies.
    pub piece: u32,
    /// The offset in the section the diagnostic points at.
    pub offset: u64,
    pub what: What,
}

impl Problem {
    /// The diagnostic key (`spec/model.md` §8): phase 60, unit `FileId`,
    /// seq `section index × 2^32 + piece`.
    #[must_use]
    pub fn key(&self) -> Key {
        Key::new(PHASE, self.file.0, (u64::from(self.section) << 32) | u64::from(self.piece))
    }

    /// The diagnostic, at `file:(section+0xoffset)`.
    #[must_use]
    pub fn diagnostic(&self, file: FileRef, section_name: &str) -> Diagnostic {
        let place = Place::section(file, section_name, self.offset);
        match self.what.refusal_class() {
            Some(class) => Diagnostic::refused(class, self.what.message()).at(place),
            None => Diagnostic::error(Kind::Malformed { what: self.what.message() }).at(place),
        }
    }
}
