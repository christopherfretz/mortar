//! The workload manifest (`spec/harness.md` "bench", "Workloads"):
//! `bench/workloads/manifest`, one section per workload, saying how its
//! inputs are rebuilt from pinned sources and which link of the build is
//! the workload.

use std::fmt::Write as _;

use super::synth::{Request, Shape};

/// The manifest's path in the checkout.
pub const PATH: &str = "bench/workloads/manifest";

/// The sources of the `sources` workloads, in the checkout.
pub const SOURCES: &str = "bench/workloads/src";

/// How a workload's inputs are produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Sources of this checkout, compiled one by one, then linked by the
    /// driver.
    Sources { sources: Vec<String> },
    /// Sources of the generator (`synth`), compiled, archived for the
    /// `archives` shape, then linked by the driver.
    Synthetic(Request),
    /// A fetched source archive built with cmake's "Unix Makefiles"
    /// generator and make; the link of `target` is the workload.
    Cmake(Cmake),
    /// A package of this repository at a pinned revision built by cargo;
    /// the link of its binary is the workload.
    Cargo(Cargo),
    /// A Ninja build tree made by a pinned script of this checkout (hours
    /// of compiling: never run by the bench); the bench captures the link
    /// of `target` from it and keeps the link's inputs, not the tree.
    Ninja(Ninja),
    /// Specified, not built: named in the report with what it needs.
    Planned { needs: String },
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Sources { .. } => "sources",
            Self::Synthetic(_) => "synthetic",
            Self::Cmake(_) => "cmake",
            Self::Cargo(_) => "cargo",
            Self::Ninja(_) => "ninja",
            Self::Planned { .. } => "planned",
        }
    }
}

/// A `cmake` workload's build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmake {
    pub url: String,
    /// Lower-case hexadecimal SHA-256 of the archive.
    pub sha256: String,
    /// The directory of the archive that has the top `CMakeLists.txt`.
    pub root: String,
    /// `-D` options for the configuration, as words.
    pub options: Vec<String>,
    /// Built first, without capture (the libraries of the program).
    pub prebuild: Vec<String>,
    /// The make target whose link is captured.
    pub target: String,
}

/// A `cargo` workload's build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cargo {
    /// A commit of this repository (`git archive`).
    pub rev: String,
    pub package: String,
    pub bin: String,
    /// Arguments for rustc (`cargo rustc -- <these>`).
    pub rustc_args: Vec<String>,
}

/// A `ninja` workload's build (`spec/harness.md` "bench", "Large
/// workloads").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ninja {
    /// The script that makes the tree and its arguments after `<work>`,
    /// as words: a path of this checkout first.
    pub script: Vec<String>,
    /// The SHA-256 of the source archive the script unpacks: `$S` of a
    /// `run` line is `<work>/src/<first 16 digits>/<root>`.
    pub sha256: String,
    pub root: String,
    /// The directory under `<work>/workloads` that holds the build tree
    /// (`build/`) and the captured inputs (`inputs/`), shared by the
    /// workloads captured from one tree.
    pub tree: String,
    /// The Ninja target (an output path of the tree) whose link is the
    /// workload.
    pub target: String,
}

/// One section of the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workload {
    pub name: String,
    /// What the workload stands for: `small`, `medium`, `large`,
    /// `extreme`, `rust` or `calibration`.
    pub class: String,
    /// One sentence for the report.
    pub about: String,
    pub kind: Kind,
    /// The compiler driver of the build and of the captured link (`gcc`,
    /// `g++`): a name on `PATH`.
    pub driver: String,
    /// Compiler flags, as words.
    pub cflags: Vec<String>,
    /// Driver flags of the link, as words.
    pub ldflags: Vec<String>,
    /// `group = yes` (synthetic): the generated inputs after `main.o`
    /// between `-Wl,--start-group` and `-Wl,--end-group`.
    pub group: bool,
    /// The glob the basename of the captured link's output must match;
    /// exactly one recorded link matches.
    pub capture: String,
    /// `run`: the arguments the linked program is run with once, mortar's
    /// output and GNU ld's alike, to compare their standard output and
    /// exit status (`spec/harness.md` "bench", "Running the outputs");
    /// `None` when the section has no `run`. `$S` in an argument is the
    /// workload's source directory ([`Workload::run_args`]).
    pub run: Option<Vec<String>>,
    /// The section as written, without comments, blank lines and the `run`
    /// key (which changes nothing that is built): part of the key the
    /// prepared workload is cached under.
    pub text: String,
}

impl Workload {
    /// Built and measured (not `planned`, not the calibration workload).
    pub fn measured(&self) -> bool {
        !matches!(self.kind, Kind::Planned { .. }) && self.class != CALIBRATION
    }

    /// The arguments of `run` with `$S` replaced by `sources`, the
    /// directory the workload is built from (the unpacked archive's root
    /// for `cmake`, `bench/workloads/src` for `sources`).
    pub fn run_args(&self, sources: &str) -> Option<Vec<String>> {
        Some(self.run.as_ref()?.iter().map(|a| a.replace("$S", sources)).collect())
    }
}

/// The class of the workload the calibration link is made on.
pub const CALIBRATION: &str = "calibration";

/// The class of the workloads built and measured only when named
/// (`--workload`): their inputs are too large to keep in the cache.
pub const ON_REQUEST: &str = "on-request";

/// The classes a workload may have.
const CLASSES: [&str; 7] = ["small", "medium", "large", "extreme", "rust", CALIBRATION, ON_REQUEST];

/// The keys each kind takes, besides `class`, `about`, `kind`.
fn keys_of(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "sources" => &["sources", "driver", "cflags", "ldflags", "capture", "run"],
        "synthetic" => {
            &["shape", "params", "group", "driver", "cflags", "ldflags", "capture", "run"]
        }
        "cmake" => &[
            "url", "sha256", "root", "options", "prebuild", "target", "driver", "cflags",
            "capture", "run",
        ],
        "cargo" => &["rev", "package", "bin", "rustc_args", "capture", "run"],
        "ninja" => &["script", "sha256", "root", "tree", "target", "driver", "capture", "run"],
        "planned" => &["needs"],
        _ => return None,
    })
}

/// A key of a section: its line, the key, the value.
type Entry = (usize, String, String);

/// Reads the manifest's text. Every error names its line.
pub fn parse(text: &str) -> Result<Vec<Workload>, String> {
    let mut sections: Vec<(usize, String, Vec<Entry>)> = Vec::new();
    for (at, raw) in text.lines().enumerate().map(|(at, line)| (at + 1, line)) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            let valid = !name.is_empty()
                && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !valid {
                return Err(format!(
                    "line {at}: a workload name is lower-case letters, digits and `-`"
                ));
            }
            if sections.iter().any(|(_, other, _)| other == name) {
                return Err(format!("line {at}: workload `{name}` appears twice"));
            }
            sections.push((at, name.to_owned(), Vec::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {at}: expected `[name]` or `key = value`"));
        };
        let Some((_, _, entries)) = sections.last_mut() else {
            return Err(format!("line {at}: a key before the first `[name]`"));
        };
        let key = key.trim();
        if entries.iter().any(|(_, k, _)| k == key) {
            return Err(format!("line {at}: key `{key}` appears twice"));
        }
        entries.push((at, key.to_owned(), value.trim().to_owned()));
    }
    sections.into_iter().map(|(at, name, entries)| section(at, name, &entries)).collect()
}

#[allow(clippy::too_many_lines, reason = "one arm per kind, read top to bottom")]
fn section(at: usize, name: String, entries: &[Entry]) -> Result<Workload, String> {
    let get = |key: &str| entries.iter().find(|(_, k, _)| k == key).map(|(_, _, v)| v.as_str());
    let fail = |what: String| format!("line {at}: workload `{name}`: {what}");
    let need =
        |key: &str| get(key).filter(|v| !v.is_empty()).ok_or_else(|| fail(format!("no `{key}`")));
    let words = |key: &str| {
        get(key).map(|v| v.split_whitespace().map(str::to_owned).collect()).unwrap_or_default()
    };

    let sha256 = || {
        let sha256 = need("sha256")?.to_owned();
        if sha256.len() != 64
            || !sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(fail("`sha256` is 64 lower-case hexadecimal digits".into()));
        }
        Ok(sha256)
    };
    let kind_name = need("kind")?;
    let allowed = keys_of(kind_name).ok_or_else(|| fail(format!("unknown kind `{kind_name}`")))?;
    for (line, key, _) in entries {
        if !["class", "about", "kind"].contains(&key.as_str()) && !allowed.contains(&key.as_str()) {
            return Err(format!(
                "line {line}: workload `{name}`: kind `{kind_name}` takes no `{key}`"
            ));
        }
    }
    let class = need("class")?.to_owned();
    if !CLASSES.contains(&class.as_str()) {
        return Err(fail(format!("unknown class `{class}` (one of {})", CLASSES.join(", "))));
    }
    let kind = match kind_name {
        "sources" => Kind::Sources { sources: words("sources") },
        "synthetic" => {
            let shape = need("shape")?;
            let shape =
                Shape::parse(shape).ok_or_else(|| fail(format!("unknown shape `{shape}`")))?;
            let params: Vec<usize> = need("params")?
                .split_whitespace()
                .map(|n| n.parse().ok().filter(|n| *n > 0))
                .collect::<Option<_>>()
                .ok_or_else(|| fail("`params` are two numbers above 0".into()))?;
            let &[a, b] = params.as_slice() else {
                return Err(fail("`params` are two numbers above 0".into()));
            };
            Kind::Synthetic(Request { shape, params: (a, b) })
        }
        "cmake" => {
            let sha256 = sha256()?;
            Kind::Cmake(Cmake {
                url: need("url")?.to_owned(),
                sha256,
                root: need("root")?.to_owned(),
                options: words("options"),
                prebuild: words("prebuild"),
                target: need("target")?.to_owned(),
            })
        }
        "cargo" => Kind::Cargo(Cargo {
            rev: need("rev")?.to_owned(),
            package: need("package")?.to_owned(),
            bin: need("bin")?.to_owned(),
            rustc_args: words("rustc_args"),
        }),
        "ninja" => {
            let script: Vec<String> = words("script");
            if script.is_empty() {
                return Err(fail("no `script`".into()));
            }
            let tree = need("tree")?.to_owned();
            if !tree.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
                return Err(fail("`tree` is lower-case letters, digits and `-`".into()));
            }
            Kind::Ninja(Ninja {
                script,
                sha256: sha256()?,
                root: need("root")?.to_owned(),
                tree,
                target: need("target")?.to_owned(),
            })
        }
        _ => Kind::Planned { needs: need("needs")?.to_owned() },
    };
    let sources_missing = matches!(&kind, Kind::Sources { sources } if sources.is_empty());
    if sources_missing {
        return Err(fail("no `sources`".into()));
    }
    let planned = matches!(kind, Kind::Planned { .. });
    let driver = match &kind {
        Kind::Planned { .. } => String::new(),
        Kind::Cargo(_) => "cc".to_owned(),
        _ => need("driver")?.to_owned(),
    };
    let group = match get("group") {
        None | Some("no") => false,
        Some("yes") => true,
        Some(_) => return Err(fail("`group` is `yes` or `no`".into())),
    };
    let capture = get("capture").unwrap_or("out").to_owned();
    if !planned && (capture.is_empty() || capture.contains('/')) {
        return Err(fail("`capture` is a file name pattern".into()));
    }
    let mut text = format!("[{name}]\n");
    for (_, key, value) in entries.iter().filter(|(_, key, _)| key != "run") {
        let _ = writeln!(text, "{key} = {value}");
    }
    Ok(Workload {
        about: get("about").unwrap_or("").to_owned(),
        class,
        kind,
        driver,
        cflags: words("cflags"),
        ldflags: words("ldflags"),
        group,
        capture,
        run: get("run").map(|v| v.split_whitespace().map(str::to_owned).collect()),
        text,
        name,
    })
}

// spec/harness.md "bench", "Workloads".
#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
# a comment
[hello-c]
class = small
kind = sources
about = A hello world in C.
sources = hello.c
driver = gcc
cflags = -O2 -g
ldflags = -pie
run =

[synthetic-objects]
class = extreme
kind = synthetic
shape = objects
params = 10 20
driver = gcc

[cxx-medium]
class = medium
kind = cmake
url = https://example.org/p.tar.gz
sha256 = 4eab9b524aa5913c6fffb20b2a8abf5ef7f95a80bc0701f3a6dbb4c607f73460
root = p-1.0
options = -Dp_TESTS=OFF
prebuild = libp
target = prog
driver = g++
capture = prog*
run = --version -I$S/src

[rust-self]
class = rust
kind = cargo
rev = 0123abc
package = mortar
bin = mortar
rustc_args = -C save-temps
capture = mortar-*

[libcxx-test]
class = small
kind = planned
needs = kiln and hod

[big]
class = on-request
kind = ninja
script = bench/b.sh g
sha256 = 4eab9b524aa5913c6fffb20b2a8abf5ef7f95a80bc0701f3a6dbb4c607f73460
root = p-1.0
tree = big-g
target = bin/big
driver = clang++
capture = big
run = -cc1 $S/x.c
";

    // "Workloads": each kind and what it takes.
    #[test]
    fn the_manifest_is_read() {
        let workloads = parse(GOOD).unwrap();
        let names: Vec<_> = workloads.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            ["hello-c", "synthetic-objects", "cxx-medium", "rust-self", "libcxx-test", "big"]
        );
        let hello = &workloads[0];
        assert_eq!(hello.kind, Kind::Sources { sources: vec!["hello.c".into()] });
        assert_eq!(
            (hello.cflags.len(), hello.ldflags.as_slice()),
            (2, ["-pie".to_owned()].as_slice())
        );
        assert_eq!((hello.capture.as_str(), hello.about.as_str()), ("out", "A hello world in C."));
        assert!(hello.text.starts_with("[hello-c]\nclass = small\n") && !hello.text.contains('#'));
        // "Running the outputs": `run =` is a run without arguments, and
        // not part of what keys the build; no `run`, no run.
        assert_eq!(hello.run, Some(Vec::new()));
        assert!(!hello.text.contains("run"));
        assert_eq!(workloads[1].run, None);
        assert_eq!(
            workloads[2].run_args("/c/src/p-1.0"),
            Some(vec!["--version".to_owned(), "-I/c/src/p-1.0/src".to_owned()])
        );
        assert_eq!(workloads[1].run_args("/x"), None);
        assert_eq!(
            workloads[1].kind,
            Kind::Synthetic(Request { shape: Shape::Objects, params: (10, 20) })
        );
        let Kind::Cmake(cmake) = &workloads[2].kind else { panic!("{:?}", workloads[2]) };
        assert_eq!((cmake.root.as_str(), cmake.target.as_str()), ("p-1.0", "prog"));
        assert_eq!(cmake.prebuild, ["libp"]);
        let Kind::Cargo(cargo) = &workloads[3].kind else { panic!("{:?}", workloads[3]) };
        assert_eq!((cargo.rev.as_str(), cargo.rustc_args.len()), ("0123abc", 2));
        assert_eq!(workloads[3].driver, "cc");
        assert_eq!(workloads[4].kind, Kind::Planned { needs: "kiln and hod".into() });
        assert!(workloads[0].measured() && !workloads[4].measured());
        let Kind::Ninja(ninja) = &workloads[5].kind else { panic!("{:?}", workloads[5]) };
        assert_eq!(ninja.script, ["bench/b.sh", "g"]);
        assert_eq!((ninja.tree.as_str(), ninja.target.as_str()), ("big-g", "bin/big"));
        assert_eq!(
            (workloads[5].driver.as_str(), workloads[5].capture.as_str()),
            ("clang++", "big")
        );
        assert!(workloads[5].measured());
    }

    // "Workloads": what the reader refuses, with the line.
    #[test]
    fn malformed_manifests() {
        let broken = |from: &str, to: &str| {
            assert!(GOOD.contains(from), "{from}");
            parse(&GOOD.replacen(from, to, 1)).unwrap_err()
        };
        assert!(broken("[hello-c]", "[Hello]").contains("line 2: a workload name"));
        assert!(broken("[synthetic-objects]", "[hello-c]").contains("`hello-c` appears twice"));
        assert!(
            broken("driver = gcc\ncflags", "driver = gcc\ndriver = cc\ncflags")
                .contains("`driver` appears twice")
        );
        assert!(
            broken("class = small\nkind = sources", "class = small\nkind sources")
                .contains("expected")
        );
        assert!(parse("a = b\n").unwrap_err().contains("before the first"));
        assert!(broken("kind = sources", "kind = make").contains("unknown kind `make`"));
        assert!(
            broken("class = small\nkind = sources", "class = tiny\nkind = sources")
                .contains("unknown class")
        );
        assert!(broken("shape = objects", "shape = blobs").contains("unknown shape"));
        assert!(broken("params = 10 20", "params = 10").contains("two numbers"));
        assert!(broken("params = 10 20", "params = 0 20").contains("two numbers"));
        assert!(broken("sha256 = 4eab", "sha256 = 4EAB").contains("`sha256`"));
        assert!(broken("sources = hello.c\n", "").contains("no `sources`"));
        assert!(broken("target = prog\n", "").contains("no `target`"));
        assert!(
            broken("bin = mortar\n", "bin = mortar\ntarget = x\n")
                .contains("kind `cargo` takes no `target`")
        );
        assert!(broken("capture = prog*", "capture = a/b").contains("file name pattern"));
        assert!(
            broken("needs = kiln and hod", "needs = kiln and hod\nrun = x")
                .contains("kind `planned` takes no `run`")
        );
        assert!(broken("tree = big-g", "tree = ../x").contains("`tree`"));
        assert!(broken("params = 10 20", "params = 10 20\ngroup = maybe").contains("`group`"));
        assert!(
            parse(&GOOD.replace("params = 10 20", "params = 10 20\ngroup = yes")).unwrap()[1].group
        );
        assert!(broken("script = bench/b.sh g\n", "").contains("no `script`"));
        assert!(broken("target = bin/big\n", "").contains("no `target`"));
        assert!(
            broken("tree = big-g", "tree = big-g\noptions = -DX").contains("takes no `options`")
        );
    }
}
