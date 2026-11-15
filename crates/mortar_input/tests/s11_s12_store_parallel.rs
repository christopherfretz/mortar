//! `spec/input.md` §11 (the store) and §12 (parallelism and determinism).
#![allow(clippy::many_single_char_names, reason = "short names for fixtures and tallies")]

mod common;

use std::io::Write as _;
use std::path::Path;

use mortar_core::par::{Pool, check_schedules};
use mortar_input::store::{CHUNK, Hooks, ReadError, ReadJob, Store, StorePolicy, content_hash};
use mortar_input::walk::{Loaded, Options};
use mortar_testutil::Scratch;

fn job(store: &Store, p: &Path) -> ReadJob {
    ReadJob { path: p.to_path_buf(), stat: store.stat(p) }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()
}

#[test]
fn s11_2_chunked_read() {
    let s = Scratch::new("s11chunk");
    let n = (2 * CHUNK + 12_345) as usize;
    let bytes = pattern(n);
    let p = s.write("big.bin", &bytes);
    let small = s.write("small.bin", b"abc");
    for threads in [1, 4] {
        let store = Store::new(StorePolicy::Resident);
        let r = store.read_many(&Pool::new(threads), &[job(&store, &p), job(&store, &small)]);
        let b = r[0].clone().unwrap();
        assert_eq!(store.whole(b), &bytes[..]);
        assert_eq!(store.len(b), n as u64);
        assert_eq!(store.hash(b), content_hash(&bytes));
        assert_eq!(store.whole(r[1].clone().unwrap()), b"abc");
        let st = store.stats();
        // Per file: one open, two fstats, one positioned read per chunk
        // plus the read at the end, one close (§11.2).
        assert_eq!((st.opens, st.fstats, st.reads, st.closes), (2, 4, 3 + 1 + 1 + 1, 2));
        assert_eq!(st.bytes, n as u64 + 3);
        // §11.1: a resident range inside the one part; none outside.
        assert!(store.resident(b, 10..20).is_some());
        assert!(store.resident(b, 0..n as u64 + 1).is_none());
        let mut out = [0u8; 4];
        store.copy_to(b, 100..104, &mut out).unwrap();
        assert_eq!(out, bytes[100..104]);
        assert!(matches!(
            store.copy_to(b, n as u64 - 2..n as u64 + 2, &mut out),
            Err(ReadError::OutOfRange)
        ));
    }
}

#[test]
fn s11_2_same_inode_read_once() {
    let s = Scratch::new("s11inode");
    let p = s.write("a.bin", b"hello");
    std::fs::hard_link(&p, s.join("b.bin")).unwrap();
    let store = Store::new(StorePolicy::Resident);
    let r = store.read_many(&Pool::new(2), &[job(&store, &p)]);
    let r2 = store.read_many(&Pool::new(2), &[job(&store, &s.join("b.bin"))]);
    assert_eq!(r[0], r2[0]);
    assert_eq!(store.stats().opens, 1);
}

#[test]
fn s11_2_size_limit() {
    let s = Scratch::new("s11limit");
    let p = s.join("huge.bin");
    let f = std::fs::File::create(&p).unwrap();
    if f.set_len(mortar_input::store::MAX_INPUT).is_err() {
        eprintln!("note: no sparse 1 TiB file here; skipped");
        return;
    }
    drop(f);
    let store = Store::new(StorePolicy::Resident);
    let r = store.read_many(&Pool::sequential(), &[job(&store, &p)]);
    assert_eq!(r[0], Err(ReadError::TooLarge("input file of 1 TiB or more")));
    assert_eq!(store.stats().opens, 0);
    std::fs::remove_file(&p).unwrap();
}

fn hooked(edit: fn(&Path)) -> Store {
    Store::with_hooks(
        StorePolicy::Resident,
        Hooks { after_fstat: Some(Box::new(move |p: &Path| edit(p))) },
    )
}

#[test]
fn s11_5_short_read() {
    let s = Scratch::new("s11short");
    let p = s.write("f.bin", pattern(CHUNK as usize + 100));
    let store = hooked(|p| {
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_len(10).unwrap();
    });
    let r = store.read_many(&Pool::new(2), &[job(&store, &p)]);
    assert_eq!(r[0], Err(ReadError::Changed));
    assert_eq!(store.stats().opens, store.stats().closes);
}

#[test]
fn s11_5_grew() {
    let s = Scratch::new("s11grew");
    let p = s.write("f.bin", b"0123456789");
    let store = hooked(|p| {
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(b"more").unwrap();
    });
    let r = store.read_many(&Pool::sequential(), &[job(&store, &p)]);
    assert_eq!(r[0], Err(ReadError::Changed));
}

#[test]
fn s11_5_changed_between_stat_and_open() {
    let s = Scratch::new("s11replaced");
    let p = s.write("f.bin", b"0123456789");
    let store = Store::new(StorePolicy::Resident);
    let j = job(&store, &p);
    // Replaced by rename after the stat: another inode.
    s.write("g.bin", b"0123456789");
    std::fs::rename(s.join("g.bin"), &p).unwrap();
    let r = store.read_many(&Pool::sequential(), &[j]);
    assert_eq!(r[0], Err(ReadError::Changed));
}

#[test]
fn s11_5_changed_is_one_diagnostic_at_the_lowest_mention() {
    let s = Scratch::new("s11walkchanged");
    if common::cc(s.path(), "native", "f.o", "int f(void){return 1;}\n", &[]).is_none() {
        return;
    }
    let store: &'static Store = Box::leak(Box::new(hooked(|p| {
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(b"x").unwrap();
    })));
    let plan = common::plan(&["-maarch64linux", "f.o", "./f.o", "f.o"]);
    let out = mortar_input::walk::walk(
        &plan,
        &mortar_input::walk::LoadEnv { cwd: s.path().to_path_buf() },
        store,
        &Pool::new(2),
        Options::default(),
    );
    assert_eq!(common::report(&out), ["error: f.o: file changed while it was read"]);
    assert_eq!(out.problems[0].unit, 1);
    assert!(out.files.is_empty());
}

#[test]
fn s11_4_descriptor_bound() {
    let s = Scratch::new("s11fds");
    let Some(o) = common::cc(s.path(), "native", "f.o", "int f;\n", &[]) else { return };
    let bytes = std::fs::read(o).unwrap();
    let mut args = vec!["-maarch64linux".to_owned()];
    for i in 0..5000 {
        let name = format!("f{i}.o");
        s.write(&name, &bytes);
        args.push(name);
    }
    s.write("big.o", {
        let mut b = bytes.clone();
        b.resize(3 * CHUNK as usize, 0);
        b
    });
    args.push("big.o".into());
    for threads in [1, 4] {
        let store: &'static Store = Box::leak(Box::new(Store::new(StorePolicy::Resident)));
        let out = mortar_input::walk::walk(
            &common::plan(&args),
            &mortar_input::walk::LoadEnv { cwd: s.path().to_path_buf() },
            store,
            &Pool::new(threads),
            Options::default(),
        );
        assert!(out.problems.is_empty(), "{:?}", &common::report(&out)[..1]);
        assert_eq!(out.files.len(), 5001);
        let st = store.stats();
        assert!(st.peak_open <= 2 * threads as u64, "{threads} threads: {} open", st.peak_open);
        assert_eq!(st.opens, st.closes);
    }
}

/// What a determinism run compares: everything but the store's own blob
/// numbering, which §12 does not make observable.
fn summary(out: &Loaded<'_>) -> String {
    let rows: Vec<_> = out
        .files
        .iter()
        .map(|f| {
            format!(
                "{:?} {}{} {:?} {:?} {:?} {} {} {} {} {:?} {:?} {:?}",
                f.kind,
                f.name.path(),
                f.name.member().unwrap_or(""),
                f.member,
                f.origin,
                f.state,
                f.base,
                f.len,
                f.sections,
                f.globals,
                f.identity,
                f.hash,
                f.soname,
            )
        })
        .collect();
    format!(
        "{:?}\n{}\n{:?}\n{:?}\n{}",
        out.target,
        rows.join("\n"),
        out.archives,
        out.link,
        common::report(out).join("\n")
    )
}

#[test]
fn s12_determinism() {
    let s = Scratch::new("s12det");
    if !common::have(&["native"]) {
        return;
    }
    std::fs::create_dir_all(s.join("lib")).unwrap();
    std::fs::create_dir_all(s.join("hid")).unwrap();
    for i in 0..12 {
        common::cc(
            s.path(),
            "native",
            &format!("o{i}.o"),
            &format!("int o{i}(void){{return {i};}}\n"),
            &[],
        );
    }
    common::cc(s.path(), "native", "big.o", "char big[5 << 20] = {1};\n", &[]);
    common::ar(s.path(), "lib/libm1.a", &["o0.o", "o1.o", "o2.o"], false);
    common::ar(s.path(), "lib/libthin.a", &["o3.o", "o4.o"], true);
    common::cc(
        &s.join("hid"),
        "native",
        "libz.so.9",
        "int z;\n",
        &["-shared", "-fPIC", "-nostdlib", "-Wl,-soname,libz.so.9"],
    );
    let hid = format!("-L{}", s.join("hid").display());
    common::cc(
        &s.join("lib"),
        "native",
        "libd.so",
        "int d;\n",
        &[
            "-shared",
            "-fPIC",
            "-nostdlib",
            &hid,
            "-Wl,--no-as-needed",
            "-l:libz.so.9",
            "-Wl,-soname,libd.so",
        ],
    );
    s.write("s.ld", "GROUP(o5.o -lm1 AS_NEEDED(-ld))\nINPUT(missing.o)\n");
    s.write("junk.o", "junk\n");
    let args = [
        "-maarch64linux",
        "-dynamic-linker",
        "/lib/ld.so",
        "-Llib",
        "o6.o",
        "big.o",
        "s.ld",
        "junk.o",
        "--start-group",
        "lib/libthin.a",
        "o7.o",
        "--end-group",
        "-lnothere",
        "o8.o",
        "o9.o",
        "lib/libm1.a",
        "-rpath-link",
        "hid",
        "o10.o",
        "o11.o",
        "big.o",
    ];
    let plan = common::plan(&args);
    let reference = check_schedules(|pool| {
        summary(&common::run_plan(s.path(), &plan, Options { closure: true }, pool))
    });
    // §16: thread counts 1, 2, 7, 64.
    for threads in [1, 2, 7, 64] {
        let got = summary(&common::run_plan(
            s.path(),
            &plan,
            Options { closure: true },
            &Pool::new(threads),
        ));
        assert_eq!(got, reference, "{threads} threads");
    }
    assert!(reference.contains("libz.so.9"), "{reference}");
    assert!(reference.contains("cannot open missing.o"), "{reference}");
}
