//! O-47's register as code (`spec/odr.md` §9.2): which checks are on by
//! default. One row per row of the spec's table, in its order, tested
//! against it (`tests` below), so that moving a check (O-46) is one line
//! here and one row there.

use mortar_core::plan::{Odr, OdrMode, OdrRuntime, OdrTable};

/// A check's status in O-47's register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// On by default (`**on**` in the table): runs in `warn`, `error` and
    /// `strict`.
    On,
    /// Runs only under `--odr=strict`.
    Strict,
    /// Runs only with its own switch (`odr-archive-shadow`,
    /// `--odr-check-archives`).
    OptIn,
    /// Off unless its own switch asks (the `.note.mason` table,
    /// `--odr-table`); then on in every mode but `--odr=off`.
    Off,
    /// Counted by `--odr-stats` only, never reported.
    StatsOnly,
    /// Never part of a link (the load-time hook).
    Never,
}

/// A row of the register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The check, as the first column of the spec's table writes it.
    pub check: &'static str,
    /// Its status.
    pub status: Status,
}

/// The checks this crate runs, by their row in [`REGISTER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// W1 `odr-size-mismatch`.
    W1,
    /// W2 `odr-symbol-type-mismatch`.
    W2,
    /// `meta-malformed`, `meta-version` of a container's contents (WP-071,
    /// WP-073): reported under strict only, the one mode that reads
    /// containers (O-42). The MT-6 warning for a section of the name that
    /// is not a container is resolution's (`mortar_resolve`), not a row
    /// here.
    MetaHeaders,
    /// W3 `odr-strong-and-comdat`.
    W3,
    /// W4 `odr-contents-mismatch`.
    W4,
    /// Kind 5, C++ linkage (O-35).
    Kind5Cxx,
    /// Kind 5, C linkage (O-33).
    Kind5C,
    /// Kind 3 layouts (O-17, O-18).
    Kind3Layout,
    /// Kind 3 definition-only differences (O-17).
    Kind3Definition,
    /// Kind 3 size, alignment and shape flags across schemes (O-11's
    /// strict exception).
    Kind3Shape,
    /// Kind 2 (O-15).
    Kind2,
    /// `odr-internal-reference` (O-16).
    InternalReference,
    /// `odr-abi-mismatch` (O-30), `odr-isa-mismatch` (O-31).
    AbiIsa,
    /// `odr-vtable-rtti-mismatch` (O-25).
    VtableRtti,
    /// O-32's `SYSTEM` `-std` differences.
    SystemStd,
    /// O-24, the check against shared-object inputs (WP-075).
    SharedInputs,
    /// O-50's `.note.mason` table (`--odr-table`).
    Table,
    /// O-23, `odr-archive-shadow` (`--odr-check-archives`; WP-076).
    ArchiveShadow,
}

impl Check {
    /// The check's row in [`REGISTER`].
    #[must_use]
    pub const fn row(self) -> &'static Row {
        &REGISTER[match self {
            Self::W1 => 0,
            Self::W2 => 1,
            Self::MetaHeaders => 2,
            Self::W3 => 3,
            Self::W4 => 4,
            Self::Kind5Cxx => 5,
            Self::Kind5C => 6,
            Self::Kind3Layout => 7,
            Self::Kind3Definition => 8,
            Self::Kind3Shape => 9,
            Self::Kind2 => 10,
            Self::InternalReference => 11,
            Self::AbiIsa => 12,
            Self::VtableRtti => 13,
            Self::SharedInputs => 14,
            Self::Table => 15,
            Self::SystemStd => 17,
            Self::ArchiveShadow => 18,
        }]
    }

    /// Whether the check runs in a link with these switches.
    #[must_use]
    pub const fn runs(self, odr: &Odr) -> bool {
        self.row().status.runs(odr)
    }
}

impl Status {
    /// Whether a check of this status runs in a link with these switches
    /// (O-41). `--odr=off` runs nothing; an opt-in check runs with
    /// `--odr-check-archives` (its only one today) in any other mode.
    #[must_use]
    pub const fn runs(self, odr: &Odr) -> bool {
        match (self, odr.mode) {
            // `--odr-runtime` (WP-198): the runtime reads the table, which
            // is written whatever `--odr` says (the two are independent).
            (Self::Off, _) if !matches!(odr.runtime, OdrRuntime::Off) => {
                !matches!(odr.table, OdrTable::Off)
            }
            (_, OdrMode::Off) => false,
            (Self::On, _) | (Self::Strict, OdrMode::Strict) => true,
            (Self::OptIn, _) => odr.check_archives,
            (Self::Off, _) => !matches!(odr.table, OdrTable::Off),
            _ => false,
        }
    }
}

/// O-47's table, in the spec's order.
pub const REGISTER: [Row; 20] = [
    Row { check: "W1 `odr-size-mismatch`", status: Status::On },
    Row { check: "W2 `odr-symbol-type-mismatch`", status: Status::On },
    Row {
        check: "`meta-malformed`, `meta-version` (a container's contents, MT-14, MT-35)",
        status: Status::Strict,
    },
    Row { check: "W3 `odr-strong-and-comdat`", status: Status::Strict },
    Row { check: "W4", status: Status::Strict },
    Row { check: "kind 5 C++ linkage `odr-declaration-mismatch`", status: Status::Strict },
    Row { check: "kind 5 C linkage", status: Status::Strict },
    Row { check: "kind 3 `odr-type-mismatch` (layout)", status: Status::Strict },
    Row { check: "kind 3 definition-only differences (X22)", status: Status::Strict },
    Row { check: "kind 3 size and alignment across schemes (O-11)", status: Status::Strict },
    Row { check: "kind 2 `odr-violation` (hash, K13 flags)", status: Status::Strict },
    Row { check: "`odr-internal-reference`", status: Status::Strict },
    Row {
        check: "`odr-abi-mismatch` (`ENFORCE` keys), `odr-isa-mismatch`",
        status: Status::Strict,
    },
    Row { check: "`odr-vtable-rtti-mismatch`", status: Status::Strict },
    Row { check: "O-24 (shared-object inputs)", status: Status::Strict },
    Row { check: "`.note.mason` table", status: Status::Off },
    Row { check: "O-34 kind 5 vs foreign sizes", status: Status::Strict },
    Row { check: "`SYSTEM` `-std` differences (O-32)", status: Status::Strict },
    Row { check: "`odr-archive-shadow`", status: Status::OptIn },
    Row { check: "load-time hook", status: Status::Never },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows of O-47's table: (check, status cell).
    fn spec_rows() -> Vec<(String, String)> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/odr.md");
        let text = std::fs::read_to_string(path).expect("spec/odr.md");
        let start = text.find("**O-47 (the register).**").expect("O-47 in spec/odr.md");
        text[start..]
            .lines()
            .skip_while(|l| !l.starts_with("| check |"))
            .skip(2)
            .take_while(|l| l.starts_with('|'))
            .map(|l| {
                let cells: Vec<&str> = l.split(" | ").collect();
                (cells[0].trim_start_matches("| ").to_owned(), cells[2].to_owned())
            })
            .collect()
    }

    fn status_of(cell: &str) -> Status {
        match cell {
            c if c.starts_with("**on**") => Status::On,
            c if c.starts_with("strict") => Status::Strict,
            c if c.starts_with("opt-in") => Status::OptIn,
            c if c.starts_with("off") => Status::Off,
            c if c.starts_with("`--odr-stats`") => Status::StatsOnly,
            c if c.starts_with("**none**") => Status::Never,
            c => panic!("O-47: a status this register cannot say: {c}"),
        }
    }

    // spec: odr.md O-47 (the register is the spec's table, row for row)
    #[test]
    fn the_register_is_the_spec_table() {
        let rows = spec_rows();
        assert_eq!(rows.len(), REGISTER.len(), "O-47 has {} rows", rows.len());
        for ((check, cell), row) in rows.iter().zip(REGISTER) {
            assert_eq!(check, row.check);
            assert_eq!(status_of(cell), row.status, "{check}");
        }
    }

    // spec: odr.md O-40, O-41 (which mode runs which check)
    #[test]
    fn modes_run_what_o41_says() {
        let at = |mode| Odr { mode, ..Odr::default() };
        for mode in [OdrMode::Warn, OdrMode::Error, OdrMode::Strict] {
            assert!(Check::W1.runs(&at(mode)) && Check::W2.runs(&at(mode)));
            assert_eq!(Check::W3.runs(&at(mode)), mode == OdrMode::Strict);
            assert_eq!(Check::W4.runs(&at(mode)), mode == OdrMode::Strict);
        }
        for check in [Check::W1, Check::W2, Check::W3, Check::W4, Check::MetaHeaders] {
            assert!(!check.runs(&at(OdrMode::Off)));
        }
        for mode in [OdrMode::Warn, OdrMode::Error, OdrMode::Strict] {
            assert_eq!(Check::MetaHeaders.runs(&at(mode)), mode == OdrMode::Strict);
        }
        // The record checks: strict only today (O-47); O-32 is reported
        // under strict and only counted otherwise.
        let records = [
            Check::Kind5Cxx,
            Check::Kind5C,
            Check::Kind3Layout,
            Check::Kind3Definition,
            Check::Kind3Shape,
            Check::Kind2,
            Check::SystemStd,
            Check::InternalReference,
            Check::AbiIsa,
            Check::VtableRtti,
        ];
        for check in records {
            for mode in [OdrMode::Off, OdrMode::Warn, OdrMode::Error, OdrMode::Strict] {
                assert_eq!(check.runs(&at(mode)), mode == OdrMode::Strict, "{check:?}");
            }
        }
        assert!(!Status::OptIn.runs(&at(OdrMode::Strict)));
        assert!(Status::OptIn.runs(&Odr { check_archives: true, ..at(OdrMode::Warn) }));
        // O-23: its own switch, in any mode but off (WP-076).
        let archives = |mode| Odr { check_archives: true, ..at(mode) };
        for mode in [OdrMode::Off, OdrMode::Warn, OdrMode::Error, OdrMode::Strict] {
            assert!(!Check::ArchiveShadow.runs(&at(mode)));
            assert_eq!(Check::ArchiveShadow.runs(&archives(mode)), mode != OdrMode::Off);
        }
        assert!(!Status::Off.runs(&at(OdrMode::Strict)));
        // The table: its own switch, in any mode but off (O-42).
        let table = |mode| Odr { table: OdrTable::Exported, ..at(mode) };
        assert!(Check::Table.runs(&table(OdrMode::Warn)));
        assert!(Check::Table.runs(&table(OdrMode::Strict)));
        assert!(!Check::Table.runs(&table(OdrMode::Off)));
        assert!(!Check::Table.runs(&at(OdrMode::Strict)));
        // WP-198: with --odr-runtime the table is written under --odr=off
        // too (the runtime reads it); nothing else runs there.
        let rt = |mode| Odr { runtime: OdrRuntime::Report, ..table(mode) };
        for mode in [OdrMode::Off, OdrMode::Warn, OdrMode::Error, OdrMode::Strict] {
            assert!(Check::Table.runs(&rt(mode)));
            assert_eq!(Check::W1.runs(&rt(mode)), mode != OdrMode::Off);
        }
        // O-24: strict only.
        for mode in [OdrMode::Off, OdrMode::Warn, OdrMode::Error, OdrMode::Strict] {
            assert_eq!(Check::SharedInputs.runs(&at(mode)), mode == OdrMode::Strict);
        }
        assert!(!Status::Never.runs(&at(OdrMode::Strict)));
    }
}
