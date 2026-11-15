//! The grammar: arguments to table rows, rows to the `Plan`
//! (`spec/cli.md` §2, §3).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

use mortar_diag::{Diagnostic, Key, Kind, RefusalClass};

use crate::PHASE;
use crate::plan::{
    BuildId, Compression, DynamicLinker, Emulation, Exit, HashStyle, Icf, Input, InputFormat,
    InputSpec, InputState, OtherOption, OutputKind, PackDynRelocs, Plan, ScriptArg, ScriptRole,
    SearchPath, Start, StartOf, SymbolArg, Target, TargetOption, Threads, UnresolvedSymbols,
};
use crate::response;
use crate::table::{
    Arg, Form, KindSet, Lands, Row, Spelling, StartKind, StateOp, Status, Values, ZField, rows,
};

/// The result of parsing a command line (`spec/cli.md` §3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// What the command line asks for.
    pub plan: Plan,
    /// Every option used, in argument order (§4.4).
    pub uses: Vec<Use>,
    /// The problems found, with the key that orders them (`spec/diag.md`
    /// §6): phase 0, the argument's index, a sequence number.
    pub diagnostics: Vec<(Key, Diagnostic)>,
    /// The command line after `@file` expansion (§2.6); argument indexes
    /// count in it.
    pub args: Vec<OsString>,
}

/// One use of an option (`spec/cli.md` §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// The row's index in [`rows`].
    pub row: usize,
    /// The argument index (after `@file` expansion) of the option.
    pub arg: u32,
    /// The option as written, completed to the full name of the spelling
    /// it matched: `-o`, `--output`, `-Tdata` (for `-Td`), `-z relro`.
    pub spelling: String,
    /// The value, if the option took one.
    pub value: Option<OsString>,
}

impl Use {
    /// The row used.
    #[must_use]
    pub fn row(&self) -> &'static Row {
        &rows()[self.row]
    }

    /// The refusal of this use, keyed on its argument (see
    /// [`Parsed::refusals`]): what a caller that takes some refused uses
    /// out of [`Parsed::refused_uses`] reports for them.
    #[must_use]
    pub fn refusal(&self) -> (Key, Diagnostic) {
        let d = match self.row().status {
            Status::Partial { note, .. } => {
                let value = self.value.as_deref().unwrap_or_default().to_string_lossy();
                Diagnostic::refused(RefusalClass::OptionValue, format!("{}={value}", self.spelling))
                    .note(note)
            }
            _ => Diagnostic::refused(RefusalClass::Option, self.spelling.clone()),
        };
        (Key::new(PHASE, self.arg, u64::MAX), d)
    }
}

impl Parsed {
    /// Whether any diagnostic is an error.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.diagnostics.iter().any(|(_, d)| d.severity() == mortar_diag::Severity::Error)
    }

    /// The refused options used (`spec/cli.md` §4.4): the first use of
    /// each refused row, and each use of a `partial` row with a refused
    /// value, in argument order. A use of a [`Lands::last_wins`] field
    /// that a later use of the same field overrides is not counted: the
    /// plan is the one the line would give without it.
    #[must_use]
    pub fn refused_uses(&self) -> Vec<&Use> {
        let mut seen = vec![false; rows().len()];
        let mut out = Vec::new();
        let mut last: HashMap<&str, usize> = HashMap::new();
        for (i, u) in self.uses.iter().enumerate() {
            if u.row().lands.last_wins() {
                last.insert(u.row().lands.field(), i);
            }
        }
        for (i, use_) in self.uses.iter().enumerate() {
            let lands = use_.row().lands;
            if lands.last_wins() && last.get(lands.field()) != Some(&i) {
                continue;
            }
            match use_.row().status {
                Status::Refused if !seen[use_.row] => {
                    seen[use_.row] = true;
                    out.push(use_);
                }
                Status::Partial { refused, .. } if refused_value(use_, refused) => out.push(use_),
                _ => {}
            }
        }
        out
    }

    /// The refusals of [`Parsed::refused_uses`] as diagnostics, keyed on
    /// their arguments: `unsupported option: -o`, or for a refused value of
    /// a `partial` row `unsupported option value: --build-id=md5` with the
    /// row's note.
    #[must_use]
    pub fn refusals(&self) -> Vec<(Key, Diagnostic)> {
        self.refused_uses().into_iter().map(Use::refusal).collect()
    }
}

/// Whether `use_`'s value is one of a `partial` row's refused values.
fn refused_value(use_: &Use, refused: &[&str]) -> bool {
    use_.value.as_deref().is_some_and(|v| refused.iter().any(|r| OsStr::new(r) == v))
}

/// The diagnostics for the options that do not belong to `target`
/// (`spec/cli.md` §2.9): each is an unknown option, as GNU ld built for
/// that target would find it. [`parse`] runs this when `-m` names the
/// target; otherwise the phase that finds the target runs it.
#[must_use]
pub fn check_target(plan: &Plan, target: Target) -> Vec<(Key, Diagnostic)> {
    plan.target_options
        .iter()
        .filter(|o| !o.targets.contains(&target))
        .map(|o| {
            let names: Vec<&str> = o.targets.iter().map(|t| t.name()).collect();
            (
                Key::new(PHASE, o.arg, 1),
                Diagnostic::error(Kind::UnknownOption { option: o.option.clone() }).note(format!(
                    "{} is an option of {} only; this link is for {}",
                    o.option,
                    names.join(" and "),
                    target.name()
                )),
            )
        })
        .collect()
}

/// Lookup tables over the rows, built once.
struct Index {
    /// Long spellings by name: (row, spelling).
    long: HashMap<&'static str, (usize, usize)>,
    /// `-z` keywords by name.
    z: HashMap<&'static str, (usize, usize)>,
    /// Short options by character.
    short: HashMap<u8, (usize, usize)>,
    /// GNU long spellings sorted by name, for abbreviations (§2.4).
    gnu_long: Vec<(&'static str, usize, usize)>,
    /// Rows whose value is glued to the name (`--lto-O2`).
    glued: Vec<(usize, usize)>,
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = Index {
            long: HashMap::new(),
            z: HashMap::new(),
            short: HashMap::new(),
            gnu_long: Vec::new(),
            glued: Vec::new(),
        };
        for (r, row) in rows().iter().enumerate() {
            for (s, sp) in row.spellings.iter().enumerate() {
                match sp.form {
                    Form::Short => {
                        index.short.insert(sp.text.as_bytes()[0], (r, s));
                    }
                    Form::Z => {
                        index.z.insert(sp.text, (r, s));
                    }
                    Form::Single | Form::Long | Form::LongTwo => {
                        if matches!(row.arg, Arg::Glued(_)) {
                            index.glued.push((r, s));
                        } else {
                            index.long.insert(sp.text, (r, s));
                        }
                        if sp.gnu {
                            index.gnu_long.push((sp.text, r, s));
                        }
                    }
                }
            }
        }
        index.gnu_long.sort_unstable();
        index
    })
}

/// Parses a command line: `args` without the program name
/// (`spec/cli.md` §2, §3). Never fails: problems are diagnostics.
#[must_use]
pub fn parse(args: &[OsString]) -> Parsed {
    let (args, diagnostics) = response::expand(args);
    let mut p = Parser {
        args: &args,
        i: 0,
        out: Parsed { diagnostics, ..Parsed::default() },
        state: InputState::default(),
        saved: Vec::new(),
        group_depth: 0,
        groups: 0,
        libs: 0,
        kind: None,
        relocatable: None,
        seq: 0,
    };
    p.run();
    let mut out = p.out;
    out.args = args;
    out
}

struct Parser<'a> {
    args: &'a [OsString],
    /// The index of the next argument.
    i: usize,
    out: Parsed,
    state: InputState,
    saved: Vec<InputState>,
    group_depth: u32,
    groups: u32,
    libs: u32,
    /// The last of `-shared`, `-pie`, `-no-pie`, as written.
    kind: Option<(KindSet, String)>,
    /// `-r`, as written.
    relocatable: Option<String>,
    seq: u64,
}

/// What an argument turned out to be.
struct Matched {
    row: usize,
    /// As written, completed: `--output`, `-Tdata`.
    name: String,
    /// The value joined to the option (`=v`, `-ov`, the glued tail).
    joined: Option<OsString>,
}

fn arg_index(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// An error was reported; the argument is skipped.
struct Reported;

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn os(bytes: &[u8]) -> OsString {
    OsStr::from_bytes(bytes).to_os_string()
}

impl Parser<'_> {
    fn error(&mut self, at: usize, diagnostic: Diagnostic) {
        self.seq += 1;
        let key = Key::new(PHASE, arg_index(at), self.seq);
        self.out.diagnostics.push((key, diagnostic));
    }

    fn run(&mut self) {
        while self.i < self.args.len() {
            let at = self.i;
            self.i += 1;
            let arg = self.args[at].as_bytes();
            if arg.len() < 2 || arg[0] != b'-' {
                // `-` alone is an input (standard input, for GNU ld).
                self.input(at, InputSpec::Path(self.args[at].clone()));
                continue;
            }
            if self.option(at) {
                return;
            }
        }
        self.finish();
    }

    /// Handles the option at `at`. True when parsing stops (`--help`,
    /// `--version`: §5).
    fn option(&mut self, at: usize) -> bool {
        let arg = self.args[at].as_bytes();
        let text = lossy(arg);
        if arg == b"--" {
            self.error(
                at,
                Diagnostic::error(Kind::UnknownOption { option: text }).note(
                    "GNU ld reads every argument after '--' as an input; mortar does not \
                     accept '--' (spec/cli.md §2.5)",
                ),
            );
            return false;
        }
        let dashes = if arg.starts_with(b"--") { 2 } else { 1 };
        let body = &arg[dashes..];
        if dashes == 1 && body[0] == b'z' {
            self.z_option(at, &body[1..]);
            return false;
        }
        let Some(m) = self.lookup(at, dashes, body) else { return false };
        let row = &rows()[m.row];
        let Ok(value) = self.value(at, row, &m) else { return false };
        self.apply(at, m, value)
    }

    /// Finds the row of a non-`-z` option (§2.2-§2.5), or reports why
    /// there is none.
    fn lookup(&mut self, at: usize, dashes: usize, body: &[u8]) -> Option<Matched> {
        let index = index();
        let arg = self.args[at].as_bytes();
        let (name, joined) = match body.iter().position(|&b| b == b'=') {
            Some(eq) => (&body[..eq], Some(os(&body[eq + 1..]))),
            None => (body, None),
        };
        let completed = |sp: &Spelling| {
            let mut s = "-".repeat(dashes);
            s.push_str(sp.text);
            s
        };
        // 1. An exact name, of any row (§2.2).
        if let Ok(name_str) = std::str::from_utf8(name)
            && let Some(&(r, s)) = index.long.get(name_str)
            && rows()[r].spellings[s].takes_dashes(dashes)
        {
            let sp = &rows()[r].spellings[s];
            return Some(Matched { row: r, name: completed(sp), joined });
        }
        // 2. A name with its value glued on (`--lto-O2`).
        for &(r, s) in &index.glued {
            let sp = &rows()[r].spellings[s];
            if sp.takes_dashes(dashes) && body.starts_with(sp.text.as_bytes()) {
                let tail = os(&body[sp.text.len()..]);
                return Some(Matched { row: r, name: completed(sp), joined: Some(tail) });
            }
        }
        // 3. One letter after one dash that is a short option (§2.3).
        if dashes == 1 && body.len() == 1 && index.short.contains_key(&body[0]) {
            return self.short(at, body);
        }
        // 4. An abbreviation of GNU ld's long names (§2.4).
        if !name.is_empty() {
            let candidates: Vec<(&'static str, usize, usize)> = index
                .gnu_long
                .iter()
                .filter(|(text, r, s)| {
                    text.as_bytes().starts_with(name)
                        && rows()[*r].spellings[*s].takes_dashes(dashes)
                })
                .copied()
                .collect();
            match candidates.as_slice() {
                [(_, r, s)] => {
                    let sp = &rows()[*r].spellings[*s];
                    return Some(Matched { row: *r, name: completed(sp), joined });
                }
                [] => {}
                many => {
                    let names: Vec<String> = many
                        .iter()
                        .map(|(text, _, _)| format!("{}{text}", "-".repeat(dashes)))
                        .collect();
                    self.error(
                        at,
                        Diagnostic::error(Kind::UnknownOption { option: lossy(arg) }).note(
                            format!(
                                "ambiguous abbreviation: it could be {} (spec/cli.md §2.4)",
                                names.join(", ")
                            ),
                        ),
                    );
                    return None;
                }
            }
        }
        // 5. A short option with its value joined (`-ofile`, `-L/lib`).
        if dashes == 1 && index.short.contains_key(&body[0]) {
            return self.short(at, body);
        }
        self.error(at, Diagnostic::error(Kind::UnknownOption { option: lossy(arg) }));
        None
    }

    /// A short option: `body` is the argument without its dash.
    fn short(&mut self, at: usize, body: &[u8]) -> Option<Matched> {
        let arg = self.args[at].as_bytes();
        let Some(&(r, s)) = index().short.get(&body[0]) else {
            self.error(at, Diagnostic::error(Kind::UnknownOption { option: lossy(arg) }));
            return None;
        };
        let row = &rows()[r];
        let name = format!("-{}", row.spellings[s].text);
        if body.len() == 1 {
            return Some(Matched { row: r, name, joined: None });
        }
        if matches!(row.arg, Arg::None) {
            self.error(
                at,
                Diagnostic::error(Kind::UnknownOption { option: lossy(arg) }).note(format!(
                    "{name} takes no value, and mortar does not read grouped short options \
                     (spec/cli.md §2.5)"
                )),
            );
            return None;
        }
        Some(Matched { row: r, name, joined: Some(os(&body[1..])) })
    }

    /// The option's value (§2.3); `Ok(None)` when it takes none.
    fn value(&mut self, at: usize, row: &Row, m: &Matched) -> Result<Option<OsString>, Reported> {
        let arg = self.args[at].as_bytes();
        let (values, value) = match row.arg {
            Arg::None => {
                if m.joined.is_some() {
                    self.error(
                        at,
                        Diagnostic::error(Kind::UnknownOption { option: lossy(arg) })
                            .note(format!("{} takes no value (spec/cli.md §2.3)", m.name)),
                    );
                    return Err(Reported);
                }
                return Ok(None);
            }
            Arg::Rest => {
                let value = self.args.get(self.i).cloned();
                self.i = self.args.len();
                return Ok(value);
            }
            Arg::Optional(values) | Arg::Glued(values) => (values, m.joined.clone()),
            Arg::Required(values) => {
                let value = if let Some(joined) = &m.joined {
                    joined.clone()
                } else if let Some(next) = self.args.get(self.i) {
                    self.i += 1;
                    next.clone()
                } else {
                    self.error(
                        at,
                        Diagnostic::error(Kind::MissingArgument { option: m.name.clone() }),
                    );
                    return Err(Reported);
                };
                (values, Some(value))
            }
        };
        if let Some(v) = &value
            && let Err(note) = check_value(values, v)
        {
            self.error(
                at,
                Diagnostic::error(Kind::InvalidOptionValue {
                    option: m.name.clone(),
                    value: v.to_string_lossy().into_owned(),
                })
                .note(note),
            );
            return Err(Reported);
        }
        Ok(value)
    }

    /// `-z keyword[=value]` (§2.7). `rest` is what follows `-z` in the
    /// same argument.
    fn z_option(&mut self, at: usize, rest: &[u8]) {
        let keyword: Vec<u8> = if rest.is_empty() {
            if let Some(next) = self.args.get(self.i) {
                self.i += 1;
                next.as_bytes().to_vec()
            } else {
                self.error(
                    at,
                    Diagnostic::error(Kind::MissingArgument { option: "-z".to_owned() }),
                );
                return;
            }
        } else {
            rest.to_vec()
        };
        let (name, value) = match keyword.iter().position(|&b| b == b'=') {
            Some(eq) => (&keyword[..eq], Some(os(&keyword[eq + 1..]))),
            None => (&keyword[..], None),
        };
        let whole = format!("-z {}", lossy(&keyword));
        let found = std::str::from_utf8(name).ok().and_then(|n| index().z.get(n)).copied();
        let Some((r, s)) = found else {
            self.error(
                at,
                Diagnostic::error(Kind::UnknownOption { option: whole })
                    .note("GNU ld, lld and mold warn and go on; mortar does not ignore a -z keyword (spec/cli.md §2.7)"),
            );
            return;
        };
        let row = &rows()[r];
        let full = format!("-z {}", row.spellings[s].text);
        let values = match (row.arg, &value) {
            (Arg::None | Arg::Optional(_), None) => None,
            (Arg::None, Some(_)) => {
                self.error(
                    at,
                    Diagnostic::error(Kind::UnknownOption { option: whole })
                        .note(format!("{full} takes no value (spec/cli.md §2.7)")),
                );
                return;
            }
            (Arg::Required(_), None) => {
                self.error(at, Diagnostic::error(Kind::MissingArgument { option: full }));
                return;
            }
            (Arg::Required(v) | Arg::Optional(v), _) => Some(v),
            (Arg::Glued(_) | Arg::Rest, _) => {
                unreachable!("no -z keyword is glued or takes the rest")
            }
        };
        if let (Some(values), Some(v)) = (values, &value)
            && let Err(note) = check_value(values, v)
        {
            self.error(
                at,
                Diagnostic::error(Kind::InvalidOptionValue {
                    option: full,
                    value: v.to_string_lossy().into_owned(),
                })
                .note(note),
            );
            return;
        }
        let m = Matched { row: r, name: full, joined: None };
        self.apply(at, m, value);
    }

    fn input(&mut self, at: usize, spec: InputSpec) {
        let arg = arg_index(at);
        self.out.plan.inputs.push(Input { arg, spec, state: self.state.clone() });
    }

    /// Records a use and lands it in the plan (§3). True when parsing
    /// stops.
    #[allow(clippy::too_many_lines)]
    fn apply(&mut self, at: usize, m: Matched, value: Option<OsString>) -> bool {
        let row = &rows()[m.row];
        let arg = arg_index(at);
        self.out.uses.push(Use { row: m.row, arg, spelling: m.name.clone(), value: value.clone() });
        if !row.targets.is_empty() {
            self.out.plan.target_options.push(TargetOption {
                arg,
                option: m.name.clone(),
                value: value.clone(),
                targets: row.targets.to_vec(),
            });
        }
        let text = || value.clone().unwrap_or_default();
        let word = || value.as_ref().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
        let plan = &mut self.out.plan;
        match row.lands {
            Lands::Output => plan.output = Some(text()),
            Lands::Kind(KindSet::Relocatable) => self.relocatable = Some(m.name),
            Lands::Kind(set) => self.kind = Some((set, m.name)),
            Lands::Emulation => {
                let name = word();
                if let Some(e) = Emulation::from_name(&name) {
                    plan.emulation = Some(e);
                } else {
                    let names: Vec<&str> = Emulation::ALL.iter().map(|e| e.name()).collect();
                    self.error(
                        at,
                        Diagnostic::refused(RefusalClass::Emulation, name)
                            .note(format!("mortar links for {}", names.join(", "))),
                    );
                }
            }
            Lands::Entry => plan.entry = Some(text()),
            Lands::Library => {
                let v = text();
                let spec = match v.as_bytes().strip_prefix(b":") {
                    Some(file) => InputSpec::LibraryFile(os(file)),
                    None => InputSpec::Library(v),
                };
                self.input(at, spec);
            }
            Lands::JustSymbols => self.input(at, InputSpec::JustSymbols(text())),
            Lands::State(op) => self.state_op(at, op, &m.name, value.as_ref()),
            Lands::LibraryPath => {
                let v = text();
                let bytes = v.as_bytes();
                let (path, in_sysroot) = if let Some(rest) = bytes.strip_prefix(b"=") {
                    (os(rest), true)
                } else if let Some(rest) = bytes.strip_prefix(b"$SYSROOT") {
                    (os(rest), true)
                } else {
                    (v.clone(), false)
                };
                plan.library_paths.push(SearchPath { arg, path, in_sysroot });
            }
            // An empty `--sysroot=` is no sysroot, as in GNU ld, lld and
            // mold (`spec/input.md` §4.3); the last one wins.
            Lands::Sysroot => plan.sysroot = Some(text()).filter(|t| !t.is_empty()),
            Lands::NoStdlib => plan.nostdlib = true,
            Lands::Script(role) => plan.scripts.push(ScriptArg { arg, role, path: text() }),
            Lands::Symbol(role) => plan.symbols.push(SymbolArg { arg, role, value: text() }),
            Lands::ExportDynamic(on) => plan.dynamic.export_dynamic = on,
            Lands::Bsymbolic(b) => plan.dynamic.bsymbolic = b,
            Lands::Soname => plan.dynamic.soname = Some(text()),
            Lands::Rpath => plan.dynamic.rpath.push(text()),
            Lands::RpathLink => plan.dynamic.rpath_link.push(text()),
            Lands::DynamicLinker => plan.dynamic.dynamic_linker = Some(DynamicLinker::Path(text())),
            Lands::NoDynamicLinker => plan.dynamic.dynamic_linker = Some(DynamicLinker::None),
            Lands::NewDtags(on) => plan.dynamic.new_dtags = Some(on),
            Lands::Init => plan.dynamic.init = Some(text()),
            Lands::Fini => plan.dynamic.fini = Some(text()),
            Lands::HashStyle => {
                plan.dynamic.hash_style = Some(match word().as_str() {
                    "sysv" => HashStyle::Sysv,
                    "gnu" => HashStyle::Gnu,
                    "both" => HashStyle::Both,
                    _ => HashStyle::None,
                });
            }
            Lands::PackDynRelocs => {
                plan.dynamic.pack_dyn_relocs = Some(match word().as_str() {
                    "relr" => PackDynRelocs::Relr,
                    "android" => PackDynRelocs::Android,
                    "android+relr" => PackDynRelocs::AndroidRelr,
                    _ => PackDynRelocs::None,
                });
            }
            Lands::Z(field) => z_field(plan, field, value),
            Lands::GcSections(on) => plan.shaping.gc_sections = on,
            Lands::PrintGcSections(on) => plan.shaping.print_gc_sections = on,
            Lands::GcKeepExported => plan.shaping.gc_keep_exported = true,
            Lands::Icf => {
                plan.shaping.icf = Some(match word().as_str() {
                    "all" => Icf::All,
                    "safe" => Icf::Safe,
                    _ => Icf::None,
                });
            }
            Lands::NoIcf => plan.shaping.icf = Some(Icf::None),
            Lands::BuildId => {
                plan.shaping.build_id = match value.as_ref().map(|v| v.to_string_lossy()) {
                    None => Some(BuildId::Default),
                    Some(v) => match v.as_ref() {
                        "none" => None,
                        "md5" => Some(BuildId::Md5),
                        "sha1" => Some(BuildId::Sha1),
                        "sha256" => Some(BuildId::Sha256),
                        "fast" => Some(BuildId::Fast),
                        "uuid" => Some(BuildId::Uuid),
                        hex => Some(BuildId::Hex(hex.to_owned())),
                    },
                };
            }
            Lands::NoBuildId => plan.shaping.build_id = None,
            Lands::Strip(s) => plan.shaping.strip = Some(s),
            Lands::Discard(d) => plan.shaping.discard = Some(d),
            Lands::CompressDebugSections => {
                plan.shaping.compress_debug_sections = Some(match word().as_str() {
                    "zlib" | "zlib-gabi" => Compression::Zlib,
                    "zlib-gnu" => Compression::ZlibGnu,
                    "zstd" => Compression::Zstd,
                    _ => Compression::None,
                });
            }
            Lands::EhFrameHdr(on) => plan.shaping.eh_frame_hdr = Some(on),
            Lands::Magic(magic) => plan.shaping.magic = Some(magic),
            Lands::ImageBase => plan.shaping.image_base = Some(text()),
            Lands::Start(kind) => {
                let v = text();
                let (what, address) = match kind {
                    StartKind::Text => (StartOf::Section(".text".into()), v),
                    StartKind::Data => (StartOf::Section(".data".into()), v),
                    StartKind::Bss => (StartOf::Section(".bss".into()), v),
                    StartKind::TextSegment => (StartOf::TextSegment, v),
                    StartKind::RodataSegment => (StartOf::RodataSegment, v),
                    StartKind::LdataSegment => (StartOf::LdataSegment, v),
                    StartKind::Section => {
                        let bytes = v.as_bytes();
                        let Some(eq) = bytes.iter().position(|&b| b == b'=') else {
                            self.error(
                                at,
                                Diagnostic::error(Kind::InvalidOptionValue {
                                    option: m.name,
                                    value: word(),
                                })
                                .note("expected SECTION=ADDRESS"),
                            );
                            return false;
                        };
                        (StartOf::Section(os(&bytes[..eq])), os(&bytes[eq + 1..]))
                    }
                };
                plan.shaping.starts.push(Start { arg, what, address });
            }
            Lands::EmitRelocs => plan.shaping.emit_relocs = true,
            Lands::GdbIndex(on) => plan.shaping.gdb_index = Some(on),
            Lands::ErrorLimit => plan.diagnostics.error_limit = word().parse().ok(),
            Lands::FatalWarnings(on) => plan.diagnostics.fatal_warnings = on,
            Lands::NoWarnings => plan.diagnostics.no_warnings = true,
            Lands::Color => {
                plan.diagnostics.color = match value.as_ref().map(|v| v.to_string_lossy()) {
                    None => crate::plan::Color::Always,
                    Some(v) => match v.as_ref() {
                        "always" => crate::plan::Color::Always,
                        "never" => crate::plan::Color::Never,
                        _ => crate::plan::Color::Auto,
                    },
                };
            }
            Lands::NoColor => plan.diagnostics.color = crate::plan::Color::Never,
            Lands::NoUndefined => plan.z.defs = Some(true),
            Lands::UnresolvedSymbols => {
                plan.diagnostics.unresolved_symbols = Some(match word().as_str() {
                    "ignore-all" => UnresolvedSymbols::IgnoreAll,
                    "ignore-in-object-files" => UnresolvedSymbols::IgnoreInObjectFiles,
                    "ignore-in-shared-libs" => UnresolvedSymbols::IgnoreInSharedLibs,
                    _ => UnresolvedSymbols::ReportAll,
                });
            }
            Lands::WarnUnresolved(on) => plan.diagnostics.warn_unresolved_symbols = Some(on),
            Lands::AllowMultipleDefinition(on) => plan.diagnostics.allow_multiple_definition = on,
            Lands::AllowShlibUndefined(on) => plan.dynamic.allow_shlib_undefined = Some(on),
            Lands::WarnBackrefs(on) => plan.diagnostics.warn_backrefs = on,
            Lands::WarnCommon(on) => plan.diagnostics.warn_common = on,
            Lands::NoinhibitExec => plan.diagnostics.noinhibit_exec = true,
            Lands::Map => plan.bookkeeping.map = Some(text()),
            Lands::PrintMap => plan.bookkeeping.print_map = true,
            Lands::MortarMap => plan.bookkeeping.mortar_map = Some(text()),
            Lands::Cref => plan.bookkeeping.cref = true,
            Lands::Reproduce => plan.bookkeeping.reproduce = Some(text()),
            Lands::Threads => {
                plan.bookkeeping.threads =
                    Some(match value.as_ref().map(|v| v.to_string_lossy()) {
                        None => Threads::All,
                        Some(v) => Threads::Count(v.parse().unwrap_or(1)),
                    });
            }
            Lands::ThreadCount => {
                plan.bookkeeping.threads = Some(Threads::Count(word().parse().unwrap_or(1)));
            }
            Lands::NoThreads => plan.bookkeeping.threads = Some(Threads::Count(1)),
            Lands::TimePhases => plan.bookkeeping.time_phases = true,
            Lands::Verify => plan.bookkeeping.verify = true,
            Lands::Version => {
                plan.exit = Some(Exit::Version);
                return true;
            }
            Lands::Help => {
                plan.exit = Some(Exit::Help);
                return true;
            }
            Lands::PrintVersion(style) => {
                // `-V` after `-v` (or before) shows the emulations.
                plan.print_version = plan.print_version.max(Some(style));
            }
            Lands::Verbose => plan.bookkeeping.verbose = true,
            Lands::Trace => plan.bookkeeping.trace = true,
            Lands::DependencyFile => plan.bookkeeping.dependency_file = Some(text()),
            Lands::Plugin => plan.plugin.path = Some(text()),
            Lands::PluginOpt => plan.plugin.options.push(text()),
            Lands::Other => plan.other.push(OtherOption { arg, name: row.name(), value }),
        }
        false
    }

    /// The positional state (§3.3).
    fn state_op(&mut self, at: usize, op: StateOp, name: &str, value: Option<&OsString>) {
        match op {
            StateOp::AsNeeded(on) => self.state.as_needed = on,
            StateOp::WholeArchive(on) => self.state.whole_archive = on,
            StateOp::Static(on) => self.state.link_static = on,
            StateOp::CopyDtNeeded(on) => self.state.copy_dt_needed = on,
            StateOp::Format => {
                let v = value.cloned().unwrap_or_default();
                self.state.format = match v.as_bytes() {
                    b"default" | b"elf" => InputFormat::Detect,
                    b"binary" => InputFormat::Binary,
                    _ => InputFormat::Named(v),
                };
            }
            StateOp::Push => self.saved.push(self.state.clone()),
            StateOp::Pop => match self.saved.pop() {
                // `--push-state` saves the four switches; the group, the
                // lib region and `-b` are not part of it (§3.3).
                Some(saved) => {
                    self.state.as_needed = saved.as_needed;
                    self.state.whole_archive = saved.whole_archive;
                    self.state.link_static = saved.link_static;
                    self.state.copy_dt_needed = saved.copy_dt_needed;
                }
                None => self.unbalanced(at, name, "--push-state"),
            },
            StateOp::StartGroup => {
                if self.group_depth == 0 {
                    self.groups += 1;
                    self.state.group = Some(self.groups);
                }
                self.group_depth += 1;
            }
            StateOp::EndGroup => {
                if self.group_depth == 0 {
                    self.unbalanced(at, name, "--start-group");
                } else {
                    self.group_depth -= 1;
                    if self.group_depth == 0 {
                        self.state.group = None;
                    }
                }
            }
            StateOp::StartLib => {
                if self.state.lib.is_some() {
                    self.unbalanced(at, name, "--end-lib");
                } else {
                    self.libs += 1;
                    self.state.lib = Some(self.libs);
                }
            }
            StateOp::EndLib => {
                if self.state.lib.is_none() {
                    self.unbalanced(at, name, "--start-lib");
                } else {
                    self.state.lib = None;
                }
            }
        }
    }

    fn unbalanced(&mut self, at: usize, option: &str, counterpart: &str) {
        self.error(
            at,
            Diagnostic::error(Kind::UnbalancedOption {
                option: option.to_owned(),
                counterpart: counterpart.to_owned(),
            }),
        );
    }

    /// What is decided at the end of the command line (§3.2, §2.9).
    fn finish(&mut self) {
        let end = self.args.len();
        // Static: no program interpreter was asked for (§3.2).
        let static_end =
            !matches!(self.out.plan.dynamic.dynamic_linker, Some(DynamicLinker::Path(_)));
        let kind = match (&self.relocatable, &self.kind) {
            (Some(_), None | Some((KindSet::NoPie, _))) => OutputKind::Relocatable,
            (Some(r), Some((_, other))) => {
                let diagnostic = Diagnostic::error(Kind::ConflictingOptions {
                    first: r.clone(),
                    second: other.clone(),
                });
                self.error(end, diagnostic);
                OutputKind::Relocatable
            }
            (None, Some((KindSet::Shared, _))) => OutputKind::Shared,
            (None, Some((KindSet::Pie, _))) if static_end => OutputKind::StaticPie,
            (None, Some((KindSet::Pie, _))) => OutputKind::PieExecutable,
            (None, _) if static_end => OutputKind::StaticExecutable,
            (None, _) => OutputKind::Executable,
        };
        self.out.plan.kind = kind;
        if let Some(emulation) = self.out.plan.emulation {
            let mut found = check_target(&self.out.plan, emulation.target());
            self.out.diagnostics.append(&mut found);
        }
        let plan = &self.out.plan;
        let scripted = plan
            .scripts
            .iter()
            .any(|s| matches!(s.role, ScriptRole::Script | ScriptRole::DefaultScript));
        if plan.inputs.is_empty() && !scripted && plan.print_version.is_none() {
            self.error(end, Diagnostic::error(Kind::NoInputFiles));
        }
    }
}

/// Sets a typed `-z` field (§3.7).
fn z_field(plan: &mut Plan, field: ZField, value: Option<OsString>) {
    let z = &mut plan.z;
    match field {
        ZField::Now(on) => z.now = Some(on),
        ZField::Relro(on) => z.relro = Some(on),
        ZField::Execstack(on) => z.execstack = Some(on),
        ZField::Text(on) => z.text = Some(on),
        ZField::SeparateCode(on) => z.separate_code = Some(on),
        ZField::Defs(on) => z.defs = Some(on),
        ZField::MaxPageSize => z.max_page_size = value,
        ZField::CommonPageSize => z.common_page_size = value,
        ZField::StackSize => z.stack_size = value,
        ZField::PackRelativeRelocs(on) => z.pack_relative_relocs = Some(on),
        ZField::Copyreloc(on) => z.copyreloc = Some(on),
        ZField::Combreloc(on) => z.combreloc = Some(on),
        ZField::Sectionheader(on) => z.sectionheader = Some(on),
        ZField::MemorySeal(on) => z.memory_seal = Some(on),
    }
}

/// Checks a value against what the option accepts (§2.3). The error is
/// the note for `invalid-option-value`.
fn check_value(values: Values, value: &OsStr) -> Result<(), String> {
    let text = value.to_str().unwrap_or("");
    match values {
        Values::Any => Ok(()),
        Values::Choice(words) => {
            if words.contains(&text) {
                Ok(())
            } else {
                Err(format!("expected one of: {}", words.join(", ")))
            }
        }
        Values::Count => {
            if !text.is_empty()
                && text.bytes().all(|b| b.is_ascii_digit())
                && text.parse::<u32>().is_ok()
            {
                Ok(())
            } else {
                Err("expected a decimal number".to_owned())
            }
        }
        Values::BuildId => {
            // spec: image.md I-70 (`0x` is case-sensitive, as GNU ld's and
            // lld's; then one or more pairs of hexadecimal digits).
            if let Some(digits) = text.strip_prefix("0x") {
                if !digits.is_empty()
                    && digits.len().is_multiple_of(2)
                    && digits.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    Ok(())
                } else {
                    Err("expected an even number of hexadecimal digits".to_owned())
                }
            } else if matches!(text, "none" | "md5" | "sha1" | "sha256" | "fast" | "uuid") {
                Ok(())
            } else {
                Err("expected one of: none, md5, sha1, sha256, fast, uuid, 0xHEX".to_owned())
            }
        }
    }
}
