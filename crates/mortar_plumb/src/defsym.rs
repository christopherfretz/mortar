//! `--defsym=SYM=EXPR` (`spec/resolve.md` §7.2; `spec/plumb.md` §4.4b,
//! WP-170 part 2): the one derivation of the line's assignments that
//! every rule asking "what is `SYM`" reads.
//!
//! The forms §7.2 accepts are derived: a number (`0x...` hexadecimal, or
//! decimal without a leading `0`), `SHN_ABS`, never relocated; `B`, an
//! alias with `B`'s address and type, size 0; `B + N` and `B - N`, `B`'s
//! address plus `N` in `B`'s section, `STT_NOTYPE`, size 0. Of several
//! `--defsym` of one `SYM` the last is the one. `def[SYM]` is the
//! option's, whatever an input defines (`SYM` keeps its own key: no
//! redirection). Every other form (the ones §7.2 refuses: octal,
//! arithmetic, suffixes, `ALIGN`, two symbols, a chain) only a
//! reference's output can have: such a `SYM` is not judged, by name. A
//! line without `--defsym` has an empty set and no rule pays for it.

use std::collections::BTreeMap;

/// The tag of a Y3 finding on a `B ± N` symbol written `SHN_ABS` or
/// `STT_FUNC` (GNU ld's form in a non-PIE link, resolve.md §16).
pub const OFFSET_ABS: &str = " (a --defsym B+N or B-N written as GNU ld writes it: SHN_ABS, B's type; resolve.md §7.2: B's section, STT_NOTYPE)";

/// The tag of a Y3 finding on an alias whose size is that of the input
/// definition `--defsym` replaced (GNU ld's form, resolve.md §16).
pub const REPLACED_SIZE: &str =
    " (the size of the definition --defsym replaced; resolve.md §7.2: size 0)";

/// The tag of a Y3 type finding on a `SYM` the line assigns more than
/// once (GNU ld keeps the type an earlier assignment gave it).
pub const EARLIER_TYPE: &str = " (SYM has several --defsym: resolve.md §7.2, the last one's type)";

/// The tag of a finding on a word that holds a `--defsym` number moved
/// by the load bias (GNU ld relocates it in a PIE or static-pie,
/// resolve.md §16).
pub const NUMBER_MOVED: &str = " (a --defsym number: SHN_ABS, never relocated; resolve.md §7.2)";

/// What `EXPR` is (§7.2's table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Expr {
    /// An absolute value (`SHN_ABS`, never relocated).
    Number(u64),
    /// `B`: `B`'s address and type, size 0.
    Alias(String),
    /// `B + N` (`N` negative for `B - N`): `B`'s section, `STT_NOTYPE`,
    /// size 0.
    Offset(String, i64),
}

impl Expr {
    /// The symbol `EXPR` names (a root, `spec/resolve.md` §4.2).
    pub(crate) fn base(&self) -> Option<&str> {
        match self {
            Expr::Number(_) => None,
            Expr::Alias(b) | Expr::Offset(b, _) => Some(b),
        }
    }
}

/// `def[SYM]`'s value as the engine resolves it (§4.4b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    /// An address (a number's value; `B`'s address, plus `N`).
    Addr(u64),
    /// An address in the TLS template (a TLS `B`).
    Tls(u64),
    /// `B` is an IFUNC: its resolver's address.
    Ifunc(u64),
    /// `B` resolves to nothing the rules can name (unresolved, discarded,
    /// a mergeable piece).
    Unknown,
}

/// One operand `SYM=EXPR` as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Operand {
    /// A form of §7.2's table.
    Judged { sym: String, expr: Expr },
    /// Any other expression: the symbols it names (roots), whether the
    /// names were read exactly (names, numbers, `+ - * / ( )` only), and
    /// why its value is not derived.
    Other { sym: String, names: Vec<String>, exact: bool, why: &'static str },
    /// No `SYM=`: nothing is read.
    Unread,
}

/// A symbol name of §7.2: letters, digits, `_`, `.` and `$`, not starting
/// with a digit, not `.` alone.
fn symbol(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && !s.as_bytes()[0].is_ascii_digit()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'$'))
}

/// A number of §7.2: `0x`/`0X` hexadecimal, or decimal without a leading
/// `0` (`0` itself included), at most 64 bits. `Err` with the reason for
/// a number the three references read differently (octal).
fn number(s: &str) -> Option<Result<u64, &'static str>> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return (!h.is_empty() && h.bytes().all(|c| c.is_ascii_hexdigit()))
            .then(|| u64::from_str_radix(h, 16).map_err(|_| "a number wider than 64 bits"));
    }
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return Some(Err(
            "a number with a leading 0 (GNU ld reads it as octal, lld and mold as decimal)",
        ));
    }
    Some(s.parse().map_err(|_| "a number wider than 64 bits"))
}

/// The names an expression refers to, and whether it was read exactly
/// (names, numbers, `+ - * / ( )` only; `.` and section names left out).
fn names_of(e: &str) -> (Vec<String>, bool) {
    let mut names = Vec::new();
    let mut exact = true;
    let mut cur = String::new();
    let flush = |cur: &mut String, names: &mut Vec<String>| {
        if !cur.is_empty() && !cur.as_bytes()[0].is_ascii_digit() {
            names.push(std::mem::take(cur));
        }
        cur.clear();
    };
    for c in e.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$') {
            cur.push(c);
        } else if c == '(' && !cur.is_empty() {
            // A builtin function (`ADDR`, `ABSOLUTE`, ...).
            exact = false;
            cur.clear();
        } else {
            flush(&mut cur, &mut names);
            if !matches!(c, ' ' | '+' | '-' | '*' | '/' | '(' | ')') {
                exact = false;
            }
        }
    }
    flush(&mut cur, &mut names);
    if names.iter().any(|n| n.starts_with('.')) {
        // `.` or a section's name.
        exact = false;
        names.retain(|n| !n.starts_with('.'));
    }
    (names, exact)
}

/// Reads one operand `SYM=EXPR` (§7.2; blanks around `=` and the operator
/// allowed).
pub(crate) fn operand(op: &str) -> Operand {
    let Some((a, e)) = op.split_once('=') else { return Operand::Unread };
    let sym = a.trim();
    if sym.is_empty() {
        return Operand::Unread;
    }
    let sym = sym.to_string();
    let e = e.trim();
    let other = |why: &'static str| {
        let (names, exact) = names_of(e);
        Operand::Other { sym: sym.clone(), names, exact, why }
    };
    match number(e) {
        Some(Ok(n)) => return Operand::Judged { sym, expr: Expr::Number(n) },
        Some(Err(why)) => return other(why),
        None => {}
    }
    if symbol(e) {
        return Operand::Judged { sym, expr: Expr::Alias(e.to_string()) };
    }
    let split = e.char_indices().skip(1).find(|&(_, c)| c == '+' || c == '-');
    if let Some((i, c)) = split {
        let (b, n) = (e[..i].trim(), e[i + 1..].trim());
        if symbol(b) {
            match number(n) {
                Some(Ok(n)) => {
                    let Ok(n) = i64::try_from(n) else {
                        return other("an offset wider than 63 bits");
                    };
                    let n = if c == '-' { -n } else { n };
                    return Operand::Judged { sym, expr: Expr::Offset(b.to_string(), n) };
                }
                Some(Err(why)) => return other(why),
                None => {}
            }
        }
    }
    other("an expression resolve.md §7.2 refuses (item 1)")
}

/// The line's `--defsym` assignments, the last of each `SYM`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Defsym {
    syms: BTreeMap<String, Expr>,
    /// `SYM`s whose last operand is a form not derived, with why.
    unjudged: BTreeMap<String, String>,
    /// `SYM`s the line assigns more than once.
    several: std::collections::BTreeSet<String>,
}

impl Defsym {
    /// The assignments of the line's operands, in line order.
    pub(crate) fn new<'s, I: IntoIterator<Item = &'s str>>(ops: I) -> Defsym {
        let mut d = Defsym::default();
        let mut seen = std::collections::BTreeSet::new();
        for op in ops {
            let o = operand(op);
            if let Operand::Judged { sym, .. } | Operand::Other { sym, .. } = &o
                && !seen.insert(sym.clone())
            {
                d.several.insert(sym.clone());
            }
            match o {
                Operand::Judged { sym, expr } => {
                    d.unjudged.remove(&sym);
                    d.syms.insert(sym, expr);
                }
                Operand::Other { sym, why, .. } => {
                    d.syms.remove(&sym);
                    d.unjudged.insert(sym, format!("--defsym={op}: {why}"));
                }
                Operand::Unread => {}
            }
        }
        // A chain (`B` itself a `--defsym`'s `SYM`, item 2): GNU ld and
        // lld follow it, mortar refuses it; not derived.
        let chained: Vec<String> = d
            .syms
            .iter()
            .filter(|(_, e)| {
                e.base().is_some_and(|b| d.syms.contains_key(b) || d.unjudged.contains_key(b))
            })
            .map(|(s, _)| s.clone())
            .collect();
        for s in chained {
            let b =
                d.syms.remove(&s).and_then(|e| e.base().map(str::to_string)).unwrap_or_default();
            d.unjudged.insert(
                s.clone(),
                format!(
                    "--defsym of {s}: {b} is another --defsym's symbol (resolve.md §7.2 item 2)"
                ),
            );
        }
        d
    }

    /// The `--defsym` of the line's options (none without them).
    pub(crate) fn of(opts: Option<&crate::opts::Options>) -> Defsym {
        opts.map_or_else(Defsym::default, |o| Defsym::new(o.defsym.iter().map(String::as_str)))
    }

    /// No `--defsym` on the line.
    pub(crate) fn is_empty(&self) -> bool {
        self.syms.is_empty() && self.unjudged.is_empty()
    }

    /// `def[SYM]` when the line's last `--defsym` of `name` is derived.
    pub(crate) fn get(&self, name: &str) -> Option<&Expr> {
        if self.syms.is_empty() {
            return None;
        }
        self.syms.get(name)
    }

    /// Whether a `--defsym` defines `name` (derived or not).
    pub(crate) fn defines(&self, name: &str) -> bool {
        !self.is_empty() && (self.syms.contains_key(name) || self.unjudged.contains_key(name))
    }

    /// Whether `name`'s value is not derived (a form only a reference
    /// accepts).
    pub(crate) fn unjudged(&self, name: &str) -> bool {
        !self.unjudged.is_empty() && self.unjudged.contains_key(name)
    }

    /// Whether the line assigns `name` more than once (the last wins).
    pub(crate) fn several(&self, name: &str) -> bool {
        self.several.contains(name)
    }

    /// The derived assignments, by `SYM`.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Expr)> + '_ {
        self.syms.iter().map(|(s, e)| (s.as_str(), e))
    }

    /// Every `SYM` the line defines (derived or not).
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.syms.keys().chain(self.unjudged.keys()).map(String::as_str)
    }

    /// The coverage line's words for the forms not derived.
    pub(crate) fn unjudged_text(&self) -> Vec<String> {
        self.unjudged.values().map(|w| format!("{w}: the value is not derived")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(sym: &str, expr: Expr) -> Operand {
        Operand::Judged { sym: sym.into(), expr }
    }

    #[test]
    fn s4_4b_the_forms_of_resolve_7_2() {
        assert_eq!(operand("k=0x1234"), j("k", Expr::Number(0x1234)));
        assert_eq!(operand("k = 4660"), j("k", Expr::Number(4660)));
        assert_eq!(operand("k=0"), j("k", Expr::Number(0)));
        assert_eq!(operand("a=b"), j("a", Expr::Alias("b".into())));
        assert_eq!(operand("c=b+4"), j("c", Expr::Offset("b".into(), 4)));
        assert_eq!(operand("c = b - 0x10"), j("c", Expr::Offset("b".into(), -16)));
        assert_eq!(operand("c=$x.y+1"), j("c", Expr::Offset("$x.y".into(), 1)));
        assert_eq!(operand("=b"), Operand::Unread);
        assert_eq!(operand("b"), Operand::Unread);
    }

    #[test]
    fn s4_4b_other_forms_are_not_derived() {
        for (op, exact) in [
            ("k=011064", true),
            ("k=0x1000+4", true),
            ("k=1<<4", false),
            ("k=1k", true),
            ("k=ALIGN(16)", false),
            ("k=b+b", true),
            ("k=.", false),
        ] {
            let Operand::Other { exact: e, .. } = operand(op) else { panic!("{op}") };
            assert_eq!(e, exact, "{op}");
        }
        let Operand::Other { names, .. } = operand("k=b+b") else { panic!() };
        assert_eq!(names, ["b", "b"]);
    }

    #[test]
    fn s4_4b_the_last_of_a_sym_and_chains() {
        let d = Defsym::new(["k=1", "k=0x10", "a=b", "x=011", "x=2", "y=3", "y=07"]);
        assert_eq!(d.get("k"), Some(&Expr::Number(16)));
        assert_eq!(d.get("x"), Some(&Expr::Number(2)));
        assert!(d.get("y").is_none() && d.unjudged("y") && d.defines("y"));
        let c = Defsym::new(["a=c", "c=b"]);
        assert!(c.unjudged("a") && c.get("c").is_some());
        assert!(Defsym::default().is_empty() && Defsym::default().get("k").is_none());
    }
}
