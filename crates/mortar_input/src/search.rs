//! Spelling and finding (`spec/input.md` §4): found paths, the sysroot,
//! the library search path's directories, the candidates of a search, and
//! the names of the targets the skip rule compares.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use mortar_core::plan::Target;
use mortar_elf::Machine;

/// A path as the phase spells it (§4.5): bytes, never normalized.
pub type Spelled = Box<[u8]>;

/// The path the system calls use for a spelled path: as written when
/// absolute, else under the working directory.
#[must_use]
pub fn fs_path(cwd: &Path, spelled: &[u8]) -> PathBuf {
    let p = Path::new(OsStr::from_bytes(spelled));
    if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
}

/// `dir` joined with `name` (§4.5): one `/` inserted unless `dir` is empty
/// or ends with `/`.
#[must_use]
pub fn join(dir: &[u8], name: &[u8]) -> Spelled {
    let mut out = Vec::with_capacity(dir.len() + 1 + name.len());
    out.extend_from_slice(dir);
    if !dir.is_empty() && !dir.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(name);
    out.into_boxed_slice()
}

/// The directory part of a found path (`sd/s.ld` → `sd`; `s.ld` → empty,
/// the working directory; `/s.ld` → `/`).
#[must_use]
pub fn dir_of(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(0) => b"/",
        Some(i) => &path[..i],
        None => b"",
    }
}

/// The last component of a path.
#[must_use]
pub fn file_name(path: &[u8]) -> &[u8] {
    path.iter().rposition(|&b| b == b'/').map_or(path, |i| &path[i + 1..])
}

/// The sysroot `S` followed by `D`, which starts with `/` (§4.3
/// spelling): every trailing `/` of `S` removed, so no slash is doubled.
#[must_use]
pub fn under_sysroot(sysroot: &[u8], d: &[u8]) -> Spelled {
    let mut end = sysroot.len();
    while end > 0 && sysroot[end - 1] == b'/' {
        end -= 1;
    }
    let mut out = sysroot[..end].to_vec();
    out.extend_from_slice(d);
    out.into_boxed_slice()
}

/// How a directory marked to be inside the sysroot reads (§4.3 table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InSysroot {
    /// The directory to search.
    Dir(Spelled),
    /// `=` + relative directory with a sysroot: `ambiguous-sysroot-path`,
    /// with the two readings (concatenated, joined).
    Ambiguous { concatenated: Spelled, joined: Spelled },
}

/// Reads `d` (the part after `=` or `$SYSROOT`) under `sysroot`.
#[must_use]
pub fn in_sysroot(sysroot: Option<&[u8]>, d: &[u8]) -> InSysroot {
    match sysroot {
        None => InSysroot::Dir(d.into()),
        Some(s) if d.starts_with(b"/") => InSysroot::Dir(under_sysroot(s, d)),
        Some(s) => {
            let mut concatenated = s.to_vec();
            concatenated.extend_from_slice(d);
            InSysroot::Ambiguous {
                concatenated: concatenated.into_boxed_slice(),
                joined: join(s, d),
            }
        }
    }
}

/// The sysroot-marked prefix of a script `SEARCH_DIR` or a closure
/// directory (`=D`, `$SYSROOT D`): the rest, if the word has one.
#[must_use]
pub fn sysroot_prefix(word: &[u8]) -> Option<&[u8]> {
    if let Some(rest) = word.strip_prefix(b"=") {
        return Some(rest);
    }
    word.strip_prefix(b"$SYSROOT")
}

/// The candidate names of `-l name` in one directory (§4.4).
#[must_use]
pub fn library_names(name: &[u8], link_static: bool) -> Vec<Spelled> {
    let file = |ext: &[u8]| {
        let mut v = b"lib".to_vec();
        v.extend_from_slice(name);
        v.extend_from_slice(ext);
        v.into_boxed_slice()
    };
    if link_static { vec![file(b".a")] } else { vec![file(b".so"), file(b".a")] }
}

/// What the skip rule and the messages call a search (§4.4): `-lname`,
/// `-l:file`, or a name as written.
#[must_use]
pub fn search_label(form: SearchForm, name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    match form {
        SearchForm::Library => format!("-l{name}"),
        SearchForm::LibraryFile => format!("-l:{name}"),
        SearchForm::Name => name.into_owned(),
    }
}

/// The form of a search, for its messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchForm {
    Library,
    LibraryFile,
    /// A relative script entry, an `INCLUDE`, a closure edge.
    Name,
}

/// The machine of a plan target.
#[must_use]
pub const fn machine_of(target: Target) -> Machine {
    match target {
        Target::Aarch64 => Machine::AArch64,
        Target::X86_64 => Machine::X86_64,
        Target::I386 => Machine::I386,
    }
}

/// The BFD name of a target (`spec/script.md` §2.5).
#[must_use]
pub const fn bfd_name(machine: Machine) -> &'static str {
    match machine {
        Machine::AArch64 => "elf64-littleaarch64",
        Machine::X86_64 => "elf64-x86-64",
        Machine::I386 => "elf32-i386",
    }
}

/// The `OUTPUT_ARCH` name of a target (`spec/script.md` §2.5).
#[must_use]
pub const fn arch_name(machine: Machine) -> &'static str {
    match machine {
        Machine::AArch64 => "aarch64",
        Machine::X86_64 => "i386:x86-64",
        Machine::I386 => "i386",
    }
}

/// BFD target names GNU binutils defines for ELF and PE on the machines
/// near mortar's three: the names the skip rule recognises as "another
/// target" in an `OUTPUT_FORMAT` (§4.4). A name not listed is not known,
/// so it skips nothing; the late check of §3.2 refuses it.
const KNOWN_BFD: &[&str] = &[
    "elf64-littleaarch64",
    "elf64-bigaarch64",
    "elf32-littleaarch64",
    "elf32-bigaarch64",
    "elf64-x86-64",
    "elf32-x86-64",
    "elf32-i386",
    "elf32-iamcu",
    "elf32-littlearm",
    "elf32-bigarm",
    "elf64-littleriscv",
    "elf32-littleriscv",
    "elf64-powerpc",
    "elf64-powerpcle",
    "elf32-powerpc",
    "elf64-s390",
    "elf64-x86-64-freebsd",
    "elf32-i386-freebsd",
    "pei-i386",
    "pei-x86-64",
    "pei-aarch64-little",
];

/// Whether `name` is a BFD target name mortar knows.
#[must_use]
pub fn is_known_bfd(name: &[u8]) -> bool {
    KNOWN_BFD.iter().any(|k| k.as_bytes() == name)
}

/// Whether `stored` (a thin member's path) is taken as written.
#[must_use]
pub fn is_absolute(p: &[u8]) -> bool {
    p.starts_with(b"/")
}

/// Splits a `:`-separated list, dropping empty components (§8.6).
#[must_use]
pub fn split_colon(list: &[u8]) -> Vec<&[u8]> {
    list.split(|&b| b == b':').filter(|c| !c.is_empty()).collect()
}

/// Lossy text of a spelled path, for messages.
#[must_use]
pub fn text(p: &[u8]) -> String {
    String::from_utf8_lossy(p).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s4_5_join_inserts_one_slash() {
        assert_eq!(&*join(b"d1", b"libf.so"), b"d1/libf.so");
        assert_eq!(&*join(b"d1/", b"libf.so"), b"d1/libf.so");
        assert_eq!(&*join(b"./d/../d", b"x"), b"./d/../d/x");
        assert_eq!(&*join(b"", b"x"), b"x");
    }

    #[test]
    fn s4_3_sysroot_spelling() {
        assert_eq!(in_sysroot(Some(b"/"), b"/usr/lib"), InSysroot::Dir((*b"/usr/lib").into()));
        assert_eq!(in_sysroot(Some(b"S/"), b"/lib"), InSysroot::Dir((*b"S/lib").into()));
        assert_eq!(in_sysroot(Some(b"S"), b"/lib"), InSysroot::Dir((*b"S/lib").into()));
        assert_eq!(in_sysroot(None, b"lib"), InSysroot::Dir((*b"lib").into()));
        assert_eq!(
            in_sysroot(Some(b"S"), b"lib"),
            InSysroot::Ambiguous { concatenated: (*b"Slib").into(), joined: (*b"S/lib").into() }
        );
    }

    #[test]
    fn s4_4_candidate_names() {
        assert_eq!(
            library_names(b"f", false),
            vec![(*b"libf.so").into(), (*b"libf.a").into()] as Vec<Spelled>
        );
        assert_eq!(library_names(b"f", true), vec![(*b"libf.a").into()] as Vec<Spelled>);
    }

    #[test]
    fn s8_6_colon_split_drops_empty() {
        assert_eq!(split_colon(b"/nonexist::hid:"), vec![&b"/nonexist"[..], b"hid"]);
    }

    #[test]
    fn s5_1_dir_of() {
        assert_eq!(dir_of(b"sd/s.ld"), b"sd");
        assert_eq!(dir_of(b"s.ld"), b"");
        assert_eq!(dir_of(b"/s.ld"), b"/");
    }
}
