//! `spec/meta.md` T-MT-42 and amendment A-8 to `spec/input.md` §11.1: the
//! staged store reads no byte of a Mason container while no check needs
//! one, for objects of at most 64 KiB and larger ones alike; a container
//! is never resident in a large object or member; with the containers
//! wanted, each is read once. The store's read hook (`Hooks::on_read`) is
//! the byte counter per file range.
#![allow(clippy::cast_possible_truncation, reason = "test fixtures are small")]

mod common;

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mortar_core::par::Pool;
use mortar_elf::Object;
use mortar_input::staged::SMALL;
use mortar_input::store::{Blob, Hooks, ReadJob, ReadSite, Store, StoreConfig, StorePolicy};
use mortar_testutil::Scratch;

/// Every read the store made: the file (by path while loading, by blob
/// after) and the range.
#[derive(Default)]
struct Log {
    loads: Vec<(PathBuf, Range<u64>)>,
    copies: Vec<(Blob, Range<u64>)>,
}

fn store(containers: bool) -> (Store, Arc<Mutex<Log>>) {
    let log = Arc::new(Mutex::new(Log::default()));
    let l = Arc::clone(&log);
    let hooks = Hooks {
        on_read: Some(Box::new(move |site, range| {
            let mut l = l.lock().unwrap();
            match site {
                ReadSite::Load(p) => l.loads.push((p.to_path_buf(), range)),
                ReadSite::Copy(b) => l.copies.push((b, range)),
            }
        })),
        ..Hooks::default()
    };
    let config = StoreConfig { containers, ..StoreConfig::default() };
    (Store::with_config(StorePolicy::Staged, config, hooks), log)
}

/// Garbage of `n` bytes as `.byte` lines (a deterministic sequence).
fn garbage(n: usize, seed: u64) -> String {
    let mut x = seed | 1;
    let mut s = String::new();
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        if i % 32 == 0 {
            s.push_str("\\n.byte ");
        } else {
            s.push(',');
        }
        let _ = write!(s, "{}", x as u8);
    }
    s
}

/// C source with one container per entry of `sizes`, of garbage bytes,
/// and `bulk` functions (a large object when it is in the thousands).
fn source(sizes: &[usize], bulk: usize, seed: u64) -> String {
    let mut s = String::new();
    for i in 0..bulk {
        let _ = writeln!(s, "int f{seed}_{i}(int x) {{ return x * {i} + {seed}; }}");
    }
    for (k, &n) in sizes.iter().enumerate() {
        let _ = writeln!(
            s,
            "__asm__(\".pushsection .mason.meta,\\\"e\\\",%progbits,unique,{}\\n.p2align 3{}\\n.popsection\");",
            k + 1,
            garbage(n, seed * 31 + k as u64)
        );
        let _ = writeln!(s, "int g{seed}_{k} = {k};");
    }
    s
}

/// The containers of the object at `p` (MT-6), from its whole bytes.
fn containers(p: &Path) -> Vec<Range<u64>> {
    let whole = std::fs::read(p).unwrap();
    let obj = Object::parse(&whole).unwrap();
    obj.sections()
        .filter(|s| mortar_meta::is_container(s.name().unwrap(), s.sh_type(), s.flags()))
        .map(|s| s.header().sh_offset..s.header().sh_offset + s.header().sh_size)
        .collect()
}

fn overlap(a: &Range<u64>, b: &Range<u64>) -> u64 {
    a.end.min(b.end).saturating_sub(a.start.max(b.start))
}

fn fixtures(s: &Scratch) -> Option<Vec<(PathBuf, Vec<Range<u64>>)>> {
    let mut out = Vec::new();
    for (name, sizes, bulk) in [
        ("small.o", &[1000usize][..], 10),
        ("small2.o", &[64, 2048][..], 50),
        ("large.o", &[3000][..], 6000),
        ("large2.o", &[800, 5000][..], 6000),
    ] {
        let o = common::cc(s.path(), "native", name, &source(sizes, bulk, out.len() as u64), &[])?;
        let c = containers(&o);
        assert_eq!(c.len(), sizes.len(), "{name}: the fixture has its containers");
        let len = std::fs::metadata(&o).unwrap().len();
        assert_eq!(len > SMALL, name.starts_with("large"), "{name}: {len} bytes");
        out.push((o, c));
    }
    Some(out)
}

fn read_all(store: &Store, files: &[(PathBuf, Vec<Range<u64>>)]) -> Vec<Blob> {
    let jobs: Vec<ReadJob> =
        files.iter().map(|(p, _)| ReadJob { path: p.clone(), stat: store.stat(p) }).collect();
    store.read_many(&Pool::new(4), &jobs).into_iter().map(Result::unwrap).collect()
}

/// With the containers unwanted (the default): no read touches a
/// container byte, no part holds one, and the parts parse as the whole
/// file does.
// spec: meta.md T-MT-42, MT-42; input.md §11.1 (A-8), §11.9
#[test]
fn no_container_byte_is_read() {
    let s = Scratch::new("tmt42a");
    let Some(files) = fixtures(&s) else { return };
    let (store, log) = store(false);
    assert!(!StoreConfig::default().containers, "unwanted is the default");
    let blobs = read_all(&store, &files);
    // The loads, taken before `copy_to` below calls the hook again.
    let loads = std::mem::take(&mut log.lock().unwrap().loads);
    for ((p, cs), &b) in files.iter().zip(&blobs) {
        assert!(loads.iter().any(|(q, _)| q == p), "{}: the hook saw its reads", p.display());
        for (q, r) in loads.iter().filter(|(q, _)| q == p) {
            for c in cs {
                assert_eq!(overlap(r, c), 0, "{}: read {r:?} touches container {c:?}", q.display());
            }
        }
        for part in store.parts(b) {
            let r = part.offset..part.offset + part.bytes.len() as u64;
            for c in cs {
                assert_eq!(overlap(&r, c), 0, "{}: part {r:?} holds {c:?}", p.display());
            }
        }
        let whole = std::fs::read(p).unwrap();
        let staged = Object::parse_view(store.view(b)).unwrap();
        let reference = Object::parse(&whole).unwrap();
        for (x, y) in staged.sections().zip(reference.sections()) {
            assert_eq!(x.header(), y.header());
            if x.is_resident() {
                assert_eq!(x.data().unwrap(), y.data().unwrap());
            }
        }
        // Everything but the containers is readable and right.
        let mut all = vec![0u8; whole.len()];
        store.copy_to(b, 0..whole.len() as u64, &mut all).unwrap();
        assert_eq!(all, whole);
    }
    let log = log.lock().unwrap();
    assert!(log.copies.iter().all(|(_, r)| r.end > r.start));
}

/// With the containers wanted: each container byte is read exactly once
/// (a small object whole while loading, a large one's container by
/// `copy_to`), and a large object's container is still not resident.
// spec: meta.md T-MT-42 ("each container read once"), MT-42
#[test]
fn wanted_containers_are_read_once() {
    let s = Scratch::new("tmt42b");
    let Some(files) = fixtures(&s) else { return };
    let (store, log) = store(true);
    let blobs = read_all(&store, &files);
    for ((p, cs), &b) in files.iter().zip(&blobs) {
        let large = store.len(b) > SMALL;
        for c in cs {
            let mut out = vec![0u8; (c.end - c.start) as usize];
            store.copy_to(b, c.clone(), &mut out).unwrap();
            let whole = std::fs::read(p).unwrap();
            assert_eq!(out, whole[c.start as usize..c.end as usize]);
            if large {
                assert!(store.resident(b, c.clone()).is_none(), "{}: not resident", p.display());
            }
        }
    }
    let log = log.lock().unwrap();
    for ((p, cs), &b) in files.iter().zip(&blobs) {
        for c in cs {
            let loaded: u64 =
                log.loads.iter().filter(|(q, _)| q == p).map(|(_, r)| overlap(r, c)).sum();
            let copied: u64 =
                log.copies.iter().filter(|(x, _)| *x == b).map(|(_, r)| overlap(r, c)).sum();
            assert_eq!(loaded + copied, c.end - c.start, "{}: {c:?} read once", p.display());
        }
    }
}

/// A large archive member's container is not resident (members are A-8's
/// exception for reads: the record of WP-071).
// spec: input.md §11.1 (A-8: "a Mason container ... is never resident")
#[test]
fn member_container_is_not_resident() {
    let s = Scratch::new("tmt42c");
    let Some(files) = fixtures(&s) else { return };
    let Some(a) = common::ar(s.path(), "lib.a", &["large.o", "small.o"], false) else { return };
    let (store, _) = store(false);
    let b = read_all(&store, &[(a.clone(), Vec::new())])[0];
    let whole = std::fs::read(&a).unwrap();
    let (_, cs) = &files[2];
    let member = std::fs::read(&files[2].0).unwrap();
    let at = whole.windows(member.len()).position(|w| w == member).unwrap() as u64;
    for c in cs {
        let container = at + c.start..at + c.end;
        for part in store.parts(b) {
            let held = part.offset..part.offset + part.bytes.len() as u64;
            assert_eq!(overlap(&held, &container), 0, "part {held:?} holds {container:?}");
        }
    }
}

/// The one exception to the zero (WP-071's record): the first 64 bytes of
/// a file are read before its class is known and stay resident (every
/// reader starts there). An ELF32 header is 52 bytes, so an ELF32 object
/// whose container is its first section has up to 12 container bytes read;
/// nothing past byte 64.
// spec: meta.md MT-42 (the record's finding); input.md §11.1 (A-8)
#[test]
fn elf32_head_is_the_one_exception() {
    let s = Scratch::new("tmt42d");
    let Some(gas) = common::need("i686-linux-gnu-as") else { return };
    let src = format!(
        ".section .mason.meta,\"e\",%progbits\n.p2align 3\n{}\n.section .rodata,\"a\",%progbits\n.long 42\n",
        garbage(400, 3).replace("\\n", "\n")
    );
    let asm = s.write("e32.s", src);
    let o = s.join("e32.o");
    let r = common::tool_run(
        &gas,
        &["--32".into(), "-o".into(), o.clone().into_os_string(), asm.into_os_string()],
        s.path(),
    );
    assert!(r.success(), "{}", r.stderr_text());
    let cs = containers(&o);
    assert_eq!(cs.len(), 1);
    assert!(cs[0].start < 64, "the fixture's container starts at {:#x}", cs[0].start);
    let (store, log) = store(false);
    let b = read_all(&store, &[(o.clone(), cs.clone())])[0];
    let loads = std::mem::take(&mut log.lock().unwrap().loads);
    let read: u64 = loads.iter().map(|(_, r)| overlap(r, &cs[0])).sum();
    assert!(read <= 64 - cs[0].start, "{read} container bytes read");
    let past = cs[0].start.max(64)..cs[0].end;
    assert!(loads.iter().all(|(_, r)| overlap(r, &past) == 0), "a read past byte 64: {loads:?}");
    let whole = std::fs::read(&o).unwrap();
    let staged = Object::parse_view(store.view(b)).unwrap();
    let rodata = staged.sections().find(|x| x.name().unwrap() == b".rodata").unwrap();
    assert_eq!(rodata.data().unwrap(), 42u32.to_le_bytes());
    assert_eq!(staged.section_count(), Object::parse(&whole).unwrap().section_count());
}
