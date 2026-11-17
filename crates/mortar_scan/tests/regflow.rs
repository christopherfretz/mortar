//! `spec/dyn.md` D-117, "Bound use through a register" (items 1-9): the
//! AArch64 analysis of `mortar_scan::regflow`, on objects assembled or
//! compiled by the test (`support::ARCH`'s GNU tools, Clang), with the
//! preemptible definitions of the default line (`regflow::default_line`:
//! every `STV_DEFAULT` global definition, as the oracle
//! `tests/shared-design/exp3/s1d.py` assumes).
//!
//! The off-gate check (record `spec/landed/dyn/WP-119.md` §4) is
//! [`corpus`], ignored by default: it prints, for every object of a
//! list, the analysis's uses in `s1d.py --hits`'s format.

mod support;

use std::fmt::Write as _;
use std::path::Path;

use mortar_diag::{FileRef, Style};
use mortar_elf::Object;
use mortar_elf::archive::Archive;
use mortar_scan::regflow::{Analysis, Reach, analyse, default_line, resident};
use mortar_testutil::Scratch;
use support::{assemble, compile, need};

/// The analysis of an object file's bytes with the default line.
fn analyse_bytes(bytes: &[u8]) -> Analysis {
    let obj = Object::parse(bytes).expect("an object");
    let defs = default_line(&obj).expect("symbols");
    analyse(&obj, &defs, &mut resident(&obj)).expect("analysis")
}

fn analyse_file(path: &Path) -> Analysis {
    analyse_bytes(&std::fs::read(path).unwrap())
}

/// The uses as `s1d.py --hits` prints them (`ANA <function>+<offset>
/// <form> <section>+<a> <definition> [boundary]`), each once.
fn hits(a: &Analysis) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in &a.uses {
        let form = match u.reach {
            Reach::Computed => "add",
            Reach::Access => "ldst",
            Reach::WriteBack => "wb",
        };
        let line = format!(
            "ANA {}+{:#x} {form} {}+{:#x} {} {}",
            u.function_name,
            u.function_offset,
            u.address_section_name,
            u.address.offset,
            u.definition_name,
            if u.boundary { "boundary" } else { "" }
        );
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// The off-gate check: `REGFLOW_LIST` names a file of object and archive
/// paths, one per line; `REGFLOW_OUT` the report, written in the format
/// of `s1d.py --hits` (an object line for every object the analysis ran
/// on, then its uses; a `TOTAL` line).
#[test]
#[ignore = "off-gate: the corpora of record WP-119 §4"]
fn corpus() {
    let (Ok(list), Ok(report)) = (std::env::var("REGFLOW_LIST"), std::env::var("REGFLOW_OUT"))
    else {
        eprintln!("set REGFLOW_LIST and REGFLOW_OUT");
        return;
    };
    let mut text = String::new();
    let (mut objs, mut pre, mut ana_objs, mut words, mut undec) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let started = std::time::Instant::now();
    let mut one = |name: &str, bytes: &[u8], text: &mut String| {
        let Ok(obj) = Object::parse(bytes) else { return };
        if obj.header().e_machine != mortar_elf::consts::EM_AARCH64 {
            return;
        }
        objs += 1;
        let defs = default_line(&obj).unwrap_or_default();
        let a = match analyse(&obj, &defs, &mut resident(&obj)) {
            Ok(a) => a,
            Err(e) => {
                writeln!(text, "{name} FAULT {e:?}").unwrap();
                return;
            }
        };
        pre += u64::from(a.ran);
        words += a.words;
        undec += a.undecoded;
        ana_objs += u64::from(a.refused());
        if a.ran {
            writeln!(
                text,
                "{name} pre=1 ana={} words={} funcs={}",
                a.uses.len(),
                a.words,
                a.functions
            )
            .unwrap();
            for h in hits(&a) {
                writeln!(text, "    {h}").unwrap();
            }
        }
    };
    for path in std::fs::read_to_string(&list).unwrap().lines().filter(|l| !l.is_empty()) {
        let Ok(bytes) = std::fs::read(path) else { continue };
        if Archive::sniff(&bytes).is_some() {
            let Ok(ar) = Archive::parse(&bytes) else { continue };
            for m in ar.members().flatten() {
                let Some(data) = m.data() else { continue };
                let name = format!("{path}({})", String::from_utf8_lossy(m.name()));
                one(&name, data, &mut text);
            }
        } else {
            one(path, &bytes, &mut text);
        }
    }
    writeln!(
        text,
        "TOTAL objs={objs} pre={pre} ana_objs={ana_objs} words={words} undec={undec} ms={}",
        started.elapsed().as_millis()
    )
    .unwrap();
    std::fs::write(report, text).unwrap();
}
