//! Differential and scan tests over the installed libraries of the three
//! targets: every object's `.eh_frame` parses with `readelf`'s record
//! counts (S-34 to S-40), and no object hits a refusal of phase 60 (the
//! `Merge` cut, S-6 to S-12, and `.eh_frame`).
mod support;

use std::path::{Path, PathBuf};

use mortar_core::par::Pool;
use mortar_merge::phase;
use mortar_testutil::{tool, tools_required};
use support::{Objects, lib_dirs, members, readelf_frames, relocs};

/// The objects and archives scanned per target.
fn inputs(gcc: &Path, libc: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for c in ["crtbegin.o", "crtbeginS.o", "crtbeginT.o", "crtend.o", "crtendS.o"] {
        v.push(gcc.join(c));
    }
    for c in ["crt1.o", "crti.o", "crtn.o", "Scrt1.o", "rcrt1.o"] {
        v.push(libc.join(c));
    }
    for a in ["libgcc.a", "libgcc_eh.a", "libstdc++.a", "libsupc++.a", "libgfortran.a"] {
        v.push(gcc.join(a));
    }
    for a in ["libc.a", "libc_nonshared.a", "libm-2.41.a", "libmvec.a", "libresolv.a"] {
        v.push(libc.join(a));
    }
    v.retain(|p| p.exists());
    v
}

#[derive(Default, Debug)]
struct Totals {
    objects: usize,
    merge_sections: usize,
    pieces: usize,
    frags: usize,
    whole: usize,
    eh_sections: usize,
    cies: usize,
    fdes: usize,
    classes: u32,
}

#[test]
fn s34_s40_libraries_parse_with_readelf_counts_and_no_refusal() {
    let Some(readelf) = tool("readelf") else {
        assert!(!tools_required(), "readelf is required");
        return;
    };
    let dirs = lib_dirs();
    if dirs.len() < 3 {
        assert!(!tools_required(), "the libraries of the three targets are required: {dirs:?}");
    }
    let pool = Pool::new(4);
    for (machine, gcc, libc) in dirs {
        let mut t = Totals::default();
        for path in inputs(&gcc, &libc) {
            let files = if path.extension().is_some_and(|e| e == "a") {
                members(&path)
            } else {
                vec![(path.display().to_string(), std::fs::read(&path).unwrap())]
            };
            if files.is_empty() {
                continue;
            }
            let objs = Objects::new(files);
            assert_eq!(objs.machine, machine, "{}", path.display());
            let r = relocs(machine);
            let out = match phase::run(&pool, &objs, r.as_ref()) {
                Ok(o) => o,
                Err(problems) => {
                    let shown: Vec<String> = problems
                        .iter()
                        .take(10)
                        .map(|p| format!("{}: {:?}", objs.names[p.file.0 as usize], p))
                        .collect();
                    panic!(
                        "{}: {} problems:\n{}",
                        path.display(),
                        problems.len(),
                        shown.join("\n")
                    );
                }
            };
            // Our counts per file.
            let mut ours = vec![(0usize, 0usize, 0usize); objs.len()];
            for (s, sec) in out.eh.sections.iter().enumerate() {
                let f = objs.file_of(sec.section);
                for r in out.eh.records_of(s) {
                    if r.cie().is_some() {
                        ours[f].0 += 1;
                    } else {
                        ours[f].1 += 1;
                    }
                }
                ours[f].2 += usize::from(sec.terminator.is_some());
            }
            let theirs = readelf_frames(&readelf, &path);
            assert_eq!(theirs.len(), ours.len(), "{}: files", path.display());
            for (f, (a, b)) in ours.iter().zip(&theirs).enumerate() {
                assert_eq!(a, b, "{}: {} (CIEs, FDEs, terminators)", path.display(), objs.names[f]);
            }
            t.objects += objs.len();
            t.merge_sections += out.split.merge_of.len();
            t.pieces += out.split.piece_off.len();
            t.frags += out.split.frags.len() as usize;
            t.whole += out.whole.len();
            t.eh_sections += out.eh.sections.len();
            t.cies += ours.iter().map(|o| o.0).sum::<usize>();
            t.fdes += ours.iter().map(|o| o.1).sum::<usize>();
            t.classes += out.eh.classes;
        }
        eprintln!("{machine:?}: {t:?}");
    }
}
