//! `--wrap=SYM` (`spec/resolve.md` §7.1; `spec/plumb.md` §4.4a, WP-170):
//! the one derivation of the redirection every rule that asks "which
//! definition does this reference bind to" reads.
//!
//! For an UNDEFINED mention (raw `st_shndx == SHN_UNDEF`) of an
//! unversioned name: `SYM` is `__wrap_SYM`, `__real_SYM` is `SYM`, one
//! step per mention (a chain `--wrap=f --wrap=__wrap_f` is two one-step
//! maps). A defined mention keeps its name (E13), and so does a
//! versioned reference `f@V`. A line without `--wrap` has an empty set,
//! and [`Wrap::key`] returns the name it is given.

use std::borrow::Cow;
use std::collections::BTreeSet;

/// The tag of a finding at a reference in `SYM`'s defining object whose
/// field binds it to `__wrap_SYM` (E13: a defined mention is not
/// wrapped; lld's form, A80).
pub const E13: &str =
    " (a reference in SYM's defining object bound to __wrap_SYM; resolve.md §7.1 E13: not wrapped)";

/// The tag of a Y3 binding finding on a weak `SYM` that a strong
/// undefined `__real_SYM` binds to (lld writes it `STB_GLOBAL`, A81).
pub const REAL_STRONG: &str =
    " (a --wrap word whose strong __real_ reference binds to its weak definition)";

/// The tag of an M12 finding on a member that defines a `--wrap` word
/// SYM whose only references are wrapped (lld extracts it, A83).
pub const WRAPPED_ONLY: &str =
    " (it defines a --wrap word SYM, and only references bound to __wrap_SYM name SYM)";

/// The tag of a P-D4 finding whose entry point is `__wrap_ENTRY` (lld
/// redirects `-e` under `--wrap=ENTRY`, A82).
pub const ENTRY_WRAPPED: &str = " (it is __wrap_ENTRY: a root is not redirected, resolve.md §7.1)";

/// The tag of a Y3 finding on a defined `__real_SYM` of a `--wrap` word
/// SYM missing from `.symtab` (lld leaves it out, A84).
pub const REAL_DROPPED: &str = " (a definition of __real_SYM under --wrap=SYM)";

/// The line's `--wrap` names, each once.
#[derive(Clone, Debug, Default)]
pub(crate) struct Wrap {
    syms: BTreeSet<String>,
}

/// A `--wrap` line whose redirection this module does not derive: the
/// shapes `spec/resolve.md` §7.1 refuses by name (items 1 and 2), which
/// only a reference's output can have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unjudged {
    /// `--wrap=f@V` (item 1).
    Versioned(String),
    /// `--wrap=__real_X` beside `--wrap=X` (item 2).
    RealPair(String),
}

impl Wrap {
    /// The set of the line's `--wrap` words.
    pub(crate) fn new<I: IntoIterator<Item = String>>(syms: I) -> Wrap {
        Wrap { syms: syms.into_iter().collect() }
    }

    /// The `--wrap` set of the line's options (none without them).
    pub(crate) fn of(opts: Option<&crate::opts::Options>) -> Wrap {
        opts.map_or_else(Wrap::default, |o| Wrap::new(o.wrap.iter().cloned()))
    }

    /// No `--wrap` on the line: every key is its name.
    pub(crate) fn is_empty(&self) -> bool {
        self.syms.is_empty()
    }

    /// Whether `name` is one of the line's `--wrap` words.
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.syms.contains(name)
    }

    /// The line's `--wrap` words.
    pub(crate) fn syms(&self) -> impl Iterator<Item = &str> + '_ {
        self.syms.iter().map(String::as_str)
    }

    /// The key an undefined, unversioned mention of `name` binds to.
    pub(crate) fn key<'n>(&self, name: &'n str) -> Cow<'n, str> {
        if self.syms.is_empty() || name.contains('@') {
            return Cow::Borrowed(name);
        }
        if self.syms.contains(name) {
            return Cow::Owned(format!("__wrap_{name}"));
        }
        if let Some(s) = name.strip_prefix("__real_")
            && self.syms.contains(s)
        {
            return Cow::Borrowed(s);
        }
        Cow::Borrowed(name)
    }

    /// The refused shapes of the line (§7.1 items 1 and 2), whose
    /// redirection is not derived.
    pub(crate) fn unjudged(&self) -> Vec<Unjudged> {
        let mut v = Vec::new();
        for s in &self.syms {
            if s.contains('@') {
                v.push(Unjudged::Versioned(s.clone()));
            } else if let Some(x) = s.strip_prefix("__real_")
                && self.syms.contains(x)
            {
                v.push(Unjudged::RealPair(s.clone()));
            }
        }
        v
    }

    /// The coverage line's words for [`Wrap::unjudged`].
    pub(crate) fn unjudged_text(&self) -> Vec<String> {
        self.unjudged()
            .into_iter()
            .map(|u| match u {
                Unjudged::Versioned(s) => format!(
                    "--wrap={s}: the redirection of a versioned symbol is not derived (resolve.md §7.1, refused item 1)"
                ),
                Unjudged::RealPair(s) => format!(
                    "--wrap={s} beside --wrap={}: the redirection of this shape is not derived (resolve.md §7.1, refused item 2)",
                    s.trim_start_matches("__real_")
                ),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &[&str]) -> Wrap {
        Wrap::new(s.iter().map(|x| (*x).to_string()))
    }

    #[test]
    fn one_step_per_mention() {
        let x = w(&["f", "__wrap_f"]);
        assert_eq!(x.key("f"), "__wrap_f");
        assert_eq!(x.key("__wrap_f"), "__wrap___wrap_f");
        assert_eq!(x.key("__real_f"), "f");
        assert_eq!(x.key("__real___wrap_f"), "__wrap_f");
        assert_eq!(x.key("g"), "g");
        assert_eq!(x.key("f@V1"), "f@V1");
        assert!(x.unjudged().is_empty());
    }

    #[test]
    fn empty_is_identity() {
        let x = Wrap::default();
        assert!(x.is_empty());
        assert_eq!(x.key("__real_f"), "__real_f");
    }

    #[test]
    fn refused_shapes_are_named() {
        let x = w(&["f@V1", "g", "__real_g"]);
        assert_eq!(
            x.unjudged(),
            vec![Unjudged::RealPair("__real_g".into()), Unjudged::Versioned("f@V1".into())]
        );
    }
}
