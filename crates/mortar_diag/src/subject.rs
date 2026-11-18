//! The subjects of a warning code (`spec/incr.md` IN-75a; `spec/diag.md`'s
//! subjects column, WP-211 §R.7): what a stored warning depends on, so
//! that an update drops and recomputes it when a subject changes. WP-213
//! (U-1): the type [`SubjectKind`] and the entry point [`subjects`];
//! **filled by U-9** (the table, from diag.md as WP-212 writes it).

/// One kind of subject (diag.md's closed set: file, key, output section,
/// link).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubjectKind {
    File,
    /// A key: a symbol (`Subject::Key`) or an ODR key (`Subject::OdrKey`,
    /// odr.md O-4; WP-212 F-12): one kind, since a code's subjects column
    /// names "key" for both.
    Key,
    OutputSection,
    Link,
}

/// The subjects of the warning `code` (a `Kind::id`), as diag.md's table
/// gives them; empty for an error or note, or a code with none.
///
/// Until U-9 lands: empty for every code.
#[must_use]
pub fn subjects(code: &str) -> &'static [SubjectKind] {
    let _ = code;
    &[]
}
