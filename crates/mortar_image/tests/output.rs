//! The output file (§7): every case of §16.1's list, on the system temp
//! directory (a local file system that enforces `flock`).
#![allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements
)]

mod support;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use mortar_core::par::Pool;
use mortar_image::fill::{self, Sink};
use mortar_image::output::{
    LockMode, Output, OutputOptions, aside_name, identity, remove_stale_output, temp_name,
};
use support::*;

fn opts() -> OutputOptions {
    OutputOptions::executable(Vec::new())
}

fn umask() -> u32 {
    let s = fs::read_to_string("/proc/self/status").unwrap();
    let v = s.lines().find_map(|l| l.strip_prefix("Umask:")).unwrap();
    u32::from_str_radix(v.trim(), 8).unwrap()
}

fn write_commit(path: &Path, opts: &OutputOptions, bytes: &[u8]) {
    let out = Output::create(path, opts).expect("create");
    out.write_at(0, bytes).unwrap();
    out.commit().expect("commit");
}

// spec: image.md I-59, I-60, §7.1 "mode": a fresh output is a regular file
// of mode 0777 & ~umask (0666 & ~umask for -r); no temporary is left.
#[test]
fn i59_fresh_output_and_modes() {
    let d = TempDir::new("fresh");
    let p = d.path("a.out");
    write_commit(&p, &opts(), b"hello");
    assert_eq!(fs::read(&p).unwrap(), b"hello");
    let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o777 & !umask());
    assert_eq!(d.names(), vec!["a.out"]);
    let r = d.path("r.o");
    write_commit(&r, &OutputOptions { mode: 0o666, ..opts() }, b"x");
    assert_eq!(fs::metadata(&r).unwrap().permissions().mode() & 0o777, 0o666 & !umask());
}

// spec: image.md I-60, I-113, §7.1: over an existing output, a new inode;
// another hard link keeps the old bytes; the old output was set aside and
// is gone after the link.
#[test]
fn i113_existing_output_and_hard_link() {
    let d = TempDir::new("existing");
    let p = d.path("a.out");
    fs::write(&p, b"old").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&p, d.path("other")).unwrap();
    let old = identity(&p).unwrap();
    let out = Output::create(&p, &opts()).unwrap();
    assert_eq!(out.aside_path(), Some(aside_name(&p).as_path()));
    assert!(!p.exists(), "set aside before fill");
    out.write_at(0, b"new").unwrap();
    out.commit().unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"new");
    assert_ne!(identity(&p).unwrap(), old);
    assert_eq!(fs::read(d.path("other")).unwrap(), b"old");
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o777 & !umask(),
        "mode not kept"
    );
    assert_eq!(d.names(), vec!["a.out", "other"]);
}

// spec: image.md I-60, §7.1: a symbolic link at P is replaced by a regular
// file, its target untouched; a dangling link is replaced too.
#[test]
fn i60_symlink_and_dangling_symlink() {
    let d = TempDir::new("symlink");
    let target = d.path("target");
    fs::write(&target, b"keep").unwrap();
    let p = d.path("a.out");
    symlink(&target, &p).unwrap();
    write_commit(&p, &opts(), b"new");
    assert!(fs::symlink_metadata(&p).unwrap().file_type().is_file());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    let q = d.path("b.out");
    symlink(d.path("nowhere"), &q).unwrap();
    write_commit(&q, &opts(), b"b");
    assert!(fs::symlink_metadata(&q).unwrap().file_type().is_file());
    assert_eq!(d.names(), vec!["a.out", "b.out", "target"]);
}

// spec: image.md I-57: a directory (also through a link) is
// `cannot-open-output` "Is a directory".
#[test]
fn i57_directory_is_refused() {
    let d = TempDir::new("dir");
    let p = d.path("sub");
    fs::create_dir(&p).unwrap();
    let (k, e) = Output::create(&p, &opts()).unwrap_err();
    assert_eq!(k, mortar_diag::Key::new(105, 0, 0));
    assert_eq!(e.id(), "cannot-open-output");
    assert!(format!("{:?}", e.kind()).contains("Is a directory"));
    let l = d.path("link");
    symlink(&p, &l).unwrap();
    assert!(Output::create(&l, &opts()).is_err());
    assert_eq!(d.names(), vec!["link", "sub"]);
}

// spec: image.md I-58: /dev/null is written in place and stays a device.
#[test]
fn i58_dev_null_is_written_in_place() {
    let p = Path::new("/dev/null");
    let out = Output::create(p, &opts()).unwrap();
    assert!(out.temp_path().is_none());
    out.write_at(1 << 20, b"bytes").unwrap();
    out.commit().unwrap();
    assert!(fs::metadata(p).unwrap().file_type().is_char_device_like());
}

trait CharDev {
    fn is_char_device_like(&self) -> bool;
}
impl CharDev for fs::FileType {
    fn is_char_device_like(&self) -> bool {
        std::os::unix::fs::FileTypeExt::is_char_device(self)
    }
}

// spec: image.md I-58, I-64: /dev/full is a character device: written in
// place, and the write fails with `cannot-write-output` naming P.
#[test]
fn i64_dev_full_write_error() {
    let p = Path::new("/dev/full");
    if !p.exists() {
        eprintln!("no /dev/full: skipped");
        return;
    }
    let world = random_world(2, 64 * 1024, 4096);
    let chunks: Vec<Chunk<'_>> = world.chunks.iter().map(|c| &**c as _).collect();
    let img = image(&world, &chunks);
    let out = Output::create(p, &opts()).unwrap();
    let e = fill::write(&img, &out, out.shown(), &Pool::new(2)).unwrap_err();
    out.abort();
    assert_eq!(e.diagnostics.len(), 1);
    let d = &e.diagnostics[0].1;
    assert_eq!(d.id(), "cannot-write-output");
    let text = format!("{:?}", d.kind());
    assert!(text.contains("/dev/full") && text.contains("No space left"), "{text}");
}

// spec: image.md I-58: a FIFO, a socket and a block device are refused by
// name, with the note.
#[test]
fn i58_fifo_socket_block_device_refused() {
    let d = TempDir::new("special");
    let fifo = d.path("fifo");
    let made = mortar_testutil::run("mkfifo", [&fifo]);
    assert!(made.success(), "mkfifo");
    let (_, e) = Output::create(&fifo, &opts()).unwrap_err();
    assert_eq!(e.id(), "cannot-open-output");
    assert!(format!("{e:?}").contains("output is a FIFO"));
    let sock = d.path("sock");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let (_, e) = Output::create(&sock, &opts()).unwrap_err();
    assert!(format!("{e:?}").contains("output is a socket"));
    let block = fs::read_dir("/dev").ok().and_then(|it| {
        it.filter_map(Result::ok).map(|e| e.path()).find(|p| {
            fs::metadata(p)
                .is_ok_and(|m| std::os::unix::fs::FileTypeExt::is_block_device(&m.file_type()))
        })
    });
    match block {
        Some(b) => {
            let (_, e) = Output::create(&b, &opts()).unwrap_err();
            assert!(format!("{e:?}").contains("output is a block device"));
        }
        None => eprintln!("no block device in /dev: the block-device case is not run here"),
    }
    assert_eq!(d.names(), vec!["fifo", "sock"]);
}

// spec: image.md I-62, I-113, I-66: an output that is an input is
// accepted; it is not set aside, an open descriptor of it keeps its bytes,
// and a failed link does not remove it.
#[test]
fn i62_output_is_an_input() {
    let d = TempDir::new("input");
    let p = d.path("in.o");
    fs::write(&p, b"input bytes").unwrap();
    let held = fs::File::open(&p).unwrap();
    let o = OutputOptions::executable(vec![identity(&p).unwrap()]);
    let out = Output::create(&p, &o).unwrap();
    assert!(out.aside_path().is_none());
    assert!(p.exists());
    out.write_at(0, b"out").unwrap();
    out.commit().unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"out");
    let mut old = String::new();
    std::io::Read::read_to_string(&mut &held, &mut old).unwrap();
    assert_eq!(old, "input bytes");
    // I-66: an input is never removed.
    let q = d.path("in2.o");
    fs::write(&q, b"x").unwrap();
    assert!(!remove_stale_output(&q, &[identity(&q).unwrap()]));
    assert!(q.exists());
}

// spec: image.md I-59, I-67: a stale temporary of a killed link (no lock
// holder) is reused under its name and leaves nothing.
#[test]
fn i59_stale_temporary_is_reused() {
    let d = TempDir::new("stale");
    let p = d.path("a.out");
    let t0 = temp_name(&p);
    fs::write(&t0, b"stale litter").unwrap();
    fs::set_permissions(&t0, fs::Permissions::from_mode(0o600)).unwrap();
    let out = Output::create(&p, &opts()).unwrap();
    let enforced = out.temp_path() == Some(t0.as_path());
    out.write_at(0, b"x").unwrap();
    out.commit().unwrap();
    if enforced {
        assert_eq!(d.names(), vec!["a.out"], "T0 reused and renamed");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o777 & !umask(),
            "this link's mode"
        );
    } else {
        eprintln!("flock is not enforced on {}: T0 kept", d.dir().display());
        assert_eq!(d.names(), vec![".a.out.mortar-tmp", "a.out"]);
    }
}

// spec: image.md I-59: another running link holds T0: this link uses
// T0.<pid> and does not touch T0.
#[test]
fn i59_running_link_holds_the_lock() {
    let d = TempDir::new("held");
    let p = d.path("a.out");
    let t0 = temp_name(&p);
    let holder = fs::OpenOptions::new().write(true).create(true).truncate(false).open(&t0).unwrap();
    holder.lock().unwrap();
    let out = Output::create(&p, &opts()).unwrap();
    let t = out.temp_path().unwrap().to_path_buf();
    assert_ne!(t, t0);
    assert!(t.to_string_lossy().ends_with(&format!(".mortar-tmp.{}", std::process::id())));
    out.write_at(0, b"x").unwrap();
    out.commit().unwrap();
    assert!(t0.exists(), "T0 is the other link's");
    drop(holder);
    assert_eq!(d.names(), vec![".a.out.mortar-tmp", "a.out"]);
}

// spec: image.md I-59: lock not enforced (or unsupported): never reuse a
// name: T0.<pid>, then .1 when that exists; T0 untouched.
#[test]
fn i59_unenforced_lock_falls_back_to_pid_names() {
    let d = TempDir::new("nolock");
    let p = d.path("a.out");
    let o = OutputOptions { lock: LockMode::Unenforced, ..opts() };
    let pid_name = format!(".a.out.mortar-tmp.{}", std::process::id());
    fs::write(d.path(&pid_name), b"stale").unwrap();
    let out = Output::create(&p, &o).unwrap();
    assert_eq!(out.temp_path().unwrap(), d.path(&format!("{pid_name}.1")));
    out.write_at(0, b"x").unwrap();
    out.commit().unwrap();
    assert_eq!(d.names(), vec![pid_name, "a.out".to_owned()]);
}

// spec: image.md I-59: an unwritable directory is `cannot-open-output`
// naming P with the system's reason (not run as root, which bypasses the
// permission).
#[test]
fn i59_read_only_directory() {
    let d = TempDir::new("ro");
    let sub = d.path("ro");
    fs::create_dir(&sub).unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
    let p = sub.join("a.out");
    let probe = fs::write(sub.join("probe"), b"");
    if probe.is_ok() {
        eprintln!(
            "running as root: permissions are bypassed; the case is checked by its error path only"
        );
        let _ = fs::remove_file(sub.join("probe"));
    } else {
        let (_, e) = Output::create(&p, &opts()).unwrap_err();
        let text = format!("{:?}", e.kind());
        assert!(
            text.contains(&*p.to_string_lossy()) && text.contains("Permission denied"),
            "{text}"
        );
    }
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    // A missing directory: the same kind, naming P.
    let (_, e) = Output::create(&d.path("missing/a.out"), &opts()).unwrap_err();
    let text = format!("{:?}", e.kind());
    assert!(text.contains("missing/a.out") && !text.contains("mortar-tmp"), "{text}");
}

// spec: image.md I-65, §16.1 "a failure in the middle of fill leaves no
// output and no temporary": the old output was set aside and is removed;
// nothing is left.
#[test]
fn i65_failed_fill_leaves_nothing() {
    let d = TempDir::new("failed");
    let p = d.path("a.out");
    fs::write(&p, b"previous program").unwrap();
    let mut world = random_world(5, 256 * 1024, 4096);
    let u = world.plan.inputs.iter().find(|u| u.section_size > 64).unwrap().clone();
    world.source.fail = Some((u.file, u.src + 8));
    world.source.files.get_mut(&u.file).unwrap().1 = false;
    let chunks: Vec<Chunk<'_>> = world.chunks.iter().map(|c| &**c as _).collect();
    let img = image(&world, &chunks);
    let out = Output::create(&p, &opts()).unwrap();
    let r = fill::write(&img, &out, out.shown(), &Pool::new(4));
    assert!(r.is_err());
    out.abort();
    assert!(d.names().is_empty(), "{:?}", d.names());
    // Dropping an output that was neither committed nor aborted is an abort.
    let out = Output::create(&p, &opts()).unwrap();
    drop(out);
    assert!(d.names().is_empty(), "{:?}", d.names());
}

// spec: image.md I-66: after a failure past the command line an existing
// regular output is removed, a symbolic link itself (not its target), a
// non-regular file is left alone.
#[test]
fn i66_stale_output_removed_after_failure() {
    let d = TempDir::new("stale-out");
    let p = d.path("a.out");
    fs::write(&p, b"old").unwrap();
    assert!(remove_stale_output(&p, &[]));
    assert!(!p.exists());
    let target = d.path("t");
    fs::write(&target, b"t").unwrap();
    symlink(&target, &p).unwrap();
    assert!(remove_stale_output(&p, &[]));
    assert!(target.exists() && fs::symlink_metadata(&p).is_err());
    let fifo = d.path("fifo");
    assert!(mortar_testutil::run("mkfifo", [&fifo]).success());
    assert!(!remove_stale_output(&fifo, &[]));
    assert!(fifo.exists());
    assert!(!remove_stale_output(&d.path("none"), &[]));
}

// spec: image.md I-79: the map file is written with the same protocol.
#[test]
fn i79_whole_file_writer() {
    let d = TempDir::new("map");
    let p = d.path("out.map");
    mortar_image::output::write_whole(&p, b"mortar-map 1\nend\n", 0o666).unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"mortar-map 1\nend\n");
    assert_eq!(d.names(), vec!["out.map"]);
    let _ = fs::metadata(&p).unwrap().ino();
}

// spec: image.md I-63, I-33: a whole link to a real file: the file's bytes
// are the reference's, written by every schedule.
#[test]
fn i63_fill_into_the_output_file() {
    let d = TempDir::new("e2e");
    let p = d.path("a.out");
    let world = random_world(21, 3 * 1024 * 1024, 65536);
    let chunks: Vec<Chunk<'_>> = world.chunks.iter().map(|c| &**c as _).collect();
    let img = image(&world, &chunks);
    let want = reference(&img);
    for threads in [1, 3, 8] {
        let out = Output::create(&p, &opts()).unwrap();
        fill::write(&img, &out, out.shown(), &Pool::new(threads)).unwrap();
        out.commit().unwrap();
        assert!(fs::read(&p).unwrap() == want, "{threads} threads");
    }
    assert_eq!(d.names(), vec!["a.out"]);
}
