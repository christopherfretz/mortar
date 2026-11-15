//! `spec/archive.md` §8: any bytes give `Ok` or `Err`, in bounded work.
//! Every truncation of the seed fixtures, and a deterministic mutation
//! sweep over them.

use mortar_elf::archive::{Archive, Error};

use crate::build::seeds;

/// What a walk met.
#[derive(Default)]
struct Tally {
    inputs: u64,
    parsed: u64,
    members: u64,
    symbols: u64,
    errors: u64,
}

impl Tally {
    /// Counts an error and checks that it can be shown.
    fn error(&mut self, error: Error) {
        self.errors += 1;
        let message = error.to_string();
        assert!(message.contains("offset") || message.starts_with("not an archive"), "{message}");
    }
}

/// Everything a linker can ask of `bytes`: the index, the member of each
/// entry, the members in order, and `member_at` at `probes`. Panics only
/// if the reader does, or does more work than the input's size allows.
fn walk(bytes: &[u8], probes: impl Iterator<Item = u64>, tally: &mut Tally) {
    tally.inputs += 1;
    let archive = match Archive::parse(bytes) {
        Ok(archive) => archive,
        Err(error) => return tally.error(error),
    };
    tally.parsed += 1;
    // A malformed index is reported and the members are still walked
    // (§6.5).
    if let Some(Err(error)) = archive.symbol_index() {
        tally.error(error);
    }
    if let Some(Ok(index)) = archive.symbol_index() {
        // An entry takes at least one byte of the archive (§6).
        assert!(index.len() <= bytes.len(), "{} entries in {} bytes", index.len(), bytes.len());
        let mut entries = 0;
        for (position, entry) in index.iter().enumerate() {
            entries += 1;
            match entry {
                Ok(symbol) => {
                    tally.symbols += 1;
                    assert!(symbol.name.len() < bytes.len());
                    assert_eq!(index.member_offset(position), Some(symbol.member_offset));
                    if let Err(error) = archive.member_at(symbol.member_offset) {
                        tally.error(error);
                    }
                }
                Err(error) => tally.error(error),
            }
        }
        assert!(entries <= index.len());
    }
    let mut items = 0;
    let mut previous = 0;
    for member in archive.members() {
        // A member takes at least its header (§3), and the walk ends after
        // an error.
        items += 1;
        assert!(items <= bytes.len() / 60 + 1, "more members than headers fit");
        match member {
            Ok(member) => {
                tally.members += 1;
                assert!(member.header_offset() > previous, "members go forward");
                previous = member.header_offset();
                if let Some(data) = member.data() {
                    assert_eq!(data.len() as u64, member.size());
                }
                assert_eq!(archive.member_at(member.header_offset()), Ok(member));
            }
            Err(error) => tally.error(error),
        }
    }
    for offset in probes {
        if let Err(error) = archive.member_at(offset) {
            tally.error(error);
        }
    }
}

#[test]
fn s8_every_truncation_of_the_seeds() {
    let mut tally = Tally::default();
    for (label, fixture) in seeds() {
        let whole = &fixture.bytes;
        let mut whole_tally = Tally::default();
        walk(whole, 0..whole.len() as u64 + 2, &mut whole_tally);
        assert_eq!(whole_tally.members, 6, "{label}: the seed itself must read");
        let symbols = if label.ends_with("no-index") { 0 } else { 6 };
        assert_eq!(whole_tally.symbols, symbols, "{label}: the seed itself must read");
        for len in 0..whole.len() {
            // Every offset is probed: the archives are small.
            walk(&whole[..len], 0..len as u64 + 2, &mut tally);
        }
    }
    eprintln!(
        "truncation: {} inputs, {} parsed, {} members and {} index entries read, {} errors",
        tally.inputs, tally.parsed, tally.members, tally.symbols, tally.errors
    );
    assert!(tally.parsed > 0 && tally.errors > tally.inputs / 2);
}

/// xorshift64*: small, deterministic, good enough to pick bytes.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// Bytes that mean something somewhere in an archive.
const TELLING: &[u8] = b"0123456789 /`\n\0#:!<>_.Sa\xff\x80\x01";

/// Words that are counts, sizes and offsets gone wrong.
const WORDS: [u64; 10] = [0, 1, 2, 7, 60, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 1 << 61, u64::MAX];

fn mutate(bytes: &mut Vec<u8>, random: &mut Random) {
    if bytes.is_empty() {
        bytes.push(random.next() as u8);
        return;
    }
    let at = random.below(bytes.len());
    match random.below(8) {
        0 => bytes[at] = random.next() as u8,
        1 | 2 => bytes[at] = TELLING[random.below(TELLING.len())],
        3 => bytes[at] ^= 1 << random.below(8),
        4 => {
            // A word, in either width and byte order.
            let word = WORDS[random.below(WORDS.len())].wrapping_add(random.below(3) as u64);
            let encoded =
                if random.below(2) == 0 { word.to_be_bytes() } else { word.to_le_bytes() };
            let width = if random.below(2) == 0 { 4 } else { 8 };
            let from = if random.below(2) == 0 { 0 } else { 8 - width };
            for (target, byte) in bytes[at..].iter_mut().zip(&encoded[from..from + width]) {
                *target = *byte;
            }
        }
        5 => {
            // A decimal field: digits, then spaces.
            let digits = WORDS[random.below(WORDS.len())].to_string();
            for (target, byte) in bytes[at..].iter_mut().zip(digits.bytes().chain([b' '; 4])) {
                *target = byte;
            }
        }
        6 => bytes.truncate(at),
        _ => {
            // Move a run of bytes elsewhere: headers land on odd offsets.
            let len = random.below(64).min(bytes.len() - at);
            let run: Vec<u8> = bytes.drain(at..at + len).collect();
            let to = random.below(bytes.len() + 1);
            bytes.splice(to..to, run);
        }
    }
}

#[test]
fn s8_a_mutation_sweep_over_the_seeds() {
    const ROUNDS: usize = 30_000;
    let mut tally = Tally::default();
    let mut random = Random(0x6d6f_7274_6172_0005);
    for (_, fixture) in seeds() {
        for _ in 0..ROUNDS {
            let mut bytes = fixture.bytes.clone();
            for _ in 0..=random.below(3) {
                mutate(&mut bytes, &mut random);
            }
            let end = bytes.len() as u64;
            let probes = [0, 8, 68, random.below(bytes.len() + 2) as u64, end, u64::MAX];
            walk(&bytes, probes.into_iter(), &mut tally);
        }
    }
    eprintln!(
        "mutation: {} inputs, {} parsed, {} members and {} index entries read, {} errors",
        tally.inputs, tally.parsed, tally.members, tally.symbols, tally.errors
    );
    // The sweep must reach both sides: inputs that still read and inputs
    // that are refused.
    assert_eq!(tally.inputs, (ROUNDS * seeds().len()) as u64);
    assert!(tally.parsed > tally.inputs / 4, "too few mutants parse");
    assert!(tally.errors > tally.inputs / 4, "too few mutants are refused");
}
