//! Stamps (`spec/incr.md` IN-22a) and the clean / racy / hashed rule
//! (IN-22), and the binding of the state to its output (IN-21).
//!
//! A stamp is the ctime of a file the link creates and removes: it is
//! compared only with timestamps of its own file system, never with a
//! clock. A file system's stamps are trusted only when its type (from
//! `/proc/self/mountinfo`) is on the allow-list and the probe passes.

use std::fs::{File, FileTimes, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The file-system types whose stamps are trusted after the probe
/// (IN-22a: the measured ones).
pub const ALLOW_LIST: [&str; 3] = ["overlay", "virtiofs", "tmpfs"];

/// The stat tuple of IN-21 and IN-22.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tuple {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

impl Tuple {
    /// The tuple of `metadata`.
    #[must_use]
    pub fn of(m: &std::fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime_ns: ns(m.mtime(), m.mtime_nsec()),
            ctime_ns: ns(m.ctime(), m.ctime_nsec()),
        }
    }
}

/// Seconds and nanoseconds as nanoseconds (saturating; good to 2262).
#[must_use]
pub fn ns(sec: i64, nsec: i64) -> i64 {
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

/// Linux's major and minor numbers of a `st_dev`.
#[must_use]
pub fn major_minor(dev: u64) -> (u64, u64) {
    let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xffff_f000);
    let minor = (dev & 0xff) | ((dev >> 12) & 0xffff_ff00);
    (major, minor)
}

/// The type of the file system mounted with device `dev`, from the text
/// of `/proc/self/mountinfo` (the last mount of that device wins, as the
/// kernel lists mounts in order).
#[must_use]
pub fn fs_type(mountinfo: &str, dev: u64) -> Option<String> {
    let (major, minor) = major_minor(dev);
    let want = format!("{major}:{minor}");
    let mut found = None;
    for line in mountinfo.lines() {
        let mut fields = line.split(' ');
        if fields.nth(2) != Some(want.as_str()) {
            continue;
        }
        // Optional fields end at "-"; the type follows it.
        let mut rest = fields.skip_while(|f| *f != "-");
        rest.next();
        if let Some(t) = rest.next() {
            found = Some(t.to_owned());
        }
    }
    found
}

/// Whether stamps of a file system of type `fs` may be trusted (before the
/// probe).
#[must_use]
pub fn allowed(fs: &str) -> bool {
    ALLOW_LIST.contains(&fs)
}

fn stamp_path(dir: &Path) -> PathBuf {
    dir.join(format!(".mortar-stamp-{}", std::process::id()))
}

/// The stamp file has one name per process and directory (IN-22a), so a
/// process that runs several links at once (a library caller; the tests'
/// in-process links, which all stamp the temporary directory) takes its
/// stamps one at a time: two threads would otherwise create and remove
/// one file between them, and the second removal is `NotFound`. Held from
/// the creation to the removal; a command-line link never waits on it.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)
}

/// Takes one stamp in `dir`: creates the stamp file, `fstat`s it and
/// removes it. Its device and ctime.
pub fn take(dir: &Path) -> io::Result<(u64, i64)> {
    let _one = one_at_a_time();
    let path = stamp_path(dir);
    let file = create(&path)?;
    let meta = file.metadata();
    let removed = std::fs::remove_file(&path);
    let t = Tuple::of(&meta?);
    removed?;
    Ok((t.dev, t.ctime_ns))
}

/// The probe of IN-22a in `dir`: ctime moves forward on a metadata change
/// and strictly forward on a data write made after one. A file system with
/// a coarse timestamp tick may give the write the metadata change's ctime;
/// the write is repeated, a tick apart, until the ctime moves or 200 ms
/// pass (a finding of WP-061: the probe needs a tick between the two).
/// `c2` is taken after the write and before the mtime is restored: the
/// restore is a metadata change of its own, which would move the ctime
/// whether or not the write did (the second finding).
pub fn probe(dir: &Path) -> io::Result<bool> {
    let _one = one_at_a_time();
    let path = stamp_path(dir);
    let file = create(&path)?;
    let result = (|| -> io::Result<bool> {
        file.write_at(b"m", 0)?;
        let c0 = Tuple::of(&file.metadata()?);
        let back = SystemTime::now()
            .checked_sub(Duration::from_hours(24))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        file.set_times(FileTimes::new().set_modified(back))?;
        let c1 = Tuple::of(&file.metadata()?);
        let start = std::time::Instant::now();
        let mut c2 = c1;
        while c2.ctime_ns <= c1.ctime_ns && start.elapsed() < Duration::from_millis(200) {
            std::thread::sleep(Duration::from_micros(50));
            file.write_at(b"m", 0)?;
            c2 = Tuple::of(&file.metadata()?);
        }
        file.set_times(FileTimes::new().set_modified(back))?;
        Ok(c1.ctime_ns >= c0.ctime_ns && c2.ctime_ns > c1.ctime_ns && c2.size == c0.size)
    })();
    let removed = std::fs::remove_file(&path);
    let ok = result?;
    removed?;
    Ok(ok)
}

/// One device's stamp, as the state records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStamp {
    /// The directory the stamp was taken in.
    pub dir: PathBuf,
    pub dev: u64,
    pub ctime_ns: i64,
    /// From mountinfo; empty if not found.
    pub fs_type: String,
    /// On the allow-list and the probe passed.
    pub trusted: bool,
}

/// One stamp per device among `dirs`, in order, the first writable
/// directory of each device winning (IN-22a). Directories that are not
/// writable or do not exist are skipped. Sorted by device.
#[must_use]
pub fn stamps(dirs: &[PathBuf], mountinfo: &str) -> Vec<DeviceStamp> {
    let mut out: Vec<DeviceStamp> = Vec::new();
    for dir in dirs {
        let Ok(meta) = std::fs::metadata(dir) else { continue };
        if !meta.is_dir() || out.iter().any(|s| s.dev == meta.dev()) {
            continue;
        }
        let Ok((dev, ctime_ns)) = take(dir) else { continue };
        let fs_type = fs_type(mountinfo, dev).unwrap_or_default();
        let trusted = allowed(&fs_type) && probe(dir).unwrap_or(false);
        out.push(DeviceStamp { dir: dir.clone(), dev, ctime_ns, fs_type, trusted });
    }
    out.sort_by_key(|s| s.dev);
    out
}

/// What the stat rule says of a file (IN-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanliness {
    /// Its tuple is the recorded one and its ctime is older than the
    /// previous link's stamp of its device: unchanged without reading it.
    Clean,
    /// Its tuple is the recorded one but its ctime is not older than the
    /// stamp: hashed.
    Racy,
    /// Its tuple differs: hashed (and changed if the hash differs).
    Changed,
    /// Its device has no trusted stamp: hashed.
    Hashed,
}

/// The rule of IN-22: `recorded` is the state's tuple, `now` today's,
/// `stamp` the previous link's trusted stamp of the recorded device, if
/// any.
#[must_use]
pub fn classify(recorded: &Tuple, now: &Tuple, stamp: Option<i64>) -> Cleanliness {
    if recorded != now {
        return Cleanliness::Changed;
    }
    match stamp {
        None => Cleanliness::Hashed,
        Some(s) if now.ctime_ns < s => Cleanliness::Clean,
        Some(_) => Cleanliness::Racy,
    }
}

/// The binding of IN-21, after the output's last write: `fstat`, stamps in
/// the output's directory until one is later than the output's ctime,
/// `fstat` again. The second tuple and that stamp, or `None` when the two
/// `fstat`s differ (something wrote the output) or no stamp could be taken
/// within 2 s.
pub fn bind_output(output: &Path) -> io::Result<Option<(Tuple, i64)>> {
    let dir = match output.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file = File::open(output)?;
    let first = Tuple::of(&file.metadata()?);
    let start = std::time::Instant::now();
    let stamp = loop {
        let (dev, ctime) = take(&dir)?;
        if dev != first.dev {
            return Ok(None);
        }
        if ctime > first.ctime_ns {
            break ctime;
        }
        if start.elapsed() > Duration::from_secs(2) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_micros(20));
    };
    let second = Tuple::of(&file.metadata()?);
    Ok((first == second).then_some((second, stamp)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: incr.md IN-22a (one stamp name per process and directory:
    // several links of one process take their stamps one at a time).
    // Without the lock a `take` or a `probe` here fails with `NotFound`
    // within a few hundred rounds (the flake of
    // `stamp_rule_b_own_output_is_clean` beside an in-process link).
    #[test]
    fn stamps_of_one_process_in_one_directory_do_not_collide() {
        let scratch = mortar_testutil::Scratch::new("incr-stamp-threads");
        let dir = scratch.path().to_path_buf();
        std::thread::scope(|s| {
            for t in 0..4 {
                let dir = &dir;
                s.spawn(move || {
                    for i in 0..150 {
                        if (i + t) % 8 == 0 {
                            probe(dir).unwrap();
                        } else {
                            take(dir).unwrap();
                        }
                    }
                });
            }
        });
        assert!(!stamp_path(&dir).exists());
    }

    // spec: incr.md IN-22a (a mountinfo line maps to its type; off-list
    // types are always hashed)
    #[test]
    fn mountinfo_types() {
        let text = "\
36 35 98:0 /mnt1 /mnt1 rw,noatime master:1 - ext3 /dev/root rw,errors=continue
40 30 0:52 / /work rw,relatime - virtiofs work rw
41 30 0:53 / /x rw shared:3 master:2 - fuse sshfs#h: rw,user_id=0
42 30 0:54 / / rw - overlay overlay rw,lowerdir=/a";
        let dev = |major: u64, minor: u64| (major << 8) | minor;
        assert_eq!(fs_type(text, dev(98, 0)).as_deref(), Some("ext3"));
        assert_eq!(fs_type(text, dev(0, 52)).as_deref(), Some("virtiofs"));
        assert_eq!(fs_type(text, dev(0, 53)).as_deref(), Some("fuse"));
        assert_eq!(fs_type(text, dev(0, 54)).as_deref(), Some("overlay"));
        assert_eq!(fs_type(text, dev(1, 1)), None);
        assert!(allowed("virtiofs") && allowed("overlay") && allowed("tmpfs"));
        assert!(!allowed("fuse") && !allowed("vfat") && !allowed("ext3") && !allowed("nfs"));
    }

    // spec: incr.md IN-22a (Linux's st_dev encoding)
    #[test]
    fn device_numbers() {
        assert_eq!(major_minor((8 << 8) | 1), (8, 1));
        // glibc's makedev.
        let makedev = |major: u64, minor: u64| {
            ((major & 0xfff) << 8)
                | ((major & !0xfff) << 32)
                | (minor & 0xff)
                | ((minor & !0xff) << 12)
        };
        for (major, minor) in [(0, 52), (259, 3), (0x1234_5678, 0xabcd_ef01), (4095, 1 << 20)] {
            assert_eq!(major_minor(makedev(major, minor)), (major, minor));
        }
    }

    // spec: incr.md IN-22 (clean, racy, changed, hashed)
    #[test]
    fn the_rule() {
        let t = Tuple { dev: 1, ino: 2, size: 3, mtime_ns: 4, ctime_ns: 100 };
        assert_eq!(classify(&t, &t, Some(101)), Cleanliness::Clean);
        assert_eq!(classify(&t, &t, Some(100)), Cleanliness::Racy);
        assert_eq!(classify(&t, &t, None), Cleanliness::Hashed);
        let moved = Tuple { mtime_ns: 3, ..t };
        assert_eq!(classify(&t, &moved, Some(1000)), Cleanliness::Changed);
    }
}
