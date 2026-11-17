//! The options resolution reads (stage A): the roots of §4.2, `--wrap`
//! (§7.1), and the switches of §3.3, §3.8, §4.6 and §10. Stage B fills
//! this from the `Plan`.

use mortar_core::plan::{OutputKind, Plan, SymbolicLine};
use mortar_core::{FileId, OptId, SymbolId};

use crate::verdict::Unresolved;

/// A key an option names: its row, if the link has one, and its name for
/// diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub key: OptId<SymbolId>,
    pub name: Box<[u8]>,
    /// The option's argument index (the `seq` of its diagnostics, and of a
    /// root's pulling reference, §4.1).
    pub arg: u32,
}

/// The entry symbol (§4.2, §5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// None of `-e`, `ENTRY`, `-shared`, `-r`: `_start`, a root (R9).
    Default(OptId<SymbolId>),
    /// `-e SYM` or `ENTRY(SYM)`.
    Named(Named),
    /// No entry (shared output or `-r`; both refused before M4).
    None,
}

/// One `--wrap=SYM` (§7.1): the rows of `SYM`, `__wrap_SYM` and
/// `__real_SYM`. The view must have rows for all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wrap {
    pub sym: SymbolId,
    pub wrap: SymbolId,
    pub real: SymbolId,
}

/// One `--defsym=SYM=EXPR` (§7.2), the last of its `SYM`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defsym {
    pub sym: SymbolId,
    pub value: DefsymValue,
    /// The option's argument index (a root's `arg`, a refusal's place).
    pub arg: u32,
    /// The option's value, `SYM=EXPR`, as given.
    pub text: Box<[u8]>,
}

/// What a `--defsym` defines its symbol as (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefsymValue {
    /// An absolute number.
    Number(u64),
    /// An alias of the key.
    Alias(SymbolId),
    /// The key's address plus the addend, in the key's section.
    Offset(SymbolId, i64),
}

impl DefsymValue {
    /// The key the expression names, if any.
    #[must_use]
    pub fn base(self) -> Option<SymbolId> {
        match self {
            Self::Number(_) => None,
            Self::Alias(b) | Self::Offset(b, _) => Some(b),
        }
    }
}

/// The output's kind, as far as §8.1 depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Output {
    /// A position-independent executable (`-pie`, `-static-pie`).
    pub pie: bool,
    /// A dynamic link (M2). M1 links are static.
    pub dynamic: bool,
}

/// The argument index of the first script `EXTERN(SYM)` root (§4.2): a
/// script directive has no command-line argument, so its roots are
/// numbered from here, after any argument index a line can have.
pub const EXTERN_ARG: u32 = 1 << 31;

/// Everything resolution reads besides the view.
#[allow(clippy::struct_excessive_bools, reason = "one field per switch of the command line")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub entry: Entry,
    /// `-u SYM` (§4.2), then each script `EXTERN(SYM)` (§4.2; its `arg`
    /// is [`EXTERN_ARG`] plus the directive's ordinal).
    pub undefined: Vec<Named>,
    /// `--require-defined=SYM` (§4.2).
    pub require_defined: Vec<Named>,
    /// `--wrap` (§7.1).
    pub wrap: Vec<Wrap>,
    /// `--defsym` (§7.2): one per symbol, the last option of each, in
    /// command-line order of those.
    pub defsym: Vec<Defsym>,
    /// `--allow-multiple-definition` / `-z muldefs` (§3.8).
    pub allow_multiple_definition: bool,
    /// `--warn-common` (§3.3).
    pub warn_common: bool,
    /// `--warn-backrefs` (§4.6): run the simulation of GNU ld's walk.
    pub warn_backrefs: bool,
    /// `-S` / `-s`: debug sections are `Dropped(Stripped)` (§10).
    pub strip_debug: bool,
    /// `--gdb-index` without `-S`/`-s`: the kept `.debug_gnu_pubnames` and
    /// `.debug_gnu_pubtypes` are `Dropped(Consumed)` (§10; `spec/synth.md`
    /// S-G6).
    pub consume_gnu_pubnames: bool,
    pub output: Output,
    /// `--unresolved-symbols`, `--warn-unresolved-symbols` (§5.2).
    pub unresolved: Unresolved,
    /// `--ignore-unresolved-symbol` keys, sorted (§5.2).
    pub ignore_unresolved: Vec<SymbolId>,
    /// What a link with shared objects reads besides the view
    /// (`spec/dyn.md` D-13, D-19a, D-21). Empty in a static link.
    pub shared: SharedLink,
    /// The output is a shared object (`spec/dyn.md` D-110 to D-118):
    /// what its export set, preemption and undefined references read.
    /// `None` for every other output kind.
    pub shared_output: Option<SharedOutput>,
    /// The version script and the options rule 12 and rule 9 read
    /// (`spec/resolve.md` §9.7; WP-125): applied in a `Shared` output.
    pub versions: crate::version::VersionLine,
    /// `--exclude-libs` (`spec/dyn.md` D-110 item 1; WP-128): one flag per
    /// file, set for a member of an archive the option names (the input
    /// walk's `InputFlags::EXCLUDE_LIBS`). Empty when no file is excluded.
    pub exclude_libs: Vec<bool>,
    /// The line's `--dynamic-list` files as one list, compiled
    /// (`spec/dyn.md` D-112, `spec/resolve.md` §9.6; WP-128): in a
    /// `Shared` output it decides preemption alone; in an executable each
    /// definition it lists is exported (as `--export-dynamic-symbol`).
    pub dynamic_list: Option<std::sync::Arc<crate::version::DynamicList>>,
}

/// What a `Shared` output's resolution reads (`spec/dyn.md` D-110, D-112,
/// D-118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SharedOutput {
    /// The line's `-Bsymbolic` form and whether it has a `--dynamic-list`
    /// (D-112; `Plan::symbolic`).
    pub symbolic: SymbolicLine,
    /// `-z defs`/`--no-undefined` (D-118): a strong undefined reference
    /// is `undefined-symbol`, as in an executable.
    pub defs: bool,
}

impl SharedOutput {
    /// The plan's, when its output kind is `Shared` and its `-Bsymbolic`
    /// form is one D-112 defines (`Plan::symbolic` is `None` for the two
    /// `NonWeak` forms, which the command line refuses by name: such a
    /// plan never reaches resolution, and is read here as `-Bno-symbolic`).
    #[must_use]
    pub fn of(plan: &Plan) -> Option<Self> {
        (plan.kind == OutputKind::Shared).then(|| Self {
            symbolic: plan.symbolic().unwrap_or_default(),
            defs: plan.z.defs == Some(true),
        })
    }

    /// The `--unresolved-symbols` mode of a link (resolve §5.2) given the
    /// line's own mode `mode`: in a `Shared` output without `-z defs` a
    /// strong undefined reference is left to the loader (D-118: D-111
    /// item 2's undefined `.dynsym` entry), so the mode is `Ignore` (the
    /// verdict `Ignored`, the target kind `Shlib`, `spec/dyn.md` D-113); a
    /// hidden one stays an error (resolve §5.2: it cannot be satisfied at
    /// run time). Otherwise `mode`.
    #[must_use]
    pub fn unresolved(this: Option<Self>, mode: Unresolved) -> Unresolved {
        match this {
            Some(s) if !s.defs => Unresolved::Ignore,
            _ => mode,
        }
    }
}

/// The group a shared-object mention is in, for `spec/dyn.md` D-19a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MentionGroup {
    /// In no `--start-group` or `GROUP`.
    #[default]
    None,
    /// In the group of that ordinal (`FileInfo::group`'s numbering).
    In(u32),
    /// The input tables do not record it (a second mention of one soname
    /// inside a linker script).
    Unknown,
}

/// One mention of a shared object of the command line or a script
/// (`spec/dyn.md` D-19a): every mention of one soname names its loaded
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedMention {
    pub file: FileId,
    /// Its position: the order of the mentions with scripts expanded
    /// inline (`spec/input.md` §10.4 `opened`).
    pub position: u32,
    /// Its own as-needed state; `None` when the input tables do not record
    /// it (a second mention inside a linker script).
    pub as_needed: Option<bool>,
    pub group: MentionGroup,
}

/// Whether and how D-13 judges the shared objects' references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShlibUndefined {
    /// `--allow-shlib-undefined`, `--unresolved-symbols=ignore-all` or
    /// `=ignore-in-shared-libs`, or an output that is not an executable.
    #[default]
    Allow,
    /// The default of `Exec` and `Pie`: the error.
    Error,
    /// Under `--warn-unresolved-symbols`: a warning.
    Warn,
}

/// The facts of a dynamic link that are not per file (`spec/dyn.md`
/// D-13, D-19a, D-21).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedLink {
    /// The mentions of shared objects, by position.
    pub mentions: Vec<SharedMention>,
    /// Per file: the position of the mention that named a relocatable
    /// object (for an archive member, its archive's mention); `u32::MAX`
    /// for other files. Positions are [`SharedMention::position`]'s.
    pub position: Vec<u32>,
    pub shlib_undefined: ShlibUndefined,
    /// `--export-dynamic`.
    pub export_dynamic: bool,
    /// `--export-dynamic-symbol` keys, sorted.
    pub export_symbols: Vec<SymbolId>,
}

impl Default for Options {
    /// A static executable with the default entry and no other option.
    fn default() -> Self {
        Self {
            entry: Entry::Default(OptId::NONE),
            undefined: Vec::new(),
            require_defined: Vec::new(),
            wrap: Vec::new(),
            defsym: Vec::new(),
            allow_multiple_definition: false,
            warn_common: false,
            warn_backrefs: false,
            strip_debug: false,
            consume_gnu_pubnames: false,
            output: Output::default(),
            unresolved: Unresolved::Error,
            ignore_unresolved: Vec::new(),
            shared: SharedLink::default(),
            shared_output: None,
            versions: crate::version::VersionLine::default(),
            exclude_libs: Vec::new(),
            dynamic_list: None,
        }
    }
}

/// A root of §4.2: a key named by an option, a strong reference from file
/// 0 with the option's argument index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Root {
    pub key: SymbolId,
    pub arg: u32,
}

impl Options {
    /// The roots of §4.2 that have a key row, in option order.
    #[must_use]
    pub fn roots(&self) -> Vec<Root> {
        let mut roots = Vec::new();
        match &self.entry {
            Entry::Default(key) => {
                if let Some(key) = key.get() {
                    roots.push(Root { key, arg: 0 });
                }
            }
            Entry::Named(named) => {
                if let Some(key) = named.key.get() {
                    roots.push(Root { key, arg: named.arg });
                }
            }
            Entry::None => {}
        }
        for named in self.undefined.iter().chain(&self.require_defined) {
            if let Some(key) = named.key.get() {
                roots.push(Root { key, arg: named.arg });
            }
        }
        // §7.2: the symbol a `--defsym` names is a root wherever the
        // option stands (lld, mold; GNU ld only when it precedes the
        // archive), and kept by `--gc-sections` (lld, mold).
        for d in &self.defsym {
            if let Some(key) = d.value.base() {
                roots.push(Root { key, arg: d.arg });
            }
        }
        roots
    }

    /// The `redirect` column (§7.1): the row an **undefined** mention of
    /// each key binds to. Several `--wrap` of one symbol are one.
    #[must_use]
    pub fn redirect(&self, keys: u32) -> Vec<SymbolId> {
        let mut redirect: Vec<SymbolId> = (0..keys).map(SymbolId).collect();
        for wrap in &self.wrap {
            let ok = [wrap.sym, wrap.wrap, wrap.real].iter().all(|k| k.0 < keys);
            if ok {
                redirect[wrap.sym.0 as usize] = wrap.wrap;
                redirect[wrap.real.0 as usize] = wrap.sym;
            }
        }
        redirect
    }
}

/// Whether `pattern` has a wildcard of [`glob_match`] (`*`, `?` or `[`).
#[must_use]
pub fn is_glob(pattern: &[u8]) -> bool {
    pattern.iter().any(|b| matches!(b, b'*' | b'?' | b'['))
}

/// Whether `name` matches the glob `pattern` (`spec/resolve.md` §4.2, the
/// patterns of `--export-dynamic-symbol`): `*` any run of bytes, `?` one
/// byte, `[set]` one byte of the set (ranges `a-z`; `!` or `^` first
/// negates; `]` first is literal), `\` makes the next byte literal; a `[`
/// without its `]` is a literal `[` (fnmatch(3), GNU ld's matcher).
#[must_use]
pub fn glob_match(pattern: &[u8], name: &[u8]) -> bool {
    // Iterative with one backtrack point: the last `*` seen.
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        let step = match pattern.get(p) {
            Some(b'*') => {
                star = Some((p + 1, n));
                p += 1;
                continue;
            }
            Some(b'?') => Some(p + 1),
            Some(b'[') => match class(pattern, p + 1, name[n]) {
                Some((hit, end)) => hit.then_some(end),
                None => (name[n] == b'[').then_some(p + 1),
            },
            Some(b'\\') if p + 1 < pattern.len() => (pattern[p + 1] == name[n]).then_some(p + 2),
            Some(&c) => (c == name[n]).then_some(p + 1),
            None => None,
        };
        match (step, star) {
            (Some(next), _) => {
                p = next;
                n += 1;
            }
            (None, Some((sp, sn))) => {
                p = sp;
                n = sn + 1;
                star = Some((sp, sn + 1));
            }
            (None, None) => return false,
        }
    }
    pattern[p..].iter().all(|&b| b == b'*')
}

/// The bracket class starting after its `[` at `start`: whether `byte` is
/// in it and the index past its `]`; `None` when it has no `]`.
fn class(pattern: &[u8], start: usize, byte: u8) -> Option<(bool, usize)> {
    let mut i = start;
    let negate = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let mut hit = false;
    let mut first = true;
    loop {
        let c = *pattern.get(i)?;
        if c == b']' && !first {
            return Some((hit != negate, i + 1));
        }
        first = false;
        let lo = if c == b'\\' {
            i += 1;
            *pattern.get(i)?
        } else {
            c
        };
        if pattern.get(i + 1) == Some(&b'-') && pattern.get(i + 2).is_some_and(|&h| h != b']') {
            let hi = pattern[i + 2];
            hit |= (lo..=hi).contains(&byte);
            i += 3;
        } else {
            hit |= lo == byte;
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn globs() {
        let yes = |p: &str, n: &str| assert!(glob_match(p.as_bytes(), n.as_bytes()), "{p} {n}");
        let no = |p: &str, n: &str| assert!(!glob_match(p.as_bytes(), n.as_bytes()), "{p} {n}");
        yes("exp_d*", "exp_data");
        yes("exp_d*", "exp_d");
        no("exp_d*", "exp_fn");
        yes("*", "");
        yes("a*b*c", "aXXbYYc");
        no("a*b*c", "aXXbYY");
        yes("f?o", "foo");
        no("f?o", "fo");
        yes("[a-c]x", "bx");
        no("[a-c]x", "dx");
        yes("[!a-c]x", "dx");
        yes("[^a]x", "bx");
        yes("[]]", "]");
        yes("a[", "a[");
        yes("\\*", "*");
        no("\\*", "a");
        yes("_ZN*Foo*", "_ZN3FooC1Ev");
        no("abc", "abcd");
    }
}
