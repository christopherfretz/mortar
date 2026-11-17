//! A hostile-state editor for the IN-28a review: decodes a format-3
//! state, changes the fields the positional replay trusts, encodes it
//! again (every table hash recomputed, so only `validate` stands between
//! the edit and the replay). Built by `build-state-edit.sh` against this
//! tree's `mortar_incr`.
//!
//! state_edit STATE VERSION OP ARGS...
//!   dump                     sections, symbols, frag_at, files
//!   swap-sections A B        swap SECTIONS rows A and B (whole rows)
//!   swap-place A B           swap the places of SECTIONS rows A and B
//!   local-index ROW V        SYMBOLS row ROW: at_index = V
//!   global-at ROW F V        SYMBOLS row ROW: at_file = F, at_index = V
//!   frag-at ROW V            FRAG_AT row ROW: its FRAGS row = V
//!   frag-at-swap A B         swap the FRAGS row of FRAG_AT rows A and B
//!   frag-at-key ROW S O      FRAG_AT row ROW: section = S, offset = O (resorted)
//!   fuzz SEED                one deterministic mutation of a positional field
//! The state is rewritten in place; one line says what was done.

#![forbid(unsafe_code)]

use mortar_incr::format;

fn num(s: &str) -> u64 {
    if let Some(h) = s.strip_prefix("0x") {
        u64::from_str_radix(h, 16).expect("hex")
    } else {
        s.parse().expect("number")
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (path, version, op) = (&a[0], &a[1], a[2].as_str());
    let bytes = std::fs::read(path).expect("read state");
    let mut s = match format::decode(&bytes, version.as_bytes()) {
        Ok(s) => s,
        Err(e) => {
            println!("decode failed: {}: {}", e.reason(), e.detail());
            std::process::exit(3);
        }
    };
    let arg = |i: usize| num(&a[3 + i]);
    match op {
        "dump" => {
            for (i, f) in s.files.iter().enumerate() {
                println!("FILES {i} {} base={} len={}", String::from_utf8_lossy(s.str(f.path)), f.base, f.len);
            }
            for (i, x) in s.sections.iter().enumerate() {
                println!(
                    "SECTIONS {i} file={} name={} size={} place={}:{:#x}",
                    x.file,
                    String::from_utf8_lossy(s.str(x.name)),
                    x.size,
                    x.place_chunk,
                    x.place_offset
                );
            }
            for (i, y) in s.symbols.iter().enumerate() {
                println!(
                    "SYMBOLS {i} kind={} file={} ord={} name={} at={}:{}",
                    y.kind,
                    y.file,
                    y.ordinal,
                    String::from_utf8_lossy(s.str(y.name)),
                    y.at_file,
                    y.at_index
                );
            }
            for (i, f) in s.frag_at.iter().enumerate() {
                println!("FRAG_AT {i} section={} offset={} frag={}", f.section, f.offset, f.frag);
            }
            for (i, f) in s.frags.iter().enumerate() {
                println!("FRAGS {i} out={} ord={} off={:#x}", f.out, f.ordinal, f.offset);
            }
            for (i, x) in s.slots.iter().enumerate() {
                println!("SLOTS {i} kind={} symbol={}", x.kind, x.symbol);
            }
            return;
        }
        "swap-sections" => s.sections.swap(arg(0) as usize, arg(1) as usize),
        "swap-place" => {
            let (i, j) = (arg(0) as usize, arg(1) as usize);
            let (c, o) = (s.sections[i].place_chunk, s.sections[i].place_offset);
            s.sections[i].place_chunk = s.sections[j].place_chunk;
            s.sections[i].place_offset = s.sections[j].place_offset;
            s.sections[j].place_chunk = c;
            s.sections[j].place_offset = o;
        }
        "local-index" => s.symbols[arg(0) as usize].at_index = arg(1) as u32,
        "global-at" => {
            let y = &mut s.symbols[arg(0) as usize];
            y.at_file = arg(1) as u32;
            y.at_index = arg(2) as u32;
        }
        "frag-at" => s.frag_at[arg(0) as usize].frag = arg(1),
        "frag-at-swap" => {
            let (i, j) = (arg(0) as usize, arg(1) as usize);
            let t = s.frag_at[i].frag;
            s.frag_at[i].frag = s.frag_at[j].frag;
            s.frag_at[j].frag = t;
        }
        "frag-at-key" => {
            let r = &mut s.frag_at[arg(0) as usize];
            r.section = arg(1) as u32;
            r.offset = arg(2) as u32;
            s.frag_at.sort_unstable();
        }
        "fuzz" => {
            // xorshift64*, deterministic per seed.
            let mut x = arg(0).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut next = || {
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                x.wrapping_mul(0x2545_F491_4F6C_DD1D)
            };
            let pick = |n: usize, r: u64| if n == 0 { 0 } else { (r % n as u64) as usize };
            let field = next() % 6;
            let interesting = [0u64, 1, 2, 3, 4, 7, 8, 0x10, 0xff, 0xffff, 0x7fff_ffff, 0xffff_fffe, 0xffff_ffff];
            let v = if next() % 2 == 0 { interesting[pick(interesting.len(), next())] } else { next() % 64 };
            let what = match field {
                0 if !s.symbols.is_empty() => {
                    let i = pick(s.symbols.len(), next());
                    s.symbols[i].at_index = v as u32;
                    format!("SYMBOLS {i} at_index={v}")
                }
                1 if !s.symbols.is_empty() => {
                    let i = pick(s.symbols.len(), next());
                    if s.symbols[i].kind == format::sym_kind::LOCAL {
                        s.symbols[i].at_index = v as u32;
                        format!("SYMBOLS {i} (local) at_index={v}")
                    } else {
                        s.symbols[i].at_file = (v % (s.files.len() as u64 + 1)) as u32;
                        format!("SYMBOLS {i} at_file={}", s.symbols[i].at_file)
                    }
                }
                2 if !s.frag_at.is_empty() => {
                    let i = pick(s.frag_at.len(), next());
                    s.frag_at[i].frag = v;
                    format!("FRAG_AT {i} at={v}")
                }
                3 if s.frag_at.len() > 1 => {
                    let (i, j) = (pick(s.frag_at.len(), next()), pick(s.frag_at.len(), next()));
                    let t = s.frag_at[i].frag;
                    s.frag_at[i].frag = s.frag_at[j].frag;
                    s.frag_at[j].frag = t;
                    format!("FRAG_AT swap {i} {j}")
                }
                4 if !s.frag_at.is_empty() => {
                    let i = pick(s.frag_at.len(), next());
                    s.frag_at[i].offset = v as u32;
                    s.frag_at.sort_unstable();
                    s.frag_at.dedup_by_key(|r| (r.section, r.offset));
                    format!("FRAG_AT {i} offset={v}")
                }
                _ if s.sections.len() > 1 => {
                    let f = s.sections[pick(s.sections.len(), next())].file;
                    let rows: Vec<usize> =
                        (0..s.sections.len()).filter(|&k| s.sections[k].file == f).collect();
                    let (i, j) = (rows[pick(rows.len(), next())], rows[pick(rows.len(), next())]);
                    s.sections.swap(i, j);
                    format!("SECTIONS swap {i} {j} (file {f})")
                }
                _ => "nothing".to_owned(),
            };
            println!("fuzz {}: {what}", arg(0));
        }
        _ => panic!("unknown op {op}"),
    }
    if op != "fuzz" {
        println!("{op} {:?}", &a[3..]);
    }
    std::fs::write(path, format::encode(&s)).expect("write state");
}
