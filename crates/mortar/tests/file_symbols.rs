//! WP-255d2b: a CHANGED file's `.symtab` input from its ONE producers
//! (`mortar::update::file_symbols::file_symbols`: the full link's keep rule
//! of ONE file, `mortar_synth::symtab::file_locals`, over the file's own
//! symbol table in the update's ids).
//!
//! `file_symbols_of_an_edited_file`: `m1.o` of `lib2.a` edited into C++
//! (a function renamed, a global added, a static function and a static
//! variable added, a function-local static table, `.L` strings in a merge
//! section), opened through the OLD state with `Ids::Update`. The oracle is
//! the FULL link of the new inputs: its OUTPUT's `.symtab`, read back with
//! `mortar_elf`, the block of `m1.o` (from its `STT_FILE` entry to the next
//! one). Entry for entry, in order: name, `st_info`, `st_other`, `st_size`;
//! the output section by NAME (the input section's `OUTSECS` row through
//! `changed_facts`); an absolute local's value; per input section one
//! constant `st_value - value` (the section's address), inside its output
//! section. `plan` itself needs the join's placement (not this package's):
//! this compares the plan's INPUT with what the full link writes for the
//! file.
//!
//! On AArch64 and x86-64.
#![allow(clippy::too_many_lines, reason = "one comparison after the other")]

mod support;
#[path = "support/update.rs"]
#[allow(dead_code, reason = "the shared helpers; this test uses some")]
mod update;

use std::collections::BTreeMap;

use mortar::update::file_front::{FrontCache, Ids, front_of};
use mortar::update::file_split::{SectionFact, changed_facts, split_of};
use mortar::update::file_symbols::file_symbols;
use mortar_core::id::{FileId, SectionId};
use mortar_core::identity::Anchor;
use mortar_core::resolution::SectionKind;
use mortar_elf::consts::{SHT_SYMTAB, STB_LOCAL, STT_FILE, STT_SECTION};
use mortar_elf::{FileHeader, SectionIndex, SectionTable, SymbolIndex};
use mortar_incr::update::rows::FileRow;
use mortar_merge::phase::ArchRelocs;
use update::{first_free, lines, member, pool, state, whole};

/// `m1.o` edited (C++): `wp255d_m1_renamed` (the new name of the old body),
/// `wp255d2b_added` (a new global), `wp255d2b_helper` and `wp255d2b_counter`
/// (static: locals), `t` (a function-local static table), the strings (`.LC*`
/// in `.rodata.str1.*`, when the assembler keeps them) and `.Lwp255d2b_s`
/// (a `.L` local in a merge section, kept by the assembler for its
/// relocation's addend: dropped by S-78's default).
const M1_SYM: &str = "__asm__(\".section .rodata.wp255d2b,\\\"aMS\\\",%progbits,1\\n.Lwp255d2b_s:\\n\"\n\
    \"\\t.asciz \\\"wp255d2b\\\"\\n\\t.data\\n\\t.globl wp255d2b_p\\nwp255d2b_p:\\n\"\n\
    \"\\t.dc.a .Lwp255d2b_s+1\\n\\t.text\\n\");\n\
    extern \"C\" { int wp255d_m1_data = 1; int wp255d2b_added = 4; }\n\
    static int wp255d2b_counter;\n\
    static __attribute__((noinline)) int wp255d2b_helper(int x) \
    { wp255d2b_counter += x; return wp255d2b_counter * 3; }\n\
    extern \"C\" const char *wp255d2b_str(int i) { static const char *const t[] = \
    { \"wp255d2b-alpha\", \"wp255d2b-beta\" }; return t[i & 1]; }\n\
    extern \"C\" __attribute__((noinline)) int wp255d_m1_renamed(int x) \
    { return wp255d2b_helper(x) + *wp255d2b_str(x) + wp255d2b_added; }\n\
    extern \"C\" int wp255d_m1(int x) { return wp255d_m1_renamed(x) + wp255d_m1_data; }\n";

const SHN_ABS: u16 = 0xfff1;

/// One output entry, or one input local, as compared.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: Vec<u8>,
    info: u8,
    other: u8,
    size: u64,
    /// The output section's name, or `None` for an absolute entry.
    out: Option<Vec<u8>>,
    /// An absolute entry's value.
    abs: Option<u64>,
}

#[test]
fn file_symbols_of_an_edited_file() {
    for line in lines(&[("m1s", "c++", M1_SYM)]) {
        let args = line.args(&["wp255d_m2", "wp255d_m1"]);
        line.archive(&["m1.o", "m2.o"]);
        let old = state(&line, &args, "old");
        std::fs::copy(line.dir.join("m1s.o"), line.dir.join("m1.o")).expect("the edit");
        line.archive(&["m1.o", "m2.o"]);
        // The full link of the new inputs: its output is the oracle.
        let new = state(&line, &args, "new");
        let store = support::store();
        let full = whole(&line, &args, &store);
        let file = member(&old, "m1.o");
        let captured = new.file(member(&new, "m1.o")).expect("FILES");
        let stored = old.file(file).expect("FILES");
        let row = FileRow { base: captured.base, len: captured.len, hash: captured.hash, ..stored };
        let first = first_free(&old);
        let fresh = support::store();
        let mut cache = FrontCache::new(&fresh, pool(), &line.dir);
        let ff =
            front_of(&mut cache, &old, file, Ids::Update { first_section: SectionId(first), row })
                .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let mut keepers: Vec<(Vec<u8>, Option<FileId>)> = Vec::new();
        for s in &ff.sections {
            let Anchor::Comdat(sig) = s.anchor else { continue };
            assert!(keepers.iter().all(|k| k.0 != sig), "no COMDAT group in this edit");
            keepers.push((sig.to_vec(), Some(file)));
        }
        keepers.sort();
        let facts = changed_facts(ff, &old, &full.plan, &keepers)
            .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let null = SectionFact { kind: SectionKind::Null, kept: false, out: None };
        let by_index = |i: SectionIndex| facts.get(i.0 as usize).copied().unwrap_or(null);
        let split = split_of(ff, &old, &by_index, &ArchRelocs(full.arch.get()), &full.pool)
            .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let got = file_symbols(ff, &facts, full.plan.shaping.discard, &split, &[])
            .unwrap_or_else(|e| panic!("{}: {e:?}", line.label));
        let what = |s: &str| format!("{}: m1.o: {s}", line.label);
        assert_eq!(got.file, file, "{}", what("the file"));
        assert_eq!(got.pieces, &split.pieces[..], "{}", what("pieces"));
        let outsecs = old.outsecs().expect("OUTSECS");
        let have: Vec<Entry> = got
            .locals
            .iter()
            .map(|l| {
                let out = l.section.map(|s| {
                    let fact = facts[(s.0 - first) as usize];
                    let o = fact.out.expect("a kept local's section has an output section");
                    old.string(outsecs[o.0 as usize].name).expect("name")
                });
                Entry {
                    name: l.name.to_vec(),
                    info: l.info,
                    other: l.other,
                    size: l.size,
                    abs: l.section.is_none().then_some(l.value),
                    out,
                }
            })
            .collect();
        // The full link's output: m1.o's block.
        let bytes = std::fs::read(line.dir.join("new")).expect("the output");
        let header = FileHeader::parse(&bytes).expect("an ELF header");
        let table = SectionTable::parse(&bytes, &header).expect("section headers");
        let syms = table.find_symbol_table(SHT_SYMTAB).expect("symbols").expect("a .symtab");
        let mut want = Vec::new();
        let mut addrs = Vec::new();
        let mut inside = false;
        let mut fillers = 0;
        for (_, s) in syms.locals() {
            let name = syms.name(&s).expect("name");
            if s.st_info & 0xf == STT_FILE {
                inside = name.ends_with(b"m1s.src");
                if !inside && !want.is_empty() {
                    break;
                }
            }
            if !inside {
                continue;
            }
            // IN-49a's fillers after the block (incremental mode): not entries.
            if s.st_info == STT_SECTION && name.is_empty() && s.st_shndx == 1 {
                fillers += 1;
                continue;
            }
            assert_eq!(s.st_info >> 4, STB_LOCAL);
            let (out, abs) = if s.st_shndx == SHN_ABS {
                (None, Some(s.st_value))
            } else {
                let sec = table.get(SectionIndex(u32::from(s.st_shndx))).expect("section");
                let sh = sec.header();
                assert!(
                    s.st_value >= sh.sh_addr && s.st_value <= sh.sh_addr + sh.sh_size,
                    "{}",
                    what("an entry inside its output section")
                );
                (Some(sec.name().expect("name").to_vec()), None)
            };
            addrs.push(s.st_value);
            want.push(Entry {
                name: name.to_vec(),
                info: s.st_info,
                other: s.st_other,
                size: s.st_size,
                out,
                abs,
            });
        }
        assert!(!want.is_empty(), "{}", what("the block in the full link's output"));
        assert_eq!(have, want, "{}", what("the kept locals, in S-74's order"));
        // Per input section one address (st_value - value).
        let mut base: BTreeMap<SectionId, u64> = BTreeMap::new();
        for (l, &addr) in got.locals.iter().zip(&addrs) {
            let Some(s) = l.section else { continue };
            let b = addr.wrapping_sub(l.value);
            assert_eq!(*base.entry(s).or_insert(b), b, "{}", what("one address per input section"));
        }
        // The cases the edit covers, counted on the input.
        let input = ff.object.symbols().expect("symbols").expect("a .symtab");
        let mut dot_l = 0;
        let named = |n: &[u8]| have.iter().any(|e| e.name.windows(n.len()).any(|w| w == n));
        for i in 1..input.first_global().0 {
            let s = input.get(SymbolIndex(i)).expect("sym");
            if input.name(&s).expect("name").starts_with(b".L") {
                dot_l += 1;
            }
        }
        let cases: BTreeMap<&str, usize> = [
            ("input locals", input.first_global().0 as usize - 1),
            ("kept", have.len()),
            (".L in the input (dropped)", dot_l),
            ("sections", base.len()),
            ("eh", usize::from(got.eh.is_some())),
            ("fillers after the block", fillers),
        ]
        .into_iter()
        .collect();
        eprintln!("{}: m1.o edited: {cases:?}", line.label);
        assert!(named(b"wp255d2b_helper") && named(b"wp255d2b_counter"), "{}", what("statics"));
        assert!(dot_l > 0, "{}", what("a .L local in the input"));
        assert!(have.iter().all(|e| !e.name.starts_with(b".L")), "{}", what(".L dropped"));
    }
}
