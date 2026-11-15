//! The `.note.gnu.property` matrix against GNU ld (WP-025 deliverable
//! 2): objects written as the review's `propgen.py` writes them (ported
//! here), linked by `ld.bfd -static`, and the output's properties
//! compared with [`PropertyPlan`] over the same input notes.

mod common;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use common::collector;
use mortar_arch::Arch;
use mortar_core::SectionId;
use mortar_elf::{Class, Notes, Object};
use mortar_synth::notes::{ObjectNotes, PropertyPlan};
use mortar_testutil::{Scratch, run, tool};

/// `propgen.py`: an object with one property note of `props` (type,
/// value, datasz), padded to 8, and a symbol `name`.
fn propgen(name: &str, props: &[(u32, u64, usize)]) -> String {
    let mut desc = Vec::new();
    for &(t, v, ds) in props {
        let mut body = v.to_le_bytes()[..ds].to_vec();
        body.resize(ds.next_multiple_of(8), 0);
        desc.push((t, ds, body));
    }
    let dsz: usize = desc.iter().map(|d| 8 + d.2.len()).sum();
    let mut out = format!(
        ".section .note.gnu.property,\"a\",@note\n.p2align 3\n.long 4, {dsz}, 5\n.asciz \"GNU\"\n"
    );
    for (t, ds, b) in desc {
        let bytes = b.iter().map(u8::to_string).collect::<Vec<_>>().join(",");
        let _ = writeln!(out, ".long {t:#x}, {ds}\n.byte {bytes}");
    }
    if props.is_empty() {
        out.clear();
    }
    let sym = if name == "o0" { "_start" } else { name };
    out + &format!(".text\n.globl {sym}\n{sym}: nop\n")
}

type Props = BTreeMap<u32, Vec<u8>>;

fn note_props(data: &[u8]) -> Props {
    let header = mortar_elf::FileHeader::parse(data).unwrap();
    let table = mortar_elf::SectionTable::parse(data, &header).unwrap();
    let mut out = Props::new();
    for s in &table {
        if s.name().unwrap() == b".note.gnu.property" {
            for n in Notes::new(s.data().unwrap(), 8, Class::Elf64, 0) {
                for p in n.unwrap().gnu_properties().into_iter().flatten() {
                    let p = p.unwrap();
                    out.insert(p.pr_type, p.data.to_vec());
                }
            }
        }
    }
    out
}

fn ours(objects: &[Vec<u8>]) -> Option<Props> {
    let notes: Vec<Vec<(SectionId, Vec<u8>)>> = objects
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let obj = Object::parse(d).unwrap();
            obj.sections()
                .filter(|s| s.name().unwrap() == b".note.gnu.property")
                .map(|s| (SectionId(i as u32 + 1), s.data().unwrap().to_vec()))
                .collect()
        })
        .collect();
    let secs: Vec<Vec<(SectionId, &[u8], u64)>> =
        notes.iter().map(|n| n.iter().map(|(id, b)| (*id, &b[..], 8)).collect()).collect();
    let objs: Vec<ObjectNotes<'_>> =
        secs.iter().map(|s| ObjectNotes { path: "x.o", sections: s }).collect();
    let arch = mortar_a64::A64::M1;
    let diag = collector();
    let plan =
        PropertyPlan::new(&objs, Class::Elf64, &|t| arch.property_merge(t), false, &diag).ok()?;
    Some(plan.properties().iter().map(|(t, v)| (*t, v.to_vec())).collect())
}

const BTI: u32 = 0xc000_0000;
const NC: u32 = 2;
const STACK: u32 = 1;
const GAND: u32 = 0xb000_0000;
const GOR: u32 = 0xb000_8000;

/// Each case: per object, its properties.
/// A property: type, value, datasz.
type Prop = (u32, u64, usize);

fn cases() -> Vec<(&'static str, Vec<Vec<Prop>>)> {
    vec![
        ("bti all", vec![vec![(BTI, 3, 4)], vec![(BTI, 3, 4)]]),
        ("bti and", vec![vec![(BTI, 3, 4)], vec![(BTI, 1, 4)]]),
        ("bti one lacks", vec![vec![(BTI, 3, 4)], vec![]]),
        ("nc one lacks (finding 2)", vec![vec![(NC, 0, 0)], vec![]]),
        ("stack max", vec![vec![(STACK, 0x1000, 8)], vec![(STACK, 0x8000, 8)], vec![]]),
        ("generic and", vec![vec![(GAND, 5, 4)], vec![(GAND, 4, 4)]]),
        ("generic and one lacks", vec![vec![(GAND, 5, 4)], vec![]]),
        ("generic or (finding 6)", vec![vec![(GOR, 1, 4)], vec![(GOR, 2, 4)], vec![]]),
        ("none", vec![vec![], vec![]]),
    ]
}

#[test]
fn s95_property_matrix_against_gnu_ld() {
    let (Some(gcc), Some(ld)) = (tool("gcc"), tool("ld.bfd")) else { return };
    if !cfg!(target_arch = "aarch64") {
        eprintln!("note: the property matrix assembles for the host, AArch64 only");
        return;
    }
    let dir = Scratch::new("synth-props");
    let mut table = String::from("| case | GNU ld | mortar |\n|---|---|---|\n");
    let mut differ = Vec::new();
    for (k, (label, objs)) in cases().into_iter().enumerate() {
        let mut paths = Vec::new();
        let mut bytes = Vec::new();
        for (i, props) in objs.iter().enumerate() {
            let s = dir.write(&format!("c{k}o{i}.s"), propgen(&format!("o{i}"), props));
            let o = dir.join(format!("c{k}o{i}.o"));
            let out = run(&gcc, ["-c", s.to_str().unwrap(), "-o", o.to_str().unwrap()]);
            assert!(out.success(), "{}", out.stderr_text());
            bytes.push(std::fs::read(&o).unwrap());
            paths.push(o.to_str().unwrap().to_owned());
        }
        let exe = dir.join(format!("c{k}.out"));
        let mut args =
            vec!["-static".to_owned(), "-o".to_owned(), exe.to_str().unwrap().to_owned()];
        args.extend(paths);
        let out = run(&ld, &args);
        assert!(out.success(), "{label}: {}", out.stderr_text());
        let gnu = note_props(&std::fs::read(&exe).unwrap());
        let mine = ours(&bytes).expect(label);
        let _ = writeln!(table, "| {label} | {gnu:x?} | {mine:x?} |");
        if gnu != mine {
            differ.push(label);
        }
    }
    eprintln!("{table}");
    assert!(differ.is_empty(), "unexplained differences: {differ:?}\n{table}");
}
