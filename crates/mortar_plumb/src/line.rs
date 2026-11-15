//! The command line of a dynamic link, read by the verifier itself
//! (stage 3, `spec/plumb.md` §12; `spec/dyn.md` P-D2): the output kind
//! (D-1), the interpreter, `-z now`/`-z norelro`, the run paths, and the
//! shared-object mentions in order with their `--as-needed` state (D-11,
//! D-19a), linker scripts expanded inline. Files are read through
//! [`Files`], so tests and `xtask` decide where bytes come from.

use crate::map::OutputKind;

/// Where the verifier reads files from.
pub trait Files: Sync {
    /// The bytes of `path`, or `None` when it cannot be read.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// The host's file system.
#[derive(Debug)]
pub struct HostFiles;

impl Files for HostFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
}

/// An input of the line, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A relocatable object or an archive, by its path as named.
    Relocatable { path: String, group: Option<usize> },
    /// A shared object mention (D-19a): the file found, the state in
    /// force, the `--start-group`/`GROUP` it is in.
    Shared { path: String, as_needed: bool, group: Option<usize> },
}

/// The line, read.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // independent options of the line
pub struct Line {
    pub kind: Option<OutputKind>,
    pub interp: Option<String>,
    /// `-z now` (or `--now`, `-z lazy` last wins).
    pub now: bool,
    /// `-z relro` unless `-z norelro` (the default on GNU ld, lld, mold).
    pub relro: bool,
    pub export_dynamic: bool,
    pub allow_shlib_undefined: bool,
    /// `--enable-new-dtags` (the default): `DT_RUNPATH`, else `DT_RPATH`.
    pub new_dtags: bool,
    pub rpath: Vec<String>,
    pub rpath_link: Vec<String>,
    pub lib_dirs: Vec<String>,
    pub items: Vec<Item>,
    /// `-u` names (file 0 references, D-11 item 2).
    pub undefined: Vec<String>,
    pub entry: Option<String>,
    /// What the verifier could not read (each a finding of P-D2), or an
    /// option it refuses to model.
    pub problems: Vec<String>,
}

/// Options whose value is the next word.
const WITH_ARG: &[&str] = &[
    "-o",
    "-m",
    "-e",
    "--entry",
    "-u",
    "--undefined",
    "-T",
    "--script",
    "-Map",
    "-plugin",
    "--plugin",
    "-plugin-opt",
    "-y",
    "--trace-symbol",
    "-soname",
    "-h",
    "--defsym",
    "-Y",
    "-A",
    "-F",
    "-f",
    "-G",
    "--sysroot",
    "--wrap",
    "--version-script",
    "--dynamic-list",
    "--image-base",
    "-Ttext",
    "-Tdata",
    "-Tbss",
    "--section-start",
    "--thread-count",
    "-mllvm",
    "--exclude-libs",
    "--chroot",
];

struct State {
    as_needed: bool,
    bstatic: bool,
}

impl Line {
    /// Reads `args` (the linker's words, without the program name).
    #[allow(clippy::too_many_lines)] // one arm per option family
    pub fn parse(args: &[String], files: &dyn Files) -> Line {
        let mut l = Line { relro: true, new_dtags: true, ..Line::default() };
        let mut pie: Option<bool> = None;
        let mut static_ = false;
        let mut static_pie = false;
        let mut no_interp = false;
        let mut shared = false;
        // The library directories come first: `-L` applies to every `-l`
        // whatever its position (GNU ld, lld, mold).
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if let Some(d) = a.strip_prefix("--library-path=") {
                l.lib_dirs.push(d.into());
            } else if a == "-L" || a == "--library-path" {
                if let Some(d) = args.get(i + 1) {
                    l.lib_dirs.push(d.clone());
                }
                i += 1;
            } else if let Some(d) = a.strip_prefix("-L") {
                l.lib_dirs.push(d.into());
            } else if WITH_ARG.contains(&a) {
                i += 1;
            }
            i += 1;
        }
        let mut st = State { as_needed: false, bstatic: false };
        let mut stack: Vec<(bool, bool)> = Vec::new();
        let mut group: Option<usize> = None;
        let mut groups = 0usize;
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            let next = |i: &mut usize| -> Option<String> {
                *i += 1;
                args.get(*i).cloned()
            };
            let val = |a: &str, long: &str| -> Option<String> {
                a.strip_prefix(long).and_then(|r| r.strip_prefix('=')).map(str::to_string)
            };
            match a {
                "-pie" | "--pie" | "-pic-executable" | "--pic-executable" => pie = Some(true),
                "-no-pie" | "--no-pie" | "-no-pic-executable" | "--no-pic-executable" => {
                    pie = Some(false);
                }
                "-static-pie" | "--static-pie" => static_pie = true,
                "-shared" | "--shared" | "-Bshareable" => shared = true,
                "-static" | "--static" | "-Bstatic" | "-dn" | "-non_shared" => {
                    if a == "-static" || a == "--static" {
                        static_ = true;
                    }
                    st.bstatic = true;
                }
                "-Bdynamic" | "-dy" | "-call_shared" => st.bstatic = false,
                "--as-needed" => st.as_needed = true,
                "--no-as-needed" => st.as_needed = false,
                "--push-state" => stack.push((st.as_needed, st.bstatic)),
                "--pop-state" => {
                    if let Some((n, b)) = stack.pop() {
                        st.as_needed = n;
                        st.bstatic = b;
                    } else {
                        l.problems.push("--pop-state without --push-state".into());
                    }
                }
                "--no-dynamic-linker" => no_interp = true,
                "-dynamic-linker" | "--dynamic-linker" | "-I" => l.interp = next(&mut i),
                "-E" | "--export-dynamic" => l.export_dynamic = true,
                "--no-export-dynamic" => l.export_dynamic = false,
                "--allow-shlib-undefined" => l.allow_shlib_undefined = true,
                "--no-allow-shlib-undefined" => l.allow_shlib_undefined = false,
                "--enable-new-dtags" => l.new_dtags = true,
                "--disable-new-dtags" => l.new_dtags = false,
                "--start-group" | "-(" => {
                    group = Some(groups);
                    groups += 1;
                }
                "--end-group" | "-)" => group = None,
                "-z" => {
                    if let Some(z) = next(&mut i) {
                        l.z(&z);
                    }
                }
                "-rpath" | "-R" | "--rpath" => {
                    if let Some(p) = next(&mut i) {
                        l.rpath.extend(p.split(':').map(str::to_string));
                    }
                }
                "-rpath-link" | "--rpath-link" => {
                    if let Some(p) = next(&mut i) {
                        l.rpath_link.extend(p.split(':').map(str::to_string));
                    }
                }
                "-u" | "--undefined" => {
                    if let Some(u) = next(&mut i) {
                        l.undefined.push(u);
                    }
                }
                "-e" | "--entry" => l.entry = next(&mut i),
                "-l" | "--library" => {
                    if let Some(n) = next(&mut i) {
                        l.library(&n, &st, group, files);
                    }
                }
                "-L" | "--library-path" => i += 1,
                "--now" => l.now = true,
                "--copy-dt-needed-entries" => {
                    l.problems.push("--copy-dt-needed-entries (dyn.md D-15)".into());
                }
                _ if WITH_ARG.contains(&a) => i += 1,
                _ => {
                    if let Some(v) =
                        val(a, "--dynamic-linker").or_else(|| val(a, "-dynamic-linker"))
                    {
                        l.interp = Some(v);
                    } else if let Some(z) = a.strip_prefix("-z") {
                        l.z(z);
                    } else if let Some(n) =
                        val(a, "--library").or_else(|| a.strip_prefix("-l").map(str::to_string))
                    {
                        l.library(&n, &st, group, files);
                    } else if let Some(p) = val(a, "--rpath").or_else(|| val(a, "-rpath")) {
                        l.rpath.extend(p.split(':').map(str::to_string));
                    } else if let Some(p) = val(a, "--rpath-link").or_else(|| val(a, "-rpath-link"))
                    {
                        l.rpath_link.extend(p.split(':').map(str::to_string));
                    } else if let Some(u) = val(a, "--undefined") {
                        l.undefined.push(u);
                    } else if let Some(e) = val(a, "--entry") {
                        l.entry = Some(e);
                    } else if a.starts_with('-') {
                        // Every other option leaves the inputs alone.
                    } else {
                        l.file(a, &st, group, files, 0);
                    }
                }
            }
            i += 1;
        }
        l.kind = Some(if shared {
            l.problems.push("-shared (M4)".into());
            OutputKind::Pie
        } else if static_pie || (pie == Some(true) && (static_ || no_interp)) {
            OutputKind::StaticPie
        } else if static_ {
            OutputKind::Static
        } else if pie == Some(true) {
            if l.interp.is_none() && no_interp { OutputKind::StaticPie } else { OutputKind::Pie }
        } else if l.interp.is_some() || l.items.iter().any(|i| matches!(i, Item::Shared { .. })) {
            OutputKind::Exec
        } else {
            OutputKind::Static
        });
        if no_interp {
            l.interp = None;
        }
        l
    }

    fn z(&mut self, z: &str) {
        match z {
            "now" => self.now = true,
            "lazy" => self.now = false,
            "relro" => self.relro = true,
            "norelro" => self.relro = false,
            _ => {}
        }
    }

    fn library(&mut self, name: &str, st: &State, group: Option<usize>, files: &dyn Files) {
        let found = if let Some(exact) = name.strip_prefix(':') {
            self.lib_dirs.iter().map(|d| format!("{d}/{exact}")).find(|p| files.read(p).is_some())
        } else {
            let mut found = None;
            for d in &self.lib_dirs {
                if !st.bstatic {
                    let so = format!("{d}/lib{name}.so");
                    if files.read(&so).is_some() {
                        found = Some(so);
                        break;
                    }
                }
                let a = format!("{d}/lib{name}.a");
                if files.read(&a).is_some() {
                    found = Some(a);
                    break;
                }
            }
            found
        };
        match found {
            Some(p) => self.file(&p, st, group, files, 0),
            None => self.problems.push(format!("-l{name}: not found in the -L directories")),
        }
    }

    /// An input file: by its first bytes, an object, an archive, a shared
    /// object or a script.
    fn file(
        &mut self,
        path: &str,
        st: &State,
        group: Option<usize>,
        files: &dyn Files,
        depth: u32,
    ) {
        let Some(bytes) = files.read(path) else {
            self.problems.push(format!("{path}: cannot be read"));
            return;
        };
        if bytes.starts_with(b"\x7fELF") {
            let e_type = bytes.get(16..18).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
            if e_type == mortar_elf::consts::ET_DYN {
                if st.bstatic {
                    self.problems
                        .push(format!("{path}: a shared object after -Bstatic (dyn.md D-19)"));
                }
                self.items.push(Item::Shared { path: path.into(), as_needed: st.as_needed, group });
            } else {
                self.items.push(Item::Relocatable { path: path.into(), group });
            }
        } else if bytes.starts_with(b"!<arch>\n") || bytes.starts_with(b"!<thin>\n") {
            self.items.push(Item::Relocatable { path: path.into(), group });
        } else if bytes.is_empty() {
            // input §2.3: an empty file is ignored.
        } else if depth > 8 {
            self.problems.push(format!("{path}: scripts nested too deeply"));
        } else {
            self.script(path, &String::from_utf8_lossy(&bytes), st, group, files, depth);
        }
    }

    /// The subset of the script language a shared-library script uses:
    /// `INPUT`, `GROUP`, `AS_NEEDED`, `OUTPUT_FORMAT`, comments. Anything
    /// else is a problem (named).
    fn script(
        &mut self,
        path: &str,
        text: &str,
        st: &State,
        group: Option<usize>,
        files: &dyn Files,
        depth: u32,
    ) {
        let mut s = String::new();
        let mut rest = text;
        while let Some(i) = rest.find("/*") {
            s.push_str(&rest[..i]);
            rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
        }
        s.push_str(rest);
        let spaced = s.replace('(', " ( ").replace(')', " ) ").replace(',', " ");
        let words: Vec<&str> = spaced.split_whitespace().collect();
        let mut i = 0;
        let mut depth_paren = 0;
        let mut as_needed_at: Option<usize> = None;
        let mut in_input = false;
        let mut grp = group;
        while i < words.len() {
            let w = words[i];
            match w {
                "OUTPUT_FORMAT" | "OUTPUT_ARCH" | "SEARCH_DIR" | "TARGET" => {
                    // Skip its parenthesised argument.
                    let mut d = 0;
                    i += 1;
                    while i < words.len() {
                        match words[i] {
                            "(" => d += 1,
                            ")" => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                "INPUT" | "GROUP" => {
                    in_input = true;
                    if w == "GROUP" && grp.is_none() {
                        grp = Some(1_000_000 + self.items.len());
                    }
                }
                "AS_NEEDED" => as_needed_at = Some(depth_paren),
                "(" => depth_paren += 1,
                ")" => {
                    depth_paren -= 1;
                    if as_needed_at == Some(depth_paren) {
                        as_needed_at = None;
                    }
                    if depth_paren == 0 {
                        in_input = false;
                        grp = group;
                    }
                }
                _ if in_input => {
                    let sub = State {
                        as_needed: st.as_needed || as_needed_at.is_some(),
                        bstatic: st.bstatic,
                    };
                    if let Some(n) = w.strip_prefix("-l") {
                        self.library(n, &sub, grp, files);
                    } else if w.starts_with('/') || files.read(w).is_some() {
                        self.file(w, &sub, grp, files, depth + 1);
                    } else {
                        // GNU ld: a relative name is tried in the current
                        // directory, the script's, then the -L path.
                        let dir = path.rsplit_once('/').map(|(d, _)| d.to_string());
                        let found = dir
                            .iter()
                            .chain(self.lib_dirs.iter())
                            .map(|d| format!("{d}/{w}"))
                            .find(|p| files.read(p).is_some());
                        match found {
                            Some(p) => self.file(&p, &sub, grp, files, depth + 1),
                            None => self.problems.push(format!("{w} (in {path}): not found")),
                        }
                    }
                }
                _ => {
                    self.problems.push(format!("{path}: script word '{w}' is not modelled"));
                    return;
                }
            }
            i += 1;
        }
    }

    /// The position of the first relocatable item named `path` (D-19a).
    pub fn position_of(&self, path: &str) -> Option<usize> {
        self.items.iter().position(|i| matches!(i, Item::Relocatable { path: p, .. } if p == path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Mem(HashMap<String, Vec<u8>>);
    impl Files for Mem {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).cloned()
        }
    }

    fn elf(e_type: u16) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[..4].copy_from_slice(b"\x7fELF");
        v[16..18].copy_from_slice(&e_type.to_le_bytes());
        v
    }

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_default_gcc_line_and_the_libc_script() {
        let mut m = HashMap::new();
        m.insert("/l/libc.so".to_string(), b"/* GNU ld script */\nOUTPUT_FORMAT(elf64-littleaarch64)\nGROUP ( /l/libc.so.6 /l/libc_nonshared.a  AS_NEEDED ( /l/ld.so.1 ) )\n".to_vec());
        m.insert("/l/libc.so.6".into(), elf(3));
        m.insert("/l/ld.so.1".into(), elf(3));
        m.insert("/l/libc_nonshared.a".into(), b"!<arch>\n".to_vec());
        m.insert("/l/libgcc_s.so".into(), elf(3));
        m.insert("/l/libgcc.a".into(), b"!<arch>\n".to_vec());
        m.insert("h.o".into(), elf(1));
        let l = Line::parse(
            &words(
                "--as-needed -dynamic-linker /lib/ld.so.1 -pie -z now -o h -L/l h.o -lgcc --push-state --no-as-needed -lgcc_s --pop-state -lc",
            ),
            &Mem(m),
        );
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        assert_eq!(l.kind, Some(OutputKind::Pie));
        assert!(l.now && l.relro);
        assert_eq!(l.interp.as_deref(), Some("/lib/ld.so.1"));
        let shared: Vec<(String, bool)> = l
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Shared { path, as_needed, .. } => Some((path.clone(), *as_needed)),
                Item::Relocatable { .. } => None,
            })
            .collect();
        assert_eq!(
            shared,
            vec![
                ("/l/libgcc_s.so".into(), false),
                ("/l/libc.so.6".into(), true),
                ("/l/ld.so.1".into(), true)
            ]
        );
        assert_eq!(l.position_of("h.o"), Some(0));
    }

    #[test]
    fn the_output_kinds() {
        let m = Mem(HashMap::new());
        let k = |s: &str| Line::parse(&words(s), &m).kind;
        assert_eq!(k("-o a"), Some(OutputKind::Static));
        assert_eq!(k("-dynamic-linker /x -o a"), Some(OutputKind::Exec));
        assert_eq!(k("-pie -dynamic-linker /x"), Some(OutputKind::Pie));
        assert_eq!(k("-static-pie"), Some(OutputKind::StaticPie));
        assert_eq!(k("-pie --no-dynamic-linker"), Some(OutputKind::StaticPie));
    }
}
