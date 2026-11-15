//! The options resolution reads (stage A): the roots of §4.2, `--wrap`
//! (§7.1), and the switches of §3.3, §3.8, §4.6 and §10. Stage B fills
//! this from the `Plan`.

use mortar_core::{OptId, SymbolId};

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

/// The output's kind, as far as §8.1 depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Output {
    /// A position-independent executable (`-pie`, `-static-pie`).
    pub pie: bool,
    /// A dynamic link (M2). M1 links are static.
    pub dynamic: bool,
}

/// Everything resolution reads besides the view.
#[allow(clippy::struct_excessive_bools, reason = "one field per switch of the command line")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub entry: Entry,
    /// `-u SYM` (§4.2).
    pub undefined: Vec<Named>,
    /// `--require-defined=SYM` (§4.2).
    pub require_defined: Vec<Named>,
    /// `--wrap` (§7.1).
    pub wrap: Vec<Wrap>,
    /// `--allow-multiple-definition` / `-z muldefs` (§3.8).
    pub allow_multiple_definition: bool,
    /// `--warn-common` (§3.3).
    pub warn_common: bool,
    /// `--warn-backrefs` (§4.6): run the simulation of GNU ld's walk.
    pub warn_backrefs: bool,
    /// `-S` / `-s`: debug sections are `Dropped(Stripped)` (§10).
    pub strip_debug: bool,
    pub output: Output,
}

impl Default for Options {
    /// A static executable with the default entry and no other option.
    fn default() -> Self {
        Self {
            entry: Entry::Default(OptId::NONE),
            undefined: Vec::new(),
            require_defined: Vec::new(),
            wrap: Vec::new(),
            allow_multiple_definition: false,
            warn_common: false,
            warn_backrefs: false,
            strip_debug: false,
            output: Output::default(),
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
