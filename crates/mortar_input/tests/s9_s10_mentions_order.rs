//! `spec/input.md` §9 (the same file twice) and §10 (load order and ids).
mod common;

use mortar_input::walk::OpenedKind;
use mortar_testutil::Scratch;

const M: &str = "-maarch64linux";

fn objs(s: &Scratch, names: &[&str]) -> bool {
    for n in names {
        if common::cc(
            s.path(),
            "native",
            n,
            &format!("int sym_{}(void){{return 1;}}\n", n.replace('.', "_")),
            &[],
        )
        .is_none()
        {
            return false;
        }
    }
    true
}

#[test]
fn s9_1_mentions() {
    let s = Scratch::new("s9mentions");
    if !objs(&s, &["f.o"]) {
        return;
    }
    common::ar(s.path(), "libf.a", &["f.o"], false);
    std::fs::hard_link(s.join("f.o"), s.join("hard.o")).unwrap();
    std::os::unix::fs::symlink("libf.a", s.join("link.a")).unwrap();
    s.write("s.ld", "INPUT(f.o)\n");
    let out = common::run(
        s.path(),
        &[M, "f.o", "./f.o", "hard.o", "libf.a", "libf.a", "link.a", "-L.", "-lf", "s.ld", "s.ld"],
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(
        common::names(&out),
        [
            "f.o",
            "./f.o",
            "hard.o",
            "libf.a(f.o)",
            "libf.a(f.o)",
            "link.a(f.o)",
            "./libf.a(f.o)",
            "f.o",
            "f.o"
        ]
    );
    assert_eq!(out.archives.len(), 4);
    // One store entry per (device, inode): f.o, ./f.o and hard.o share it.
    assert_eq!(out.files[0].blob, out.files[1].blob);
    assert_eq!(out.files[0].blob, out.files[2].blob);
}

#[test]
fn s9_3_error_keyed_to_lowest_mention() {
    let s = Scratch::new("s9once");
    std::fs::create_dir(s.join("d.o")).unwrap();
    s.write("s.ld", "INPUT(d.o)\n");
    let out = common::run(s.path(), &[M, "s.ld", "d.o", "d.o"]);
    let r = common::report(&out);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(
        r[0],
        "error: s.ld:1:7: cannot open d.o: Is a directory\n>>> named by argument 's.ld'"
    );
    assert_eq!(out.problems[0].unit, 1);
}

#[test]
fn s9_4_identity() {
    let s = Scratch::new("s9ident");
    let Some(ar) = common::need("ar") else { return };
    if !objs(&s, &["a.o"]) {
        return;
    }
    std::fs::create_dir(s.join("x")).unwrap();
    common::cc(&s.join("x"), "native", "a.o", "int other(void){return 2;}\n", &[]);
    // Two members named a.o (`q` appends).
    assert!(
        common::tool_run(
            &ar,
            &["qc".into(), "liba.a".into(), "a.o".into(), "x/a.o".into()],
            s.path()
        )
        .success()
    );
    let out = common::run_hashed(s.path(), &[M, "a.o", "a.o", "./a.o", "liba.a", "liba.a"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    let id: Vec<_> = out
        .files
        .iter()
        .map(|f| {
            (
                String::from_utf8_lossy(&f.identity.path).into_owned(),
                f.identity.mention,
                f.identity
                    .member
                    .as_ref()
                    .map(|(n, o)| (String::from_utf8_lossy(n).into_owned(), *o)),
            )
        })
        .collect();
    let m = |n: &str, o| Some((n.to_owned(), o));
    assert_eq!(
        id,
        [
            ("a.o".to_owned(), 0, None),
            ("a.o".to_owned(), 1, None),
            ("./a.o".to_owned(), 0, None),
            ("liba.a".to_owned(), 0, m("a.o", 0)),
            ("liba.a".to_owned(), 0, m("a.o", 1)),
            ("liba.a".to_owned(), 1, m("a.o", 0)),
            ("liba.a".to_owned(), 1, m("a.o", 1)),
        ]
    );
    // Equal bytes, equal hash; other bytes, another.
    assert_eq!(out.files[0].hash, out.files[2].hash);
    assert_eq!(out.files[0].hash, out.files[3].hash);
    assert_ne!(out.files[3].hash, out.files[4].hash);
}

#[test]
fn s10_1_walk_numbering() {
    let s = Scratch::new("s10walk");
    if !objs(&s, &["a.o", "b.o", "c.o", "d.o"]) {
        return;
    }
    common::ar(s.path(), "libbc.a", &["b.o", "c.o"], false);
    s.write("t.txt", "x\n");
    common::ar(s.path(), "libnone.a", &["t.txt"], false);
    s.write("s.ld", "INPUT(libbc.a)\nINPUT(d.o)\n");
    s.write("empty.o", b"");
    let out = common::run(
        s.path(),
        &[M, "a.o", "empty.o", "libnone.a", "s.ld", "--start-lib", "a.o", "--end-lib"],
    );
    assert_eq!(common::ids(&out), ["ignored-member"]);
    assert_eq!(common::names(&out), ["a.o", "libbc.a(b.o)", "libbc.a(c.o)", "d.o", "a.o"]);
    let arch: Vec<_> =
        out.archives.iter().map(|a| String::from_utf8_lossy(&a.path).into_owned()).collect();
    assert_eq!(arch, ["libnone.a", "libbc.a"]);
    let lazy: Vec<_> = out.files.iter().map(|f| f.state.lazy).collect();
    assert_eq!(lazy, [false, false, false, false, true]);
    let kinds: Vec<_> = out.link.opened.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            OpenedKind::Object,
            OpenedKind::Empty,
            OpenedKind::Archive,
            OpenedKind::Script,
            OpenedKind::Archive,
            OpenedKind::Object,
            OpenedKind::Object,
        ]
    );
    // Script entries carry the entry index and the script.
    assert_eq!(out.files[1].origin.entry, Some(0));
    assert_eq!(out.files[3].origin.entry, Some(1));
    assert_eq!(out.files[3].origin.arg, 4);
}

#[test]
fn s10_1_errors_do_not_stop_the_walk() {
    let s = Scratch::new("s10errors");
    if !objs(&s, &["a.o", "b.o"]) {
        return;
    }
    s.write("bad.ld", "INPUT(a.o\n");
    s.write("s.ld", "INPUT(missing.o b.o)\n");
    s.write("junk.o", "junk junk\n");
    let out = common::run(s.path(), &[M, "nothere.o", "junk.o", "bad.ld", "s.ld", "-lnone", "a.o"]);
    let ids = common::ids(&out);
    assert!(out.failed());
    assert_eq!(ids.len(), 5, "{:?}", common::report(&out));
    assert_eq!(common::names(&out), ["b.o", "a.o"]);
    // Keys: by argument.
    let units: Vec<_> = out.problems.iter().map(|p| p.unit).collect();
    let mut sorted = units.clone();
    sorted.sort_unstable();
    assert_eq!(units, sorted);
    assert_eq!(units.first(), Some(&1));
}

#[test]
fn s10_2_group_ordinals() {
    let s = Scratch::new("s10groups");
    if !objs(&s, &["a.o", "b.o", "c.o", "d.o"]) {
        return;
    }
    s.write("g.ld", "GROUP(c.o)\n");
    s.write("in.ld", "GROUP(d.o)\n");
    let out = common::run(
        s.path(),
        &[M, "-(", "a.o", "-)", "b.o", "g.ld", "--start-group", "in.ld", "a.o", "--end-group"],
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert_eq!(out.link.group, [Some(1), None, Some(2), Some(3), Some(3)]);
}

#[test]
fn s10_3_exclude_libs() {
    let s = Scratch::new("s10exclude");
    if !objs(&s, &["a.o"]) {
        return;
    }
    std::fs::create_dir(s.join("ex")).unwrap();
    common::ar(&s.join("ex"), "libex.a", &["../a.o"], false);
    common::ar(s.path(), "libother.a", &["a.o"], false);
    let ex = |names: &str| {
        let out = common::run(s.path(), &[M, "--exclude-libs", names, "ex/libex.a", "libother.a"]);
        (out.files.iter().map(|f| f.state.exclude_libs).collect::<Vec<_>>(), common::report(&out))
    };
    assert_eq!(ex("libex.a"), (vec![true, false], vec![]));
    assert_eq!(ex("ALL"), (vec![true, true], vec![]));
    assert_eq!(ex("ex/libex.a"), (vec![false, false], vec![]));
    let (flags, r) = ex("libex");
    assert_eq!(flags, [false, false]);
    assert_eq!(
        r,
        [
            "error: argument '--exclude-libs libex': --exclude-libs: invalid value 'libex'\n>>> GNU ld matches an archive name without its .a; lld and mold do not"
        ]
    );
}

#[test]
fn s10_4_link_table() {
    let s = Scratch::new("s10table");
    if !objs(&s, &["a.o"]) {
        return;
    }
    s.write("s.ld", "OUTPUT(out.bin)\nENTRY(start)\nEXTERN(x y)\nINPUT(a.o)\n");
    let out = common::run(s.path(), &[M, "s.ld", "-R", "/usr/lib"]);
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    let d: Vec<_> = out
        .link
        .directives
        .iter()
        .map(|d| (d.command, String::from_utf8_lossy(&d.value).into_owned()))
        .collect();
    assert_eq!(
        d,
        [
            ("OUTPUT", "out.bin".into()),
            ("ENTRY", "start".into()),
            ("EXTERN", "x".into()),
            ("EXTERN", "y".into())
        ]
    );
    assert_eq!(out.link.script_entries.len(), 1);
    assert_eq!(out.link.rpath_from_r, [(2, (*b"/usr/lib").into())]);
}
