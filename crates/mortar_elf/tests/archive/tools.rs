//! L1 (`docs/VALIDATION.md`): the reader agrees with `ar` and `nm`, and
//! with `llvm-ar` and `llvm-nm`, on the system's archives and on archives
//! built here. `spec/archive.md` §9 says what is compared; §10 lists
//! where the tools differ from each other, which
//! `s10_where_the_tools_differ` pins.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use mortar_elf::archive::{Archive, Content, Error, IndexKind, Kind, Member, Special};
use mortar_testutil::{DEFAULT_TIMEOUT, Outcome, Scratch, run, run_with, tool, tools_required};

use crate::build::{self, REGULAR, THIN, concat, gnu_index, header, header_raw, member};

/// An archiver and the `nm` that reads its archives.
struct Tools {
    ar: PathBuf,
    nm: PathBuf,
    /// The line `nm --print-armap` puts before the index.
    index_heading: &'static [u8],
}

fn gnu() -> Option<Tools> {
    Some(Tools { ar: tool("ar")?, nm: tool("nm")?, index_heading: b"Archive index:" })
}

fn llvm() -> Option<Tools> {
    Some(Tools { ar: tool("llvm-ar")?, nm: tool("llvm-nm")?, index_heading: b"Archive map" })
}

/// What was compared.
#[derive(Default, Debug)]
struct Totals {
    archives: u64,
    members: u64,
    symbols: u64,
    bytes: u64,
}

/// Runs `program args <archive>` in the archive's directory, where a thin
/// archive's paths are the paths the tools print.
fn on_archive(program: &Path, args: &[&str], archive: &Path) -> Outcome {
    let mut command = Command::new(program);
    command.args(args).arg(archive.file_name().expect("an archive file"));
    command.current_dir(archive.parent().expect("an archive in a directory"));
    let outcome = run_with(&mut command, &[], DEFAULT_TIMEOUT);
    assert!(
        !outcome.timed_out(),
        "{} {args:?} {}: timed out",
        program.display(),
        archive.display()
    );
    outcome
}

fn lines(output: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = output.split(|&b| b == b'\n').collect();
    assert_eq!(lines.pop(), Some(b"".as_slice()), "output ends with a newline");
    lines
}

fn show(bytes: &[u8]) -> String {
    bytes.escape_ascii().to_string()
}

/// A member as the tools show it: for a member of a nested archive
/// (§7.3) that is the member the thin archive refers to.
struct Shown {
    name: Vec<u8>,
    data: Vec<u8>,
}

fn resolve(directory: &Path, member: &Member<'_>) -> Shown {
    let file = |path: &[u8]| {
        let path = directory.join(OsStr::from_bytes(path));
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    };
    match member.content() {
        Content::Inline { data, .. } => Shown { name: member.name().to_vec(), data: data.to_vec() },
        Content::External { path } => Shown { name: path.to_vec(), data: file(path) },
        Content::ExternalMember { archive_path, header_offset } => {
            let bytes = file(archive_path);
            let nested = Archive::parse(&bytes).expect("the nested archive");
            let inner = nested.member_at(header_offset).expect("the nested member");
            let data = inner.data().expect("a regular nested archive").to_vec();
            Shown { name: inner.name().to_vec(), data }
        }
    }
}

/// Compares the reader with `tools` on the archive at `path`: the member
/// list with `ar t`, the sizes with `ar tv`, the content offsets with
/// `ar tO`, the contents with `ar p`, the index with `nm --print-armap`.
fn check(path: &Path, tools: &Tools, totals: &mut Totals) {
    let at = path.display();
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {at}: {e}"));
    let archive = Archive::parse(&bytes).unwrap_or_else(|e| panic!("{at}: {e}"));
    let members: Vec<Member<'_>> =
        archive.members().collect::<Result<_, _>>().unwrap_or_else(|e| panic!("{at}: {e}"));
    let directory = path.parent().expect("an archive in a directory");
    let shown: Vec<Shown> = members.iter().map(|member| resolve(directory, member)).collect();

    // Names, order and count.
    let listing = on_archive(&tools.ar, &["t"], path);
    assert!(listing.success(), "{at}: ar t: {}", listing.stderr_text());
    let listed = lines(&listing.stdout);
    let names: Vec<&[u8]> = shown.iter().map(|member| member.name.as_slice()).collect();
    assert_eq!(listed.len(), names.len(), "{at}: member count");
    for (position, (listed, name)) in listed.iter().zip(&names).enumerate() {
        assert_eq!(show(listed), show(name), "{at}: name of member {position}");
    }

    // Sizes: the third column of `ar tv`.
    let verbose = on_archive(&tools.ar, &["tv"], path);
    assert!(verbose.success(), "{at}: ar tv: {}", verbose.stderr_text());
    let verbose = lines(&verbose.stdout);
    assert_eq!(verbose.len(), members.len(), "{at}: ar tv lines");
    for ((line, member), shown) in verbose.iter().zip(&members).zip(&shown) {
        let size = line.split(|&b| b == b' ').filter(|column| !column.is_empty()).nth(2);
        let size = size.map(show).and_then(|size| size.parse::<u64>().ok());
        assert_eq!(size, Some(member.size()), "{at}: size in `{}`", show(line));
        assert!(line.ends_with(&shown.name), "{at}: name in `{}`", show(line));
        assert_eq!(shown.data.len() as u64, member.size(), "{at}: {}", show(&shown.name));
    }

    // Where the content starts: `<name> 0x<offset>` per member.
    if archive.kind() == Kind::Regular {
        let offsets = on_archive(&tools.ar, &["tO"], path);
        assert!(offsets.success(), "{at}: ar tO: {}", offsets.stderr_text());
        let offsets = lines(&offsets.stdout);
        assert_eq!(offsets.len(), members.len(), "{at}: ar tO lines");
        for (line, member) in offsets.iter().zip(&members) {
            let offset = show(line).rsplit_once(" 0x").map(|(_, hex)| u64::from_str_radix(hex, 16));
            let Content::Inline { offset: start, .. } = member.content() else {
                panic!("{at}: a regular archive has inline members");
            };
            assert_eq!(offset, Some(Ok(start)), "{at}: offset in `{}`", show(line));
        }
    }

    // Contents: `ar p` prints every member in order.
    let printed = on_archive(&tools.ar, &["p"], path);
    assert!(printed.success(), "{at}: ar p: {}", printed.stderr_text());
    let contents: Vec<u8> = shown.iter().flat_map(|member| member.data.iter().copied()).collect();
    assert!(printed.stdout == contents, "{at}: `ar p` prints other bytes than the members hold");

    // The index: `<symbol> in <member>` per entry, in order, after the
    // heading and up to the first empty line. The status is not checked:
    // `nm` fails on a member that is not an object and prints the index.
    let by_offset: HashMap<u64, usize> =
        members.iter().enumerate().map(|(position, m)| (m.header_offset(), position)).collect();
    let armap = on_archive(&tools.nm, &["--print-armap"], path);
    let output = lines(&armap.stdout);
    let heading = output.iter().position(|line| *line == tools.index_heading);
    let entries: Vec<&[u8]> = heading.map_or(Vec::new(), |heading| {
        output[heading + 1..].iter().take_while(|line| !line.is_empty()).copied().collect()
    });
    let mut read = 0;
    if let Some(index) = archive.symbol_index() {
        assert_eq!(entries.len(), index.len(), "{at}: index entries");
        for (entry, symbol) in entries.iter().zip(index) {
            let symbol = symbol.unwrap_or_else(|e| panic!("{at}: {e}"));
            let defined_by =
                archive.member_at(symbol.member_offset).unwrap_or_else(|e| panic!("{at}: {e}"));
            let position = by_offset[&symbol.member_offset];
            assert_eq!(defined_by, members[position], "{at}: member of `{}`", show(symbol.name));
            let expected = concat(&[symbol.name, b" in ", &shown[position].name]);
            assert_eq!(show(entry), show(&expected), "{at}: index entry {read}");
            read += 1;
        }
    } else {
        assert!(entries.is_empty(), "{at}: the tools see an index and the reader does not");
    }

    totals.archives += 1;
    totals.members += members.len() as u64;
    totals.symbols += read;
    totals.bytes += contents.len() as u64;
}

// The system's archives.

/// Every `*.a` under `/usr/lib`, to the depth the toolchains install at,
/// sorted.
fn system_archives() -> Vec<PathBuf> {
    fn visit(directory: &Path, depth: u32, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < 3 {
                    visit(&path, depth + 1, found);
                }
            } else if path.extension() == Some(OsStr::new("a")) && path.is_file() {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(Path::new("/usr/lib"), 0, &mut found);
    found.sort();
    found
}

const SHARDS: usize = 4;

/// One of `SHARDS` slices of the system's archives, so that the tests run
/// side by side. A `*.a` that is not an archive (glibc installs a linker
/// script as `libm.a` and an object as `libmcheck.a`) is skipped.
fn system_shard(shard: usize) {
    let Some(tools) = gnu() else { return };
    let paths = system_archives();
    if shard == 0 && tools_required() {
        for required in ["libc.a", "libgcc.a", "libgcc_eh.a", "libstdc++.a"] {
            let present = paths.iter().any(|path| path.file_name() == Some(OsStr::new(required)));
            assert!(present, "no {required} under /usr/lib (MORTAR_REQUIRE_TOOLS=1)");
        }
    }
    let mut totals = Totals::default();
    let mut skipped = 0;
    for path in paths.iter().skip(shard).step_by(SHARDS) {
        let mut magic = [0; 8];
        let read = std::fs::File::open(path).and_then(|mut file| {
            use std::io::Read;
            file.read(&mut magic)
        });
        if !matches!(read, Ok(8)) || Archive::sniff(&magic).is_none() {
            skipped += 1;
            continue;
        }
        check(path, &tools, &mut totals);
    }
    eprintln!("system archives, shard {shard} of {SHARDS}: {totals:?}, {skipped} not archives");
}

#[test]
fn s9_system_archives_0() {
    system_shard(0);
}

#[test]
fn s9_system_archives_1() {
    system_shard(1);
}

#[test]
fn s9_system_archives_2() {
    system_shard(2);
}

#[test]
fn s9_system_archives_3() {
    system_shard(3);
}

// Archives built here.

/// The files the built archives hold, in a scratch directory: objects with
/// short, long and spaced names, in a subdirectory, and a file of odd size
/// that is not an object.
const INPUTS: [&str; 7] = [
    "a.o",
    "b.o",
    "sub/a_member_name_longer_than_the_field.o",
    "sp ace.o",
    "fifteen_chars.o",
    "sixteen_chars_.o",
    "odd.txt",
];

fn inputs() -> Option<Scratch> {
    let cc = tool("gcc")?;
    let scratch = Scratch::new("archive");
    std::fs::create_dir_all(scratch.join("sub")).expect("a subdirectory");
    std::fs::create_dir_all(scratch.join("out")).expect("a subdirectory");
    let sources = [
        ("a.o", "int alpha(void) { return 1; } int alpha2;"),
        ("b.o", "int beta(void) { return 2; }"),
        (INPUTS[2], "int gamma_fn(void) { return 3; } char odd[3] = {1, 2, 3};"),
    ];
    for (object, source) in sources {
        let mut compile = Command::new(&cc);
        compile.args(["-c", "-x", "c", "-", "-o"]).arg(scratch.join(object));
        let outcome = run_with(&mut compile, source.as_bytes(), DEFAULT_TIMEOUT);
        assert!(outcome.success(), "gcc: {}", outcome.stderr_text());
    }
    for copy in ["sp ace.o", "fifteen_chars.o", "sixteen_chars_.o"] {
        std::fs::copy(scratch.join("b.o"), scratch.join(copy)).expect("a copy");
    }
    scratch.write("odd.txt", "odd");
    Some(scratch)
}

/// Runs an archiver in the scratch directory and returns the archive.
fn make(scratch: &Scratch, ar: &Path, env: &[(&str, &str)], args: &[&str]) -> PathBuf {
    let mut command = Command::new(ar);
    command.args(args).current_dir(scratch.path());
    command.envs(env.iter().copied());
    let outcome = run_with(&mut command, &[], DEFAULT_TIMEOUT);
    assert!(outcome.success(), "{} {args:?}: {}", ar.display(), outcome.stderr_text());
    let is_archive = |arg: &&&str| Path::new(arg).extension() == Some(OsStr::new("a"));
    scratch.join(args.iter().find(is_archive).expect("an archive argument"))
}

fn with_inputs<'a>(front: &[&'a str]) -> Vec<&'a str> {
    front.iter().copied().chain(INPUTS).collect()
}

/// The kind, the index and the member contents of the archive at `path`.
fn forms(path: &Path) -> (Kind, Option<IndexKind>, Vec<String>) {
    let bytes = std::fs::read(path).expect("the archive");
    let archive = Archive::parse(&bytes).expect("an archive");
    let contents = archive
        .members()
        .map(|member| match member.expect("a member").content() {
            Content::Inline { .. } => "inline".to_owned(),
            Content::External { path } => format!("file {}", show(path)),
            Content::ExternalMember { archive_path, .. } => format!("in {}", show(archive_path)),
        })
        .collect();
    (archive.kind(), archive.symbol_index().map(|index| index.kind()), contents)
}

#[test]
fn s9_archives_built_by_gnu_ar() {
    let (Some(gnu), Some(scratch)) = (gnu(), inputs()) else { return };
    let mut totals = Totals::default();
    let ar = gnu.ar.clone();
    let make = |args: &[&str]| make(&scratch, &ar, &[], args);

    let regular = make(&with_inputs(&["rcs", "gnu.a"]));
    check(&regular, &gnu, &mut totals);
    let (kind, index, contents) = forms(&regular);
    assert_eq!((kind, index), (Kind::Regular, Some(IndexKind::Gnu)));
    assert_eq!(contents.len(), INPUTS.len());

    let no_index = make(&with_inputs(&["rcS", "no-index.a"]));
    check(&no_index, &gnu, &mut totals);
    assert_eq!(forms(&no_index).1, None);

    let empty = make(&["rcs", "empty.a"]);
    check(&empty, &gnu, &mut totals);
    assert_eq!(std::fs::read(&empty).expect("the archive"), REGULAR);

    // No objects: `s` writes no index.
    let no_symbols = make(&["rcs", "no-symbols.a", "odd.txt"]);
    check(&no_symbols, &gnu, &mut totals);
    assert_eq!(forms(&no_symbols).1, None);

    // The same member name three times: a plain `q` appends (with `s` or
    // `S` it replaces, like `r`).
    make(&["rc", "duplicates.a", "a.o", "b.o"]);
    make(&["q", "duplicates.a", "a.o"]);
    let duplicates = make(&["q", "duplicates.a", "a.o"]);
    check(&duplicates, &gnu, &mut totals);
    assert_eq!(forms(&duplicates).2.len(), 4);

    // Thin: paths relative to the archive's directory, and an absolute one.
    let thin = make(&with_inputs(&["rcsT", "thin.a"]));
    check(&thin, &gnu, &mut totals);
    let (kind, index, contents) = forms(&thin);
    assert_eq!((kind, index), (Kind::Thin, Some(IndexKind::Gnu)));
    assert_eq!(contents[2], format!("file {}", INPUTS[2]));
    let absolute = scratch.join("b.o");
    let absolute = absolute.to_str().expect("a UTF-8 scratch path");
    let elsewhere = make(&["rcsT", "out/elsewhere.a", "a.o", INPUTS[2], absolute, "odd.txt"]);
    check(&elsewhere, &gnu, &mut totals);
    let contents = forms(&elsewhere).2;
    assert_eq!(contents[0], "file ../a.o");
    assert_eq!(contents[2], format!("file {absolute}"));

    // Nested (§7.3): a regular archive in a thin one is referred to member
    // by member, a thin archive in a thin one is merged into it.
    make(&["rcs", "inner.a", "a.o", "b.o", "odd.txt"]);
    make(&["rcsT", "inner-thin.a", "b.o", INPUTS[2]]);
    let nested = make(&["rcsT", "nested.a", "inner.a", "inner-thin.a", "sp ace.o"]);
    check(&nested, &gnu, &mut totals);
    let expected = [
        "in inner.a",
        "in inner.a",
        "in inner.a",
        "file b.o",
        "file sub/a_member_name_longer_than_the_field.o",
        "file sp ace.o",
    ];
    assert_eq!(forms(&nested).2, expected);
    // A thin archive in a regular one is a member like any other.
    let holds_thin = make(&["rcs", "holds-thin.a", "inner-thin.a", "a.o"]);
    check(&holds_thin, &gnu, &mut totals);
    assert_eq!(forms(&holds_thin).2, ["inline", "inline"]);

    // llvm-ar reads what GNU ar wrote, except the nested form (§10).
    if let Some(llvm) = llvm() {
        for archive in [&regular, &no_index, &empty, &duplicates, &thin, &elsewhere, &holds_thin] {
            check(archive, &llvm, &mut totals);
        }
    }
    eprintln!("built by GNU ar: {totals:?}");
}

#[test]
fn s9_archives_built_by_llvm_ar() {
    let (Some(llvm), Some(scratch)) = (llvm(), inputs()) else { return };
    let mut totals = Totals::default();
    let ar = llvm.ar.clone();
    let make = |args: &[&str]| make(&scratch, &ar, &[], args);
    // LLVM's archive writer takes the offset from which it writes the
    // 64-bit index from this variable, for tests like this one.
    let make_sym64 = |args: &[&str]| self::make(&scratch, &ar, &[("SYM64_THRESHOLD", "0")], args);
    let mut both = Vec::new();

    let regular = make(&with_inputs(&["--format=gnu", "rcs", "gnu.a"]));
    assert_eq!(forms(&regular).1, Some(IndexKind::Gnu));
    both.push(regular);
    both.push(make(&with_inputs(&["--format=gnu", "rcS", "no-index.a"])));
    let thin = make(&with_inputs(&["rcsT", "thin.a"]));
    assert_eq!(forms(&thin).0, Kind::Thin);
    both.push(thin);
    let sym64 = make_sym64(&with_inputs(&["--format=gnu", "rcs", "sym64.a"]));
    assert_eq!(forms(&sym64).1, Some(IndexKind::Gnu64), "SYM64_THRESHOLD was not honoured");
    both.push(sym64);

    // A regular archive in a thin one is stored whole, as one member
    // (§10); a thin one is merged.
    make(&["rcs", "inner.a", "a.o", "b.o"]);
    make(&["rcsT", "inner-thin.a", "b.o", INPUTS[2]]);
    let nested = make(&["rcsT", "nested.a", "inner.a", "inner-thin.a", "sp ace.o"]);
    let expected = [
        "file inner.a",
        "file b.o",
        "file sub/a_member_name_longer_than_the_field.o",
        "file sp ace.o",
    ];
    assert_eq!(forms(&nested).2, expected);
    both.push(nested);

    let gnu = gnu();
    for archive in &both {
        check(archive, &llvm, &mut totals);
        if let Some(gnu) = &gnu {
            check(archive, gnu, &mut totals);
        }
    }

    // BSD names and the BSD index: GNU ar takes the index for a member
    // (§10), so LLVM's tools alone are the oracle.
    let bsd = make(&with_inputs(&["--format=bsd", "rcs", "bsd.a"]));
    assert_eq!(forms(&bsd).1, Some(IndexKind::Bsd));
    check(&bsd, &llvm, &mut totals);
    let bsd_no_index = make(&with_inputs(&["--format=bsd", "rcS", "bsd-no-index.a"]));
    assert_eq!(forms(&bsd_no_index).1, None);
    check(&bsd_no_index, &llvm, &mut totals);
    let darwin = make(&with_inputs(&["--format=darwin", "rcs", "darwin.a"]));
    assert_eq!(forms(&darwin).1, Some(IndexKind::Bsd));
    check(&darwin, &llvm, &mut totals);
    // BSD names under a 64-bit GNU index is what `--format=bsd` gives past
    // the threshold.
    let bsd_sym64 = make_sym64(&with_inputs(&["--format=bsd", "rcs", "bsd-sym64.a"]));
    assert_eq!(forms(&bsd_sym64).1, Some(IndexKind::Gnu64), "SYM64_THRESHOLD was not honoured");
    check(&bsd_sym64, &llvm, &mut totals);
    eprintln!("built by llvm-ar: {totals:?}");
}

/// The builder of the hand-built fixtures writes what the tools read: the
/// unit tests and the hostile-input seeds stand on it.
#[test]
fn s9_hand_built_fixtures_are_read_alike_by_the_tools() {
    let (Some(gnu), Some(llvm)) = (gnu(), llvm()) else { return };
    let scratch = Scratch::new("archive-fixtures");
    let mut totals = Totals::default();
    for (label, fixture) in build::seeds() {
        // The thin seed names one file twice with two sizes: no directory
        // can hold it.
        if label == "thin" {
            continue;
        }
        let path = scratch.write(&format!("{label}.a"), &fixture.bytes);
        check(&path, &llvm, &mut totals);
        if label.starts_with("gnu") {
            check(&path, &gnu, &mut totals);
        }
    }
    // A thin fixture of files that exist, with a short name among them.
    scratch.write("one.txt", "1");
    scratch.write("two.txt", "22");
    scratch.write("three.txt", "333");
    let thin = concat(&[
        THIN,
        &member(b"/", &gnu_index(&[], false)),
        &member(b"//", b"one.txt/\ntwo.txt/\n"),
        &header(b"/0", 1),
        &header(b"three.txt/", 3),
        &header(b"/9", 2),
    ]);
    let path = scratch.write("thin.a", &thin);
    check(&path, &llvm, &mut totals);
    check(&path, &gnu, &mut totals);
    eprintln!("hand-built: {totals:?}");
}

// Where the tools differ (§10).

/// An input, and what GNU `ar t`, `llvm-ar t` and the reader make of it:
/// the names of the members, or a refusal.
struct Difference {
    what: &'static str,
    bytes: Vec<u8>,
    gnu: Option<Vec<&'static str>>,
    llvm: Option<Vec<&'static str>>,
    mortar: Result<Vec<&'static str>, Error>,
}

/// What a tool lists; `None` when it fails.
fn listing(ar: &Path, archive: &Path) -> Option<Vec<String>> {
    let outcome = on_archive(ar, &["t"], archive);
    outcome.success().then(|| lines(&outcome.stdout).into_iter().map(show).collect())
}

/// What the reader lists, or its first error.
fn reading(bytes: &[u8]) -> Result<Vec<String>, Error> {
    Archive::parse(bytes)?.members().map(|member| member.map(|m| show(m.name()))).collect()
}

fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|&name| name.to_owned()).collect()
}

/// Sizes, padding and the order of special members.
fn layout_differences() -> Vec<Difference> {
    let index = Special::Index(IndexKind::Gnu);
    vec![
        Difference {
            what: "the last member's padding is missing",
            bytes: concat(&[REGULAR, &header(b"x.txt/", 3), b"abc"]),
            gnu: Some(vec!["x.txt"]),
            llvm: None,
            mortar: Ok(vec!["x.txt"]),
        },
        Difference {
            what: "bytes after the last member",
            bytes: concat(&[REGULAR, &member(b"x.txt/", b"abcd"), b"zzz"]),
            gnu: Some(vec!["x.txt"]),
            llvm: None,
            mortar: Err(Error::TruncatedHeader { offset: 72, available: 3 }),
        },
        Difference {
            what: "a size with leading spaces",
            bytes: concat(&[REGULAR, &header_raw(b"x.txt/", b"         4"), b"abcd"]),
            gnu: Some(vec!["x.txt"]),
            llvm: None,
            mortar: Err(Error::BadSize { offset: 8, field: *b"         4" }),
        },
        Difference {
            what: "a size with a sign",
            bytes: concat(&[REGULAR, &header_raw(b"x.txt/", b"+4"), b"abcd"]),
            gnu: Some(vec!["x.txt"]),
            llvm: None,
            mortar: Err(Error::BadSize { offset: 8, field: *b"+4        " }),
        },
        Difference {
            what: "a symbol index after a member",
            bytes: concat(&[REGULAR, &member(b"x/", b"abcd"), &member(b"/", &[0; 4])]),
            gnu: Some(vec!["x", ""]),
            llvm: Some(vec!["x", "/"]),
            mortar: Err(Error::MisplacedSpecialMember { offset: 72, special: index }),
        },
        Difference {
            what: "a second symbol index (COFF)",
            bytes: concat(&[
                REGULAR,
                &member(b"/", &[0; 4]),
                &member(b"/", &[0; 4]),
                &member(b"x/", b""),
            ]),
            gnu: Some(vec!["x"]),
            llvm: Some(vec!["x"]),
            mortar: Err(Error::CoffArchive { offset: 72 }),
        },
        Difference {
            what: "a BSD index named in the name field",
            bytes: concat(&[REGULAR, &member(b"__.SYMDEF", &[0; 8]), &member(b"x.o", b"abcd")]),
            gnu: Some(vec!["x.o"]),
            llvm: Some(vec!["__.SYMDEF", "x.o"]),
            mortar: Ok(vec!["x.o"]),
        },
        Difference {
            what: "a BSD index named by a BSD name, as llvm-ar writes it",
            bytes: concat(&[
                REGULAR,
                &build::bsd_member(b"__.SYMDEF", &[0; 8]),
                &member(b"x.o", b"abcd"),
            ]),
            gnu: Some(vec!["__.SYMDEF", "x.o"]),
            llvm: Some(vec!["x.o"]),
            mortar: Ok(vec!["x.o"]),
        },
    ]
}

/// Names.
fn name_differences() -> Vec<Difference> {
    let long = b"a_name_of_twenty_two.o";
    vec![
        Difference {
            what: "a space in a name without a terminating slash",
            bytes: concat(&[REGULAR, &member(b"y z", b"abcd")]),
            gnu: Some(vec!["y"]),
            llvm: Some(vec!["y z"]),
            mortar: Err(Error::AmbiguousName { offset: 8, field: *b"y z             " }),
        },
        Difference {
            what: "a NUL in a name",
            bytes: concat(&[REGULAR, &member(b"ab\0cd/", b"abcd")]),
            gnu: Some(vec!["ab"]),
            llvm: Some(vec!["ab\\x00cd"]),
            mortar: Err(Error::NulInName { offset: 8, field: *b"ab\0cd/          " }),
        },
        Difference {
            what: "a long-name reference without a table",
            bytes: concat(&[REGULAR, &member(b"/0", b"abcd")]),
            gnu: Some(vec![""]),
            llvm: None,
            mortar: Err(Error::NoLongNameTable { offset: 8, name_offset: 0 }),
        },
        Difference {
            what: "a long name without a newline",
            bytes: concat(&[
                REGULAR,
                &member(b"//", &concat(&[long, b"/"])),
                &member(b"/0", b"abcd"),
            ]),
            gnu: Some(vec!["a_name_of_twenty_two.o/"]),
            llvm: None,
            mortar: Err(Error::LongNameUnterminated { offset: 92, name_offset: 0 }),
        },
        Difference {
            what: "a long name without a slash",
            bytes: concat(&[
                REGULAR,
                &member(b"//", &concat(&[long, b"\n\n"])),
                &member(b"/0", b"abcd"),
            ]),
            gnu: Some(vec!["a_name_of_twenty_two.o"]),
            llvm: None,
            mortar: Err(Error::LongNameUnterminated { offset: 92, name_offset: 0 }),
        },
    ]
}

/// §10: inputs that GNU `ar` and `llvm-ar` read differently, or that both
/// read and mortar refuses. Pinned so that the table in the spec cannot go
/// stale unnoticed (binutils 2.44; LLVM 19.1 and 22.1).
#[test]
fn s10_where_the_tools_differ() {
    let (Some(gnu), Some(llvm)) = (gnu(), llvm()) else { return };
    let scratch = Scratch::new("archive-differences");
    for case in layout_differences().into_iter().chain(name_differences()) {
        let what = case.what;
        let path = scratch.write("case.a", &case.bytes);
        assert_eq!(listing(&gnu.ar, &path), case.gnu.as_deref().map(owned), "GNU ar: {what}");
        assert_eq!(listing(&llvm.ar, &path), case.llvm.as_deref().map(owned), "llvm-ar: {what}");
        assert_eq!(
            reading(&case.bytes),
            case.mortar.as_deref().map(owned).map_err(|e| *e),
            "{what}"
        );
    }
}

/// §10, in what the tools write.
#[test]
fn s10_what_the_tools_write_differently() {
    let (Some(gnu), Some(llvm), Some(inputs)) = (gnu(), llvm(), inputs()) else { return };
    let field_of = |archive: &Path, position: usize| {
        let bytes = std::fs::read(archive).expect("the archive");
        let members = Archive::parse(&bytes).expect("an archive").members();
        let offset = members.map(|m| m.expect("a member").header_offset()).nth(position);
        let offset = offset.expect("a member at the position") as usize;
        show(&bytes[offset..offset + 16])
    };

    // A regular archive in a thin one: GNU ar refers to its members (§7.3),
    // which llvm-ar refuses by name; llvm-ar stores the archive whole.
    make(&inputs, &gnu.ar, &[], &["rcs", "inner.a", "a.o", "b.o"]);
    let nested = make(&inputs, &gnu.ar, &[], &["rcsT", "nested.a", "inner.a"]);
    assert_eq!(listing(&gnu.ar, &nested), Some(owned(&["a.o", "b.o"])));
    let refused = on_archive(&llvm.ar, &["t"], &nested);
    assert!(!refused.success());
    assert!(refused.stderr_text().contains("long name offset characters after the '/'"));
    let read = reading(&std::fs::read(&nested).expect("the archive"));
    assert_eq!(read, Ok(owned(&["inner.a", "inner.a"])));
    let whole = make(&inputs, &llvm.ar, &[], &["rcsT", "llvm-nested.a", "inner.a"]);
    assert_eq!(listing(&gnu.ar, &whole), Some(owned(&["inner.a"])));
    assert_eq!(field_of(&whole, 0), "/0              ");

    // A thin member with a name of 15 bytes: GNU ar leaves a `/` at the end
    // of the name field (§4).
    let thin = make(&inputs, &gnu.ar, &[], &["rcsT", "gnu-thin.a", "a.o", "fifteen_chars.o"]);
    assert_eq!(field_of(&thin, 1), "/5             /");
    let thin = make(&inputs, &llvm.ar, &[], &["rcsT", "llvm-thin.a", "a.o", "fifteen_chars.o"]);
    assert_eq!(field_of(&thin, 1), "/5              ");

    // What they write alike: a name of 15 bytes is the longest short name,
    // and the long-name table's size counts the newline that makes it even
    // (39 bytes of entry here).
    let table_of = |ar: &Path, name: &str| {
        let archive = make(&inputs, ar, &[], &["rcS", name, "fifteen_chars.o", INPUTS[2]]);
        let bytes = std::fs::read(&archive).expect("the archive");
        assert_eq!(show(&bytes[8..24]), "//              ");
        assert_eq!(field_of(&archive, 0), "fifteen_chars.o/");
        assert_eq!(field_of(&archive, 1), "/0              ");
        show(&bytes[8 + 48..8 + 58])
    };
    assert_eq!(table_of(&gnu.ar, "gnu-table.a"), "40        ");
    assert_eq!(table_of(&llvm.ar, "llvm-table.a"), "40        ");
}

/// A sample of members extracted with `ar x` holds the bytes the reader
/// gives.
#[test]
fn s9_extracted_members_hold_the_readers_bytes() {
    let Some(gnu) = gnu() else { return };
    let scratch = Scratch::new("archive-extract");
    let Some(libc) = system_archives().into_iter().find(|path| path.ends_with("libc.a")) else {
        assert!(!tools_required(), "no libc.a under /usr/lib (MORTAR_REQUIRE_TOOLS=1)");
        return;
    };
    let bytes = std::fs::read(&libc).expect("libc.a");
    let archive = Archive::parse(&bytes).expect("libc.a");
    // Every hundredth member: by name, so a name must be the only one.
    let members: Vec<Member<'_>> = archive.members().map(|m| m.expect("a member")).collect();
    let mut sample = Vec::new();
    for member in members.iter().step_by(100) {
        let name = std::str::from_utf8(member.name()).expect("a UTF-8 member name");
        if members.iter().filter(|other| other.name() == member.name()).count() == 1 {
            sample.push((name, member));
        }
    }
    assert!(sample.len() >= 5, "libc.a has {} members", members.len());
    let mut extract = Command::new(&gnu.ar);
    extract.arg("x").arg(&libc).args(sample.iter().map(|(name, _)| name));
    extract.current_dir(scratch.path());
    let outcome = run_with(&mut extract, &[], DEFAULT_TIMEOUT);
    assert!(outcome.success(), "ar x: {}", outcome.stderr_text());
    for (name, member) in sample {
        let extracted = std::fs::read(scratch.join(name)).expect("the extracted member");
        assert!(member.data() == Some(extracted.as_slice()), "libc.a: {name}");
    }
    // `run` is the plain form of the same oracle: `ar t` on the system's
    // archive lists as many members as the reader walks.
    let listed = run(&gnu.ar, [OsStr::new("t"), libc.as_os_str()]);
    assert_eq!(lines(&listed.stdout).len(), members.len());
}
