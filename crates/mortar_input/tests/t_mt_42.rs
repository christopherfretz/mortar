//! `spec/meta.md` T-MT-42 and amendment A-8 to `spec/input.md` §11.1: with
//! no check that needs a Mason container, the staged store makes exactly
//! the reads it makes when containers are wanted (a small file whole; a
//! large object's head and tail windows, then its body ranges); container
//! bytes inside those reads are dropped, never resident; no other container
//! byte is read. A container is never resident in a large object or
//! member; with the containers wanted, each is read once. The store's read
//! hook (`Hooks::on_read`) is the counter per file range.
#![allow(clippy::cast_possible_truncation, reason = "test fixtures are small")]

mod common;

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mortar_core::par::Pool;
use mortar_elf::Object;
use mortar_input::staged::SMALL;
use mortar_input::store::{
    Blob, HEAD, Hooks, ReadJob, ReadSite, Store, StoreConfig, StorePolicy, TAIL,
};
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

/// The reads of each file, in order of offset.
type Loads = Vec<Vec<Range<u64>>>;

/// The loads of each of `files` by `store`, in order of offset.
fn loads_of(log: &Mutex<Log>, files: &[(PathBuf, Vec<Range<u64>>)]) -> Loads {
    let l = log.lock().unwrap();
    files
        .iter()
        .map(|(p, _)| {
            let mut v: Vec<Range<u64>> =
                l.loads.iter().filter(|(q, _)| q == p).map(|(_, r)| r.clone()).collect();
            v.sort_by_key(|r| (r.start, r.end));
            v
        })
        .collect()
}

/// The loads of `files`, read once with the containers unwanted and once
/// wanted.
fn both_ways(files: &[(PathBuf, Vec<Range<u64>>)]) -> (Loads, Loads) {
    let (unwanted, log) = store(false);
    let _ = read_all(&unwanted, files);
    let (wanted, log2) = store(true);
    let _ = read_all(&wanted, files);
    (loads_of(&log, files), loads_of(&log2, files))
}

/// Files with no container cost exactly what they cost with the
/// containers wanted (the plan of §11.9 before A-8, main's): one read of a
/// small object; the head, the tail and the body ranges of a large one
/// (three reads for this fixture's one body range); an archive's chain and
/// members as before. (A process makes one read per file more, with or
/// without the rule: the one-byte end-of-file probe of the final checks,
/// which the read hook does not see.)
// spec: meta.md T-MT-42, MT-42 ("exactly the reads"); input.md §11.1 (A-8)
#[test]
fn container_free_files_cost_what_they_cost_without_the_rule() {
    let s = Scratch::new("tmt42g");
    let mut files = Vec::new();
    for (name, bulk) in [("free-small.o", 10), ("free-large.o", 6000)] {
        let Some(o) = common::cc(s.path(), "native", name, &source(&[], bulk, 7), &[]) else {
            return;
        };
        assert!(containers(&o).is_empty());
        files.push((o, Vec::new()));
    }
    let Some(a) = common::ar(s.path(), "free.a", &["free-large.o", "free-small.o"], false) else {
        return;
    };
    files.push((a, Vec::new()));
    let (unwanted, wanted) = both_ways(&files);
    assert_eq!(unwanted, wanted, "the same reads, range for range");
    let counts: Vec<usize> = unwanted.iter().map(Vec::len).collect();
    assert_eq!(counts[0], 1, "a small object: one read, {:?}", unwanted[0]);
    assert_eq!(counts[1], 3, "a large object: head, tail, one body range: {:?}", unwanted[1]);
    assert!(counts[2] >= 2, "the archive: its chain and its large member: {:?}", unwanted[2]);
}

/// With the containers unwanted (the default), on Mason objects: the same
/// reads as with them wanted; no read lies wholly inside a container; no
/// part holds a container byte; a large object's container bytes are read
/// only where its head and tail windows cover them (none, for a container
/// in its body); and the parts parse as the whole file does.
// spec: meta.md T-MT-42, MT-42; input.md §11.1 (A-8), §11.9
#[test]
fn no_container_byte_is_resident() {
    let s = Scratch::new("tmt42a");
    let Some(mut files) = fixtures(&s) else { return };
    let Some(t) = tail_fixture(&s) else { return };
    files.push(t);
    assert!(!StoreConfig::default().containers, "unwanted is the default");
    let (store, log) = store(false);
    let blobs = read_all(&store, &files);
    let loads = loads_of(&log, &files);
    let (_, wanted) = both_ways(&files);
    assert_eq!(loads, wanted, "exactly the reads made with the containers wanted");
    for (((p, cs), &b), reads) in files.iter().zip(&blobs).zip(&loads) {
        let name = p.display();
        assert!(!reads.is_empty(), "{name}: the hook saw its reads");
        for r in reads {
            for c in cs {
                assert!(!(c.start <= r.start && r.end <= c.end), "{name}: read {r:?} inside {c:?}");
            }
        }
        for part in store.parts(b) {
            let r = part.offset..part.offset + part.bytes.len() as u64;
            for c in cs {
                assert_eq!(overlap(&r, c), 0, "{name}: part {r:?} holds {c:?}");
            }
        }
        let len = store.len(b);
        if len > SMALL {
            let windows = [0..HEAD, len.saturating_sub(TAIL)..len];
            let expected: u64 =
                cs.iter().map(|c| windows.iter().map(|w| overlap(w, c)).sum::<u64>()).sum();
            assert_eq!(container_bytes_loaded(&log, p, cs), expected, "{name}: only the windows");
            let in_tail = p.file_name().unwrap() == "tail.o";
            assert_eq!(expected > 0, in_tail, "{name}: the fixture's container is where meant");
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
        // Everything is readable and right, the containers by `copy_to`.
        let mut all = vec![0u8; whole.len()];
        store.copy_to(b, 0..whole.len() as u64, &mut all).unwrap();
        assert_eq!(all, whole);
    }
    let log = log.lock().unwrap();
    assert!(log.copies.iter().all(|(_, r)| r.end > r.start));
}

/// A large object whose container lies in its tail window: 100 000 bytes
/// of `.rodata`, then the container, then little else.
fn tail_fixture(s: &Scratch) -> Option<(PathBuf, Vec<Range<u64>>)> {
    let src = format!(
        "__asm__(\".section .rodata,\\\"a\\\",%progbits\\n.fill 100000,1,7\\n.section .mason.meta,\\\"e\\\",%progbits\\n.p2align 3{}\\n.text\");\nint tail_g = 1;\n",
        garbage(3000, 11)
    );
    let o = common::cc(s.path(), "native", "tail.o", &src, &[])?;
    let c = containers(&o);
    assert_eq!(c.len(), 1, "tail.o has its container");
    let len = std::fs::metadata(&o).unwrap().len();
    assert!(len > SMALL && c[0].start >= len - TAIL, "tail.o: {c:?} of {len}");
    Some((o, c))
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

/// The container bytes the loads of `p` read (the counter of `log`).
fn container_bytes_loaded(log: &Mutex<Log>, p: &Path, cs: &[Range<u64>]) -> u64 {
    let l = log.lock().unwrap();
    l.loads
        .iter()
        .filter(|(q, _)| q == p)
        .map(|(_, r)| cs.iter().map(|c| overlap(r, c)).sum::<u64>())
        .sum()
}

/// Named exception MT-42 E2: a
/// regular archive's members are read before their section headers are
/// known, so a member of at most 64 KiB is read whole and stays resident,
/// its container with it. This test pins the extent: the small member's
/// container is read and resident, the large member's is not resident.
// spec: meta.md MT-42 (exception: archive members); input.md §11.1 (A-8)
#[test]
fn small_member_container_is_the_archive_exception() {
    let s = Scratch::new("tmt42e");
    let Some(files) = fixtures(&s) else { return };
    let Some(a) = common::ar(s.path(), "lib.a", &["large.o", "small.o"], false) else { return };
    let whole = std::fs::read(&a).unwrap();
    let at = |i: usize| {
        let m = std::fs::read(&files[i].0).unwrap();
        whole.windows(m.len()).position(|w| w == m).unwrap() as u64
    };
    let shift = |i: usize| -> Vec<Range<u64>> {
        let base = at(i);
        files[i].1.iter().map(|c| base + c.start..base + c.end).collect()
    };
    let (small, large) = (shift(0), shift(2));
    let (store, log) = store(false);
    let b = read_all(&store, &[(a.clone(), Vec::new())])[0];
    let read_small = container_bytes_loaded(&log, &a, &small);
    let total_small: u64 = small.iter().map(|c| c.end - c.start).sum();
    assert_eq!(read_small, total_small, "the small member is read whole (the exception)");
    let held = |cs: &[Range<u64>]| -> u64 {
        store
            .parts(b)
            .iter()
            .map(|p| {
                let r = p.offset..p.offset + p.bytes.len() as u64;
                cs.iter().map(|c| overlap(&r, c)).sum::<u64>()
            })
            .sum()
    };
    assert_eq!(held(&small), total_small, "and resident with its member");
    assert_eq!(held(&large), 0, "a large member's container is never resident");
}

/// Named exception MT-42 E3: the
/// whole-file paths read containers. A file made fully resident because
/// the descriptor budget ran out (`input.md` §11.4, `make_full`), and
/// every file under the resident policy, is read whole.
// spec: meta.md MT-42 (exception: whole-file reads); input.md §11.4
#[test]
fn whole_file_paths_are_the_named_exception() {
    let s = Scratch::new("tmt42f");
    let Some(files) = fixtures(&s) else { return };
    let large: Vec<(PathBuf, Vec<Range<u64>>)> = (0..8)
        .map(|i| {
            let p = s.write(&format!("l{i}.o"), std::fs::read(&files[2].0).unwrap());
            (p, files[2].1.clone())
        })
        .collect();
    for (policy, max_open) in [(StorePolicy::Staged, Some(2)), (StorePolicy::Resident, None)] {
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
        let config = StoreConfig { max_open, ..StoreConfig::default() };
        let store = Store::with_config(policy, config, hooks);
        let _ = read_all(&store, &large);
        let l = log.lock().unwrap();
        let read: u64 = large
            .iter()
            .map(|(p, cs)| {
                let loads: u64 = l
                    .loads
                    .iter()
                    .filter(|(q, _)| q == p)
                    .map(|(_, r)| cs.iter().map(|c| overlap(r, c)).sum::<u64>())
                    .sum();
                loads
            })
            .sum();
        let copied: u64 = l
            .copies
            .iter()
            .map(|(_, r)| files[2].1.iter().map(|c| overlap(r, c)).sum::<u64>())
            .sum();
        let one: u64 = files[2].1.iter().map(|c| c.end - c.start).sum();
        match policy {
            // Two files keep their descriptors; six are made full.
            StorePolicy::Staged => assert_eq!(read + copied, 6 * one, "{policy:?}"),
            StorePolicy::Resident => assert_eq!(read + copied, 8 * one, "{policy:?}"),
        }
    }
}
