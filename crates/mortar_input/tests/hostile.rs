//! Hostile inputs (`spec/input.md` §16): errors, never panics or hangs.
//! Script cycles, directories and FIFOs are in `s5_scripts.rs` and
//! `s2_what_a_file_is.rs`; a file that changes during the read in
//! `s11_s12_store_parallel.rs`.
mod common;

use std::fmt::Write as _;

use mortar_input::facts::analyse;
use mortar_testutil::Scratch;

const M: &str = "-maarch64linux";

fn object(s: &Scratch) -> Option<Vec<u8>> {
    let o = common::cc(s.path(), "native", "f.o", "int f(void){return 1;}\nint g;\n", &["-g"])?;
    Some(std::fs::read(o).unwrap())
}

#[test]
fn hostile_truncated_archive() {
    let s = Scratch::new("hosttrunc");
    if object(&s).is_none() {
        return;
    }
    common::cc(s.path(), "native", "h.o", "int h;\n", &[]);
    common::ar(s.path(), "libf.a", &["h.o", "f.o"], false);
    let whole = std::fs::read(s.join("libf.a")).unwrap();
    let mut seen_member = false;
    // Every cut of the archive: an error or a smaller archive, never a
    // panic; the member before the cut is still read.
    for cut in (0..whole.len()).step_by(37) {
        s.write("cut.a", &whole[..cut]);
        let out = common::run(s.path(), &[M, "cut.a"]);
        assert!(
            !(cut > 8 && cut < whole.len() && out.problems.is_empty()),
            "cut at {cut} of {} accepted",
            whole.len()
        );
        seen_member |= !out.files.is_empty() && out.failed();
    }
    assert!(seen_member, "a cut after the first member keeps the first member");
}

#[test]
fn hostile_corrupt_archive_headers() {
    let s = Scratch::new("hostcorrupt");
    if object(&s).is_none() {
        return;
    }
    common::ar(s.path(), "libf.a", &["f.o"], false);
    let whole = std::fs::read(s.join("libf.a")).unwrap();
    // Each byte of the first two member headers, flipped.
    let mut errors = 0;
    for i in 8..(8 + 60 * 2).min(whole.len()) {
        let mut b = whole.clone();
        b[i] ^= 0x5a;
        s.write("bad.a", &b);
        let out = common::run(s.path(), &[M, "bad.a"]);
        errors += usize::from(out.failed());
        for p in &out.problems {
            assert!(
                ["malformed-input", "ignored-member", "refused", "incompatible-file"]
                    .contains(&p.id()),
                "{}",
                p.render()
            );
        }
    }
    assert!(errors > 0);
    // A member size past the end.
    let mut b = whole.clone();
    let size_at = whole.windows(2).position(|w| w == b"`\n").unwrap() - 10;
    b[size_at..size_at + 10].copy_from_slice(b"9999999999");
    s.write("big.a", &b);
    let out = common::run(s.path(), &[M, "big.a"]);
    assert_eq!(common::ids(&out), ["malformed-input"], "{:?}", common::report(&out));
}

#[test]
fn hostile_corrupt_objects() {
    let s = Scratch::new("hostobj");
    let Some(bytes) = object(&s) else { return };
    // Every prefix, and every header byte flipped: the analysis returns.
    for cut in (0..bytes.len()).step_by(13) {
        let _ = analyse(&bytes[..cut], false);
        let _ = analyse(&bytes[..cut], true);
    }
    for i in 0..64 {
        let mut b = bytes.clone();
        b[i] ^= 0xff;
        let _ = analyse(&b, false);
    }
    // Section header offset past the end, through the walk.
    let mut b = bytes.clone();
    b[0x28..0x30].copy_from_slice(&u64::MAX.to_le_bytes());
    s.write("shoff.o", &b);
    let out = common::run(s.path(), &[M, "shoff.o"]);
    assert_eq!(common::ids(&out), ["malformed-input"]);
    // A truncated object.
    s.write("short.o", &bytes[..bytes.len() / 2]);
    let out = common::run(s.path(), &[M, "short.o"]);
    assert_eq!(common::ids(&out), ["malformed-input"]);
}

#[test]
fn hostile_unreadable_and_loops() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new("hostperm");
    s.write("locked.o", b"\x7fELF");
    std::fs::set_permissions(s.join("locked.o"), std::fs::Permissions::from_mode(0o000)).unwrap();
    std::os::unix::fs::symlink("loop1.o", s.join("loop2.o")).unwrap();
    std::os::unix::fs::symlink("loop2.o", s.join("loop1.o")).unwrap();
    let out = common::run(s.path(), &[M, "loop1.o"]);
    assert_eq!(
        common::report(&out),
        ["error: loop1.o: cannot open loop1.o: Too many levels of symbolic links"]
    );
    let readable_anyway = std::fs::read(s.join("locked.o")).is_ok();
    let out = common::run(s.path(), &[M, "locked.o"]);
    if readable_anyway {
        // Root reads mode 000 files: the bytes are a short ELF header.
        assert_eq!(common::ids(&out), ["malformed-input"]);
    } else {
        assert_eq!(
            common::report(&out),
            ["error: locked.o: cannot open locked.o: Permission denied"]
        );
    }
    std::fs::set_permissions(s.join("locked.o"), std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn hostile_zero_length_files() {
    let s = Scratch::new("hostzero");
    for n in ["z.o", "libz.a", "libz.so", "z.ld"] {
        s.write(n, b"");
    }
    let out = common::run(
        s.path(),
        &[M, "z.o", "libz.a", "-L.", "-lz", "z.ld", "--whole-archive", "libz.a"],
    );
    assert!(out.problems.is_empty(), "{:?}", common::report(&out));
    assert!(out.files.is_empty());
    assert_eq!(out.link.opened.len(), 5);
}

#[test]
fn hostile_ten_thousand_small_files() {
    let s = Scratch::new("host10k");
    let Some(o) = common::cc(s.path(), "native", "f.o", "int f;\n", &[]) else { return };
    let bytes = std::fs::read(o).unwrap();
    let mut args = vec![M.to_owned()];
    // Distinct files, a third of them empty, a third inside scripts.
    let mut script = String::new();
    for i in 0..10_000 {
        let name = format!("f{i}.o");
        match i % 3 {
            0 => {
                s.write(&name, &bytes);
                args.push(name);
            }
            1 => {
                s.write(&name, b"");
                args.push(name);
            }
            _ => {
                s.write(&name, &bytes);
                let _ = writeln!(script, "INPUT({name})");
            }
        }
    }
    s.write("all.ld", script);
    args.push("all.ld".into());
    let pool = mortar_core::par::Pool::new(4);
    let out = common::run_plan(
        s.path(),
        &common::plan(&args),
        mortar_input::walk::Options::default(),
        &pool,
    );
    assert!(out.problems.is_empty(), "{:?}", &common::report(&out)[..1]);
    assert_eq!(out.files.len(), 3334 + 3333);
    assert!(out.waves <= 3, "{} waves", out.waves);
}
