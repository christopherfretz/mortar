//! `cargo xtask mason programs`: the hand-written programs of
//! `tests/mason/programs/` compiled by kiln + hod (and GCC, Clang, gfortran
//! for the mixed ones), linked from the same objects by GNU ld, lld, mold
//! and mortar, run, and compared; mortar's output goes through the
//! verifier. Contract: `tests/mason/README.md` "Programs".

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;

use super::{Mason, Plumbed, RUN_LIMITS, TOOL_LIMITS, census, run_in, tool, words};

/// The language of a source, from its name: `<x>.kiln.cpp`, `<x>.gcc.cpp`,
/// `<x>.clang.cpp`, `<x>.gfortran.f90`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lang {
    Kiln,
    Gxx,
    Clangxx,
    Gfortran,
}

impl Lang {
    fn of(name: &str) -> Option<Lang> {
        if name.ends_with(".kiln.cpp") {
            Some(Lang::Kiln)
        } else if name.ends_with(".gcc.cpp") {
            Some(Lang::Gxx)
        } else if name.ends_with(".clang.cpp") {
            Some(Lang::Clangxx)
        } else if name.ends_with(".gfortran.f90") {
            Some(Lang::Gfortran)
        } else {
            None
        }
    }
}

/// One program: a directory of sources and an optional `link` file
/// (`<driver> <extra link arguments...>`, the driver `kiln` or `g++`).
#[derive(Clone, Debug)]
pub struct Program {
    pub name: String,
    pub dir: PathBuf,
    pub sources: Vec<(PathBuf, Lang)>,
    /// The driver whose `-static -###` gives the link line.
    pub gnu_driver: bool,
    pub extra: Vec<String>,
}

impl Program {
    /// Whether libc++ variants apply: only kiln and Clang sources.
    fn libcxx_ok(&self) -> bool {
        self.sources.iter().all(|(_, l)| matches!(l, Lang::Kiln | Lang::Clangxx))
    }
}

/// The programs under `dir`, by name.
pub fn programs(dir: &Path) -> Result<Vec<Program>, String> {
    let read = |d: &Path| std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()));
    let mut out = Vec::new();
    for entry in read(dir)? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if !p.is_dir() {
            continue;
        }
        let mut sources = Vec::new();
        for f in read(&p)? {
            let f = f.map_err(|e| e.to_string())?.path();
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if let Some(lang) = Lang::of(name) {
                sources.push((f.clone(), lang));
            }
        }
        sources.sort();
        let link = std::fs::read_to_string(p.join("link")).unwrap_or_default();
        let mut w = link.split_whitespace().map(str::to_owned);
        let gnu_driver = match w.next().as_deref() {
            None | Some("kiln") => false,
            Some("g++") => true,
            Some(other) => return Err(format!("{}/link: unknown driver '{other}'", p.display())),
        };
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        out.push(Program { name, dir: p, sources, gnu_driver, extra: w.collect() });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// A build variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: &'static str,
    pub flags: &'static [&'static str],
    pub stdlib: Stdlib,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stdlib {
    /// GCC 14's libstdc++.a, the default of every driver here.
    Libstdcxx,
    /// libc++ 22's static archives as Debian ships them.
    Libcxx,
    /// The same archives with `.ARM.attributes` and `.deplibs` removed
    /// (`objcopy -R`): what mortar does once those two are accepted.
    LibcxxStripped,
}

pub const VARIANTS: &[Variant] = &[
    Variant { name: "O0", flags: &[], stdlib: Stdlib::Libstdcxx },
    Variant { name: "g", flags: &["-g"], stdlib: Stdlib::Libstdcxx },
    Variant {
        name: "sections",
        flags: &["-ffunction-sections", "-fdata-sections"],
        stdlib: Stdlib::Libstdcxx,
    },
    Variant { name: "nopic", flags: &["-fno-pic"], stdlib: Stdlib::Libstdcxx },
    Variant { name: "libcxx", flags: &[], stdlib: Stdlib::Libcxx },
    Variant { name: "libcxx-stripped", flags: &["-g"], stdlib: Stdlib::LibcxxStripped },
];

/// What mortar is expected to say about the shipped libc++ archives until
/// `SHT_AARCH64_ATTRIBUTES` is accepted (layout.md L-35, resolve.md
/// "Section types"): the refusal names the section type.
pub const LIBCXX_REFUSAL: &str = "unsupported section type: SHT_AARCH64_ATTRIBUTES";

/// The libc++ tree of `setup`: headers and static archives.
#[derive(Clone, Debug)]
pub struct LibcxxTree {
    pub include: PathBuf,
    pub lib: PathBuf,
    pub stripped: PathBuf,
}

impl LibcxxTree {
    pub fn find() -> Option<LibcxxTree> {
        let lane = super::lane_dir();
        let root = lane.join("libcxx-22-static");
        let t = LibcxxTree {
            include: root.join("usr/lib/llvm-22/include/c++/v1"),
            lib: root.join("usr/lib/aarch64-linux-gnu"),
            stripped: lane.join("libcxx-22-stripped"),
        };
        (t.include.is_dir()
            && t.lib.join("libc++.a").is_file()
            && t.stripped.join("libc++.a").is_file())
        .then_some(t)
    }
}

/// The tools of a run.
#[derive(Clone, Debug)]
struct Tools {
    mason: Mason,
    gxx: PathBuf,
    clangxx: PathBuf,
    gfortran: PathBuf,
    linkers: Vec<(&'static str, PathBuf)>,
    libcxx: Option<LibcxxTree>,
}

#[derive(Debug, Default)]
struct Args {
    mortar: Option<PathBuf>,
    filters: Vec<String>,
    keep: bool,
    jobs: usize,
    census: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { jobs: 4, ..Args::default() };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        let mut value = |n: &str| it.next().cloned().ok_or_else(|| format!("{n} needs a value"));
        match x.as_str() {
            "--mortar" => a.mortar = Some(PathBuf::from(value("--mortar")?)),
            "--filter" => a.filters.push(value("--filter")?),
            "--keep" => a.keep = true,
            "--census" => a.census = true,
            "-j" | "--jobs" => {
                a.jobs = value("-j")?
                    .parse()
                    .ok()
                    .filter(|n| (1..=16).contains(n))
                    .ok_or("-j needs 1 to 16")?;
            }
            s => return Err(format!("unknown argument '{s}'")),
        }
    }
    Ok(a)
}

/// A program's run: standard output, standard error, exit status.
type Run = (String, String, Option<i32>);

/// One cell's result.
#[derive(Clone, Debug, Default)]
struct Cell {
    program: String,
    variant: &'static str,
    /// Per linker: its run (stdout, stderr, exit status) or why there is none.
    links: Vec<(&'static str, Result<Run, String>)>,
    plumb: Option<Result<Plumbed, String>>,
    notes: Vec<String>,
    /// The cell holds: mortar's program ran as GNU ld's, or mortar refused
    /// exactly as expected.
    ok: bool,
}

pub fn main(args: &[String]) -> Result<ExitCode, String> {
    let args = parse(args)?;
    let Some(mason) = Mason::find_or_skip("programs")? else { return Ok(ExitCode::SUCCESS) };
    if std::env::consts::ARCH != "aarch64" {
        println!(
            "xtask mason programs: skipped: kiln and hod's lane runs natively on AArch64 only here"
        );
        return Ok(ExitCode::SUCCESS);
    }
    let need = |n: &str| tool(n).ok_or_else(|| format!("`{n}` is not on PATH"));
    let mut linkers = vec![("bfd", need("ld.bfd")?)];
    for (n, t) in [("lld", "ld.lld"), ("mold", "mold")] {
        if let Some(p) = tool(t) {
            linkers.push((n, p));
        }
    }
    let mortar = super::mortar_binary(args.mortar.as_deref())?;
    linkers.push(("mortar", mortar));
    let tools = Tools {
        mason,
        gxx: need("g++")?,
        clangxx: need("clang++-22").or_else(|_| need("clang++"))?,
        gfortran: need("gfortran")?,
        linkers,
        libcxx: LibcxxTree::find(),
    };
    if tools.libcxx.is_none() {
        println!(
            "xtask mason programs: no libc++ tree (cargo xtask mason setup): libc++ variants skipped"
        );
    }
    let base = crate::linktest::workspace_root().join("tests/mason/programs");
    let programs: Vec<Program> = programs(&base)?
        .into_iter()
        .filter(|p| {
            args.filters.is_empty() || args.filters.iter().any(|f| p.name.contains(f.as_str()))
        })
        .collect();
    let scratch =
        crate::linktest::build_dir().join("mason").join(format!("programs-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let mut work = Vec::new();
    for p in &programs {
        for v in VARIANTS {
            if v.stdlib != Stdlib::Libstdcxx && (!p.libcxx_ok() || tools.libcxx.is_none()) {
                continue;
            }
            work.push((p, v));
        }
    }
    let queue = Mutex::new(work.into_iter().enumerate());
    let cells = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..args.jobs {
            s.spawn(|| {
                loop {
                    let next =
                        queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).next();
                    let Some((i, (p, v))) = next else { break };
                    let dir = scratch.join(format!("{}-{}", p.name, v.name));
                    let cell = run_cell(&tools, p, v, &dir, args.census);
                    cells.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((i, cell));
                }
            });
        }
    });
    let mut cells = cells.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    cells.sort_by_key(|(i, _)| *i);
    let cells: Vec<Cell> = cells.into_iter().map(|(_, c)| c).collect();
    print!("{}", table(&cells, &tools));
    if args.census {
        let groups = census_groups(&scratch);
        print!("{}", census::compare(&groups)?);
    }
    let bad = cells.iter().filter(|c| !c.ok).count();
    if args.keep {
        println!("xtask mason programs: kept {}", scratch.display());
    } else {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    println!(
        "xtask mason programs: {} cells, {} hold, {bad} do not",
        cells.len(),
        cells.len() - bad
    );
    Ok(if bad == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// The objects of the O0 and g cells, by producer, for the census.
fn census_groups(scratch: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let mut groups: Vec<(String, Vec<PathBuf>)> =
        ["kiln+hod", "gcc", "clang"].iter().map(|n| ((*n).to_owned(), Vec::new())).collect();
    let Ok(dirs) = std::fs::read_dir(scratch) else { return groups };
    let mut dirs: Vec<PathBuf> = dirs.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for d in dirs {
        // Only the cells that made reference objects: like for like.
        let n = d.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !(n.ends_with("-O0") || n.ends_with("-g")) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&d) else { continue };
        let mut files: Vec<PathBuf> = files.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for f in files {
            let n = f.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
            let slot = if n.ends_with(".kiln.cpp.o") {
                0
            } else if n.ends_with(".ref-gcc.o") {
                1
            } else if n.ends_with(".ref-clang.o") {
                2
            } else {
                continue;
            };
            groups[slot].1.push(f);
        }
    }
    groups
}

fn first_line(o: &crate::process::Output) -> String {
    let e = o.first_error_line();
    if e.is_empty() { o.end.to_string() } else { e }
}

/// Compiles the program's sources for the variant into `dir`; the objects.
fn compile(
    tools: &Tools,
    prog: &Program,
    var: &Variant,
    dir: &Path,
    census: bool,
) -> Result<Vec<PathBuf>, String> {
    let stats = dir.join("stats.jsonl");
    let stats_s = stats.display().to_string();
    let hod_s = tools.mason.hod.display().to_string();
    let kiln_env: [(&str, &str); 4] = [
        ("KILN_BACKEND", "hod"),
        ("KILN_HOD", &hod_s),
        ("KILN_HOD_STRICT", "1"),
        ("KILN_STATS_LOG", &stats_s),
    ];
    let libcxx_flags: Vec<String> = match (&tools.libcxx, var.stdlib) {
        (Some(tree), Stdlib::Libcxx | Stdlib::LibcxxStripped) => vec![
            "-stdlib=libc++".into(),
            "-nostdinc++".into(),
            "-isystem".into(),
            tree.include.display().to_string(),
        ],
        _ => Vec::new(),
    };
    let mut objects = Vec::new();
    for (src, lang) in &prog.sources {
        let name = src.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let obj = dir.join(format!("{name}.o"));
        let mut argv: Vec<String> = Vec::new();
        if *lang != Lang::Gfortran {
            argv.push("-std=c++20".into());
        }
        argv.extend(var.flags.iter().map(|f| (*f).to_owned()));
        argv.push("-O0".into());
        let (prog, env): (&Path, &[(&str, &str)]) = match lang {
            Lang::Kiln => {
                argv.extend(libcxx_flags.iter().cloned());
                (&tools.mason.kiln, &kiln_env)
            }
            Lang::Clangxx => {
                argv.extend(libcxx_flags.iter().cloned());
                (&tools.clangxx, &[])
            }
            Lang::Gxx => (&tools.gxx, &[]),
            Lang::Gfortran => {
                argv.extend(["-J".into(), dir.display().to_string()]);
                (&tools.gfortran, &[])
            }
        };
        argv.extend([
            "-c".into(),
            src.display().to_string(),
            "-o".into(),
            obj.display().to_string(),
        ]);
        let output = run_in(prog, &argv, dir, env, TOOL_LIMITS)?;
        if !output.end.success() {
            return Err(format!("compile {name}: {}", first_line(&output)));
        }
        objects.push(obj);
        if census
            && *lang == Lang::Kiln
            && var.stdlib == Stdlib::Libstdcxx
            && matches!(var.name, "O0" | "g")
        {
            for (suffix, cc) in [("ref-gcc", &tools.gxx), ("ref-clang", &tools.clangxx)] {
                let mut r = argv.clone();
                let n = r.len();
                r[n - 1] = dir.join(format!("{name}.{suffix}.o")).display().to_string();
                let output = run_in(cc, &r, dir, &[], TOOL_LIMITS)?;
                if !output.end.success() {
                    return Err(format!("compile {name} ({suffix}): {}", first_line(&output)));
                }
            }
        }
    }
    // Every kiln job must have been hod's: a decline means llc made it.
    let log = std::fs::read_to_string(&stats).unwrap_or_default();
    for line in log.lines().filter(|l| l.contains("\"output_kind\":\"obj\"")) {
        if !line.contains("\"backend\":\"hod\"") {
            return Err(format!(
                "not compiled by hod: {}",
                line.chars().take(300).collect::<String>()
            ));
        }
    }
    Ok(objects)
}

/// The driver's static link line for `objects` (the linker's arguments,
/// `-o out`).
fn link_line(
    tools: &Tools,
    prog: &Program,
    var: &Variant,
    objects: &[PathBuf],
    dir: &Path,
) -> Result<Vec<String>, String> {
    let mut argv: Vec<String> = vec!["-static".into()];
    argv.extend(objects.iter().map(|o| o.display().to_string()));
    argv.extend(prog.extra.iter().cloned());
    if let Some(tree) = &tools.libcxx {
        match var.stdlib {
            Stdlib::Libstdcxx => {}
            Stdlib::Libcxx => argv.extend([
                "-stdlib=libc++".into(),
                format!("-L{}", tree.lib.display()),
                "-lc++abi".into(),
            ]),
            Stdlib::LibcxxStripped => {
                argv.extend([
                    "-stdlib=libc++".into(),
                    format!("-L{}", tree.stripped.display()),
                    "-lc++abi".into(),
                ]);
            }
        }
    }
    argv.extend(["-o".into(), "out".into(), "-###".into()]);
    let driver = if prog.gnu_driver {
        tools.gxx.clone()
    } else {
        // kiln's own link step: the clang++ it runs, asked in turn.
        let output = run_in(&tools.mason.kiln, &argv, dir, &[], TOOL_LIMITS)?;
        let text = String::from_utf8_lossy(&output.stderr).into_owned();
        let line = text
            .lines()
            .map(words)
            .find(|w| w.first().is_some_and(|p| p.ends_with("clang++")))
            .ok_or_else(|| format!("kiln++ -###: no clang++ line in {text}"))?;
        PathBuf::from(&line[0])
    };
    let output = run_in(&driver, &argv, dir, &[], TOOL_LIMITS)?;
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    let line = text
        .lines()
        .rev()
        .map(words)
        .find(|w| {
            w.first().is_some_and(|p| {
                p.ends_with("collect2") || p.ends_with("/ld") || p.ends_with("ld.bfd")
            })
        })
        .ok_or_else(|| format!("{} -###: no link line in {text}", driver.display()))?;
    Ok(line[1..].to_vec())
}

fn with_output(args: &[String], out: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-o" {
            it.next();
            v.extend(["-o".into(), out.to_owned()]);
        } else {
            v.push(a.clone());
        }
    }
    v
}

fn run_cell(
    tools: &Tools,
    prog: &Program,
    var: &'static Variant,
    dir: &Path,
    census: bool,
) -> Cell {
    let mut cell = Cell { program: prog.name.clone(), variant: var.name, ..Cell::default() };
    if let Err(e) = std::fs::create_dir_all(dir) {
        cell.notes.push(format!("{}: {e}", dir.display()));
        return cell;
    }
    let objects = match compile(tools, prog, var, dir, census) {
        Ok(o) => o,
        Err(e) => {
            cell.notes.push(e);
            return cell;
        }
    };
    let line = match link_line(tools, prog, var, &objects, dir) {
        Ok(l) => l,
        Err(e) => {
            cell.notes.push(e);
            return cell;
        }
    };
    for (name, linker) in &tools.linkers {
        let out = format!("out.{name}");
        let mut argv = with_output(&line, &out);
        if *name == "mortar" {
            // The verifier's map is mortar's machine map (`--mortar-map`,
            // image.md I-120); `-Map` is the GNU text map (WP-027e).
            argv.push(format!("--mortar-map={out}.map"));
        }
        let r = match run_in(linker, &argv, dir, &[], TOOL_LIMITS) {
            Err(e) => Err(e),
            Ok(linked) if !linked.end.success() => Err(first_line(&linked)),
            Ok(linked) => {
                if !linked.stderr.is_empty() {
                    cell.notes.push(format!("{name} warns: {}", first_line(&linked)));
                }
                match run_in(&dir.join(&out), &[], dir, &[], RUN_LIMITS) {
                    Err(e) => Err(format!("run: {e}")),
                    Ok(r) => Ok((
                        String::from_utf8_lossy(&r.stdout).into_owned(),
                        String::from_utf8_lossy(&r.stderr).into_owned(),
                        match r.end {
                            crate::process::End::Exit(c) => Some(c),
                            _ => None,
                        },
                    )),
                }
            }
        };
        cell.links.push((name, r));
    }
    let get = |n: &str| cell.links.iter().find(|(l, _)| *l == n).map(|(_, r)| r);
    let reference = get("bfd").and_then(|r| r.as_ref().ok()).cloned();
    let ours = get("mortar").cloned();
    for (name, r) in &cell.links {
        if let (Ok(run), Some(refr)) = (r, &reference)
            && run != refr
        {
            cell.notes.push(format!("{name}: runs differently from GNU ld's"));
        }
    }
    let expect_refusal = var.stdlib == Stdlib::Libcxx;
    cell.ok = match (&ours, &reference, expect_refusal) {
        (Some(Err(e)), _, true) => {
            let named = e.contains(LIBCXX_REFUSAL);
            if !named {
                cell.notes.push(format!("expected the refusal '{LIBCXX_REFUSAL}'"));
            }
            named
        }
        (Some(Ok(_)), _, true) => {
            cell.notes.push(
                "XPASS: mortar linked the shipped libc++ archives; update LIBCXX_REFUSAL".into(),
            );
            false
        }
        (Some(Ok(run)), Some(refr), false) => run == refr,
        _ => false,
    };
    if let Some(Ok(_)) = ours {
        let out = dir.join("out.mortar");
        let pl = super::plumb(&out, &dir.join("out.mortar.map"), dir);
        if !matches!(&pl, Ok(p) if p.findings == 0) {
            cell.ok = false;
        }
        cell.plumb = Some(pl);
    }
    cell
}

fn table(cells: &[Cell], t: &Tools) -> String {
    let mut s = String::new();
    let names: Vec<&str> = t.linkers.iter().map(|(n, _)| *n).collect();
    let _ = writeln!(s, "| program | variant | {} | plumb (mortar) | holds |", names.join(" | "));
    let _ = writeln!(s, "|---|---|{}---|---|", "---|".repeat(names.len()));
    for c in cells {
        let reference =
            c.links.iter().find(|(n, _)| *n == "bfd").and_then(|(_, r)| r.as_ref().ok());
        let mut row = format!("| {} | {} |", c.program, c.variant);
        for n in &names {
            let text = match c.links.iter().find(|(l, _)| l == n).map(|(_, r)| r) {
                None => "-".to_owned(),
                Some(Err(e)) => format!("refused: `{}`", short(e)),
                Some(Ok(run)) => {
                    let status = run.2.map_or_else(|| "signal".to_owned(), |c| c.to_string());
                    match reference {
                        Some(r) if r == run => format!("ok ({status})"),
                        Some(_) => format!("DIFFERS ({status})"),
                        None => format!("ran ({status})"),
                    }
                }
            };
            let _ = write!(row, " {text} |");
        }
        let plumb = match &c.plumb {
            None => "-".to_owned(),
            Some(Ok(p)) => p.to_string(),
            Some(Err(e)) => format!("error: {e}"),
        };
        let _ = writeln!(row, " {plumb} | {} |", if c.ok { "yes" } else { "NO" });
        s.push_str(&row);
        for n in &c.notes {
            let _ = writeln!(s, "|   | note: {} |", short(n));
        }
    }
    s
}

/// A diagnostic without its input's directory, at most 160 characters.
fn short(e: &str) -> String {
    let e = e.strip_prefix("mortar: error: ").unwrap_or(e);
    let e = match (e.find('('), e.rfind('/')) {
        (Some(paren), Some(slash)) if slash < paren => &e[slash + 1..],
        _ => e,
    };
    e.chars().take(160).collect::<String>().replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_by_name() {
        assert_eq!(Lang::of("a.kiln.cpp"), Some(Lang::Kiln));
        assert_eq!(Lang::of("lib.gcc.cpp"), Some(Lang::Gxx));
        assert_eq!(Lang::of("m.clang.cpp"), Some(Lang::Clangxx));
        assert_eq!(Lang::of("f.gfortran.f90"), Some(Lang::Gfortran));
        assert_eq!(Lang::of("t.h"), None);
    }

    #[test]
    fn output_is_replaced() {
        let a: Vec<String> = ["-static", "-o", "out", "x.o"].map(str::to_owned).into();
        assert_eq!(with_output(&a, "out.bfd"), ["-static", "-o", "out.bfd", "x.o"]);
    }

    #[test]
    fn diagnostics_are_shortened() {
        assert_eq!(
            short(
                "mortar: error: /a/b/libc++abi.a(x.o):(.ARM.attributes+0x0): unsupported section type"
            ),
            "libc++abi.a(x.o):(.ARM.attributes+0x0): unsupported section type"
        );
    }

    #[test]
    fn the_programs_parse() {
        let dir = crate::linktest::workspace_root().join("tests/mason/programs");
        let p = programs(&dir).unwrap();
        assert!(p.iter().any(|p| p.name == "hello" && !p.gnu_driver));
        assert!(p.iter().all(|p| !p.sources.is_empty()), "a program without sources");
        let f = p.iter().find(|p| p.name == "fortran").unwrap();
        assert!(f.gnu_driver && f.extra == ["-lgfortran"]);
    }
}
