//! One rule for every file a link writes (`spec/cli.md` §3.14, WP-087):
//! no file the link writes is a file it read, or another file it writes,
//! decided after the front and before anything is written.
//!
//! The registry is [`mortar_cli::table::WRITES`]: the last use of each row
//! there names a written file, so a row that becomes honored is checked
//! without this module knowing it. Two entries are not plain uses: the
//! output (`-o`, else `a.out`) and incremental mode's files, which exist
//! only in incremental mode (the session gives them).
//!
//! Identity is the device and inode, following symbolic links (std's
//! `MetadataExt`); a file that does not exist yet has none, and is
//! compared with the other written files by its directory's identity and
//! its name. Non-regular files (character devices such as `/dev/null`,
//! FIFOs, a terminal behind `/dev/stdout`) are outside the rule: writing
//! one destroys no file, and `-o /dev/null -Map=/dev/null` is an idiom.
//!
//! The check is a guard against mistakes on the command line, not against
//! a concurrent rename: a file renamed between the check and the write is
//! not seen.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use mortar_cli::Parsed;
use mortar_cli::plan::ScriptRole;
use mortar_cli::table::Status;
use mortar_core::inputs::{Opened, OpenedKind};
use mortar_diag::{Collector, Diagnostic, Key, Kind};

/// The phase of the check (`spec/model.md` §8): after layout, before the
/// image writes anything.
pub const PHASE: u16 = 95;

/// One file the link writes (an entry of the registry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The option and its path, as a diagnostic shows it: `-Map=a.o`,
    /// `-o out`; `the default output a.out` without `-o`.
    pub spelling: String,
    /// Its role and path, as another written file's clash names it:
    /// `the map a.o`.
    pub role: String,
    /// The path the writer uses (relative paths under the working
    /// directory).
    pub path: PathBuf,
    /// The path as given, for diagnostics.
    pub given: PathBuf,
    /// The output (`-o`): it may replace a file the link read
    /// (`spec/image.md` I-62: it is renamed into place after every input
    /// was read, and GNU's `ld -r -o a.o a.o b.o` idiom keeps working).
    pub output: bool,
}

/// The registry of one link: the output first, then the last use of each
/// other honored row of [`mortar_cli::table::WRITES`] in table order, then
/// `incremental` (the files incremental mode writes, with their options).
#[must_use]
pub fn registry(
    parsed: &Parsed,
    cwd: &Path,
    out_path: &Path,
    incremental: &[(&'static str, &Path)],
) -> Vec<Written> {
    let mut v = Vec::new();
    let output_use = parsed.uses.iter().rev().find(|u| u.row().name() == "-o");
    let shown = |p: &OsStr| p.to_string_lossy().into_owned();
    v.push(match output_use.and_then(|u| u.value.as_deref().map(|p| (u, p))) {
        Some((u, p)) => Written {
            spelling: format!("{} {}", u.spelling, shown(p)),
            role: format!("the output {}", shown(p)),
            path: out_path.to_path_buf(),
            given: PathBuf::from(p),
            output: true,
        },
        None => Written {
            spelling: "the default output a.out".to_owned(),
            role: "the output a.out".to_owned(),
            path: out_path.to_path_buf(),
            given: PathBuf::from("a.out"),
            output: true,
        },
    });
    for (name, role) in mortar_cli::table::WRITES {
        if matches!(*name, "-o" | "--incremental-state" | "--incremental-report") {
            continue;
        }
        let Some(u) = parsed.uses.iter().rev().find(|u| u.row().name() == *name) else {
            continue;
        };
        // A refused row stops the line before the link (cli.md §4.3).
        if u.row().status == Status::Refused {
            continue;
        }
        let Some(p) = u.value.as_deref() else { continue };
        v.push(Written {
            spelling: format!("{}={}", u.spelling, shown(p)),
            role: format!("{role} {}", shown(p)),
            path: cwd.join(p),
            given: PathBuf::from(p),
            output: false,
        });
    }
    for (option, path) in incremental {
        let role = mortar_cli::table::WRITES
            .iter()
            .find(|(n, _)| n == option)
            .map_or("the file", |(_, r)| *r);
        v.push(Written {
            spelling: format!("{option}={}", path.display()),
            role: format!("{role} {}", path.display()),
            path: path.to_path_buf(),
            given: path.to_path_buf(),
            output: false,
        });
    }
    v
}

/// The names the image's writer touches for a written file
/// (`spec/image.md` I-59, I-113): the file, its temporary `T0` and its
/// set-aside `A`. A name `T0.<pid>` is created with `create_new` and can
/// replace nothing.
fn names(path: &Path) -> [PathBuf; 3] {
    [
        path.to_path_buf(),
        mortar_image::output::temp_name(path),
        mortar_image::output::aside_name(path),
    ]
}

/// What a name is, for the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    /// A regular file, following symbolic links: its device and inode.
    File(u64, u64),
    /// Nothing there yet: its directory's identity (when it has one) and
    /// its name, else the path itself.
    Absent(Option<(u64, u64)>, OsString, PathBuf),
    /// A directory, a device, a FIFO or a socket: outside the rule.
    Special,
}

fn ident(path: &Path, absent_key: bool) -> Ident {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Ident::File(m.dev(), m.ino()),
        Ok(_) => Ident::Special,
        Err(_) if absent_key => {
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let dir_id = std::fs::metadata(dir).ok().map(|m| (m.dev(), m.ino()));
            let name = path.file_name().map(OsStr::to_os_string).unwrap_or_default();
            Ident::Absent(dir_id, name, path.to_path_buf())
        }
        Err(_) => Ident::Absent(None, OsString::new(), path.to_path_buf()),
    }
}

/// Whether two names are one file for the rule.
fn same(a: &Ident, b: &Ident) -> bool {
    match (a, b) {
        (Ident::File(..), Ident::File(..)) => a == b,
        (Ident::Absent(da, na, pa), Ident::Absent(db, nb, pb)) => match (da, db) {
            (Some(x), Some(y)) => x == y && na == nb,
            _ => pa == pb,
        },
        _ => false,
    }
}

/// The files the link read, by identity, for the rule: every file the
/// store read (objects, archives, thin members, scripts, shared objects:
/// `Store::inodes`, already known), the response files (identities taken
/// as they were read), and the scripts the command line names.
#[derive(Debug, Default)]
pub struct Reads {
    /// Sorted.
    store: Vec<(u64, u64)>,
    /// `(identity, role and path)`.
    named: Vec<((u64, u64), String)>,
}

impl Reads {
    /// The read set of `parsed`'s link, whose store read `store` (sorted,
    /// `Store::inodes`).
    #[must_use]
    pub fn new(parsed: &Parsed, cwd: &Path, store: Vec<(u64, u64)>) -> Self {
        let mut named = Vec::new();
        for r in &parsed.response_files {
            named.push(((r.dev, r.ino), format!("the response file {}", r.path.display())));
        }
        for s in &parsed.plan.scripts {
            let Ok(m) = std::fs::metadata(cwd.join(&s.path)) else { continue };
            let what = match s.role {
                ScriptRole::Script | ScriptRole::DefaultScript => "the linker script",
                ScriptRole::VersionScript => "the version script",
                ScriptRole::DynamicList => "the dynamic list",
                ScriptRole::ExportDynamicSymbolList => "the export list",
                ScriptRole::RetainSymbolsFile => "the retain-symbols file",
            };
            named.push(((m.dev(), m.ino()), format!("{what} {}", s.path.to_string_lossy())));
        }
        Self { store, named }
    }

    /// Every identity read, sorted (the output's checks: image.md I-62,
    /// I-66, I-113).
    #[must_use]
    pub fn identities(&self) -> Vec<(u64, u64)> {
        let mut v = self.store.clone();
        v.extend(self.named.iter().map(|n| n.0));
        v.sort_unstable();
        v.dedup();
        v
    }

    /// What the link read as `id`, with its role and path, if it read it.
    /// `opened` names the files the store read (looked up only on a
    /// clash).
    fn find(&self, id: (u64, u64), opened: &[Opened<'_>], cwd: &Path) -> Option<String> {
        if let Some((_, what)) = self.named.iter().find(|n| n.0 == id) {
            return Some(what.clone());
        }
        self.store.binary_search(&id).ok()?;
        let found = opened.iter().find(|o| {
            let p = mortar_input::search::fs_path(cwd, o.path);
            std::fs::metadata(p).is_ok_and(|m| (m.dev(), m.ino()) == id)
        });
        Some(found.map_or_else(
            || "an input of the link".to_owned(),
            |o| {
                let what = match o.kind {
                    OpenedKind::Archive(_) => "the archive",
                    OpenedKind::Shared(_) | OpenedKind::SharedNotLoaded(_) => "the shared object",
                    OpenedKind::Script => "the linker script",
                    OpenedKind::Object(_) | OpenedKind::Empty | OpenedKind::Ignored => "the input",
                };
                format!("{what} {}", OsStr::from_bytes(o.path).to_string_lossy())
            },
        ))
    }
}

/// The rule (`spec/cli.md` §3.14) over `written` and `reads`: one
/// `written-file-clash` per written file that names a file read (unless it
/// is the output, I-62) or an earlier written file. Emitted to
/// `diagnostics`; `true` when there is none.
pub fn clear(
    written: &[Written],
    reads: &Reads,
    opened: &[Opened<'_>],
    cwd: &Path,
    diagnostics: &Collector,
) -> bool {
    let several = written.len() > 1;
    // A file outside the rule is written in place (I-58, I-79a): its
    // temporary names are never made.
    let idents: Vec<[Ident; 3]> = written
        .iter()
        .map(|w| {
            let main = ident(&w.path, several);
            if main == Ident::Special {
                return [Ident::Special, Ident::Special, Ident::Special];
            }
            let n = names(&w.path);
            [main, ident(&n[1], several), ident(&n[2], several)]
        })
        .collect();
    let mut ok = true;
    for (i, w) in written.iter().enumerate() {
        let own = names(&w.given);
        let mut clash = None;
        for (k, id) in idents[i].iter().enumerate() {
            let Ident::File(dev, ino) = *id else { continue };
            if k == 0 && w.output {
                continue;
            }
            if let Some(what) = reads.find((dev, ino), opened, cwd) {
                clash = Some((k, what));
                break;
            }
        }
        if clash.is_none() {
            'earlier: for (j, earlier) in written.iter().enumerate().take(i) {
                for (k, id) in idents[i].iter().enumerate() {
                    for (l, other) in idents[j].iter().enumerate() {
                        if same(id, other) {
                            let what = if l == 0 {
                                earlier.role.clone()
                            } else {
                                format!(
                                    "the temporary name {} of {}",
                                    names(&earlier.given)[l].display(),
                                    earlier.role
                                )
                            };
                            clash = Some((k, what));
                            break 'earlier;
                        }
                    }
                }
            }
        }
        if let Some((k, what)) = clash {
            let written = if k == 0 {
                w.spelling.clone()
            } else {
                format!("the temporary name {} of {}", own[k].display(), w.spelling)
            };
            let d = Diagnostic::error(Kind::WrittenFileClash { written, what });
            diagnostics.emit(Key::new(PHASE, 0, i as u64), d);
            ok = false;
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Parsed {
        mortar_cli::parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    // spec: cli.md §3.14 (one registry: every honored row of WRITES is in
    // the facade's check, so an option that becomes honored is checked
    // without being remembered here)
    #[test]
    fn every_honored_written_row_is_in_the_registry() {
        let cwd = Path::new("/w");
        for row in mortar_cli::rows() {
            let Some(role) = row.writes() else { continue };
            if row.status == Status::Refused {
                continue;
            }
            let name = row.name();
            let p = parsed(&["a.o", &format!("{name}=x")]);
            let incremental: Vec<(&'static str, &Path)> = match name.as_str() {
                "--incremental-state" => vec![("--incremental-state", Path::new("/w/x"))],
                "--incremental-report" => vec![("--incremental-report", Path::new("/w/x"))],
                _ => Vec::new(),
            };
            let r = registry(&p, cwd, Path::new("/w/x"), &incremental);
            assert!(
                r.iter().any(|w| w.path == Path::new("/w/x") && w.role.starts_with(role)),
                "{name} is honored but not in the registry: {r:?}"
            );
        }
    }

    // spec: cli.md §3.14 (the last use of a row is the file written; the
    // output is `a.out` without -o)
    #[test]
    fn the_registry_takes_the_last_use_and_the_default_output() {
        let cwd = Path::new("/w");
        let p = parsed(&["a.o", "-Map=m1", "-Map", "m2", "--dependency-file=d"]);
        let r = registry(&p, cwd, Path::new("/w/a.out"), &[]);
        let spellings: Vec<&str> = r.iter().map(|w| w.spelling.as_str()).collect();
        assert_eq!(
            spellings,
            ["the default output a.out", "-Map=m2", "--dependency-file=d"]
        );
        assert!(r[0].output && !r[1].output);
    }
}
