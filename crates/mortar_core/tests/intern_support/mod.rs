//! Realistic symbol names and symbol-table workloads, shared by the
//! interning tests and the benchmarks (`examples/intern_bench.rs`,
//! `examples/hash_bench.rs`).

#![allow(dead_code, clippy::cast_precision_loss, clippy::range_plus_one)]

use std::ops::Range;

/// splitmix64.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A float in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const SPACES: [&str; 12] =
    ["std", "__1", "llvm", "clang", "mortar", "detail", "boost", "absl", "internal", "v8", "base", "impl"];
const CLASSES: [&str; 16] = [
    "vector", "basic_string", "unique_ptr", "shared_ptr", "map", "unordered_map", "SmallVector",
    "DenseMap", "StringRef", "Optional", "function", "allocator", "char_traits", "Twine", "APInt",
    "raw_ostream",
];
const METHODS: [&str; 16] = [
    "push_back", "emplace_back", "reserve", "operator[]", "find", "insert", "erase", "begin", "end",
    "size", "data", "grow", "reset", "swap", "append", "compare",
];
const PARAMS: [&str; 10] = ["i", "j", "m", "Ev", "RKS_", "OS_", "PKc", "mm", "NS_9allocatorIcEE", "S3_S4_"];

fn push_ident(out: &mut Vec<u8>, ident: &str) {
    out.extend_from_slice(ident.len().to_string().as_bytes());
    out.extend_from_slice(ident.as_bytes());
}

/// A mangled C++ name in the shapes real objects carry: nested names with
/// long common prefixes (`_ZNSt3__1...`), template arguments, and a
/// distinguishing tail drawn from `n` so that distinct `n` give distinct
/// names.
pub fn mangled(rng: &mut Rng, n: usize, out: &mut Vec<u8>) {
    out.extend_from_slice(b"_ZN");
    if rng.below(3) == 0 {
        out.extend_from_slice(b"St3__1");
    }
    for _ in 0..1 + rng.below(3) {
        push_ident(out, SPACES[rng.below(SPACES.len())]);
    }
    push_ident(out, CLASSES[rng.below(CLASSES.len())]);
    if rng.below(2) == 0 {
        out.extend_from_slice(b"IcNS_11char_traitsIcEENS_9allocatorIcEEE");
    }
    let method = METHODS[rng.below(METHODS.len())];
    push_ident(out, &format!("{method}{n}"));
    out.push(b'E');
    for _ in 0..rng.below(4) {
        out.extend_from_slice(PARAMS[rng.below(PARAMS.len())].as_bytes());
    }
}

/// A link's worth of symbol tables.
pub struct Workload {
    /// Every name's bytes.
    pub arena: Vec<u8>,
    /// The distinct names, as ranges of `arena`.
    pub names: Vec<Range<usize>>,
    /// Per file, the names it mentions, in symbol-table order.
    pub files: Vec<Vec<u32>>,
}

impl Workload {
    pub fn name(&self, i: u32) -> &[u8] {
        &self.arena[self.names[i as usize].clone()]
    }

    pub fn mentions(&self) -> usize {
        self.files.iter().map(Vec::len).sum()
    }

    pub fn counts(&self) -> Vec<u32> {
        self.files.iter().map(|f| f.len() as u32).collect()
    }
}

/// `files` files of `per_file` mentions each (give or take a third):
/// `own` of every hundred are names of the file's own (its definitions),
/// the others drawn from `shared` names with a heavy skew towards the
/// popular ones (most of a C++ file's undefined references are to the same
/// few thousand library names).
pub fn workload(seed: u64, files: usize, per_file: usize, shared: usize, own: usize) -> Workload {
    let mut rng = Rng(seed);
    let mut arena = Vec::new();
    let mut names = Vec::new();
    let mut add = |rng: &mut Rng, arena: &mut Vec<u8>| {
        let start = arena.len();
        mangled(rng, names.len(), arena);
        names.push(start..arena.len());
        (names.len() - 1) as u32
    };
    for _ in 0..shared {
        add(&mut rng, &mut arena);
    }
    let mut out = Vec::with_capacity(files);
    for _ in 0..files {
        let n = if per_file == 0 { 0 } else { per_file * 2 / 3 + rng.below(per_file * 2 / 3 + 1) };
        let mut file = Vec::with_capacity(n);
        for _ in 0..n {
            if shared == 0 || rng.below(100) < own {
                file.push(add(&mut rng, &mut arena));
            } else {
                let u = rng.unit();
                file.push(((u * u * u) * shared as f64) as u32);
            }
        }
        out.push(file);
    }
    Workload { arena, names, files: out }
}

/// A workload laid out as real inputs are: each file has a string table of
/// its own holding the names it mentions, in order.
pub struct Strtabs {
    pub files: Vec<(Vec<u8>, Vec<Range<usize>>)>,
}

impl Strtabs {
    pub fn new(w: &Workload) -> Self {
        let files = w
            .files
            .iter()
            .map(|file| {
                let mut strtab = Vec::new();
                let mut names = Vec::with_capacity(file.len());
                for &n in file {
                    let start = strtab.len();
                    strtab.extend_from_slice(w.name(n));
                    names.push(start..strtab.len());
                    strtab.push(0);
                }
                (strtab, names)
            })
            .collect();
        Self { files }
    }

    pub fn names(&self, file: usize) -> impl ExactSizeIterator<Item = &[u8]> + '_ {
        let (strtab, names) = &self.files[file];
        names.iter().map(move |r| &strtab[r.clone()])
    }
}
