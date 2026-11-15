//! L0: the hostile-input contract of `spec/elf.md` §6.12 for the
//! shared-object reader. For the sample shared object of each target (one
//! hash style each), read by its sections, by its dynamic table, and
//! stripped of its sections: every truncation, every header, table and
//! dynamic-content field at every edge value, and a deterministic random
//! sweep. `SharedObject::parse` and a walk that calls every accessor
//! return `Ok` or `Err`; they never panic, and the work done is bounded by
//! the size of the file.

mod support;

use std::fmt::Write as _;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use mortar_elf::Entry;
use mortar_elf::consts::{EM_386, EM_AARCH64, EM_X86_64};
use support::dso::{DsoWalk, HASH_STYLES, dso_sample, phdr_size, walk_dso};
use support::{Rng, TARGETS, machine_name, shdr_size};

/// Outcomes of a sweep.
#[derive(Debug, Default)]
struct Tally {
    inputs: u64,
    /// Inputs `parse` refused.
    refused: u64,
    /// Inputs that parsed and whose walk met more errors than the intact
    /// subject's.
    partial: u64,
    /// Inputs that parsed and walked with no more errors than the intact
    /// subject's (the walk asks for some errors on purpose: `unique` of a
    /// repeated segment type, the notes of a segment that is not a note).
    clean: u64,
    /// The intact subject's walk errors.
    baseline: u64,
}

impl Tally {
    fn of(subject: &Subject) -> Tally {
        let walk = walk_dso(&subject.bytes, subject.entry).expect("the intact subject parses");
        Tally { baseline: walk.errors, ..Tally::default() }
    }

    fn add(&mut self, other: &Tally) {
        self.inputs += other.inputs;
        self.refused += other.refused;
        self.partial += other.partial;
        self.clean += other.clean;
    }
}

/// A subject: a name, the bytes, the entry path, and the byte ranges that
/// hold fields the reader decodes.
struct Subject {
    name: String,
    bytes: Vec<u8>,
    entry: Option<Entry>,
    fields: Vec<Range<usize>>,
}

/// Walks `data`; a panic becomes a failure that says which input it was.
/// No accessor visits more records than the bytes can hold: the walk's
/// steps are bounded by a small multiple of the file's size.
fn check(data: &[u8], entry: Option<Entry>, tally: &mut Tally, what: &dyn Fn() -> String) {
    let result = catch_unwind(AssertUnwindSafe(|| walk_dso(data, entry)));
    let Ok(result) = result else { panic!("the reader panicked on {}", what()) };
    tally.inputs += 1;
    match result {
        Err(error) => {
            let text = error.to_string();
            assert!(!text.is_empty() && !text.contains('\n'), "{}: {text:?}", what());
            tally.refused += 1;
        }
        Ok(DsoWalk { errors, steps, .. }) => {
            // Each symbol is visited a handful of times (name, section,
            // version, segment); every other record once or twice.
            let bound = 16 * (data.len() as u64 + 64);
            assert!(steps <= bound, "{steps} steps for {} bytes on {}", data.len(), what());
            if errors <= tally.baseline {
                tally.clean += 1;
            } else {
                tally.partial += 1;
            }
        }
    }
}

fn subjects() -> Vec<Subject> {
    let mut subjects = Vec::new();
    for (i, (class, machine)) in TARGETS.into_iter().enumerate() {
        let hash = HASH_STYLES[i % HASH_STYLES.len()];
        let dso = dso_sample(class, machine, hash);
        let ehsize = if class == mortar_elf::Class::Elf32 { 52 } else { 64 };
        let phdrs = dso.phoff..dso.phoff + dso.phnum as usize * phdr_size(class);
        let shdrs = dso.shoff..dso.shoff + dso.shnum as usize * shdr_size(class);
        // Every part the reader decodes as records or words: the symbol,
        // hash and version tables, the notes, the dynamic table.
        let contents: Vec<Range<usize>> = dso
            .parts
            .iter()
            .filter(|part| {
                !matches!(part.name, ".dynstr" | ".rodata" | ".data" | ".bss" | ".tdata")
            })
            .map(|part| part.offset..part.offset + part.size)
            .collect();
        let label = format!("{} {hash:?}", machine_name(machine));
        let mut fields = vec![0..ehsize, phdrs.clone(), shdrs];
        fields.extend(contents.iter().cloned());
        subjects.push(Subject {
            name: format!("{label} by sections"),
            bytes: dso.bytes.clone(),
            entry: None,
            fields: fields.clone(),
        });
        // Damage to the section header table meets `parse` alike on both
        // paths, and the subject by sections has it.
        fields.remove(2);
        subjects.push(Subject {
            name: format!("{label} by the dynamic table"),
            bytes: dso.bytes.clone(),
            entry: Some(Entry::Dynamic),
            fields,
        });
        let stripped = dso.stripped();
        let mut fields = vec![0..ehsize, phdrs];
        fields.extend(contents.into_iter().filter(|range| range.end <= stripped.len()));
        subjects.push(Subject {
            name: format!("{label} stripped"),
            bytes: stripped,
            entry: None,
            fields,
        });
    }
    subjects
}

/// §6.12: every proper prefix of every subject is an answer.
#[test]
fn s6_12_every_truncation_is_an_answer() {
    let mut total = Tally::default();
    for subject in subjects() {
        let mut tally = Tally::of(&subject);
        for len in 0..subject.bytes.len() {
            check(&subject.bytes[..len], subject.entry, &mut tally, &|| {
                format!("{} cut to {len} bytes", subject.name)
            });
        }
        // Whatever follows the file is not the file's business.
        let mut longer = subject.bytes.clone();
        longer.extend_from_slice(&[0xa5; 37]);
        assert_eq!(
            format!("{:?}", walk_dso(&longer, subject.entry)),
            format!("{:?}", walk_dso(&subject.bytes, subject.entry)),
            "{}",
            subject.name
        );
        eprintln!("{}: {} bytes, {tally:?}", subject.name, subject.bytes.len());
        total.add(&tally);
    }
    eprintln!("truncations: {total:?}");
    assert!(total.inputs > 10_000 && total.partial > 1_000, "{total:?}");
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
        5,
        7,
        8,
        16,
        24,
        0xff,
        0x7fff,
        0x8000,
        0xffff,
        0x1_0000,
        0x6fff_fef5,
        0x6fff_fff0,
        0x6fff_fffc,
        0x6fff_fffe,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_ffff,
        0x1_0000_0000,
        1 << 63,
        u64::MAX - 7,
        u64::MAX,
        file_len - 1,
        file_len,
        file_len + 1,
        file_len / 2,
        0x10000 + file_len / 2,
        max.wrapping_sub(file_len),
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

/// §6.12: every field-sized slot of the headers, the program and section
/// header tables and the decoded contents of the subjects of `e_machine`,
/// set to every edge value in every width.
fn every_field_at_every_edge(e_machine: u16) {
    let mut total = Tally::default();
    let prefix = machine_name(e_machine);
    for subject in subjects().into_iter().filter(|s| s.name.starts_with(&format!("{prefix} "))) {
        let mut tally = Tally::of(&subject);
        let file_len = subject.bytes.len() as u64;
        for range in &subject.fields {
            for offset in range.clone() {
                for width in [1usize, 2, 4, 8] {
                    if (offset - range.start) % width != 0 {
                        continue;
                    }
                    for value in interesting(width, file_len) {
                        let mut bytes = subject.bytes.clone();
                        poke(&mut bytes, offset, width, value);
                        check(&bytes, subject.entry, &mut tally, &|| {
                            format!(
                                "{} with {value:#x} in {width} bytes at offset {offset}",
                                subject.name
                            )
                        });
                    }
                }
            }
        }
        assert!(
            tally.refused > 0 && tally.partial > 0 && tally.clean > 0,
            "{}: {tally:?}",
            subject.name
        );
        eprintln!("{}: {tally:?}", subject.name);
        total.add(&tally);
    }
    eprintln!("{prefix} edge values: {total:?}");
    assert!(total.inputs > 100_000, "{total:?}");
}

#[test]
fn s6_12_every_field_at_every_edge_is_an_answer_aarch64() {
    every_field_at_every_edge(EM_AARCH64);
}

#[test]
fn s6_12_every_field_at_every_edge_is_an_answer_x86_64() {
    every_field_at_every_edge(EM_X86_64);
}

#[test]
fn s6_12_every_field_at_every_edge_is_an_answer_i386() {
    every_field_at_every_edge(EM_386);
}

/// §6.12: a deterministic pseudo-random sweep: one to four mutations at a
/// time (an edge value or a random value in a field slot, a flipped bit, a
/// byte anywhere), so that damage in one table meets damage in another.
#[test]
fn s6_12_random_mutations_are_answers() {
    const ROUNDS: u64 = 20_000;
    let subjects = subjects();
    let mut total = Tally::default();
    for (index, subject) in subjects.iter().enumerate() {
        let mut tally = Tally::of(subject);
        let file_len = subject.bytes.len() as u64;
        let ranges: Vec<&Range<usize>> = subject.fields.iter().filter(|r| !r.is_empty()).collect();
        let mut rng = Rng(0x6473_6f68_6f73 ^ ((index as u64) << 48));
        for round in 0..ROUNDS {
            let mut bytes = subject.bytes.clone();
            let mut log = String::new();
            for _ in 0..=rng.below(4) {
                let range = ranges[rng.below(ranges.len() as u64) as usize];
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
            check(&bytes, subject.entry, &mut tally, &|| {
                format!("{}, round {round}:{log}", subject.name)
            });
        }
        assert!(
            tally.refused > 0 && tally.partial > 0 && tally.clean > 0,
            "{}: {tally:?}",
            subject.name
        );
        eprintln!("{}: {tally:?}", subject.name);
        total.add(&tally);
    }
    eprintln!("random mutations: {total:?}");
    assert_eq!(total.inputs, ROUNDS * subjects.len() as u64);
}
