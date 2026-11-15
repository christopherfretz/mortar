//! `spec/input.md` §11 for the staged store (§11.1 (b), §11.3, §11.4,
//! §11.5, §11.6): what is resident, the parts, `copy_to` from every worker,
//! the descriptor budget, and files that change between its stages.
#![allow(clippy::cast_possible_truncation, reason = "test fixtures are small")]
#![allow(clippy::many_single_char_names, reason = "short names for fixtures")]

mod common;

use std::fmt::Write as _;
use std::ops::Range;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mortar_core::inputs::{Blob as ModelBlob, CopyError, InputStore, Reader};
use mortar_core::par::{Pool, check_schedules};
use mortar_elf::consts::{SHF_ALLOC, SHT_NOBITS, SHT_RELA, SHT_STRTAB, SHT_SYMTAB};
use mortar_elf::{Object, View};
use mortar_input::facts::{self, Kind, MemberContent};
use mortar_input::staged::{GAP, SMALL};
use mortar_input::store::{
    Blob, Hooks, ReadError, ReadJob, Store, StoreConfig, StorePolicy, content_hash,
};
use mortar_input::walk::{Loaded, Options, WalkEnv, walk};
use mortar_testutil::Scratch;

const M: &str = "-maarch64linux";

fn job(store: &Store, p: &Path) -> ReadJob {
    ReadJob { path: p.to_path_buf(), stat: store.stat(p) }
}

/// C source whose object has sections well above [`SMALL`]: `.text`,
/// `.symtab`, `.debug_info` and their relocations (with `-g`).
fn big_source(seed: usize) -> String {
    let mut s = String::new();
    for i in 0..3000 {
        let _ = writeln!(
            s,
            "struct s{i} {{ int a{i}; long b{i}; }};\nint f{seed}_{i}(struct s{i} *p, int x) \
             {{ int y = x * {i} + p->a{i}; return y + (int)p->b{i}; }}"
        );
    }
    s
}

/// A large `-g` object, or `None` without a compiler.
fn big_object(s: &Scratch, name: &str, seed: usize) -> Option<PathBuf> {
    let o = common::cc(s.path(), "native", name, &big_source(seed), &["-g"])?;
    let len = std::fs::metadata(&o).unwrap().len();
    assert!(len > 4 * SMALL, "{name}: {len} bytes");
    Some(o)
}

fn staged() -> Store {
    Store::new(StorePolicy::Staged)
}

fn read_one(store: &Store, p: &Path) -> Result<Blob, ReadError> {
    store.read_many(&Pool::sequential(), &[job(store, p)]).remove(0)
}

/// Every byte of the blob, through `copy_to`.
fn all_bytes(store: &Store, b: Blob) -> Result<Vec<u8>, ReadError> {
    let mut out = vec![0u8; store.len(b) as usize];
    store.copy_to(b, 0..store.len(b), &mut out)?;
    Ok(out)
}

fn walk_with(store: &'static Store, cwd: &Path, args: &[&str], pool: &Pool) -> Loaded<'static> {
    walk(&common::plan(args), &WalkEnv { cwd: cwd.to_path_buf() }, store, pool, Options::default())
}

fn leak(store: Store) -> &'static Store {
    Box::leak(Box::new(store))
}

/// §11.1 (b): of a large object, the structure is resident and the bulk
/// is not; the parts view parses as the whole file does.
#[test]
fn s11_1_staged_resident_and_bulk() {
    let s = Scratch::new("s11staged1");
    let Some(o) = big_object(&s, "big.o", 0) else { return };
    let whole = std::fs::read(&o).unwrap();
    let store = staged();
    let b = read_one(&store, &o).unwrap();
    assert_eq!(store.len(b), whole.len() as u64);
    assert!(store.whole(b).is_none(), "a large object is not read whole");
    let resident: u64 = store.parts(b).iter().map(|p| p.bytes.len() as u64).sum();
    assert!(resident < whole.len() as u64 / 2, "{resident} of {} resident", whole.len());
    assert_eq!(store.stats().resident, resident);
    let staged = Object::parse_view(store.view(b)).unwrap();
    let reference = Object::parse(&whole).unwrap();
    assert_eq!(staged.section_count(), reference.section_count());
    let mut bulk = 0u64;
    for (x, y) in staged.sections().zip(reference.sections()) {
        assert_eq!(x.header(), y.header());
        assert_eq!(x.name().unwrap(), y.name().unwrap());
        let h = x.header();
        let name = String::from_utf8_lossy(x.name().unwrap()).into_owned();
        let alloc_target =
            || reference.sections().nth(h.sh_info as usize).unwrap().flags() & SHF_ALLOC != 0;
        let structural = matches!(h.sh_type, SHT_SYMTAB | SHT_STRTAB)
            || (h.sh_type == SHT_RELA && alloc_target());
        // Small sections are resident, but not the relocations of a
        // non-allocated section, whatever their size.
        let small = h.sh_size <= SMALL && h.sh_type != SHT_RELA;
        if structural || small {
            assert!(x.is_resident(), "{name} is resident");
            assert_eq!(x.data().unwrap(), y.data().unwrap(), "{name}");
        }
        let debug = name.starts_with(".debug_info") || name == ".rela.debug_info";
        if h.sh_type != SHT_NOBITS && debug && h.sh_size >= GAP {
            assert!(!x.is_resident(), "{name} is not resident");
            assert!(matches!(x.data(), Err(mortar_elf::Error::NotResident { .. })), "{name}");
            bulk += h.sh_size;
        }
    }
    assert!(bulk > 0, "the fixture has bulk sections");
    // The tables do not depend on the store.
    let facts = facts::analyse_view(store.view(b), false);
    let reference = facts::analyse(&whole, false);
    assert_eq!(common::mask(&format!("{facts:?}")), common::mask(&format!("{reference:?}")));
}

/// §11.1: the walk's tables under both stores, over objects, a regular and
/// a thin archive with large members.
#[test]
fn s11_1_both_stores_agree_on_large_inputs() {
    let s = Scratch::new("s11staged2");
    let (Some(_), Some(_)) = (big_object(&s, "a.o", 1), big_object(&s, "b.o", 2)) else {
        return;
    };
    common::cc(s.path(), "native", "c.o", "int c(void){return 3;}\n", &[]).unwrap();
    let Some(_) = common::ar(s.path(), "libab.a", &["a.o", "c.o", "b.o"], false) else { return };
    common::ar(s.path(), "libt.a", &["a.o", "c.o"], true).unwrap();
    for threads in [1, 4] {
        let out = common::run_plan(
            s.path(),
            &common::plan(&[M, "a.o", "libab.a", "--whole-archive", "libt.a"]),
            Options::default(),
            &Pool::new(threads),
        );
        assert!(out.problems.is_empty(), "{:?}", common::report(&out));
        assert!(out.files.len() >= 5);
    }
}

/// §11.3: the parts are sorted, disjoint and at least [`GAP`] apart;
/// `resident` is `Some` exactly inside one part; `copy_to` equals the file
/// and refuses a range past its end.
#[test]
fn s11_3_parts_and_copy_to() {
    let s = Scratch::new("s11staged3");
    let Some(o) = big_object(&s, "big.o", 3) else { return };
    let whole = std::fs::read(&o).unwrap();
    let store = staged();
    let b = read_one(&store, &o).unwrap();
    let parts = store.parts(b);
    assert!(parts.len() > 1);
    for w in parts.windows(2) {
        assert!(w[0].end() + GAP <= w[1].offset, "{:?} {:?}", w[0], w[1]);
    }
    for p in parts {
        assert_eq!(&p.bytes[..], &whole[p.offset as usize..p.end() as usize]);
        let r = store.resident(b, p.offset..p.end()).unwrap();
        assert_eq!(store.bytes(r), &p.bytes[..]);
        assert!(store.resident(b, p.offset..p.end() + 1).is_none());
        if p.offset > 0 {
            assert!(store.resident(b, p.offset - 1..p.offset + 1).is_none());
        }
    }
    assert_eq!(all_bytes(&store, b).unwrap(), whole);
    // Ranges that straddle parts and gaps, of many sizes.
    let len = whole.len() as u64;
    let mut at = 0u64;
    let mut step = 1u64;
    while at < len {
        let end = (at + step).min(len);
        let mut out = vec![0u8; (end - at) as usize];
        store.copy_to(b, at..end, &mut out).unwrap();
        assert_eq!(out, &whole[at as usize..end as usize], "{at}..{end}");
        at += step / 2 + 1;
        step = step * 3 % 200_003 + 1;
    }
    let mut out = [0u8; 2];
    assert_eq!(store.copy_to(b, len - 1..len + 1, &mut out), Err(ReadError::OutOfRange));
    assert_eq!(
        InputStore::copy_to(&store, ModelBlob(b.index()), len - 1..len + 1, &mut out),
        Err(CopyError::OutOfRange)
    );
    assert_eq!(
        InputStore::copy_to(&store, ModelBlob(99), 0..1, &mut out[..1]),
        Err(CopyError::OutOfRange)
    );
    assert!(InputStore::resident(&store, ModelBlob(b.index()), 0..64).is_some());
    let st = store.stats();
    assert!(st.copy_reads > 0 && st.copy_bytes > 0);
    assert_eq!(st.open_now, 1, "the large object stays open (§11.4)");
}

/// §11.3: `copy_to` from 64 workers at once, of every section of every
/// file of a link, equals the file's bytes.
#[test]
fn s11_3_copy_to_from_64_threads() {
    let s = Scratch::new("s11staged64");
    let (Some(_), Some(_)) = (big_object(&s, "a.o", 4), big_object(&s, "b.o", 5)) else {
        return;
    };
    common::cc(s.path(), "native", "c.o", "int c(void){return 3;}\n", &[]).unwrap();
    let Some(_) = common::ar(s.path(), "liball.a", &["b.o", "c.o"], false) else { return };
    let store = leak(staged());
    let out = walk_with(store, s.path(), &[M, "a.o", "--whole-archive", "liball.a"], &Pool::new(4));
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    let mut jobs: Vec<(Blob, Range<u64>, PathBuf)> = Vec::new();
    for f in &out.files {
        let Reader::Object(object) = f.reader else { continue };
        let path = s.join(f.name.path());
        for section in object.sections() {
            if let Ok(Some(r)) = section.file_range() {
                jobs.push((f.blob, f.base + r.start as u64..f.base + r.end as u64, path.clone()));
            }
        }
        jobs.push((f.blob, f.base..f.base + f.len, path));
    }
    assert!(jobs.len() > 30);
    let files: Vec<(PathBuf, Vec<u8>)> = ["a.o", "liball.a"]
        .iter()
        .map(|n| (s.join(n), std::fs::read(s.join(n)).unwrap()))
        .collect();
    let pool = Pool::new(64);
    let rounds = 4;
    let ok = pool.map(jobs.len() * rounds, |i| {
        let (b, r, path) = &jobs[i % jobs.len()];
        let whole = &files.iter().find(|(p, _)| p == path).unwrap().1;
        let mut buf = vec![0u8; (r.end - r.start) as usize];
        store.copy_to(*b, r.clone(), &mut buf).unwrap();
        buf == whole[r.start as usize..r.end as usize]
    });
    assert!(ok.iter().all(|x| *x));
}

/// §11.4: with a budget of two descriptors, the third file on is made
/// fully resident and closed; every byte still reads back.
#[test]
fn s11_4_staged_descriptor_budget() {
    let s = Scratch::new("s11staged4");
    let Some(o) = big_object(&s, "big.o", 6) else { return };
    let whole = std::fs::read(&o).unwrap();
    let paths: Vec<PathBuf> = (0..8).map(|i| s.write(&format!("o{i}.o"), &whole)).collect();
    let config = StoreConfig { max_open: Some(2), ..StoreConfig::default() };
    let store = Store::with_config(StorePolicy::Staged, config, Hooks::default());
    let jobs: Vec<ReadJob> = paths.iter().map(|p| job(&store, p)).collect();
    let blobs: Vec<Blob> =
        store.read_many(&Pool::new(4), &jobs).into_iter().map(Result::unwrap).collect();
    let st = store.stats();
    assert_eq!(st.open_now, 2);
    assert_eq!(st.fully_resident, 6);
    assert!(st.peak_open <= 8);
    for b in blobs {
        assert_eq!(all_bytes(&store, b).unwrap(), whole);
        assert_eq!(store.recheck(b), Ok(()));
    }
}

/// §11.4: `EMFILE` makes a held file fully resident instead of failing.
/// Runs itself again with `ulimit -n 48` and a budget above the limit.
#[test]
fn s11_4_staged_emfile_falls_back() {
    if std::env::var_os("MORTAR_EMFILE_CHILD").is_none() {
        let s = Scratch::new("s11emfile");
        let Some(o) = big_object(&s, "big.o", 7) else { return };
        drop(o);
        let exe = std::env::current_exe().unwrap();
        let mut c = Command::new("sh");
        c.args(["-c", "ulimit -n 48 && exec \"$0\" \"$@\""])
            .arg(exe)
            .args(["--exact", "s11_4_staged_emfile_falls_back", "--test-threads=1"])
            .env("MORTAR_EMFILE_CHILD", s.path());
        let out = mortar_testutil::run_with(&mut c, b"", Duration::from_mins(2));
        assert!(out.success(), "{}\n{}", out.stdout_text(), out.stderr_text());
        assert!(out.stdout_text().contains("1 passed"), "{}", out.stdout_text());
        return;
    }
    let dir = PathBuf::from(std::env::var_os("MORTAR_EMFILE_CHILD").unwrap());
    let whole = std::fs::read(dir.join("big.o")).unwrap();
    let paths: Vec<PathBuf> = (0..120)
        .map(|i| {
            let p = dir.join(format!("e{i}.o"));
            std::fs::write(&p, &whole).unwrap();
            p
        })
        .collect();
    // A budget the limit cannot hold, and reads of 8 files at a time: the
    // budget never makes a file fully resident, so every one that is was
    // released because `open` failed with `EMFILE`.
    let config = StoreConfig { max_open: Some(1000), ..StoreConfig::default() };
    let store = Store::with_config(StorePolicy::Staged, config, Hooks::default());
    let mut blobs = Vec::new();
    for batch in paths.chunks(8) {
        let jobs: Vec<ReadJob> = batch.iter().map(|p| job(&store, p)).collect();
        for (r, p) in store.read_many(&Pool::new(2), &jobs).into_iter().zip(batch) {
            blobs.push(r.unwrap_or_else(|e| panic!("{}: {e:?}", p.display())));
        }
    }
    for b in blobs {
        assert_eq!(all_bytes(&store, b).unwrap(), whole);
    }
    let st = store.stats();
    assert!(st.fully_resident > 0 && st.open_now < 48, "{st:?}");
}

/// §11.4 under the walk: thousands of small files are read whole and
/// closed at once; the budget bounds what stays open.
#[test]
fn s11_4_staged_many_small_files() {
    let s = Scratch::new("s11stagedmany");
    let Some(o) = common::cc(s.path(), "native", "f.o", "int f;\n", &[]) else { return };
    let bytes = std::fs::read(o).unwrap();
    let mut args = vec![M.to_owned()];
    for i in 0..3000 {
        let name = format!("f{i}.o");
        s.write(&name, &bytes);
        args.push(name);
    }
    let store = leak(staged());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = walk_with(store, s.path(), &args, &Pool::new(4));
    assert!(out.problems.is_empty());
    assert_eq!(out.files.len(), 3000);
    let st = store.stats();
    assert_eq!((st.opens, st.closes, st.open_now), (3000, 3000, 0));
    assert!(st.peak_open <= 1024, "{}", st.peak_open);
}

/// §11.5: truncated between the staged store's first reads and its parts:
/// `input-changed`, never garbage or a panic, and nothing left open.
#[test]
fn s11_5_staged_truncated_between_stages() {
    let s = Scratch::new("s11stagedcut");
    let Some(o) = big_object(&s, "big.o", 8) else { return };
    let hooks = Hooks {
        after_head: Some(Box::new(|p: &Path| {
            let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
            f.set_len(f.metadata().unwrap().len() / 3).unwrap();
        })),
        ..Hooks::default()
    };
    let store = Store::with_config(StorePolicy::Staged, StoreConfig::default(), hooks);
    assert_eq!(read_one(&store, &o), Err(ReadError::Changed));
    let st = store.stats();
    assert_eq!((st.opens, st.closes, st.open_now), (1, 1, 0));
    // And under the walk: the diagnostic of §11.5.
    let s = Scratch::new("s11stagedcutwalk");
    let Some(_) = big_object(&s, "big.o", 8) else { return };
    let store = leak(Store::with_config(
        StorePolicy::Staged,
        StoreConfig::default(),
        Hooks {
            after_head: Some(Box::new(|p: &Path| {
                std::fs::OpenOptions::new().write(true).open(p).unwrap().set_len(100).unwrap();
            })),
            ..Hooks::default()
        },
    ));
    let out = walk_with(store, s.path(), &[M, "big.o"], &Pool::new(2));
    assert_eq!(common::report(&out), ["error: big.o: file changed while it was read"]);
}

/// §11.4, §11.5: replaced (renamed over) or removed. Between the stages
/// it is `input-changed` (the unlink changes the inode's `st_ctime`: the
/// check is conservative); after loading, the store still reads the inode
/// it opened, and `recheck` reports the change.
#[test]
fn s11_4_rename_between_stages() {
    for remove in [false, true] {
        let s = Scratch::new("s11stagedswap");
        let Some(o) = big_object(&s, "big.o", 9) else { return };
        let whole = std::fs::read(&o).unwrap();
        let hook: mortar_input::store::Hook = if remove {
            Box::new(|p: &Path| std::fs::remove_file(p).unwrap())
        } else {
            Box::new(|p: &Path| {
                let other = p.with_extension("new");
                std::fs::write(&other, b"not an object").unwrap();
                std::fs::rename(&other, p).unwrap();
            })
        };
        let hooks = Hooks { after_head: Some(hook), ..Hooks::default() };
        let store = Store::with_config(StorePolicy::Staged, StoreConfig::default(), hooks);
        assert_eq!(read_one(&store, &o), Err(ReadError::Changed), "remove: {remove}");
        assert_eq!(store.stats().open_now, 0);
        // After loading.
        std::fs::write(&o, &whole).unwrap();
        let store = staged();
        let b = read_one(&store, &o).unwrap();
        if remove {
            std::fs::remove_file(&o).unwrap();
        } else {
            std::fs::write(o.with_extension("new"), b"not an object").unwrap();
            std::fs::rename(o.with_extension("new"), &o).unwrap();
        }
        assert_eq!(all_bytes(&store, b).unwrap(), whole, "remove: {remove}");
        assert_eq!(store.recheck(b), Err(ReadError::Changed), "remove: {remove}");
    }
}

/// §11.5 after loading: a held file truncated or rewritten is
/// `Changed` from `copy_to`, `hash` and `recheck`, never stale or other
/// bytes passed off as the file's.
#[test]
fn s11_5_staged_changed_after_load() {
    let s = Scratch::new("s11stagedlater");
    let Some(o) = big_object(&s, "big.o", 10) else { return };
    let whole = std::fs::read(&o).unwrap();
    let store = staged();
    let b = read_one(&store, &o).unwrap();
    assert_eq!(store.recheck(b), Ok(()));
    // Rewritten in place, same size: the times change.
    std::thread::sleep(Duration::from_millis(20));
    let f = std::fs::OpenOptions::new().write(true).open(&o).unwrap();
    f.write_all_at(b"\x7fELF", 0).unwrap();
    assert_eq!(store.recheck(b), Err(ReadError::Changed));
    // Truncated: a read of what is not resident is short.
    f.set_len(whole.len() as u64 / 4).unwrap();
    let len = store.len(b);
    let mut out = vec![0u8; len as usize];
    assert_eq!(store.copy_to(b, 0..len, &mut out), Err(ReadError::Changed));
    assert_eq!(store.hash(b), Err(ReadError::Changed));
    assert_eq!(store.recheck(b), Err(ReadError::Changed));
    // What is resident still reads: it is the file as it was read.
    let p = &store.parts(b)[0];
    let r = store.resident(b, p.offset..p.end()).unwrap();
    assert_eq!(store.bytes(r), &whole[p.offset as usize..p.end() as usize]);
}

/// §2.5 under the staged store: a FIFO is never opened.
#[test]
fn s11_5_staged_fifo() {
    let s = Scratch::new("s11stagedfifo");
    let Some(mkfifo) = common::need("mkfifo") else { return };
    let p = s.join("pipe");
    let mut c = Command::new(mkfifo);
    c.arg(&p);
    assert!(mortar_testutil::run_with(&mut c, b"", Duration::from_secs(30)).success());
    let store = staged();
    assert_eq!(read_one(&store, &p), Err(ReadError::NotRegular("is a FIFO")));
    assert_eq!(store.stats().opens, 0);
}

/// A huge sparse file: an archive with a 1 GiB member of zeros. The staged
/// store reads its header and a window, not the gigabyte.
#[test]
fn s11_1_staged_huge_sparse_archive() {
    let s = Scratch::new("s11stagedsparse");
    let p = s.join("libsparse.a");
    let size: u64 = 1 << 30;
    let mut head = b"!<arch>\n".to_vec();
    head.extend(
        format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", "zero.o/", 0, 0, 0, 644, size).bytes(),
    );
    std::fs::write(&p, &head).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    if f.set_len(head.len() as u64 + size).is_err() {
        eprintln!("note: no sparse files here; skipped");
        return;
    }
    let store = staged();
    let b = read_one(&store, &p).unwrap();
    let st = store.stats();
    assert!(st.bytes < 4 << 20, "{st:?}");
    assert!(st.resident < 4 << 20, "{st:?}");
    let facts = facts::analyse_view(store.view(b), false);
    let Kind::Archive(a) = &facts.kind else { panic!("{:?}", facts.kind) };
    assert_eq!(a.members.len(), 1);
    let MemberContent::Inline { offset, len, .. } = a.members[0].content else { panic!() };
    assert_eq!(len, size);
    let member = facts::analyse_view(store.view(b).sub(offset, len).unwrap(), true);
    assert!(matches!(member.kind, Kind::NotObject("not an ELF file")), "{:?}", member.kind);
    let mut tail = [1u8; 16];
    store.copy_to(b, offset + len - 16..offset + len, &mut tail).unwrap();
    assert_eq!(tail, [0u8; 16]);
}

/// §11.6: the staged store hashes when asked, by reading again; equal to
/// the resident store's hash of the same bytes, for files and members.
#[test]
fn s11_6_staged_hash_on_demand() {
    let s = Scratch::new("s11stagedhash");
    let (Some(a), Some(_)) = (big_object(&s, "a.o", 11), big_object(&s, "b.o", 12)) else {
        return;
    };
    let Some(lib) = common::ar(s.path(), "libab.a", &["a.o", "b.o"], false) else { return };
    let store = staged();
    assert!(!store.hashes());
    let b = read_one(&store, &a).unwrap();
    assert_eq!(store.hash(b), Ok(content_hash(&std::fs::read(&a).unwrap())));
    let whole = std::fs::read(&lib).unwrap();
    let lb = read_one(&store, &lib).unwrap();
    assert_eq!(store.hash(lb), Ok(content_hash(&whole)));
    assert_eq!(store.hash_range(lb, 8..68), Ok(content_hash(&whole[8..68])));
    // The walk fills `File::hash` when the configuration asks it to, with
    // the resident store's values.
    let hashed = |policy, hashes| {
        let config = StoreConfig { hashes, ..StoreConfig::default() };
        let store = leak(Store::with_config(policy, config, Hooks::default()));
        let out = walk_with(store, s.path(), &[M, "--whole-archive", "libab.a"], &Pool::new(2));
        assert!(out.problems.is_empty());
        out.files.iter().map(|f| f.hash.0).collect::<Vec<_>>()
    };
    let resident = hashed(StorePolicy::Resident, None);
    assert_eq!(resident.len(), 2);
    assert!(resident.iter().all(|h| *h != 0));
    assert_eq!(hashed(StorePolicy::Staged, Some(true)), resident);
    assert_eq!(hashed(StorePolicy::Staged, None), [0, 0]);
}

/// §12 for the staged store: the tables and every blob's parts are the
/// same under every schedule.
#[test]
fn s12_staged_determinism() {
    let s = Scratch::new("s11stageddet");
    let (Some(_), Some(_)) = (big_object(&s, "a.o", 13), big_object(&s, "b.o", 14)) else {
        return;
    };
    common::cc(s.path(), "native", "c.o", "int c(void){return 3;}\n", &[]).unwrap();
    let Some(_) = common::ar(s.path(), "libabc.a", &["a.o", "c.o", "b.o"], false) else { return };
    let args = [M, "c.o", "libabc.a", "--whole-archive", "libabc.a", "a.o"];
    check_schedules(|pool| {
        let store = leak(staged());
        let out = walk_with(store, s.path(), &args, pool);
        let mut parts: Vec<(String, Vec<(u64, u64)>)> = out
            .files
            .iter()
            .map(|f| {
                let ps = store.parts(f.blob).iter().map(|p| (p.offset, p.end())).collect();
                (f.name.path().to_owned(), ps)
            })
            .collect();
        parts.dedup();
        (common::fingerprint(&out), parts)
    });
}

/// `spec/elf.md` §3.0 from the store's side: a view of a member is the
/// member's bytes, offsets relative to it.
#[test]
fn s11_3_member_view() {
    let s = Scratch::new("s11stagedmember");
    let Some(_) = big_object(&s, "a.o", 15) else { return };
    let Some(lib) = common::ar(s.path(), "liba.a", &["a.o"], false) else { return };
    let whole = std::fs::read(&lib).unwrap();
    let store = staged();
    let b = read_one(&store, &lib).unwrap();
    let Kind::Archive(a) = facts::analyse_view(store.view(b), false).kind else { panic!() };
    let MemberContent::Inline { offset, len, .. } = a.members[0].content else { panic!() };
    let view: View<'_> = store.view(b).sub(offset, len).unwrap();
    let staged = Object::parse_view(view).unwrap();
    let reference = Object::parse(&whole[offset as usize..(offset + len) as usize]).unwrap();
    for (x, y) in staged.sections().zip(reference.sections()) {
        if x.is_resident() {
            assert_eq!(x.data().unwrap(), y.data().unwrap());
        }
    }
}
