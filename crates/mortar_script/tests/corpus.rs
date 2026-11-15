//! `spec/script.md` §6 (L1): the text files a linker meets on this
//! machine, found by content, parsed, and their structure asserted.

mod common;

use std::path::{Path, PathBuf};

use common::summary;
use mortar_script::{looks_like_script, parse};
use mortar_testutil::tools_required;

/// Directories a linker searches for libraries on a Debian-style system,
/// native and cross: `/usr/lib`, `/usr/lib/<triple>`, `/usr/<triple>/lib*`,
/// `/usr/lib/gcc/<triple>/<version>`, `/usr/lib/gcc-cross/<triple>/<version>`.
fn library_dirs() -> Vec<PathBuf> {
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        out.sort();
        out
    };
    let triple =
        |p: &Path| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("-linux-gnu"));
    let mut dirs =
        vec![PathBuf::from("/usr/lib"), PathBuf::from("/usr/lib64"), PathBuf::from("/usr/lib32")];
    dirs.extend(subdirs(Path::new("/usr/lib")).into_iter().filter(|p| triple(p)));
    for t in subdirs(Path::new("/usr")).into_iter().filter(|p| triple(p)) {
        dirs.extend(
            subdirs(&t)
                .into_iter()
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("lib"))),
        );
    }
    for gcc in ["/usr/lib/gcc", "/usr/lib/gcc-cross"] {
        for t in subdirs(Path::new(gcc)) {
            dirs.extend(subdirs(&t));
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

/// Every `lib*.so` and `lib*.a` in the library directories whose bytes are
/// script text (§3): neither ELF nor an archive.
fn text_libraries() -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    for dir in library_dirs() {
        let mut entries: Vec<PathBuf> =
            std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let library = name.starts_with("lib")
                && Path::new(&name).extension().is_some_and(|e| e == "so" || e == "a");
            let small = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() < 64 * 1024);
            if !library || !small {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if looks_like_script(&bytes) {
                found.push((path, bytes));
            }
        }
    }
    found.sort();
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

/// The known text libraries of this host: [`KNOWN_ON_AARCH64`], or
/// [`KNOWN_ON_X86_64`] on an x86-64 host (the hosted x86-64 lane,
/// `spec/harness.md` "Hosted CI").
const KNOWN: &[(&str, &[&str])] =
    if cfg!(target_arch = "x86_64") { KNOWN_ON_X86_64 } else { KNOWN_ON_AARCH64 };

/// The ten text libraries of the reference machine (Debian, glibc 2.41,
/// GCC 14, cross sysroots for x86-64 and i686), with their structure.
const KNOWN_ON_AARCH64: &[(&str, &[&str])] = &[
    (
        "/usr/lib/aarch64-linux-gnu/libc.so",
        &[
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "GROUP(/lib/aarch64-linux-gnu/libc.so.6 /usr/lib/aarch64-linux-gnu/libc_nonshared.a AS_NEEDED(/lib/ld-linux-aarch64.so.1))",
        ],
    ),
    (
        "/usr/lib/aarch64-linux-gnu/libm.so",
        &[
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "GROUP(/lib/aarch64-linux-gnu/libm.so.6 AS_NEEDED(/lib/aarch64-linux-gnu/libmvec.so.1))",
        ],
    ),
    (
        "/usr/lib/aarch64-linux-gnu/libm.a",
        &[
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "GROUP(/usr/lib/aarch64-linux-gnu/libm-2.41.a /usr/lib/aarch64-linux-gnu/libmvec.a)",
        ],
    ),
    ("/usr/lib/gcc/aarch64-linux-gnu/14/libgcc_s.so", &["GROUP(libgcc_s.so.1 -lgcc)"]),
    (
        "/usr/x86_64-linux-gnu/lib/libc.so",
        &[
            "OUTPUT_FORMAT(elf64-x86-64)",
            "GROUP(/usr/x86_64-linux-gnu/lib/libc.so.6 /usr/x86_64-linux-gnu/lib/libc_nonshared.a AS_NEEDED(/usr/x86_64-linux-gnu/lib64/ld-linux-x86-64.so.2))",
        ],
    ),
    (
        "/usr/x86_64-linux-gnu/lib/libm.so",
        &[
            "OUTPUT_FORMAT(elf64-x86-64)",
            "GROUP(/usr/x86_64-linux-gnu/lib/libm.so.6 AS_NEEDED(/usr/x86_64-linux-gnu/lib/libmvec.so.1))",
        ],
    ),
    (
        "/usr/x86_64-linux-gnu/lib/libm.a",
        &[
            "OUTPUT_FORMAT(elf64-x86-64)",
            "GROUP(/usr/x86_64-linux-gnu/lib/libm-2.41.a /usr/x86_64-linux-gnu/lib/libmvec.a)",
        ],
    ),
    ("/usr/lib/gcc-cross/x86_64-linux-gnu/14/libgcc_s.so", &["GROUP(libgcc_s.so.1 -lgcc)"]),
    (
        "/usr/i686-linux-gnu/lib/libc.so",
        &[
            "OUTPUT_FORMAT(elf32-i386)",
            "GROUP(/usr/i686-linux-gnu/lib/libc.so.6 /usr/i686-linux-gnu/lib/libc_nonshared.a AS_NEEDED(/usr/i686-linux-gnu/lib/ld-linux.so.2))",
        ],
    ),
    ("/usr/lib/gcc-cross/i686-linux-gnu/14/libgcc_s.so", &["GROUP(libgcc_s.so.1 -lgcc)"]),
];

/// The same ten on an x86-64 host with the same packages (x86-64 native,
/// cross sysroots for AArch64 and i686): the table above with native and
/// cross swapped, after the layout of Debian's cross packages (`/usr/<triple>/lib`,
/// the loader under `lib64` for x86-64 only). Written by analogy, before an
/// x86-64 host ran it; the first hosted x86-64 run confirms or corrects it.
const KNOWN_ON_X86_64: &[(&str, &[&str])] = &[
    (
        "/usr/aarch64-linux-gnu/lib/libc.so",
        &[
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "GROUP(/usr/aarch64-linux-gnu/lib/libc.so.6 /usr/aarch64-linux-gnu/lib/libc_nonshared.a AS_NEEDED(/usr/aarch64-linux-gnu/lib/ld-linux-aarch64.so.1))",
        ],
    ),
    (
        "/usr/aarch64-linux-gnu/lib/libm.so",
        &[
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "GROUP(/usr/aarch64-linux-gnu/lib/libm.so.6 AS_NEEDED(/usr/aarch64-linux-gnu/lib/libmvec.so.1))",
        ],
    ),
    (
        "/usr/aarch64-linux-gnu/lib/libm.a",
        &[
            "OUTPUT_FORMAT(elf64-littleaarch64)",
            "GROUP(/usr/aarch64-linux-gnu/lib/libm-2.41.a /usr/aarch64-linux-gnu/lib/libmvec.a)",
        ],
    ),
    ("/usr/lib/gcc-cross/aarch64-linux-gnu/14/libgcc_s.so", &["GROUP(libgcc_s.so.1 -lgcc)"]),
    (
        "/usr/lib/x86_64-linux-gnu/libc.so",
        &[
            "OUTPUT_FORMAT(elf64-x86-64)",
            "GROUP(/lib/x86_64-linux-gnu/libc.so.6 /usr/lib/x86_64-linux-gnu/libc_nonshared.a AS_NEEDED(/lib64/ld-linux-x86-64.so.2))",
        ],
    ),
    (
        "/usr/lib/x86_64-linux-gnu/libm.so",
        &[
            "OUTPUT_FORMAT(elf64-x86-64)",
            "GROUP(/lib/x86_64-linux-gnu/libm.so.6 AS_NEEDED(/lib/x86_64-linux-gnu/libmvec.so.1))",
        ],
    ),
    (
        "/usr/lib/x86_64-linux-gnu/libm.a",
        &[
            "OUTPUT_FORMAT(elf64-x86-64)",
            "GROUP(/usr/lib/x86_64-linux-gnu/libm-2.41.a /usr/lib/x86_64-linux-gnu/libmvec.a)",
        ],
    ),
    ("/usr/lib/gcc/x86_64-linux-gnu/14/libgcc_s.so", &["GROUP(libgcc_s.so.1 -lgcc)"]),
    (
        "/usr/i686-linux-gnu/lib/libc.so",
        &[
            "OUTPUT_FORMAT(elf32-i386)",
            "GROUP(/usr/i686-linux-gnu/lib/libc.so.6 /usr/i686-linux-gnu/lib/libc_nonshared.a AS_NEEDED(/usr/i686-linux-gnu/lib/ld-linux.so.2))",
        ],
    ),
    ("/usr/lib/gcc-cross/i686-linux-gnu/14/libgcc_s.so", &["GROUP(libgcc_s.so.1 -lgcc)"]),
];

// spec: script.md §6 — every text library found parses; the known ten are
// all found (a failure under MORTAR_REQUIRE_TOOLS=1, a note otherwise) and
// have the recorded structure.
#[test]
fn system_text_libraries() {
    let found = text_libraries();
    for (path, bytes) in &found {
        let shown = path.display().to_string();
        let script = parse(bytes, &shown).unwrap_or_else(|d| panic!("{shown}: {d:?}"));
        assert!(!script.commands.is_empty(), "{shown}: no commands");
    }
    let mut missing = Vec::new();
    for &(path, expected) in KNOWN {
        let Some((_, bytes)) = found.iter().find(|(p, _)| p == Path::new(path)) else {
            missing.push(path);
            continue;
        };
        let script = parse(bytes, path).unwrap();
        assert_eq!(summary(&script), expected, "{path}");
    }
    if !missing.is_empty() {
        assert!(
            !tools_required(),
            "known text libraries not found (MORTAR_REQUIRE_TOOLS=1): {missing:?}"
        );
        eprintln!("note: known text libraries not on this machine, skipped: {missing:?}");
    }
    eprintln!("{} text libraries parsed", found.len());
}

/// GNU ld's own default scripts (`ldscripts`), several hundred.
fn ldscripts() -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for dir in library_dirs() {
        let ld = dir.join("ldscripts");
        let mut entries: Vec<PathBuf> =
            std::fs::read_dir(&ld).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if let Ok(bytes) = std::fs::read(&path) {
                out.push((path, bytes));
            }
        }
    }
    out
}

// spec: script.md §6 — GNU ld's default scripts are read up to their first
// command stage 1 does not implement, which is refused by name: never a
// syntax error. The commands before it (OUTPUT_FORMAT with three names,
// OUTPUT_ARCH, ENTRY, SEARCH_DIR("=...")) parse.
#[test]
fn gnu_default_scripts_refuse_never_fail() {
    let scripts = ldscripts();
    if scripts.is_empty() {
        assert!(!tools_required(), "no ldscripts directory (MORTAR_REQUIRE_TOOLS=1)");
        eprintln!("note: no ldscripts directory; skipped");
        return;
    }
    let mut refused = std::collections::BTreeMap::<String, usize>::new();
    for (path, bytes) in &scripts {
        let shown = path.display().to_string();
        match parse(bytes, &shown) {
            Ok(_) => *refused.entry("(none)".to_owned()).or_default() += 1,
            Err(d) => {
                let (_, name) = d.refusal().unwrap_or_else(|| panic!("{shown}: {d:?}"));
                *refused.entry(name.to_owned()).or_default() += 1;
            }
        }
    }
    eprintln!("{} default scripts: {refused:?}", scripts.len());
    assert!(refused.contains_key("SECTIONS"), "{refused:?}");
}
