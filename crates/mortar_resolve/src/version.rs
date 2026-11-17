//! Version scripts applied (spec/resolve.md §9.7, spec/dyn.md D-114; M4,
//! WP-125).
//!
//! [`VersionScript::compile`] turns the parsed script (`mortar_script`,
//! spec/script.md §9) into the matcher: each entry classified (exact,
//! glob, star; §9.7 "Entries"), `extern "C++"` refused by name (rule 6),
//! the duplicate rule checked (`duplicate-version-pattern`), the exact
//! `global:` names listed twice noted (`version-reassigned`).
//! [`VersionScript::decide`] is §9.7's decision for a plain name and
//! [`VersionScript::symver`] rule 12 for a `.symver` definition.
//!
//! [`apply`] runs in the shared stage (`crate::shared`) of a `Shared`
//! output, after D-110's export set: it makes version-local keys
//! unexported (D-110 item 1; so D-112 binds them and D-124 does not root
//! them) and gives every exported key its output version index
//! (`SymProps.version`: the node's `Verdef` index, 2 for the first named
//! node, with `VERSYM_HIDDEN` for a non-default `foo@V`; 0 for an
//! unversioned export, written as 1). A `.symver` definition needs a node
//! of its version, script or not (`undefined-version`).

use std::collections::HashMap;
use std::sync::Arc;

use mortar_core::id::{FileId, SymbolId};
use mortar_diag::{Diagnostic, Kind, RefusalClass};
pub use mortar_script::version::SourceFile;
use mortar_script::{Language, Scope, VersionScript as Ast};

use crate::problem::DiagKey;
use crate::resolve::Def;
use crate::view::View;

/// `VERSYM_HIDDEN`: the bit of a non-default `foo@V` export.
pub const VERSYM_HIDDEN: u16 = 0x8000;

/// What a `Shared` line gives the version stage (`Options::versions`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionLine {
    /// The `--version-script` files as one script (rule 13), compiled.
    pub script: Option<Arc<VersionScript>>,
    /// `-E` (`--export-dynamic`): rule 12 only.
    pub export_dynamic: bool,
    /// `--no-undefined-version` (the last of it and `--undefined-version`).
    pub no_undefined_version: bool,
}

/// A line's `--dynamic-list` files, read as one list in command-line
/// order and compiled (`spec/dyn.md` D-112, `spec/resolve.md` §9.6;
/// WP-128). Several files are one list: a name any of them lists is
/// listed (GNU ld, lld and mold, measured). An entry is matched against a
/// key's name without its version, as a version script's entry
/// ([`Pattern::of`]: quoted exact, glob, `*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicList {
    patterns: Box<[Pattern]>,
}

impl DynamicList {
    /// Compiles a parsed list.
    ///
    /// # Errors
    ///
    /// `extern "C++"` entries: refused by name (class `script-command`),
    /// as in a version script (§9.7 rule 6: no demangler).
    pub fn compile(ast: &mortar_script::DynamicList) -> Result<Self, Vec<Diagnostic>> {
        if ast.entries.iter().any(|e| e.language == Language::Cxx) {
            return Err(vec![Diagnostic::refused(
                RefusalClass::ScriptCommand,
                "extern \"C++\" in a dynamic list (spec/resolve.md §9.7 rule 6: it needs the \
                 demangler; list the mangled names, or use a glob such as _ZN2ns*)",
            )]);
        }
        let patterns =
            ast.entries.iter().map(|e| Pattern::of(&e.pattern.text, e.pattern.quoted)).collect();
        Ok(Self { patterns })
    }

    /// Parses the `--dynamic-list` files of a line, in command-line order,
    /// as one list (`spec/script.md` §9.1), and compiles it.
    ///
    /// # Errors
    ///
    /// The parser's diagnostics, and [`Self::compile`]'s.
    pub fn load(files: &[SourceFile<'_>]) -> Result<Self, Vec<Diagnostic>> {
        let ast = mortar_script::version::parse_dynamic_list(files).map_err(|d| vec![*d])?;
        Self::compile(&ast)
    }

    /// Whether the list names `name` (a key's name without its version).
    #[must_use]
    pub fn matches(&self, name: &[u8]) -> bool {
        self.patterns.iter().any(|p| p.matches(name))
    }
}

/// One glob token: each but `Star` matches one byte.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Star,
    Any,
    Byte(u8),
    /// A class: inclusive byte ranges, negated or not.
    Class(Box<[(u8, u8)]>, bool),
}

/// A compiled glob (`fnmatch(3)` without flags, §9.7 "Entries").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    /// `None`: the glob ends in a lone `\` and matches nothing.
    tokens: Option<Box<[Token]>>,
}

impl Glob {
    /// Compiles the glob text `p` as written.
    #[must_use]
    pub fn new(p: &[u8]) -> Self {
        let mut out = Vec::new();
        let mut i = 0;
        while i < p.len() {
            match p[i] {
                b'*' => {
                    if out.last() != Some(&Token::Star) {
                        out.push(Token::Star);
                    }
                    i += 1;
                }
                b'?' => {
                    out.push(Token::Any);
                    i += 1;
                }
                b'\\' => {
                    if i + 1 == p.len() {
                        return Self { tokens: None };
                    }
                    out.push(Token::Byte(p[i + 1]));
                    i += 2;
                }
                b'[' => {
                    if let Some((tok, next)) = class(p, i) {
                        out.push(tok);
                        i = next;
                    } else {
                        // No closing `]`: the `[` is itself.
                        out.push(Token::Byte(b'['));
                        i += 1;
                    }
                }
                c => {
                    out.push(Token::Byte(c));
                    i += 1;
                }
            }
        }
        Self { tokens: Some(out.into()) }
    }

    /// Whether the whole of `s` matches.
    #[must_use]
    pub fn matches(&self, s: &[u8]) -> bool {
        let Some(t) = &self.tokens else { return false };
        let one = |tok: &Token, c: u8| match tok {
            Token::Star => false,
            Token::Any => true,
            Token::Byte(b) => *b == c,
            Token::Class(items, neg) => items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *neg,
        };
        // Greedy with backtracking to the last star: every other token
        // matches exactly one byte.
        let (mut ti, mut si) = (0usize, 0usize);
        let mut star: Option<(usize, usize)> = None;
        while si < s.len() {
            if ti < t.len() && t[ti] == Token::Star {
                star = Some((ti, si));
                ti += 1;
            } else if ti < t.len() && one(&t[ti], s[si]) {
                ti += 1;
                si += 1;
            } else if let Some((st, ss)) = star {
                ti = st + 1;
                si = ss + 1;
                star = Some((st, ss + 1));
            } else {
                return false;
            }
        }
        t[ti..].iter().all(|x| *x == Token::Star)
    }
}

/// The class opening at `p[at] == '['`: the token and the index after its
/// `]`; `None` when no `]` closes it.
fn class(p: &[u8], at: usize) -> Option<(Token, usize)> {
    let mut j = at + 1;
    let mut neg = false;
    if j < p.len() && (p[j] == b'!' || p[j] == b'^') {
        neg = true;
        j += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    while j < p.len() && (p[j] != b']' || first) {
        first = false;
        let mut a = p[j];
        if a == b'\\' && j + 1 < p.len() {
            a = p[j + 1];
            j += 1;
        }
        j += 1;
        if j + 1 < p.len() && p[j] == b'-' && p[j + 1] != b']' {
            let mut b = p[j + 1];
            if b == b'\\' && j + 2 < p.len() {
                b = p[j + 2];
                j += 1;
            }
            j += 2;
            items.push((a, b));
        } else {
            items.push((a, a));
        }
    }
    if j >= p.len() {
        return None;
    }
    Some((Token::Class(items.into(), neg), j + 1))
}

/// An entry's pattern (§9.7 "Entries").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Exact: its name.
    Exact(Box<[u8]>),
    /// A glob other than `*`: its text as written, compiled.
    Glob(Box<[u8]>, Glob),
    /// `*` alone.
    Star,
}

impl Pattern {
    /// Classifies an entry: `quoted` text is exact and verbatim; unquoted
    /// text with an unescaped `*`, `?` or `[` is a glob; any other is exact
    /// with its `\c` pairs replaced by `c` (a final lone `\` kept).
    #[must_use]
    pub fn of(text: &[u8], quoted: bool) -> Self {
        if quoted {
            return Self::Exact(text.into());
        }
        let mut i = 0;
        let mut meta = false;
        while i < text.len() {
            match text[i] {
                b'\\' => i += 2,
                b'*' | b'?' | b'[' => {
                    meta = true;
                    break;
                }
                _ => i += 1,
            }
        }
        if meta {
            return if text == b"*" {
                Self::Star
            } else {
                Self::Glob(text.into(), Glob::new(text))
            };
        }
        let mut name = Vec::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            if text[i] == b'\\' && i + 1 < text.len() {
                name.push(text[i + 1]);
                i += 2;
            } else {
                name.push(text[i]);
                i += 1;
            }
        }
        Self::Exact(name.into())
    }

    /// Whether `name` matches.
    #[must_use]
    pub fn matches(&self, name: &[u8]) -> bool {
        match self {
            Self::Exact(n) => **n == *name,
            Self::Glob(_, g) => g.matches(name),
            Self::Star => true,
        }
    }

    /// The duplicate rule's "same": both exact with equal names, or both
    /// globs (the star included) with identical text.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) | (Self::Glob(a, _), Self::Glob(b, _)) => a == b,
            (Self::Star, Self::Star) => true,
            _ => false,
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Exact(n) | Self::Glob(n, _) => String::from_utf8_lossy(n).into_owned(),
            Self::Star => "*".to_owned(),
        }
    }
}

/// One node of the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The version name; `None` for the anonymous node.
    pub name: Option<Box<[u8]>>,
    /// The parents, as indices of earlier nodes, in the order written.
    pub parents: Box<[usize]>,
    global: Box<[Pattern]>,
    local: Box<[Pattern]>,
}

/// §9.7's decision for a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The version of node `n` (an index of [`VersionScript::nodes`]);
    /// the anonymous node's is the base version.
    Node(usize),
    /// Local (not exported).
    Local,
    /// The base version: exported unversioned (rule 7).
    Base,
}

/// A `version-reassigned` name: `(name, first node, second node)`.
type Reassigned = (Box<[u8]>, usize, usize);

/// A compiled version script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionScript {
    nodes: Box<[Node]>,
    /// Exact `global:` names: the first node naming each.
    exact_global: HashMap<Box<[u8]>, usize>,
    /// Exact `local:` names: the first node naming each.
    exact_local: HashMap<Box<[u8]>, usize>,
    /// The exact `global:` entries in script order, first of each name:
    /// `(node, name)` (rule 9's check).
    global_names: Box<[(usize, Box<[u8]>)]>,
    /// `version-reassigned`: `(name, first node, second node)`; reported
    /// when a plain key of that name is decided (§9.7 rule 1).
    reassigned: Box<[Reassigned]>,
}

impl VersionScript {
    /// Compiles a parsed script.
    ///
    /// # Errors
    ///
    /// Every refusal: `extern "C++"` entries (rule 6, refused by name,
    /// class `script-command`) and `duplicate-version-pattern`.
    pub fn compile(ast: &Ast) -> Result<Self, Vec<Diagnostic>> {
        let mut errors = Vec::new();
        if ast.nodes.iter().flat_map(|n| &n.entries).any(|e| e.language == Language::Cxx) {
            errors.push(Diagnostic::refused(
                RefusalClass::ScriptCommand,
                "extern \"C++\" in a version script (spec/resolve.md §9.7 rule 6: it needs the \
                 demangler; list the mangled names, or use a glob such as _ZN2ns*)",
            ));
            return Err(errors);
        }
        let mut nodes = Vec::with_capacity(ast.nodes.len());
        for n in &ast.nodes {
            let mut global = Vec::new();
            let mut local = Vec::new();
            for e in &n.entries {
                let p = Pattern::of(&e.pattern.text, e.pattern.quoted);
                match e.scope {
                    Scope::Global => global.push(p),
                    Scope::Local => local.push(p),
                }
            }
            nodes.push(Node {
                name: n.name.as_ref().map(|w| w.text.clone().into_boxed_slice()),
                parents: n.parents.iter().map(|p| p.node).collect(),
                global: global.into(),
                local: local.into(),
            });
        }
        let name = |i: usize| -> String {
            nodes[i].name.as_deref().map_or_else(
                || "(anonymous)".to_owned(),
                |n| String::from_utf8_lossy(n).into_owned(),
            )
        };
        // The duplicate rule: the same entry under `global:` of one node
        // and `local:` of another, either order; reported once per pair.
        for (a, na) in nodes.iter().enumerate() {
            for g in &*na.global {
                for (b, nb) in nodes.iter().enumerate() {
                    if a == b {
                        continue;
                    }
                    if nb.local.iter().any(|l| l.same(g)) {
                        errors.push(Diagnostic::error(Kind::DuplicateVersionPattern {
                            pattern: g.text(),
                            first: name(a),
                            second: name(b),
                        }));
                    }
                }
            }
        }
        if !errors.is_empty() {
            errors.dedup();
            return Err(errors);
        }
        let mut exact_global: HashMap<Box<[u8]>, usize> = HashMap::new();
        let mut exact_local: HashMap<Box<[u8]>, usize> = HashMap::new();
        let mut global_names = Vec::new();
        let mut reassigned = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            for p in &*n.global {
                if let Pattern::Exact(x) = p {
                    match exact_global.get(x) {
                        None => {
                            exact_global.insert(x.clone(), i);
                            global_names.push((i, x.clone()));
                        }
                        Some(&first) if first != i => {
                            if !reassigned.iter().any(|r: &Reassigned| r.0 == *x) {
                                reassigned.push((x.clone(), first, i));
                            }
                        }
                        Some(_) => {}
                    }
                }
            }
            for p in &*n.local {
                if let Pattern::Exact(x) = p {
                    exact_local.entry(x.clone()).or_insert(i);
                }
            }
        }
        Ok(Self {
            nodes: nodes.into(),
            exact_global,
            exact_local,
            global_names: global_names.into(),
            reassigned: reassigned.into(),
        })
    }

    /// Parses the `--version-script` files of a line, in command-line
    /// order, as one script (rule 13; `spec/script.md` §9.1), and
    /// compiles it.
    ///
    /// # Errors
    ///
    /// The parser's `version-script-syntax`, and [`Self::compile`]'s.
    pub fn load(files: &[SourceFile<'_>]) -> Result<Self, Vec<Diagnostic>> {
        let ast = mortar_script::version::parse_version_script(files).map_err(|d| vec![*d])?;
        Self::compile(&ast)
    }

    /// The nodes in script order.
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Whether the script is one anonymous node (rule 11): no version is
    /// defined.
    #[must_use]
    pub fn is_anonymous(&self) -> bool {
        self.nodes.iter().all(|n| n.name.is_none())
    }

    /// The `Verdef` index of node `n` (D-115: the base is 1, the named
    /// nodes 2… in script order); 0 for the anonymous node (unversioned).
    #[must_use]
    pub fn index(&self, n: usize) -> u16 {
        if self.nodes[n].name.is_none() { 0 } else { (n + 2).min(0x7fff) as u16 }
    }

    /// The node named `version`.
    #[must_use]
    pub fn node(&self, version: &[u8]) -> Option<usize> {
        self.nodes.iter().position(|n| n.name.as_deref() == Some(version))
    }

    /// §9.7's decision for the plain name `name`.
    #[must_use]
    pub fn decide(&self, name: &[u8]) -> Decision {
        let eg = self.exact_global.get(name).copied();
        let el = self.exact_local.get(name).copied();
        match (eg, el) {
            (Some(g), Some(l)) if g <= l => return self.node_decision(g),
            (Some(g), None) => return self.node_decision(g),
            (_, Some(_)) => return Decision::Local,
            (None, None) => {}
        }
        let (mut g, mut l, mut gs, mut ls) = (None, false, None, false);
        for (i, n) in self.nodes.iter().enumerate() {
            for p in &*n.global {
                match p {
                    Pattern::Glob(_, x) if x.matches(name) => g = Some(i),
                    Pattern::Star => gs = Some(i),
                    _ => {}
                }
            }
            for p in &*n.local {
                match p {
                    Pattern::Glob(_, x) if x.matches(name) => l = true,
                    Pattern::Star => ls = true,
                    _ => {}
                }
            }
        }
        if let Some(i) = g {
            self.node_decision(i)
        } else if l {
            Decision::Local
        } else if let Some(i) = gs {
            self.node_decision(i)
        } else if ls {
            Decision::Local
        } else {
            Decision::Base
        }
    }

    fn node_decision(&self, i: usize) -> Decision {
        if self.nodes[i].name.is_none() { Decision::Base } else { Decision::Node(i) }
    }

    /// Rule 12 for a `.symver` definition `name@version` or
    /// `name@@version`: `None` when the script has no node `version`;
    /// otherwise `Local` exactly when one of its `local:` entries matches,
    /// none of its `global:` entries does and the line has no `-E`.
    #[must_use]
    pub fn symver(&self, name: &[u8], version: &[u8], export_dynamic: bool) -> Option<Decision> {
        let i = self.node(version)?;
        let n = &self.nodes[i];
        let local = n.local.iter().any(|p| p.matches(name))
            && !n.global.iter().any(|p| p.matches(name))
            && !export_dynamic;
        Some(if local { Decision::Local } else { Decision::Node(i) })
    }

    fn display(&self, n: usize) -> String {
        self.nodes[n]
            .name
            .as_deref()
            .map_or_else(|| "(anonymous)".to_owned(), |s| String::from_utf8_lossy(s).into_owned())
    }
}

/// A diagnostic of [`apply`]: its key, and the file to place it at.
pub type VersionProblem = (DiagKey, Diagnostic, Option<FileId>);

/// The version stage of a `Shared` output (module docs). `exported` is
/// D-110's column, cleared for version-local keys; `versions` (one per
/// key, zeroed) gets every exported key's version index.
pub(crate) fn apply<V: View>(
    view: &V,
    line: &VersionLine,
    def: &[Def],
    exported: &mut [bool],
    versions: &mut [u16],
) -> Vec<VersionProblem> {
    let mut out: Vec<VersionProblem> = Vec::new();
    let script = line.script.as_deref();
    // The names of `version-reassigned` that a plain key has (a `.symver`
    // definition is matched by its own node only, rule 12).
    let mut reassigned_used: Vec<Vec<u8>> = Vec::new();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    for k in 0..def.len() {
        if !exported[k] {
            continue;
        }
        let d = def[k];
        let entry = match d {
            Def::Strong(e) | Def::Weak(e) | Def::Common(e) => Some(e),
            _ => None,
        };
        let (name, key_version) = view.key_name(SymbolId(k as u32));
        let default = entry.and_then(|e| view.default_version(e.file, e.index));
        let symver = match (key_version, default) {
            // The `foo@V` row of a `foo@@V` definition (resolve §9.2):
            // the key `foo` exports it.
            (Some(_), Some(_)) => {
                exported[k] = false;
                continue;
            }
            (Some(v), None) => Some((v, true)),
            (None, Some(v)) => Some((v, false)),
            (None, None) => None,
        };
        if let Some((v, hidden)) = symver {
            let at = entry.map_or(DiagKey::of(FileId(0), k as u64), DiagKey::at);
            let file = entry.map(|e| e.file);
            match script.and_then(|s| s.symver(name, v, line.export_dynamic)) {
                None => out.push((
                    at,
                    Diagnostic::error(Kind::UndefinedVersion {
                        name: text(name),
                        version: text(v),
                    }),
                    file,
                )),
                Some(Decision::Local) => {
                    exported[k] = false;
                    out.push((
                        at,
                        Diagnostic::warning(Kind::VersionedDefinitionHidden {
                            name: text(name),
                            version: text(v),
                        }),
                        file,
                    ));
                }
                Some(Decision::Node(i)) => {
                    let s = script.map_or(0, |s| s.index(i));
                    versions[k] = if hidden { s | VERSYM_HIDDEN } else { s };
                }
                Some(Decision::Base) => {}
            }
            continue;
        }
        let Some(s) = script else { continue };
        if !s.reassigned.is_empty() && s.reassigned.iter().any(|r| *r.0 == *name) {
            reassigned_used.push(name.to_vec());
        }
        match s.decide(name) {
            Decision::Node(i) => versions[k] = s.index(i),
            Decision::Local => exported[k] = false,
            Decision::Base => {}
        }
    }
    if let Some(s) = script {
        out.extend(script_problems(view, line, s, def, &reassigned_used));
    }
    out
}

/// The script-wide diagnostics of [`apply`]: `version-reassigned` for the
/// names a plain key has, and rule 9's undefined exact `global:` names.
fn script_problems<V: View>(
    view: &V,
    line: &VersionLine,
    s: &VersionScript,
    def: &[Def],
    reassigned_used: &[Vec<u8>],
) -> Vec<VersionProblem> {
    let mut out: Vec<VersionProblem> = Vec::new();
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut seq = 0u64;
    let mut next = || {
        seq += 1;
        DiagKey::of(FileId(0), seq)
    };
    for (name, first, second) in &*s.reassigned {
        if !reassigned_used.iter().any(|u| **u == **name) {
            continue;
        }
        out.push((
            next(),
            Diagnostic::warning(Kind::VersionReassigned {
                name: text(name),
                first: s.display(*first),
                second: s.display(*second),
            }),
            None,
        ));
    }
    // Rule 9: an exact `global:` name no relocatable object or the
    // linker defines.
    for (node, name) in &*s.global_names {
        let defined = view.find_key(name).is_some_and(|k| {
            matches!(
                def[k.0 as usize],
                Def::Strong(_) | Def::Weak(_) | Def::Common(_) | Def::Provide { .. }
            )
        });
        if defined {
            continue;
        }
        let (name, version) = (text(name), s.display(*node));
        let d = if line.no_undefined_version {
            Diagnostic::error(Kind::UndefinedVersion { name, version })
        } else {
            Diagnostic::warning(Kind::VersionScriptUndefinedName { name, version })
        };
        out.push((next(), d, None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(p: &str, s: &str) -> bool {
        Glob::new(p.as_bytes()).matches(s.as_bytes())
    }

    #[test]
    fn globs() {
        assert!(g("*", ""));
        assert!(g("f*o", "foo"));
        assert!(g("f?o", "foo"));
        assert!(!g("f?o", "fo"));
        assert!(g("[!f]oo", "goo"));
        assert!(g("[^f]oo", "goo"));
        assert!(!g("[!f]oo", "foo"));
        assert!(g("[]o]", "]"));
        assert!(g("[!]]", "x"));
        assert!(!g("[!]]", "]"));
        assert!(!g("f[o-a]?", "foo"));
        assert!(g("[oo", "[oo"));
        assert!(g("f[o", "f[o"));
        assert!(!g("foo\\", "foo\\"));
        assert!(!g("foo\\", "foo"));
        assert!(g("fo\\*", "fo*"));
        assert!(!g("fo\\*", "foo"));
        assert!(g("[-a]", "-"));
        assert!(g("[a-]", "-"));
        assert!(g("*a*b*c", "xxaxxbxxc"));
        assert!(!g("*a*b*c", "xxaxxbxx"));
    }

    #[test]
    fn patterns() {
        assert_eq!(Pattern::of(b"fo\\o", false), Pattern::Exact((*b"foo").into()));
        assert_eq!(Pattern::of(b"foo\\", false), Pattern::Exact((*b"foo\\").into()));
        assert_eq!(Pattern::of(b"foo*", true), Pattern::Exact((*b"foo*").into()));
        assert_eq!(Pattern::of(b"*", false), Pattern::Star);
        assert!(matches!(Pattern::of(b"fo*", false), Pattern::Glob(..)));
        assert!(Pattern::of(b"fo\\*", false).same(&Pattern::of(b"fo*", true)));
        assert!(!Pattern::of(b"fo*", false).same(&Pattern::of(b"fo*", true)));
    }
}
