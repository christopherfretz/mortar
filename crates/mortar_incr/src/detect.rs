//! Change detection (`spec/incr.md` IN-22, IN-23, IN-24): which inputs
//! changed between the link a state records and the present one.
//!
//! Both sides are states in identity terms: the old one read from disk,
//! the new one captured from the present link (in the dry run, the full
//! link; in an update, the stat results and the hashes of what was read).
//! Paths (the `STAT` rows) are judged by the stat rule first (IN-22: a
//! clean path is unchanged without reading it), then by content hash; a
//! file or archive member (the `FILES` rows, matched by identity, model
//! §7.1) by its content hash, which for a member of a clean archive need
//! not be computed (its archive's bytes are those of the previous link).

use std::collections::HashMap;

use crate::format::{FileRow, State, StatRow, def_kind, stat_tag};

/// The stat rule's verdict on one path (IN-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stat {
    /// Equal tuple, ctime strictly older than a trusted stamp: unchanged.
    Clean,
    /// Equal tuple, ctime not older than the stamp: hashed.
    Racy,
    /// No trusted stamp for its device, or no recorded hash: hashed.
    Untrusted,
    /// The tuple differs: hashed (a `touch` is not a change).
    Differs,
}

/// The stat rule (IN-22, IN-22a): `old` is the state's row, `now` the
/// present `(dev, ino, size, mtime_ns, ctime_ns)`, `devices` the state's
/// device rows.
#[must_use]
pub fn stat_rule(old: &StatRow, now: (u64, u64, u64, i64, i64), devices: &[StatRow]) -> Stat {
    if (old.dev, old.ino, old.size, old.mtime_ns, old.ctime_ns) != now {
        return Stat::Differs;
    }
    let stamp = devices.iter().find(|d| d.tag == stat_tag::DEVICE && d.dev == old.dev);
    match stamp {
        Some(d) if d.trust == 0 && old.trust == 0 => {
            if old.ctime_ns < d.stamp_ns {
                Stat::Clean
            } else {
                Stat::Racy
            }
        }
        _ => Stat::Untrusted,
    }
}

/// One path's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    /// Clean by the stat rule.
    Clean,
    /// Not clean, and its content hash is the recorded one.
    HashedSame,
    /// Its content differs (or no hash could be compared).
    Changed,
    /// Opened by this link only.
    Added,
    /// Opened by the previous link only.
    Removed,
}

impl PathState {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::HashedSame => "hashed-same",
            Self::Changed => "changed",
            Self::Added => "added",
            Self::Removed => "removed",
        }
    }
}

/// One file's (or member's) outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Live in both links, same content.
    Unchanged,
    /// Live in both, the content differs.
    Changed,
    /// Live now, not in the previous link at all.
    Appeared,
    /// Live now; in the previous link a member that was not extracted.
    Extracted,
    /// Not live now (a member not extracted, here or before).
    Dead,
    /// Old rows only: live before, not in this link at all.
    Gone,
    /// Old rows only: live before, now a member that is not extracted.
    Unextracted,
}

impl FileState {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Changed => "changed",
            Self::Appeared => "appeared",
            Self::Extracted => "extracted",
            Self::Dead => "dead",
            Self::Gone => "gone",
            Self::Unextracted => "unextracted",
        }
    }

    /// Whether every unit of the file is refilled (IN-27's first row).
    #[must_use]
    pub const fn refills_all(self) -> bool {
        matches!(self, Self::Changed | Self::Appeared | Self::Extracted)
    }
}

/// The outcome of change detection.
#[derive(Debug, Clone, Default)]
pub struct Changes {
    /// Per new `STAT` input row, and the old rows no longer opened.
    pub paths: Vec<(PathState, Vec<u8>)>,
    /// Per new `FILES` row: its old row and its state.
    pub new_files: Vec<(Option<u32>, FileState)>,
    /// Per old `FILES` row: its new row and its state (`Gone`,
    /// `Unextracted`, or the state of the new row).
    pub old_files: Vec<(Option<u32>, FileState)>,
    /// A path whose stat rule said clean but whose content hash differs
    /// (never expected on a trusted file system; the delta uses the
    /// hash).
    pub stat_missed: Vec<Vec<u8>>,
    /// The `PROBES` whose answer changed (IN-23: a search finds another
    /// file): `(path, old present, new present)`.
    pub probes: Vec<(Vec<u8>, Option<bool>, Option<bool>)>,
    /// A changed path no object or archive of either link has (a linker
    /// script): `options-changed` (§7), with the path.
    pub script_changed: Option<Vec<u8>>,
}

/// A file's identity key (model §7.1).
fn file_key<'s>(s: &'s State, f: &FileRow) -> (&'s [u8], u32, u8, &'s [u8], u32) {
    (s.str(f.path), f.mention, f.member, s.str(f.member_name), f.member_ordinal)
}

/// The live `FILES` rows of a state (`RESOLVED` kind 7).
#[must_use]
pub fn live_files(s: &State) -> Vec<bool> {
    let mut live = vec![false; s.files.len()];
    for r in s.resolved.iter().filter(|r| r.kind == def_kind::LIVE) {
        if let Some(l) = live.get_mut(r.file as usize) {
            *l = true;
        }
    }
    live
}

/// Detects what changed from `old` to `new` (IN-22..IN-24).
#[must_use]
pub fn detect(old: &State, new: &State) -> Changes {
    let mut c = Changes::default();
    // Paths: the stat rule, then the hash.
    let old_paths: HashMap<&[u8], &StatRow> = old
        .stat
        .iter()
        .filter(|r| r.tag == stat_tag::INPUT)
        .map(|r| (old.str(r.path), r))
        .collect();
    let mut path_state: HashMap<&[u8], PathState> = HashMap::new();
    for r in new.stat.iter().filter(|r| r.tag == stat_tag::INPUT) {
        let path = new.str(r.path);
        let state = match old_paths.get(path) {
            None => PathState::Added,
            Some(o) => {
                let now = (r.dev, r.ino, r.size, r.mtime_ns, r.ctime_ns);
                let same_hash = o.trust == 0 && r.trust == 0 && o.hash == r.hash;
                match stat_rule(o, now, &old.stat) {
                    Stat::Clean => {
                        if !same_hash {
                            c.stat_missed.push(path.to_vec());
                        }
                        PathState::Clean
                    }
                    _ if same_hash => PathState::HashedSame,
                    _ => PathState::Changed,
                }
            }
        };
        path_state.insert(path, state);
        c.paths.push((state, path.to_vec()));
    }
    for r in old.stat.iter().filter(|r| r.tag == stat_tag::INPUT) {
        if !path_state.contains_key(old.str(r.path)) {
            c.paths.push((PathState::Removed, old.str(r.path).to_vec()));
        }
    }
    // Files and members, by identity.
    let (old_live, new_live) = (live_files(old), live_files(new));
    let by_key: HashMap<_, u32> =
        new.files.iter().enumerate().map(|(i, f)| (file_key(new, f), i as u32)).collect();
    c.old_files = vec![(None, FileState::Gone); old.files.len()];
    c.new_files = vec![(None, FileState::Dead); new.files.len()];
    for (i, f) in old.files.iter().enumerate() {
        if let Some(&n) = by_key.get(&file_key(old, f)) {
            c.old_files[i].0 = Some(n);
            c.new_files[n as usize].0 = Some(i as u32);
        }
    }
    for (n, f) in new.files.iter().enumerate() {
        let old_row = c.new_files[n].0;
        let state = if !new_live[n] {
            FileState::Dead
        } else {
            match old_row {
                None => FileState::Appeared,
                Some(o) if !old_live[o as usize] => FileState::Extracted,
                Some(o) => {
                    let of = &old.files[o as usize];
                    // The content hashes (IN-22, IN-24). An update need
                    // not hash a member of a clean archive (its bytes are
                    // the previous link's); the dry run has every hash
                    // from the full link, and a clean path whose hash
                    // differs is `stat_missed` above.
                    let same = of.hash == f.hash && of.kind == f.kind && of.len == f.len;
                    if same { FileState::Unchanged } else { FileState::Changed }
                }
            }
        };
        c.new_files[n].1 = state;
    }
    for (i, slot) in c.old_files.iter_mut().enumerate() {
        if !old_live[i] {
            slot.1 = FileState::Dead;
            continue;
        }
        slot.1 = match slot.0 {
            None => FileState::Gone,
            Some(n) if !new_live[n as usize] => FileState::Unextracted,
            Some(n) => c.new_files[n as usize].1,
        };
    }
    // Probes (IN-23).
    let old_probes: HashMap<&[u8], bool> =
        old.probes.iter().map(|p| (old.str(p.path), p.present != 0)).collect();
    let new_probes: HashMap<&[u8], bool> =
        new.probes.iter().map(|p| (new.str(p.path), p.present != 0)).collect();
    let mut probes: Vec<&[u8]> = old_probes.keys().chain(new_probes.keys()).copied().collect();
    probes.sort_unstable();
    probes.dedup();
    for p in probes {
        let (a, b) = (old_probes.get(p).copied(), new_probes.get(p).copied());
        if a != b {
            c.probes.push((p.to_vec(), a, b));
        }
    }
    // A changed path that is no file of either link: a script.
    let file_paths: std::collections::HashSet<&[u8]> = old
        .files
        .iter()
        .map(|f| old.str(f.path))
        .chain(new.files.iter().map(|f| new.str(f.path)))
        .collect();
    c.script_changed = c
        .paths
        .iter()
        .find(|(s, p)| *s != PathState::Clean && *s != PathState::HashedSame && !file_paths.contains(p.as_slice()))
        .map(|(_, p)| p.clone());
    c
}

/// Counts of [`Changes`], for the report's `delta inputs` line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub clean: usize,
    pub hashed_same: usize,
    pub changed: usize,
    pub added: usize,
    pub removed: usize,
    pub files_changed: usize,
    pub appeared: usize,
    pub extracted: usize,
    pub gone: usize,
    pub unextracted: usize,
}

impl Changes {
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut n = Counts::default();
        for (s, _) in &self.paths {
            match s {
                PathState::Clean => n.clean += 1,
                PathState::HashedSame => n.hashed_same += 1,
                PathState::Changed => n.changed += 1,
                PathState::Added => n.added += 1,
                PathState::Removed => n.removed += 1,
            }
        }
        for (_, s) in &self.new_files {
            match s {
                FileState::Changed => n.files_changed += 1,
                FileState::Appeared => n.appeared += 1,
                FileState::Extracted => n.extracted += 1,
                _ => {}
            }
        }
        for (_, s) in &self.old_files {
            match s {
                FileState::Gone => n.gone += 1,
                FileState::Unextracted => n.unextracted += 1,
                _ => {}
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(dev: u64, trust: u8, stamp: i64) -> StatRow {
        StatRow { tag: stat_tag::DEVICE, dev, trust, stamp_ns: stamp, ..StatRow::default() }
    }

    // spec: incr.md IN-22 (clean, racy, untrusted, differs)
    #[test]
    fn the_stat_rule() {
        let row = StatRow {
            tag: stat_tag::INPUT,
            dev: 7,
            ino: 1,
            size: 10,
            mtime_ns: 5,
            ctime_ns: 100,
            ..StatRow::default()
        };
        let t = (7, 1, 10, 5, 100);
        assert_eq!(stat_rule(&row, t, &[dev(7, 0, 101)]), Stat::Clean);
        assert_eq!(stat_rule(&row, t, &[dev(7, 0, 100)]), Stat::Racy);
        assert_eq!(stat_rule(&row, t, &[dev(7, 1, 101)]), Stat::Untrusted);
        assert_eq!(stat_rule(&row, t, &[dev(8, 0, 101)]), Stat::Untrusted);
        assert_eq!(stat_rule(&row, (7, 1, 10, 6, 100), &[dev(7, 0, 101)]), Stat::Differs);
        let untrusted = StatRow { trust: 1, ..row };
        assert_eq!(stat_rule(&untrusted, t, &[dev(7, 0, 101)]), Stat::Untrusted);
    }
}
