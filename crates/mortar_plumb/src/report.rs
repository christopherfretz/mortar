//! Findings and coverage (`spec/plumb.md` §7).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use mortar_diag::{Diagnostic, FileRef, Kind, Place};

/// A rule of the spec a finding breaks (§6, §7.1). The id is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// O1: the output cannot be read.
    OutputUnreadable,
    /// S1 .. S12 (§6.2).
    Structure(u8),
    /// I1: an input the map names was not given.
    InputMissing,
    /// I2: an input cannot be read, or is not an object of the output's machine.
    InputUnreadable,
    /// M1 .. M5 (§3.3).
    Map(u8),
    /// R1: a relocated field holds the wrong value (§5).
    Field,
    /// R2: the value fails the type's overflow check.
    Overflow,
    /// R3: the value has low bits the field drops.
    Misaligned,
    /// R4: a GOT-indirect reference reaches no slot that holds S+A.
    GotSlot,
    /// R5: a relocation refers to a section that is not in the output.
    DiscardedTarget,
    /// R6: a reference to a piece of a mergeable section reaches other bytes.
    MergePiece,
    /// R7: the relocated place is not in the output's memory.
    Place,
    /// Y1: an output symbol's value is not where its definition was placed.
    SymbolValue,
}

impl Rule {
    /// The stable id (§7.1).
    pub fn id(self) -> String {
        match self {
            Rule::OutputUnreadable => "O1".into(),
            Rule::Structure(n) => format!("S{n}"),
            Rule::InputMissing => "I1".into(),
            Rule::InputUnreadable => "I2".into(),
            Rule::Map(n) => format!("M{n}"),
            Rule::Field => "R1".into(),
            Rule::Overflow => "R2".into(),
            Rule::Misaligned => "R3".into(),
            Rule::GotSlot => "R4".into(),
            Rule::DiscardedTarget => "R5".into(),
            Rule::MergePiece => "R6".into(),
            Rule::Place => "R7".into(),
            Rule::SymbolValue => "Y1".into(),
        }
    }
}

/// A rewriting a linker may perform, accepted in its rewritten form (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rewrite {
    /// W1: `ADRP`+`ADD` to `NOP`+`ADR`.
    AdrpAddToAdr,
    /// W2: `ADRP`+`LDR` of a GOT slot to `ADRP`+`ADD` of the address.
    GotToAdrpAdd,
    /// W3: `ADRP`+`LDR` of a GOT slot to `NOP`+`ADR` of the address.
    GotToAdr,
    /// W4: a branch to an undefined weak symbol made a `NOP`.
    WeakBranchNop,
    /// W5: a branch to an undefined weak symbol to the next instruction.
    WeakBranchNext,
    /// W6: a branch through a range-extension veneer.
    Veneer,
    /// W7: erratum 843419: the `ADRP` made an `ADR` of the same page.
    Erratum843419Adr,
    /// W8: erratum 843419: the load or store moved to a patch.
    Erratum843419Patch,
}

impl Rewrite {
    pub const ALL: [Rewrite; 8] = [
        Rewrite::AdrpAddToAdr,
        Rewrite::GotToAdrpAdd,
        Rewrite::GotToAdr,
        Rewrite::WeakBranchNop,
        Rewrite::WeakBranchNext,
        Rewrite::Veneer,
        Rewrite::Erratum843419Adr,
        Rewrite::Erratum843419Patch,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Rewrite::AdrpAddToAdr => "adrp-add-to-adr",
            Rewrite::GotToAdrpAdd => "got-to-adrp-add",
            Rewrite::GotToAdr => "got-to-adr",
            Rewrite::WeakBranchNop => "weak-branch-nop",
            Rewrite::WeakBranchNext => "weak-branch-next",
            Rewrite::Veneer => "veneer",
            Rewrite::Erratum843419Adr => "erratum-843419-adr",
            Rewrite::Erratum843419Patch => "erratum-843419-patch",
        }
    }
}

/// Why a relocation record of a placed section was not checked (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// A TLS type: stage 2.
    Tls,
    /// The symbol is an IFUNC (reached through IRELATIVE): stage 2.
    Ifunc,
    /// The record is in a non-alloc section (debug information): stage 2.
    NonAlloc,
    /// The record is in `.eh_frame` whose records the map does not place
    /// one by one (GNU ld, mold): stage 2 checks `.eh_frame` itself.
    EhFrame,
    /// A type the stage-1 table does not have.
    Type,
    /// The machine is not AArch64: stage 1 has only the AArch64 table.
    Machine,
    /// The section's place could not be decided (§3.3).
    SectionNotLocatable,
    /// The target is in a mergeable section and no reference gives the
    /// piece's address (§5.6).
    PieceNotLocatable,
    /// The record is in a mergeable section.
    InMergeable,
    /// The global symbol has no value in the output's `.symtab` and no
    /// unique placed definition (§4.4).
    SymbolUnresolved,
    /// A GOT-indirect page reference with no low-bits partner, or the
    /// reverse (§5.5).
    GotUnpaired,
    /// The input file of the section cannot be read.
    InputUnreadable,
}

impl Reason {
    pub const fn id(self) -> &'static str {
        match self {
            Reason::Tls => "tls",
            Reason::Ifunc => "ifunc",
            Reason::NonAlloc => "non-alloc",
            Reason::EhFrame => "eh-frame",
            Reason::Type => "type",
            Reason::Machine => "machine",
            Reason::SectionNotLocatable => "section-not-locatable",
            Reason::PieceNotLocatable => "piece-not-locatable",
            Reason::InMergeable => "in-mergeable",
            Reason::SymbolUnresolved => "symbol-unresolved",
            Reason::GotUnpaired => "got-unpaired",
            Reason::InputUnreadable => "input-unreadable",
        }
    }

    /// The reasons stage 2 removes (§7.2): the only ones allowed on the
    /// reference corpus.
    pub const fn is_stage2(self) -> bool {
        matches!(self, Reason::Tls | Reason::Ifunc | Reason::NonAlloc | Reason::EhFrame)
    }
}

/// How a verified record's target was established (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Via {
    /// S from the tables (symbol table, placement).
    Direct,
    /// Through a GOT slot found from the fields and checked to hold S+A.
    GotSlot,
    /// To a mergeable piece found from the fields and checked by content.
    Piece,
}

impl Via {
    pub const fn id(self) -> &'static str {
        match self {
            Via::Direct => "direct",
            Via::GotSlot => "got-slot",
            Via::Piece => "piece",
        }
    }
}

/// Where a finding is (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    /// The input file (`path` or `archive(member)`).
    pub file: String,
    pub member: Option<String>,
    pub section: String,
    /// The offset in the input section.
    pub offset: u64,
}

/// One finding (§7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: Rule,
    /// The output address it is about, when it has one: the sort key.
    pub addr: Option<u64>,
    pub site: Option<Site>,
    /// The relocation type's name.
    pub reloc: Option<String>,
    pub symbol: Option<String>,
    pub expected: Option<u64>,
    pub found: Option<u64>,
    /// One line, lower-case, no final period.
    pub message: String,
}

impl Finding {
    pub(crate) fn new(rule: Rule, message: impl Into<String>) -> Self {
        Finding {
            rule,
            addr: None,
            site: None,
            reloc: None,
            symbol: None,
            expected: None,
            found: None,
            message: message.into(),
        }
    }

    pub(crate) fn at(mut self, addr: u64) -> Self {
        self.addr = Some(addr);
        self
    }

    pub(crate) fn site(mut self, site: Site) -> Self {
        self.site = Some(site);
        self
    }

    /// The finding as a `mortar_diag` diagnostic (§7.3): kind
    /// `malformed-input`, placed at the input section and offset.
    pub fn to_diagnostic(&self, output: &str) -> Diagnostic {
        let mut what = format!("verifier rule {}: {}", self.rule.id(), self.message);
        if let Some(r) = &self.reloc {
            let _ = write!(what, "; {r}");
        }
        if let Some(s) = &self.symbol {
            let _ = write!(what, " against '{s}'");
        }
        if let Some(a) = self.addr {
            let _ = write!(what, " at {a:#x}");
        }
        if let (Some(e), Some(f)) = (self.expected, self.found) {
            let _ = write!(what, ": expected {e:#x}, found {f:#x}");
        }
        let d = Diagnostic::error(Kind::Malformed { what });
        match &self.site {
            Some(s) => {
                let file = match &s.member {
                    Some(m) => FileRef::in_archive(s.file.clone(), m.clone()),
                    None => FileRef::new(s.file.clone()),
                };
                d.at(Place::section(file, s.section.clone(), s.offset))
            }
            None => d.at(Place::file(output)),
        }
    }
}

/// What was checked, and what was not (§7.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Relocation records of placed sections: the sum of the four below.
    pub records: u64,
    pub verified: BTreeMap<Via, u64>,
    pub accepted: BTreeMap<Rewrite, u64>,
    pub not_checked: BTreeMap<Reason, u64>,
    /// `R_AARCH64_NONE` records: nothing to check.
    pub none: u64,
    /// Records that broke a rule (each has a finding).
    pub failed: u64,
    /// Records of sections the map does not place (discarded, absent):
    /// not part of the output, not counted in `records`.
    pub absent_records: u64,
    /// Input sections placed whole, placed as pieces, merged, discarded,
    /// or absent from the map.
    pub sections_placed: u64,
    pub sections_pieces: u64,
    pub sections_merged: u64,
    pub sections_discarded: u64,
    pub sections_absent: u64,
    /// Output symbols checked against their input definition (Y1), and
    /// those without a unique placed definition.
    pub symbols_checked: u64,
    pub symbols_unchecked: u64,
    /// Bytes of placed sections compared with the input (S12).
    pub content_bytes: u64,
    /// Verified or accepted records by relocation type.
    pub checked_types: BTreeMap<u32, u64>,
}

impl Coverage {
    pub fn verified_total(&self) -> u64 {
        self.verified.values().sum()
    }
    pub fn accepted_total(&self) -> u64 {
        self.accepted.values().sum()
    }
    pub fn not_checked_total(&self) -> u64 {
        self.not_checked.values().sum()
    }
    pub fn not_checked_of(&self, reason: Reason) -> u64 {
        self.not_checked.get(&reason).copied().unwrap_or(0)
    }
    pub fn accepted_of(&self, rewrite: Rewrite) -> u64 {
        self.accepted.get(&rewrite).copied().unwrap_or(0)
    }
    pub fn verified_of(&self, via: Via) -> u64 {
        self.verified.get(&via).copied().unwrap_or(0)
    }

    pub(crate) fn add_verified(&mut self, via: Via, r_type: u32) {
        *self.verified.entry(via).or_default() += 1;
        *self.checked_types.entry(r_type).or_default() += 1;
    }
    pub(crate) fn add_accepted(&mut self, r: Rewrite, r_type: u32) {
        *self.accepted.entry(r).or_default() += 1;
        *self.checked_types.entry(r_type).or_default() += 1;
    }
    pub(crate) fn add_not_checked(&mut self, r: Reason, n: u64) {
        *self.not_checked.entry(r).or_default() += n;
    }
}

/// The verifier's answer (§7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Sorted by output address, then rule, then site (§7.1).
    pub findings: Vec<Finding>,
    pub coverage: Coverage,
}

impl Report {
    /// No finding.
    pub fn clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// The findings with the rule `id`.
    pub fn with_rule(&self, id: &str) -> impl Iterator<Item = &Finding> {
        let id = id.to_string();
        self.findings.iter().filter(move |f| f.rule.id() == id)
    }

    pub(crate) fn sort(&mut self) {
        self.findings.sort_by(|a, b| {
            (a.addr.is_none(), a.addr, a.rule, &a.site, &a.message).cmp(&(
                b.addr.is_none(),
                b.addr,
                b.rule,
                &b.site,
                &b.message,
            ))
        });
    }

    /// The findings as `mortar_diag` renders them, in report order (§7.3).
    pub fn render(&self, output: &str) -> String {
        let style = mortar_diag::Style::plain("mortar-plumb");
        self.findings.iter().map(|f| f.to_diagnostic(output).render(&style)).collect()
    }

    /// The coverage summary, one item per line (§7.2).
    pub fn summary(&self) -> String {
        let c = &self.coverage;
        let mut s = String::new();
        let list = |items: Vec<(&str, u64)>| -> String {
            let parts: Vec<String> =
                items.into_iter().filter(|i| i.1 > 0).map(|(k, v)| format!("{k} {v}")).collect();
            if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) }
        };
        let _ = writeln!(s, "relocation records in placed sections: {}", c.records);
        let _ = writeln!(
            s,
            "  verified: {}{}",
            c.verified_total(),
            list(c.verified.iter().map(|(k, v)| (k.id(), *v)).collect())
        );
        let _ = writeln!(
            s,
            "  accepted as a specified rewriting: {}{}",
            c.accepted_total(),
            list(c.accepted.iter().map(|(k, v)| (k.id(), *v)).collect())
        );
        let _ = writeln!(
            s,
            "  not checked: {}{}",
            c.not_checked_total(),
            list(c.not_checked.iter().map(|(k, v)| (k.id(), *v)).collect())
        );
        let _ = writeln!(s, "  none: {}", c.none);
        let _ = writeln!(s, "  failed: {}", c.failed);
        let _ = writeln!(s, "records of sections not in the output: {}", c.absent_records);
        let _ = writeln!(
            s,
            "input sections: placed {}, as pieces {}, merged {}, discarded {}, absent from the map {}",
            c.sections_placed,
            c.sections_pieces,
            c.sections_merged,
            c.sections_discarded,
            c.sections_absent
        );
        let _ = writeln!(
            s,
            "symbols: checked {}, without a unique definition {}",
            c.symbols_checked, c.symbols_unchecked
        );
        let _ = writeln!(s, "content compared: {} bytes", c.content_bytes);
        let _ = writeln!(s, "findings: {}", self.findings.len());
        s
    }
}
