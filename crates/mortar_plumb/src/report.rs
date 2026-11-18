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
    /// R8: the instruction at a relocated place is not of the class the
    /// type is defined for, or a thread-local type names a symbol that is
    /// not thread-local (§4.3, §4.7).
    Insn,
    /// R9: in an output marked BTI, a range-extension veneer ends in an
    /// indirect branch to a word that is not a landing pad (§5.4).
    LandingPad,
    /// T1: a thread-local relocation in an output without `PT_TLS` (§4.7).
    NoTls,
    /// Y1: an output symbol's value is not where its definition was placed.
    SymbolValue,
    /// E1-E5: `.eh_frame` and `.eh_frame_hdr` (§6.4).
    EhFrame(u8),
    /// Y2-Y5: the symbol table (§6.5).
    SymbolTable(u8),
    /// F1-F4: IFUNC in a static output (§4.8).
    Ifunc(u8),
    /// P1-P2: program properties (§6.6).
    Property(u8),
    /// B1: mortar's build id (§6.7).
    BuildId,
    /// C1-C6: erratum 843419 fixes by site (§5.10).
    Erratum(u8),
    /// P-D1 .. P-D15: stage 3, dynamic executables and static-pie (§12).
    Dyn(u8),
    /// P-I1 .. P-I6: holes and dead ranges of an incremental link (§6.8;
    /// `spec/incr.md` IN-3a, IN-60).
    Incr(u8),
    /// P-E1 .. P-E6: an update and a full link not equivalent (§6.9;
    /// `spec/incr.md` IN-2, IN-62).
    Equiv(u8),
    /// P-M1 .. P-M3: stage 4, Mason metadata and the ODR table (§13).
    Meta(u8),
    /// P-G1 .. P-G9: `.gdb_index` (§6.10; `spec/synth.md` §17).
    GdbIndex(u8),
    /// P-P2 .. P-P11: stage R, a relocatable (`-r`) output (§14).
    Rel(u8),
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
            Rule::Insn => "R8".into(),
            Rule::LandingPad => "R9".into(),
            Rule::NoTls => "T1".into(),
            Rule::SymbolValue => "Y1".into(),
            Rule::EhFrame(n) => format!("E{n}"),
            Rule::SymbolTable(n) => format!("Y{n}"),
            Rule::Ifunc(n) => format!("F{n}"),
            Rule::Property(n) => format!("P{n}"),
            Rule::BuildId => "B1".into(),
            Rule::Erratum(n) => format!("C{n}"),
            Rule::Dyn(n) => format!("P-D{n}"),
            Rule::Incr(n) => format!("P-I{n}"),
            Rule::Equiv(n) => format!("P-E{n}"),
            Rule::Meta(n) => format!("P-M{n}"),
            Rule::GdbIndex(n) => format!("P-G{n}"),
            Rule::Rel(n) => format!("P-P{n}"),
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
    /// W9: TLS initial-exec to local-exec: `ADRP`+`LDR` to `MOVZ`+`MOVK`.
    TlsIeToLe,
    /// W10: a TLSDESC sequence to local-exec (either of two forms).
    TlsDescToLe,
    /// W11: a TLSDESC sequence to initial-exec.
    TlsDescToIe,
    /// Stage 3: a GD sequence rewritten to initial-exec (GNU ld, x86-64).
    TlsGdToIe,
    /// W12: a traditional general-dynamic sequence to local-exec.
    TlsGdToLe,
    /// W13: a traditional local-dynamic sequence to local-exec.
    TlsLdToLe,
    /// X1 (x86-64): a GOT load made a `lea` of the address (§5.9).
    GotToLea,
    /// X2 (x86-64): an indirect call or jump through the GOT made direct.
    GotToDirect,
    /// X3 (x86-64): a GOT operand of an ALU operation or `test` made an
    /// immediate.
    GotToImm,
    /// X4 (x86-64): a GOT load made `mov $S` (GNU ld).
    GotToMovImm,
    /// Stage R: one of P-P3's per-linker forms, from its linker (§14.5).
    PerLinker,
}

impl Rewrite {
    pub const ALL: [Rewrite; 19] = [
        Rewrite::AdrpAddToAdr,
        Rewrite::GotToAdrpAdd,
        Rewrite::GotToAdr,
        Rewrite::WeakBranchNop,
        Rewrite::WeakBranchNext,
        Rewrite::Veneer,
        Rewrite::Erratum843419Adr,
        Rewrite::Erratum843419Patch,
        Rewrite::TlsIeToLe,
        Rewrite::TlsDescToLe,
        Rewrite::TlsDescToIe,
        Rewrite::TlsGdToIe,
        Rewrite::TlsGdToLe,
        Rewrite::TlsLdToLe,
        Rewrite::GotToLea,
        Rewrite::GotToDirect,
        Rewrite::GotToImm,
        Rewrite::GotToMovImm,
        Rewrite::PerLinker,
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
            Rewrite::TlsIeToLe => "tlsie-to-le",
            Rewrite::TlsDescToLe => "tlsdesc-to-le",
            Rewrite::TlsDescToIe => "tlsdesc-to-ie",
            Rewrite::TlsGdToIe => "tlsgd-to-ie",
            Rewrite::TlsGdToLe => "tlsgd-to-le",
            Rewrite::TlsLdToLe => "tlsld-to-le",
            Rewrite::GotToLea => "got-to-lea",
            Rewrite::GotToDirect => "got-to-direct",
            Rewrite::GotToImm => "got-to-imm",
            Rewrite::GotToMovImm => "got-to-mov-imm",
            Rewrite::PerLinker => "per-linker",
        }
    }
}

/// Why a relocation record of a placed section was not checked (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// A thread-local construct stage 2 does not judge (§4.7): an
    /// undefined weak TLS symbol, a non-TLS type against a TLS symbol, a
    /// TLSDESC sequence kept for the dynamic loader, an unusable `PT_TLS`.
    Tls,
    /// A TLSDESC record that is not in a whole four-instruction sequence.
    TlsSequence,
    /// The symbol is an IFUNC (reached through IRELATIVE): stage 2.
    Ifunc,
    /// The record is in a non-alloc section (debug information): stage 2.
    NonAlloc,
    /// The record or its section is compressed (`SHF_COMPRESSED`): not
    /// judged (§5.8).
    Compressed,
    /// The record is in `.eh_frame` whose records the map does not place
    /// one by one (GNU ld, mold): stage 2 checks `.eh_frame` itself.
    EhFrame,
    /// A type the stage-1 table does not have.
    Type,
    /// The machine has no table: i386 (§4.5).
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
    /// Stage 3: a dynamic construct the loader model does not judge
    /// (named in the record, `spec/plumb.md` §12).
    Dynamic,
    /// A mention of a `--wrap` word in a shape `spec/resolve.md` §7.1
    /// refuses by name, in a reference's output (§4.4a, WP-170).
    Wrap,
    /// A reference to a `--defsym` symbol whose expression is a form
    /// `spec/resolve.md` §7.2 refuses, in a reference's output (§4.4b,
    /// WP-170 part 2).
    Defsym,
    /// Stage R: the clause needs an option of the line and there is no
    /// line (§14.1, §14.6).
    OptionsUnknown,
}

impl Reason {
    pub const fn id(self) -> &'static str {
        match self {
            Reason::Tls => "tls",
            Reason::TlsSequence => "tls-sequence",
            Reason::Ifunc => "ifunc",
            Reason::NonAlloc => "non-alloc",
            Reason::Compressed => "compressed",
            Reason::EhFrame => "eh-frame",
            Reason::Type => "type",
            Reason::Machine => "machine",
            Reason::SectionNotLocatable => "section-not-locatable",
            Reason::PieceNotLocatable => "piece-not-locatable",
            Reason::InMergeable => "in-mergeable",
            Reason::SymbolUnresolved => "symbol-unresolved",
            Reason::GotUnpaired => "got-unpaired",
            Reason::InputUnreadable => "input-unreadable",
            Reason::Dynamic => "dynamic",
            Reason::Wrap => "wrap-shape",
            Reason::Defsym => "defsym-shape",
            Reason::OptionsUnknown => "options-unknown",
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
    /// A pointer field of a CIE or FDE, checked with its record (§6.4).
    EhFrame,
    /// To an IFUNC: an IPLT entry or IRELATIVE slot of its resolver (§4.8).
    Iplt,
    /// A non-alloc reference to a section not in the output: the
    /// linker's tombstone (§5.8).
    Tombstone,
    /// A non-alloc reference to a dropped COMDAT copy, filled against the
    /// kept copy (§5.8: `spec/image.md` I-47a, GNU ld's kept section).
    KeptCopy,
    /// Stage 3: a branch to a PLT entry whose `.got.plt` word holds S
    /// after the loader's work (§12, P-D9).
    Plt,
    /// Stage R: matched by place and target (P-P3, §14.6).
    Place,
    /// Stage R: matched by content into a merged section (§14.4).
    String,
}

impl Via {
    pub const fn id(self) -> &'static str {
        match self {
            Via::Direct => "direct",
            Via::GotSlot => "got-slot",
            Via::Piece => "piece",
            Via::EhFrame => "eh-frame",
            Via::Iplt => "iplt",
            Via::Tombstone => "tombstone",
            Via::KeptCopy => "kept-copy",
            Via::Plt => "plt",
            Via::Place => "place",
            Via::String => "string",
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

/// The start of a P-D1 finding that names why an output is outside
/// stage 3 (`spec/plumb.md` §12.1): a link the verifier does not model
/// (an option `spec/dyn.md` refuses, version definitions, a
/// `DT_NEEDED` object no search finds). It is a finding, never a pass;
/// a harness may count a reference linker's output with only such
/// findings as not judged ([`Finding::outside`]).
pub const OUTSIDE: &str = "outside stage 3: ";

impl Finding {
    /// Whether this is a P-D1 "outside stage 3" finding ([`OUTSIDE`]).
    pub fn outside(&self) -> bool {
        self.rule == Rule::Dyn(1) && self.message.starts_with(OUTSIDE)
    }

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
    /// Live input FDEs found in the output's `.eh_frame`; input FDEs
    /// dropped with their function; output FDEs no live input gives that
    /// are neutralised (§6.4).
    pub fdes_checked: u64,
    pub fdes_dropped: u64,
    pub fdes_neutralised: u64,
    /// Output FDEs a reference linker made for its own PLT (stage 3).
    pub fdes_linker_plt: u64,
    /// PLT instructions E6 judged those FDEs at (x86-64).
    pub plt_fde_insns: u64,
    /// Search table entries of `.eh_frame_hdr` checked (§6.4).
    pub eh_frame_hdr_entries: u64,
    /// Output symbols Y3 checked against their global definitions, and
    /// those it could not (an IFUNC, an unplaced definition).
    pub globals_checked: u64,
    pub globals_unchecked: u64,
    /// Output locals Y4 checked (one input local has the name), and the
    /// others an input local names.
    pub locals_checked: u64,
    pub locals_unchecked: u64,
    /// Linker-defined symbols Y5 checked, and the names it does not judge.
    pub linker_defined_checked: u64,
    /// IRELATIVE records checked by F1 and F2, and IPLT entries found (§4.8).
    pub irelative_checked: u64,
    pub iplt_entries: u64,
    /// Property types P1 judged, and those it does not judge (§6.6).
    pub properties_judged: u64,
    pub properties_not_judged: u64,
    /// What B1 made of the build id, when the output has one (§6.7).
    pub build_id: Option<String>,
    /// What §6.10 made of `.gdb_index`, when the output has one.
    pub gdb_index: Option<String>,
    pub linker_defined_not_judged: Vec<String>,
    /// Output sections whose placement no rule judges, by name: x86-64
    /// large sections (`SHF_X86_64_LARGE`), whose order differs by linker
    /// (spec/x64.md X-54; Y5 leaves them out of `_edata`/`_end`).
    pub placement_not_judged: Vec<String>,
    /// What a stripped output (no `.symtab`) leaves a rule unable to
    /// judge, by name (WP-164: P-D6, P-D22's `DT_INIT`/`DT_FINI`).
    pub stripped_not_judged: Vec<String>,
    /// The `--wrap` shapes `spec/resolve.md` §7.1 refuses by name whose
    /// redirection the verifier does not derive (§4.4a, WP-170).
    pub wrap_not_judged: Vec<String>,
    /// The `--defsym` forms `spec/resolve.md` §7.2 refuses whose value
    /// the verifier does not derive (§4.4b, WP-170 part 2).
    pub defsym_not_judged: Vec<String>,
    /// Erratum 843419 sites found with their fix (C1), and sites not
    /// judged: the option unknown, beyond what both references fix, or
    /// unproved code in a reference's output (§5.10).
    pub erratum_sites: u64,
    pub erratum_not_judged: u64,
    /// The erratum option the sites were judged under (§5.10): `None`
    /// unknown (no fix found and nothing states it).
    pub erratum_option: Option<bool>,
    /// Verified or accepted records by relocation type.
    pub checked_types: BTreeMap<u32, u64>,
    /// Stage 4 (§13).
    pub meta: MetaCoverage,
    /// What the input set made of the link (§3.8): its coverage line.
    pub input_set: Option<String>,
    /// Stage R (§14.6), when the output is relocatable.
    pub relocatable: Option<RelCoverage>,
    /// P-P1 (§14.5, §14.6), on a composition's report.
    pub composition: Option<CompCoverage>,
    /// The decision records' coverage line (§3.2, §7.2), for a map that
    /// has them.
    pub decisions: Option<String>,
    /// The fillers P-E3 passed over (§6.9, `fillers=<n>`), for a side of
    /// `--equiv` whose map says `incremental`.
    pub fillers: Option<u64>,
}

/// What P-P1 compared (§14.6: `composition: {x} judged, {y} not judged
/// ({letters})`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompCoverage {
    /// Compositions judged (0 or 1 per report).
    pub judged: u64,
    /// The exclusions that applied, `(letter, reason)` (P-P1 (a)-(d)).
    pub not_judged: Vec<(char, String)>,
    /// Input sections compared by place (item 2), records by target
    /// (item 3), bytes outside the relocated fields (item 3), globals
    /// (item 4).
    pub pieces: u64,
    pub records: u64,
    pub bytes: u64,
    pub symbols: u64,
    /// Records whose target has no input piece to map (a linker-made
    /// place, a slot, a TLS offset): judged by their form only.
    pub records_by_form: u64,
    /// Records whose target lies in a piece of an `SHF_MERGE` section:
    /// judged by content (item 3, WP-232), not by address.
    pub merged_by_content: u64,
    /// What P-P1 leaves to other rules, by name (`.eh_frame` pieces).
    pub not_compared: BTreeMap<String, u64>,
    /// The tolerances used: (t1)-(t5) and how often.
    pub tolerances: BTreeMap<String, u64>,
}

/// What stage R judged (§14.6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelCoverage {
    /// Input sections placed whole, merged (a foreign map's), framed.
    pub pieces: u64,
    pub merged: u64,
    pub framed: u64,
    /// Output `SHT_GROUP` sections judged.
    pub groups: u64,
    /// Input FDEs live and removed.
    pub fdes_live: u64,
    pub fdes_removed: u64,
    /// Output symbols judged.
    pub symbols: u64,
    /// Output section headers judged.
    pub sections: u64,
}

/// What stage 4 judged (§13.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetaCoverage {
    /// Executables and shared objects stage 4 ran on (0 or 1 per report).
    pub outputs: u64,
    /// Whether the link's options were known (a static map has no `arg`
    /// records, §13.1).
    pub options_known: bool,
    /// Live units that parse, and the malformed or other-major ones.
    pub units: u64,
    pub units_malformed: u64,
    /// `.note.mason` sections, table rows and `=full` rows checked.
    pub tables: u64,
    pub rows: u64,
    pub full_rows: u64,
    /// Names whose keeper the map does not decide (§13.1).
    pub not_judged: u64,
    /// Bytes of the kept `.mason.meta` compared.
    pub kept_bytes: u64,
    /// Outputs with the load-time ODR runtime judged (§13.2 P-M3, WP-197),
    /// its entries placed and judged, and its imports judged.
    pub runtime_outputs: u64,
    pub runtime_entries: u64,
    pub runtime_imports: u64,
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
    #[allow(clippy::too_many_lines)] // one line per coverage item
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
        let _ = writeln!(
            s,
            "FDEs: checked {}, dropped with their function {}, neutralised {}, \
             the linker's PLT FDEs {} (at {} instructions); .eh_frame_hdr entries checked {}",
            c.fdes_checked,
            c.fdes_dropped,
            c.fdes_neutralised,
            c.fdes_linker_plt,
            c.plt_fde_insns,
            c.eh_frame_hdr_entries
        );
        let _ = writeln!(
            s,
            "symbol attributes: globals {} (unchecked {}), locals {} (unchecked {}), linker-defined {}",
            c.globals_checked,
            c.globals_unchecked,
            c.locals_checked,
            c.locals_unchecked,
            c.linker_defined_checked
        );
        let _ = writeln!(
            s,
            "IFUNC: IRELATIVE records {}, IPLT entries {}",
            c.irelative_checked, c.iplt_entries
        );
        let _ = writeln!(
            s,
            "program properties: types judged {}, not judged {}",
            c.properties_judged, c.properties_not_judged
        );
        if c.erratum_sites + c.erratum_not_judged > 0 {
            let why = match c.erratum_option {
                _ if c.erratum_not_judged == 0 => "",
                Some(false) => " (the link was without --fix-cortex-a53-843419)",
                None => " (the option is unknown and no fix was found)",
                Some(true) => "",
            };
            let _ = writeln!(
                s,
                "erratum 843419 sites: fixed {}, not judged {}{why}",
                c.erratum_sites, c.erratum_not_judged
            );
        }
        if c.meta.outputs > 0 && (c.meta.units + c.meta.units_malformed + c.meta.tables > 0) {
            let m = &c.meta;
            let _ = writeln!(
                s,
                "ODR metadata: live units {} (malformed or other major {}), tables {}, rows {}, =full rows {}, names not judged {}, kept bytes {}, options {}",
                m.units,
                m.units_malformed,
                m.tables,
                m.rows,
                m.full_rows,
                m.not_judged,
                m.kept_bytes,
                if m.options_known { "known" } else { "none (a reference's map)" }
            );
            if m.runtime_outputs > 0 {
                let _ = writeln!(
                    s,
                    "ODR runtime: outputs {}, entries {}, imports {}",
                    m.runtime_outputs, m.runtime_entries, m.runtime_imports
                );
            }
        }
        if let Some(b) = &c.build_id {
            let _ = writeln!(s, "build id: {b}");
        }
        if let Some(g) = &c.gdb_index {
            let _ = writeln!(s, "gdb index: {g}");
        }
        if let Some(d) = &c.decisions {
            let _ = writeln!(s, "{d}");
        }
        if let Some(n) = c.fillers {
            let _ = writeln!(s, "symbol table: fillers={n}");
        }
        if !c.linker_defined_not_judged.is_empty() {
            let _ = writeln!(
                s,
                "linker-defined symbols not judged: {} ({})",
                c.linker_defined_not_judged.len(),
                c.linker_defined_not_judged.join(", ")
            );
        }
        if !c.placement_not_judged.is_empty() {
            let _ = writeln!(
                s,
                "placement not judged (large sections, X-54): {}",
                c.placement_not_judged.join(", ")
            );
        }
        if !c.wrap_not_judged.is_empty() {
            let _ = writeln!(s, "--wrap, not judged: {}", c.wrap_not_judged.join("; "));
        }
        if !c.defsym_not_judged.is_empty() {
            let _ = writeln!(s, "--defsym, not judged: {}", c.defsym_not_judged.join("; "));
        }
        if !c.stripped_not_judged.is_empty() {
            let _ = writeln!(
                s,
                "stripped output (no .symtab), not judged: {}",
                c.stripped_not_judged.join("; ")
            );
        }
        if let Some(r) = &c.relocatable {
            let _ = writeln!(
                s,
                "relocatable: pieces {}, merged {}, framed {}; groups {}; FDEs {} live, {} removed; symbols {}; sections {}",
                r.pieces,
                r.merged,
                r.framed,
                r.groups,
                r.fdes_live,
                r.fdes_removed,
                r.symbols,
                r.sections
            );
        }
        if let Some(p) = &c.composition {
            let letters: Vec<String> = p.not_judged.iter().map(|(l, _)| l.to_string()).collect();
            let _ = writeln!(
                s,
                "composition: {} judged, {} not judged ({})",
                p.judged,
                p.not_judged.len(),
                letters.join(", ")
            );
            let _ = writeln!(
                s,
                "composition compared: pieces {}, records {} ({} by form), bytes {}, globals {}, merged-by-content={}",
                p.pieces, p.records, p.records_by_form, p.bytes, p.symbols, p.merged_by_content
            );
            for (l, why) in &p.not_judged {
                let _ = writeln!(s, "not judged: relocatable composition ({l}): {why}");
            }
            if !p.tolerances.is_empty() || !p.not_compared.is_empty() {
                let t: Vec<String> =
                    p.tolerances.iter().map(|(k, n)| format!("({k}) {n}")).collect();
                let nc: Vec<String> =
                    p.not_compared.iter().map(|(k, n)| format!("{k} {n}")).collect();
                let _ = writeln!(
                    s,
                    "composition tolerances: {}; not compared: {}",
                    t.join(", "),
                    nc.join(", ")
                );
            }
        }
        if let Some(i) = &c.input_set {
            let _ = writeln!(s, "{i}");
        }
        let _ = writeln!(s, "findings: {}", self.findings.len());
        s
    }
}
