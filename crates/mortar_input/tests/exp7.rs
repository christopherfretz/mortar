//! WP-071's acceptance test, experiment 7 (`spec/odr.md` §13, row WP-071),
//! the store half: run by hand on objects that carry real units (the test
//! producer at the row counts of protoc's objects and of the heavy TU of
//! `spec/meta.md` §10; the scripts and numbers are in
//! `spec/landed/meta/WP-071.md`).
//!
//! `MORTAR_EXP7="a.o lib.a ..." cargo test --release -p mortar_input --test
//! exp7 -- --ignored --nocapture`
//!
//! Prints, for the staged store: the reads and bytes read with the
//! containers unwanted (MT-42: the reads of a link with no container) and wanted (each
//! container byte once), and the cost of reading every unit and grouping
//! its rows by key (`spec/odr.md` O-22) on one thread.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, reason = "a measurement")]

use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use mortar_core::par::Pool;
use mortar_elf::Object;
use mortar_elf::archive::{Archive, Content};
use mortar_input::store::{Blob, Hooks, ReadJob, ReadSite, Store, StoreConfig, StorePolicy};
use mortar_meta::read::{FileReader, SectionAttrs};

/// A container: the file it is in (index of the input), its range in that
/// file, and its section header's attributes.
struct Container {
    file: usize,
    range: Range<u64>,
    attrs: SectionAttrs,
}

fn containers_of(file: usize, base: u64, object: &[u8], out: &mut Vec<Container>) {
    let Ok(obj) = Object::parse(object) else { return };
    for s in obj.sections() {
        if mortar_meta::is_container(s.name().unwrap_or(b""), s.sh_type(), s.flags()) {
            let h = s.header();
            out.push(Container {
                file,
                range: base + h.sh_offset..base + h.sh_offset + h.sh_size,
                attrs: SectionAttrs {
                    index: s.index().0,
                    addralign: h.sh_addralign,
                    entsize: h.sh_entsize,
                    link: h.sh_link,
                    info: h.sh_info,
                },
            });
        }
    }
}

fn overlap(a: &Range<u64>, b: &Range<u64>) -> u64 {
    a.end.min(b.end).saturating_sub(a.start.max(b.start))
}

type Reads = Arc<Mutex<Vec<(Option<PathBuf>, Option<Blob>, Range<u64>)>>>;

fn store(containers: bool) -> (Store, Reads) {
    let log: Reads = Arc::default();
    let l = Arc::clone(&log);
    let hooks = Hooks {
        on_read: Some(Box::new(move |site, range| {
            let mut l = l.lock().unwrap();
            match site {
                ReadSite::Load(p) => l.push((Some(p.to_path_buf()), None, range)),
                ReadSite::Copy(b) => l.push((None, Some(b), range)),
            }
        })),
        ..Hooks::default()
    };
    (
        Store::with_config(
            StorePolicy::Staged,
            StoreConfig { containers, ..StoreConfig::default() },
            hooks,
        ),
        log,
    )
}

#[test]
#[ignore = "experiment 7: run by hand with MORTAR_EXP7"]
#[allow(clippy::too_many_lines)] // one measurement, read top to bottom
fn experiment_7() {
    let Ok(list) = std::env::var("MORTAR_EXP7") else { return };
    let files: Vec<PathBuf> = list.split_whitespace().map(PathBuf::from).collect();
    let mut cs = Vec::new();
    let mut members = 0;
    for (i, p) in files.iter().enumerate() {
        let whole = std::fs::read(p).unwrap();
        if let Ok(a) = Archive::parse(&whole) {
            for m in a.members() {
                let m = m.unwrap();
                if let Content::Inline { offset, data } = m.content()
                    && data.starts_with(b"\x7fELF")
                {
                    members += 1;
                    containers_of(i, offset, data, &mut cs);
                }
            }
        } else {
            containers_of(i, 0, &whole, &mut cs);
        }
    }
    let total: u64 = cs.iter().map(|c| c.range.end - c.range.start).sum();
    println!(
        "inputs {}, archive members {members}, containers {}, container bytes {total}",
        files.len(),
        cs.len()
    );

    let pool = Pool::new(1);
    for wanted in [false, true] {
        let (st, log) = store(wanted);
        let jobs: Vec<ReadJob> =
            files.iter().map(|p| ReadJob { path: p.clone(), stat: st.stat(p) }).collect();
        let t = Instant::now();
        let blobs: Vec<Blob> = st.read_many(&pool, &jobs).into_iter().map(Result::unwrap).collect();
        let load_ns = t.elapsed().as_nanos();
        let loads = std::mem::take(&mut *log.lock().unwrap());
        let container_bytes_loaded: u64 = loads
            .iter()
            .map(|(p, _, r)| {
                cs.iter()
                    .filter(|c| p.as_ref() == Some(&files[c.file]))
                    .map(|c| overlap(r, &c.range))
                    .sum::<u64>()
            })
            .sum();
        let bytes: u64 = loads.iter().map(|(_, _, r)| r.end - r.start).sum();
        let s = st.stats();
        let mut resident_container = 0u64;
        let mut by_file = vec![0u64; files.len()];
        for (i, &b) in blobs.iter().enumerate() {
            for part in st.parts(b) {
                let held = part.offset..part.offset + part.bytes.len() as u64;
                resident_container += cs
                    .iter()
                    .filter(|c| c.file == i)
                    .map(|c| overlap(&held, &c.range))
                    .sum::<u64>();
            }
        }
        for (p, _, r) in &loads {
            for c in &cs {
                if p.as_ref() == Some(&files[c.file]) {
                    by_file[c.file] += overlap(r, &c.range);
                }
            }
        }
        println!("  container bytes resident {resident_container}; read by file {by_file:?}");
        println!(
            "containers wanted={wanted}: load {:.1} ms, reads {}, bytes read {bytes}, resident {}, \
             container bytes read while loading {container_bytes_loaded}",
            load_ns as f64 / 1e6,
            loads.len(),
            s.resident
        );
        if !wanted {
            continue;
        }
        // O-22: read every unit, group its rows by key (one thread; the
        // spec's parallel hash partition merged in file order has the same
        // work).
        let t = Instant::now();
        let mut owned: Vec<Vec<u8>> = Vec::with_capacity(cs.len());
        for c in &cs {
            let mut buf = vec![0u8; (c.range.end - c.range.start) as usize];
            st.copy_to(blobs[c.file], c.range.clone(), &mut buf).unwrap();
            owned.push(buf);
        }
        let read_ns = t.elapsed().as_nanos();
        let t = Instant::now();
        let mut keys: Vec<(u64, u32, u8)> = Vec::new();
        let mut findings = 0;
        for (n, (c, bytes)) in cs.iter().zip(&owned).enumerate() {
            let mut r = FileReader::new();
            r.section(c.attrs, bytes);
            findings += r.findings.len();
            for u in &r.units {
                keys.extend(u.definitions().iter().map(|x| (x.key, n as u32, 2)));
                keys.extend(u.types().iter().map(|x| (x.key, n as u32, 3)));
                keys.extend(u.declarations().iter().map(|x| (x.key, n as u32, 5)));
            }
        }
        let parse_ns = t.elapsed().as_nanos();
        let t = Instant::now();
        keys.sort_unstable();
        let mut groups = 0u64;
        let mut shared = 0u64;
        let mut i = 0;
        while i < keys.len() {
            let mut j = i + 1;
            while j < keys.len() && keys[j].0 == keys[i].0 && keys[j].2 == keys[i].2 {
                j += 1;
            }
            groups += 1;
            shared += u64::from(j - i > 1);
            i = j;
        }
        let group_ns = t.elapsed().as_nanos();
        let log = log.lock().unwrap();
        let copied: u64 = log
            .iter()
            .filter_map(|(_, b, r)| b.map(|b| (b, r)))
            .map(|(b, r)| {
                cs.iter().filter(|c| blobs[c.file] == b).map(|c| overlap(r, &c.range)).sum::<u64>()
            })
            .sum();
        println!(
            "  O-22: rows {}, findings {findings}; read {:.1} ms ({} container bytes by copy_to), \
             parse {:.1} ms, group {:.1} ms ({groups} keys, {shared} in more than one unit); \
             total {:.1} ms, {:.0} ns a row",
            keys.len(),
            read_ns as f64 / 1e6,
            copied,
            parse_ns as f64 / 1e6,
            group_ns as f64 / 1e6,
            (read_ns + parse_ns + group_ns) as f64 / 1e6,
            (read_ns + parse_ns + group_ns) as f64 / keys.len().max(1) as f64
        );
    }
}
