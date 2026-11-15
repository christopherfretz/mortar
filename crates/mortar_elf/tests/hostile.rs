//! L0: the hostile-input contract of `spec/elf.md` §4.2. For small valid
//! objects of both classes, every truncation and a deterministic sweep of
//! mutations of the header and table fields: `Object::parse` and a walk
//! that calls every accessor return `Ok` or `Err`; they never panic, and
//! the work done is bounded by the size of the file.

mod support;

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64, SHN_XINDEX, SHT_STRTAB, SHT_SYMTAB};
use mortar_elf::{Class, Shdr, Sym};
use support::{
    Builder, Built, Rng, Walk, enc_sym, machine_name, sample, shdr_size, sym_size, walk,
};

/// Outcomes of a sweep.
#[derive(Debug, Default)]
struct Tally {
    inputs: u64,
    /// Inputs `Object::parse` refused.
    refused: u64,
    /// Inputs that parsed and whose walk met at least one error.
    partial: u64,
    /// Inputs that parsed and walked without an error.
    clean: u64,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.inputs += other.inputs;
        self.refused += other.refused;
        self.partial += other.partial;
        self.clean += other.clean;
    }
}

/// Walks `data`; a panic becomes a failure that says which input it was.
/// The walk's work must be bounded by the file: no accessor visits more
/// records than the bytes it was given can hold.
fn check(data: &[u8], tally: &mut Tally, what: &dyn Fn() -> String) {
    let result = catch_unwind(AssertUnwindSafe(|| walk(data)));
    let Ok(result) = result else { panic!("the reader panicked on {}", what()) };
    tally.inputs += 1;
    match result {
        Err(error) => {
            assert!(!error.to_string().is_empty());
            tally.refused += 1;
        }
        Ok(Walk { errors, steps, sections, .. }) => {
            // Each section's accessors see at most the whole file, and a
            // record is at least a byte: a generous bound that an
            // unbounded loop would still break.
            let bound = (sections + 2) * (data.len() as u64 + 64);
            assert!(steps <= bound, "{steps} steps for {} bytes on {}", data.len(), what());
            if errors == 0 {
                tally.clean += 1;
            } else {
                tally.partial += 1;
            }
        }
    }
}

/// The same object laid out as file header, section header table, name
/// table, then everything else, so that a truncation removes section
/// contents and leaves the headers and the names: the lazily checked
/// accessors are what meets the damage.
fn table_first(built: &Built) -> Built {
    let ehsize = if built.class == Class::Elf32 { 52 } else { 64 };
    let table_len = built.bytes.len() - built.shoff;
    // The builder writes `.shstrtab` last, just before the table.
    let last = built.count - 1;
    let object = mortar_elf::Object::parse(&built.bytes).expect("valid");
    let names = object.section(mortar_elf::SectionIndex(last)).unwrap().file_range().unwrap();
    let names = names.expect("the name table has contents");
    let shift = (table_len + names.len()) as u64;
    let mut moved = built.clone();
    for index in 0..last {
        moved.set_shdr(index, |s| {
            if s.sh_offset != 0 {
                s.sh_offset += shift;
            }
        });
    }
    moved.set_shdr(last, |s| s.sh_offset = (ehsize + table_len) as u64);
    moved.set_ehdr(|e| e.e_shoff = ehsize as u64);
    let mut bytes = moved.bytes[..ehsize].to_vec();
    bytes.extend_from_slice(&moved.bytes[built.shoff..]);
    bytes.extend_from_slice(&built.bytes[names.clone()]);
    bytes.extend_from_slice(&built.bytes[ehsize..names.start]);
    moved.bytes = bytes;
    moved.shoff = ehsize;
    moved
}

/// A small object whose symbols use the extended section index table.
fn with_extended_indexes(class: Class, e_machine: u16) -> Built {
    let mut b = Builder::new(class, e_machine);
    b.progbits(".text", 1, 6, vec![0x90; 4]);
    let symbols: Vec<u8> = [
        Sym::default(),
        Sym { st_name: 1, st_info: 0x12, st_shndx: SHN_XINDEX, ..Sym::default() },
        Sym { st_name: 3, st_info: 0x10, st_shndx: 1, st_value: 2, ..Sym::default() },
    ]
    .iter()
    .flat_map(|s| enc_sym(class, s))
    .collect();
    b.add(
        ".symtab",
        Shdr {
            sh_type: SHT_SYMTAB,
            sh_link: 4,
            sh_info: 1,
            sh_entsize: sym_size(class),
            ..Shdr::default()
        },
        symbols,
    );
    b.add(
        ".symtab_shndx",
        Shdr {
            sh_type: mortar_elf::consts::SHT_SYMTAB_SHNDX,
            sh_link: 2,
            sh_entsize: 4,
            ..Shdr::default()
        },
        support::words(&[0, 1, 0]),
    );
    b.progbits(".strtab", SHT_STRTAB, 0, b"\0f\0g\0".to_vec());
    b.build()
}

/// The objects every sweep runs over: the sample of each target, each in
/// both layouts, and the extended-index objects. Two of the three targets
/// are `ELFCLASS64` and one is `ELFCLASS32`.
fn subjects() -> Vec<(String, Built)> {
    let mut subjects = Vec::new();
    for (class, e_machine) in support::TARGETS {
        let name = machine_name(e_machine);
        let built = sample(class, e_machine);
        assert_eq!(walk(&built.bytes).expect("valid").errors, 0);
        let first = table_first(&built);
        assert_eq!(walk(&first.bytes), walk(&built.bytes), "{name}: table-first walks the same");
        subjects.push((format!("{name} sample"), built));
        subjects.push((format!("{name} sample, table first"), first));
    }
    for (class, e_machine) in [(Class::Elf32, EM_386), (Class::Elf64, EM_X86_64)] {
        let built = with_extended_indexes(class, e_machine);
        assert_eq!(walk(&built.bytes).expect("valid").errors, 0);
        let name = machine_name(e_machine);
        subjects.push((format!("{name} extended indexes, table first"), table_first(&built)));
    }
    subjects
}

/// §4.2: every prefix of a valid object, from the empty file to the file
/// less one byte, is read without a panic. With the table at the end
/// every proper prefix is refused by `parse`; with the table first the
/// prefixes that keep the name table parse and the accessors of the cut
/// sections report the damage.
#[test]
fn s4_2_every_truncation_is_an_answer() {
    let mut total = Tally::default();
    for (name, built) in subjects() {
        let mut tally = Tally::default();
        for len in 0..built.bytes.len() {
            check(&built.bytes[..len], &mut tally, &|| format!("{name} cut to {len} bytes"));
        }
        assert_eq!(tally.inputs, built.bytes.len() as u64);
        if name.ends_with("table first") {
            assert!(tally.partial > 0, "{name}: {tally:?}");
        } else {
            assert_eq!(tally.refused, tally.inputs, "{name}: {tally:?}");
        }
        // Whatever follows the file is not the file's business.
        let mut longer = built.bytes.clone();
        longer.extend_from_slice(&[0xa5; 37]);
        assert_eq!(walk(&longer), walk(&built.bytes), "{name}");
        eprintln!("{name}: {} bytes, {tally:?}", built.bytes.len());
        total.add(&tally);
    }
    eprintln!("truncations: {total:?}");
    assert!(total.inputs > 8_000 && total.partial > 1_000, "{total:?}");
}

/// Values that sit on the edges the reader checks, for a field of `width`
/// bytes in a file of `file_len` bytes.
fn interesting(width: usize, file_len: u64) -> Vec<u64> {
    let max = if width == 8 { u64::MAX } else { (1u64 << (width * 8)) - 1 };
    let mut values = vec![
        0,
        1,
        2,
        3,
        4,
        7,
        8,
        12,
        16,
        23,
        24,
        40,
        64,
        0xff,
        0xff00,
        0xfff1,
        0xfff2,
        0xffff,
        0x1_0000,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_fff0,
        0xffff_ffff,
        0x1_0000_0000,
        (1 << 63) - 1,
        1 << 63,
        u64::MAX - 7,
        u64::MAX,
        file_len - 1,
        file_len,
        file_len + 1,
        file_len / 2,
        max.wrapping_sub(file_len),
        max.wrapping_sub(file_len).wrapping_add(1),
    ];
    for value in &mut values {
        *value &= max;
    }
    values.sort_unstable();
    values.dedup();
    values
}

fn poke(bytes: &mut [u8], offset: usize, width: usize, value: u64) {
    if let Some(field) = bytes.get_mut(offset..offset + width) {
        field.copy_from_slice(&value.to_le_bytes()[..width]);
    }
}

/// The byte ranges that hold header and table fields: the file header,
/// the section header table, and the contents of every section that is a
/// table of records the reader decodes.
fn field_ranges(built: &Built) -> Vec<std::ops::Range<usize>> {
    let ehsize = if built.class == Class::Elf32 { 52 } else { 64 };
    let table = built.shoff..built.shoff + built.count as usize * shdr_size(built.class);
    let mut ranges = vec![0..ehsize, table];
    let object = mortar_elf::Object::parse(&built.bytes).expect("valid");
    for section in object.sections() {
        let compressed = section.flags() & mortar_elf::consts::SHF_COMPRESSED != 0;
        let table_like = !matches!(section.sh_type(), 0 | 1 | 8) || compressed;
        if let (true, Ok(Some(range))) = (table_like, section.file_range()) {
            ranges.push(range);
        }
    }
    ranges
}

/// §4.2: every field-sized slot of the file header, the section header
/// table and the record tables, set to every edge value in every width.
#[test]
fn s4_2_every_field_at_every_edge_is_an_answer() {
    let mut total = Tally::default();
    for (name, built) in subjects() {
        let mut tally = Tally::default();
        let file_len = built.bytes.len() as u64;
        for range in field_ranges(&built) {
            for offset in range.clone() {
                for width in [1usize, 2, 4, 8] {
                    // Fields are aligned to their width from the start of
                    // their table.
                    if (offset - range.start) % width != 0 {
                        continue;
                    }
                    for value in interesting(width, file_len) {
                        let mut bytes = built.bytes.clone();
                        poke(&mut bytes, offset, width, value);
                        check(&bytes, &mut tally, &|| {
                            format!("{name} with {value:#x} in {width} bytes at offset {offset}")
                        });
                    }
                }
            }
        }
        assert!(tally.refused > 0 && tally.partial > 0 && tally.clean > 0, "{name}: {tally:?}");
        eprintln!("{name}: {tally:?}");
        total.add(&tally);
    }
    eprintln!("edge values: {total:?}");
    assert!(total.inputs > 400_000, "{total:?}");
}

/// §4.2: a deterministic pseudo-random sweep: one to four mutations at a
/// time (an edge value or a random value in a field slot, a flipped bit,
/// a byte anywhere), so that damage in one table meets damage in another.
#[test]
fn s4_2_random_mutations_are_answers() {
    const ROUNDS: u64 = 40_000;
    let mut total = Tally::default();
    for (subject, (name, built)) in subjects().into_iter().enumerate() {
        let mut tally = Tally::default();
        let file_len = built.bytes.len() as u64;
        let ranges = field_ranges(&built);
        let mut rng = Rng(0x6d6f_7274_6172 ^ ((subject as u64) << 48));
        for round in 0..ROUNDS {
            let mut bytes = built.bytes.clone();
            let mut log = String::new();
            for _ in 0..=rng.below(4) {
                let range = &ranges[rng.below(ranges.len() as u64) as usize];
                let width = [1usize, 2, 4, 8][rng.below(4) as usize];
                let slots = (range.len() / width).max(1) as u64;
                let offset = range.start + rng.below(slots) as usize * width;
                match rng.below(8) {
                    0..=3 => {
                        let values = interesting(width, file_len);
                        let value = values[rng.below(values.len() as u64) as usize];
                        poke(&mut bytes, offset, width, value);
                        let _ = write!(log, " [{offset}; {width}] = {value:#x}");
                    }
                    4 | 5 => {
                        let value = rng.next();
                        poke(&mut bytes, offset, width, value);
                        let _ = write!(log, " [{offset}; {width}] = {value:#x}");
                    }
                    6 => {
                        let bit = rng.below(8);
                        bytes[offset] ^= 1 << bit;
                        let _ = write!(log, " [{offset}] ^= 1 << {bit}");
                    }
                    _ => {
                        let anywhere = rng.below(file_len) as usize;
                        let value = rng.next() as u8;
                        bytes[anywhere] = value;
                        let _ = write!(log, " [{anywhere}] = {value:#x}");
                    }
                }
            }
            check(&bytes, &mut tally, &|| format!("{name}, round {round}:{log}"));
        }
        assert!(tally.refused > 0 && tally.partial > 0 && tally.clean > 0, "{name}: {tally:?}");
        eprintln!("{name}: {tally:?}");
        total.add(&tally);
    }
    eprintln!("random mutations: {total:?}");
    assert_eq!(total.inputs, ROUNDS * subjects().len() as u64);
}

/// §4.2: a file header that claims the largest possible tables in a file
/// that is only a header is refused at once: nothing is sized by a count
/// the file states.
#[test]
fn s4_2_huge_claims_cost_nothing() {
    for (class, e_machine) in [(Class::Elf64, EM_AARCH64), (Class::Elf32, EM_386)] {
        let mut built = Builder::new(class, e_machine).build();
        let max = if class == Class::Elf32 { u64::from(u32::MAX) } else { u64::MAX };
        // Header 0 claims 2^32 - 1 sections; the name table index escapes.
        built.set_shdr(0, |s| {
            s.sh_size = u64::from(u32::MAX);
            s.sh_link = u32::MAX;
        });
        built.set_ehdr(|e| {
            e.e_shnum = 0;
            e.e_shstrndx = SHN_XINDEX;
        });
        assert!(walk(&built.bytes).is_err());
        built.set_shdr(0, |s| s.sh_size = max);
        assert!(walk(&built.bytes).is_err());
        // A symbol table and a relocation section that claim the address
        // space: errors of their accessors.
        let mut built = sample(class, e_machine);
        for index in [3, 4, 10] {
            built.set_shdr(index, |s| s.sh_size = max - (max % s.sh_entsize));
        }
        let found = walk(&built.bytes).expect("the header is intact");
        assert!(found.errors >= 3, "{found:?}");
        assert_eq!((found.symbols, found.relocations), (0, 0));
    }
}
