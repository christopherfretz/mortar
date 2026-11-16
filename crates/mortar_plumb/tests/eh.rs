//! Call frame information (`spec/plumb.md` §6.4): every live FDE of the
//! inputs is in the output once, with its CIE, personality and LSDA, and
//! `.eh_frame_hdr` indexes every one of them.

mod common;

use common::{Linker, Size, assert_clean, corpus};

// spec/plumb.md §6.4: the hand-written frames of every augmentation the
// assembler writes, by each linker, with a search table; the FDE of the
// collected function is dropped with it.
#[test]
fn s6_4_frames_of_each_augmentation_are_checked() {
    let Some(c) = corpus(Size::Tls) else { return };
    for linker in Linker::ALL {
        let link = c.get("eh-frames", linker);
        assert_eq!(link.run(), Some(0), "{}: the program fails", link.label());
        let r = link.verify();
        assert_clean(link, &r);
        let cov = &r.coverage;
        let s = r.summary();
        assert_eq!(cov.not_checked_total(), 0, "{}\n{s}", link.label());
        assert_eq!(cov.fdes_checked, 5, "{}\n{s}", link.label());
        assert_eq!(cov.fdes_dropped, 1, "{}\n{s}", link.label());
        assert_eq!(cov.eh_frame_hdr_entries, 5, "{}\n{s}", link.label());
        // Five initial locations, one LSDA, one personality.
        assert_eq!(cov.verified_of(mortar_plumb::Via::EhFrame), 7, "{}\n{s}", link.label());
    }
}

// ---------------------------------------------------------------------
// spec/plumb.md §4.4, "offset 0 of an input `.eh_frame`" (synth.md S-50
// rule 1; WP-062's finding 1): under mortar's map a reference to offset 0
// of an input `.eh_frame` is the start of the output `.eh_frame`, whatever
// the input holds; a non-zero offset follows its record (rule 2).

mod s50 {
    use std::path::Path;

    use mortar_plumb::{Format, Input};
    use mortar_testutil::target::Arch;
    use mortar_testutil::{Scratch, run_with};

    /// `a.s`: `_start` with an FDE (the first records of the output);
    /// `b.s`: a CIE of its own (augmentation "zRS": no other CIE is
    /// equal to it) and an FDE, `__B_BEGIN__` at offset 0, `__B_FDE__` at
    /// the FDE (0x18), and two words referring to them as crtbeginT.o
    /// refers to its `__EH_FRAME_BEGIN__` (a section symbol plus the
    /// offset); `c.s`: an empty `.eh_frame` with `__C_BEGIN__` and a word
    /// referring to it (crtbeginT.o's shape on x86-64 and i386).
    fn sources(arch: Arch) -> [(&'static str, String); 3] {
        let (ty, word, ret, align, code_align, data_align, ra, def_cfa) = match arch {
            Arch::Aarch64 => ("%function", ".quad", "ret", 3, 4, -8, 30, "0x0c, 31, 0"),
            Arch::X86_64 => ("@function", ".quad", "ret", 3, 1, -8, 16, "0x0c, 7, 8"),
            Arch::I386 => ("@function", ".long", "ret", 2, 1, -4, 8, "0x0c, 4, 4"),
        };
        let sec = if arch == Arch::Aarch64 { "%progbits" } else { "@progbits" };
        let a = format!(
            "\t.text\n\t.globl _start\n\t.type _start,{ty}\n_start:\n\t.cfi_startproc\n\
             \t{ret}\n\t.cfi_endproc\n\t.size _start, .-_start\n"
        );
        let b = format!(
            "\t.text\n\t.globl getf\n\t.type getf,{ty}\ngetf:\n\t{ret}\n\t{ret}\n\t{ret}\n\t{ret}\n\
             \t.size getf, .-getf\n\
             \t.data\n\t.globl words\nwords:\n\t{word} __B_BEGIN__\n\t{word} __B_FDE__\n\
             \t.section .eh_frame,\"a\",{sec}\n\t.p2align {align}\n__B_BEGIN__:\n\
             .Lcie:\t.4byte .Lcie_end - .Lcie_start\n.Lcie_start:\n\t.4byte 0\n\t.byte 1\n\
             \t.asciz \"zRS\"\n\t.uleb128 {code_align}\n\t.sleb128 {data_align}\n\t.uleb128 {ra}\n\
             \t.uleb128 1\n\t.byte 0x1b\n\t.byte {def_cfa}\n\t.p2align 2\n.Lcie_end:\n\
             __B_FDE__:\n\t.4byte .Lfde_end - .Lfde_start\n.Lfde_start:\n\
             \t.4byte .Lfde_start - .Lcie\n\t.4byte getf - .\n\t.4byte 4\n\t.uleb128 0\n\
             \t.p2align 2\n.Lfde_end:\n"
        );
        let c = format!(
            "\t.data\n\t.globl cwords\ncwords:\n\t{word} __C_BEGIN__\n\
             \t.section .eh_frame,\"a\",{sec}\n\t.p2align {align}\n__C_BEGIN__:\n"
        );
        [("a", a), ("b", b), ("c", c)]
    }

    fn run(dir: &Path, program: &Path, args: &[&str]) {
        let mut c = std::process::Command::new(program);
        c.current_dir(dir).args(args);
        let out = run_with(&mut c, b"", mortar_testutil::DEFAULT_TIMEOUT);
        assert!(out.success(), "{} {args:?}: {}", program.display(), out.stderr_text());
    }

    /// A section of the output: address, size, file offset.
    fn section(bytes: &[u8], name: &str) -> (u64, u64, u64) {
        let h = mortar_elf::FileHeader::parse(bytes).unwrap();
        let t = mortar_elf::SectionTable::parse(bytes, &h).unwrap();
        let s = t.iter().find(|s| s.name().is_ok_and(|n| n == name.as_bytes())).unwrap();
        (s.header().sh_addr, s.header().sh_size, s.header().sh_offset)
    }

    /// The GNU ld link of the three files, its map read as mortar's.
    #[derive(Clone)]
    pub struct Built {
        pub arch: Arch,
        pub output: Vec<u8>,
        pub text: String,
        pub files: Vec<(String, Vec<u8>)>,
    }

    impl Built {
        fn wide(&self) -> bool {
            self.arch != Arch::I386
        }
        fn word(&self) -> usize {
            if self.wide() { 8 } else { 4 }
        }
        /// The file offset of `.symtab` entry `name`'s `st_value`.
        fn sym_value_at(&self, name: &str) -> usize {
            let (_, size, off) = section(&self.output, ".symtab");
            let (_, _, stroff) = section(&self.output, ".strtab");
            let (ent, val) = if self.wide() { (24, 8) } else { (16, 4) };
            (0..size as usize / ent)
                .map(|i| off as usize + i * ent)
                .find(|&e| {
                    let n = u32::from_le_bytes(self.output[e..e + 4].try_into().unwrap()) as usize;
                    let s = &self.output[stroff as usize + n..];
                    s.starts_with(name.as_bytes()) && s.get(name.len()) == Some(&0)
                })
                .map_or_else(|| panic!("no symbol {name}"), |e| e + val)
        }
        fn read(&self, at: usize) -> u64 {
            self.output[at..at + self.word()].iter().rev().fold(0, |v, &b| v << 8 | u64::from(b))
        }
        fn write(&mut self, at: usize, v: u64) {
            let w = self.word();
            self.output[at..at + w].copy_from_slice(&v.to_le_bytes()[..w]);
        }
        pub fn sym(&self, name: &str) -> u64 {
            self.read(self.sym_value_at(name))
        }
        pub fn set_sym(&mut self, name: &str, v: u64) {
            let at = self.sym_value_at(name);
            self.write(at, v);
        }
        /// The file offset of the `n`th word at symbol `name` (in `.data`).
        fn data_at(&self, name: &str, n: usize) -> usize {
            let (addr, _, off) = section(&self.output, ".data");
            (off + (self.sym(name) - addr)) as usize + n * self.word()
        }
        pub fn data(&self, name: &str, n: usize) -> u64 {
            self.read(self.data_at(name, n))
        }
        pub fn set_data(&mut self, name: &str, n: usize, v: u64) {
            let at = self.data_at(name, n);
            self.write(at, v);
        }
        pub fn eh_frame(&self) -> u64 {
            section(&self.output, ".eh_frame").0
        }
        /// The verifier's findings with the map read in `format`, each
        /// `"<id> <message>"`. On x86 GNU ld's map lists its own empty
        /// sections (`.got`, `.iplt`, ...) under the first input, which a
        /// map in mortar's format may not do (M1): those lines are an
        /// artefact of the conversion and are not counted.
        pub fn judge(&self, format: Format) -> Vec<String> {
            let mut map = mortar_plumb::map::parse(Format::Gnu, &self.text).unwrap();
            map.format = format;
            let map = if format == Format::Mortar {
                let t = mortar_plumb::map::mortar::write(&map);
                mortar_plumb::map::parse(Format::Mortar, &t).unwrap()
            } else {
                map
            };
            let inputs: Vec<Input<'_>> =
                self.files.iter().map(|(p, b)| Input { path: p, bytes: b }).collect();
            let r = mortar_plumb::verify(&self.output, &inputs, &map);
            r.findings
                .iter()
                .map(|f| format!("{} {}", f.rule.id(), f.message))
                .filter(|f| !(f.starts_with("M1 ") && f.contains("a.o has no section")))
                .collect()
        }
    }

    pub fn build(arch: Arch) -> Option<(Scratch, Built)> {
        let asm = arch.tool("as")?;
        let ld = arch.tool("ld.bfd")?;
        let dir = Scratch::new(&format!("s50-{}", arch.triple()));
        let mut objs = Vec::new();
        for (name, text) in sources(arch) {
            std::fs::write(dir.join(format!("{name}.s")), text).unwrap();
            let mut args = vec![format!("{name}.s"), "-o".into(), format!("{name}.o")];
            if arch == Arch::I386 {
                args.insert(0, "--32".into());
            }
            let a: Vec<&str> = args.iter().map(String::as_str).collect();
            run(dir.path(), &asm, &a);
            objs.push(format!("{name}.o"));
        }
        let mut args = vec!["-e", "_start", "-o", "out", "-Map=out.map", "-z", "noexecstack"];
        if arch == Arch::I386 {
            args.extend(["-m", "elf_i386"]);
        }
        args.extend(objs.iter().map(String::as_str));
        run(dir.path(), &ld, &args);
        let output = std::fs::read(dir.join("out")).unwrap();
        let text = std::fs::read_to_string(dir.join("out.map")).unwrap();
        let files = objs.iter().map(|o| (o.clone(), std::fs::read(dir.join(o)).unwrap())).collect();
        Some((dir, Built { arch, output, text, files }))
    }
}

fn s50_archs() -> [mortar_testutil::target::Arch; 3] {
    use mortar_testutil::target::Arch;
    [Arch::Aarch64, Arch::X86_64, Arch::I386]
}

// GNU ld places `b.o`'s CIE after `a.o`'s records and gives offset 0 of
// `b.o`'s `.eh_frame` that record's place (rule 2's value; lld does the
// same), and offset 0 of `c.o`'s empty one the place after `b.o`'s
// records. Judged as GNU ld's own output: clean. The same bytes judged
// under mortar's map, where S-50 rule 1 holds, are the mutant: the words
// and the labels are found (R1, Y4). With them at the section's start
// (what mortar writes) the output is ACCEPTED, and the FDE's reference
// (offset 0x18) still follows its record.
#[test]
fn s4_4_eh_frame_offset_zero_is_the_output_section_start() {
    use mortar_plumb::Format;
    for arch in s50_archs() {
        let Some((_dir, mut b)) = s50::build(arch) else { return };
        let start = b.eh_frame();
        let begin = b.sym("__B_BEGIN__");
        assert_ne!(begin, start, "{arch:?}: b.o's CIE is not the first record");
        assert_eq!(b.data("words", 0), begin, "{arch:?}: GNU ld gives the record's place");
        assert_eq!(b.data("words", 1), begin + 0x18, "{arch:?}: the FDE follows its record");
        assert_ne!(b.data("cwords", 0), start, "{arch:?}: GNU ld's empty .eh_frame");
        assert!(b.judge(Format::Gnu).is_empty(), "{arch:?}: {:#?}", b.judge(Format::Gnu));

        // The mutant: rule 2's value under mortar's map.
        let found = b.judge(Format::Mortar);
        let has = |id: &str, needle: &str| {
            found.iter().any(|f| f.starts_with(&format!("{id} ")) && f.contains(needle))
        };
        assert!(has("Y4", "'__B_BEGIN__'"), "{arch:?}: {found:#?}");
        assert!(has("Y4", "'__C_BEGIN__'"), "{arch:?}: {found:#?}");
        assert_eq!(
            found.iter().filter(|f| f.starts_with("R1 ")).count(),
            2,
            "{arch:?}: words[0] and cwords[0]: {found:#?}"
        );
        assert_eq!(found.len(), 4, "{arch:?}: {found:#?}");

        // The accepted shape: offset 0 is the section's start.
        b.set_data("words", 0, start);
        b.set_data("cwords", 0, start);
        b.set_sym("__B_BEGIN__", start);
        b.set_sym("__C_BEGIN__", start);
        let accepted = b.judge(Format::Mortar);
        assert!(accepted.is_empty(), "{arch:?}: {accepted:#?}");

        // Elsewhere is found: one word past the start, and the FDE's
        // reference taken to the start as well.
        for (name, n, v) in [("words", 0, start + 4), ("cwords", 0, start + 4), ("words", 1, start)]
        {
            let mut m = b.clone();
            m.set_data(name, n, v);
            let f = m.judge(Format::Mortar);
            assert!(
                f.len() == 1 && f[0].starts_with("R1 "),
                "{arch:?}: {name}[{n}] = {v:#x}: {f:#?}"
            );
        }
        let mut m = b.clone();
        m.set_sym("__B_BEGIN__", start + 4);
        let f = m.judge(Format::Mortar);
        assert!(f.len() == 1 && f[0].starts_with("Y4 "), "{arch:?}: {f:#?}");
    }
}
