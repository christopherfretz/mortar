//! Self-checking i386 relaxation sites (the review harness's
//! `genrelax32.py`, ported): every form of x86.md §8 (G1–G5) and §9 (I1–I5,
//! D1–D3, L1–L2, T1–T2), each with the registers and operand kinds the
//! form admits, the GOT loads of `abs` and `weak0` (X86-24), plain `GOT32`
//! (X86-26) and the `_TLS_MODULE_BASE_` descriptor with a `TLS_LDO_32` in
//! code (X86-61, X86-63).
//!
//! Each check pushes the value the instruction under test computed and
//! stores it with the value an unrelaxable computation gives (`GOTOFF`,
//! `TLS_LE`, an immediate) into `res`; a binary operation also checks the
//! flags (mask `0x8d5`: CF PF ZF SF OF). `relaxmain.c` compares the pairs
//! and prints "N checks, F failures".
//!
//! Three sets, because the references refuse different forms:
//! - [`Set::Common`]: what GNU ld and lld both link, with the values both
//!   compute (lld's I5 `lea` fails the flags checks of I5, §15.1);
//! - [`Set::Lld`]: L1 and D3 with a base other than `%ebx` (GNU ld
//!   refuses both: X86-35 for L1; for D3 the spec does not say so yet),
//!   in static-pie the `PC32` partners of D1/D3/L1 (GNU ld makes them a
//!   text relocation; not in the spec either) and the thread-local `weak0` (GNU ld writes `TpRel(0)`,
//!   X86-23);
//! - [`Set::Bfd`]: T1 into a register other than `%eax` (lld refuses it,
//!   X86-37).
#![allow(dead_code)]

use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Set {
    Common,
    Lld,
    Bfd,
}

/// Every 32-bit register, in `ModRM` order.
const R32: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
/// The registers a base-plus-`disp32` operand can name (rm ≠ 100).
const BASES: [&str; 7] = ["eax", "ecx", "edx", "ebx", "ebp", "esi", "edi"];
/// G2's operations, in opcode order (`03 0b 13 1b 23 2b 33 3b`).
const BINOPS: [&str; 8] = ["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"];

/// `%ebx` = `_GLOBAL_OFFSET_TABLE_` (a `GOTPC` with `A = 3`).
const GOT: &str = "  call 9f\n9: popl %ebx\n  addl $_GLOBAL_OFFSET_TABLE_+(.-9b), %ebx\n";
const K: u32 = 0x10000;

struct Gen {
    out: String,
    names: Vec<String>,
    /// The `call`s of the GD and LD forms: `PLT32`, and `PC32` unless it
    /// is left out (GNU ld's static-pie refuses a `PC32` partner).
    calls: &'static [(&'static str, &'static str)],
}

const BOTH: &[(&str, &str)] = &[("___tls_get_addr@PLT", "PLT32"), ("___tls_get_addr", "PC32")];
const PLT_ONLY: &[(&str, &str)] = &[("___tls_get_addr@PLT", "PLT32")];
const PC32_ONLY: &[(&str, &str)] = &[("___tls_get_addr", "PC32")];

impl Gen {
    /// One check: `test` (with `%ebx` = GOT) leaves the value pushed;
    /// `want` (with `%ebx` = GOT) leaves the expected value in `%eax`.
    fn chk(&mut self, desc: &str, test: &str, want: &str) {
        let k = self.names.len();
        self.names.push(desc.to_string());
        let _ = write!(
            self.out,
            "# {desc}\n{GOT}{test}\n{GOT}{want}\n  movl %eax, res@GOTOFF+{}(%ebx)\n  popl %eax\n  \
             movl %eax, res@GOTOFF+{}(%ebx)\n",
            8 * k + 4,
            8 * k
        );
    }

    /// Two checks, the value and the flags: `test` pushes the flags, then
    /// the value; `want` leaves the value in `%eax` and the flags in `%edx`.
    fn chk2(&mut self, desc: &str, test: &str, want: &str) {
        let k = self.names.len();
        self.names.push(format!("{desc} (value)"));
        self.names.push(format!("{desc} (flags)"));
        let _ = write!(
            self.out,
            "# {desc}\n{GOT}{test}\n{GOT}{want}\n  andl $0x8d5, %edx\n  movl %eax, res@GOTOFF+{}(%ebx)\n  \
             movl %edx, res@GOTOFF+{}(%ebx)\n  popl %eax\n  movl %eax, res@GOTOFF+{}(%ebx)\n  \
             popl %eax\n  andl $0x8d5, %eax\n  movl %eax, res@GOTOFF+{}(%ebx)\n",
            8 * k + 4,
            8 * k + 12,
            8 * k,
            8 * k + 8
        );
    }
}

/// `movl %ebx, %b` (the base gets the GOT), nothing for `%ebx` or no base.
fn setbase(b: &str) -> String {
    if matches!(b, "ebx" | "none") { String::new() } else { format!("  movl %ebx, %{b}\n") }
}

/// The memory operand `sym@GOT(%b)`, or `sym@GOT` without a base.
fn got_op(sym: &str, b: &str) -> String {
    if b == "none" { format!("{sym}@GOT") } else { format!("{sym}@GOT(%{b})") }
}

/// `%ecx` = the address of `sym`, by a computation no linker relaxes.
fn addr_ecx(sym: &str) -> String {
    match sym {
        "absv" => "  movl $0x1234, %ecx".into(),
        "wund" => "  xorl %ecx, %ecx".into(),
        _ => format!("  leal {sym}@GOTOFF(%ebx), %ecx"),
    }
}

/// `%eax` = the address of thread-local `x` (`TLS_LE`); TP for `weak0`
/// (X86-23: its TP offset is 0).
fn tls_want(x: &str) -> String {
    if x == "tw" {
        "  movl %gs:0, %eax".into()
    } else {
        format!("  movl %gs:0, %eax\n  leal {x}@ntpoff(%eax), %eax")
    }
}

/// `ModRM` `10 rr rm` of a base register.
fn rm(b: &str) -> u8 {
    u8::try_from(R32.iter().position(|r| *r == b).unwrap()).unwrap()
}

/// `call *___tls_get_addr@GOT(%b)` as a `GOT32` (gas types the `call`
/// `GOT32X`).
fn call_got32(b: &str) -> String {
    format!(
        "  .byte 0xff, {:#04x}\n  .reloc ., R_386_GOT32, ___tls_get_addr\n  .long 0",
        0x90 | rm(b)
    )
}

/// The GOT forms G1–G5 (X86-25), plain `GOT32` (X86-26), and the
/// `abs`/`weak0` slots (X86-24).
fn got_sites(g: &mut Gen, pic: bool) {
    let mut bases: Vec<&str> = BASES.to_vec();
    if !pic {
        bases.push("none");
    }
    let few: &[&str] = if pic { &["ebx", "esi"] } else { &["ebx", "esi", "none"] };
    // G1: every destination with every base for a local symbol; the
    // other targets with `%ebx` and no base.
    for sym in ["ldata", "lfn", "absv", "wund"] {
        for r in R32 {
            let bs: &[&str] = if sym == "ldata" { &bases } else { few };
            for &b in bs {
                if r == "esp" && !matches!(b, "ebx" | "none") {
                    continue;
                }
                let test = if r == "esp" {
                    format!(
                        "  movl %esp, %edi\n  movl {}, %esp\n  movl %esp, %eax\n  movl %edi, %esp\n  pushl %eax",
                        got_op(sym, b)
                    )
                } else {
                    format!("{}  movl {}, %{r}\n  pushl %{r}", setbase(b), got_op(sym, b))
                };
                g.chk(
                    &format!("G1 mov {sym}@GOT({b}) -> {r}"),
                    &test,
                    &format!("{}\n  movl %ecx, %eax", addr_ecx(sym)),
                );
            }
        }
    }
    // G2, G3: every operation and destination, the value and the flags.
    for sym in ["ldata", "absv", "wund"] {
        for op in BINOPS.iter().chain(&["test"]) {
            for r in R32 {
                if r == "esp" {
                    continue;
                }
                for &b in few {
                    if b == r {
                        continue;
                    }
                    let pre = if matches!(*op, "adc" | "sbb") { "  stc" } else { "  clc" };
                    let insn = if *op == "test" {
                        format!("  testl %{r}, {}", got_op(sym, b))
                    } else {
                        format!("  {op}l {}, %{r}", got_op(sym, b))
                    };
                    let form = if *op == "test" { "G3" } else { "G2" };
                    let wop = if *op == "test" {
                        "  testl %ecx, %eax".to_string()
                    } else {
                        format!("  {op}l %ecx, %eax")
                    };
                    g.chk2(
                        &format!("{form} {op} {sym}@GOT({b}), {r}"),
                        &format!(
                            "{}  movl ${K}, %{r}\n{pre}\n{insn}\n  pushfl\n  pushl %{r}",
                            setbase(b)
                        ),
                        &format!(
                            "{}\n  movl ${K}, %eax\n{pre}\n{wop}\n  pushfl\n  popl %edx",
                            addr_ecx(sym)
                        ),
                    );
                }
            }
        }
    }
    // G4, G5: `call`/`jmp` through the GOT of a local function, every base.
    for &b in &bases {
        g.chk(
            &format!("G4 call *lfn@GOT({b})"),
            &format!("{}  call *{}\n  pushl %eax", setbase(b), got_op("lfn", b)),
            "  movl $0x1111, %eax",
        );
        g.chk(
            &format!("G5 jmp *lfn@GOT({b})"),
            &format!(
                "  leal 7f@GOTOFF(%ebx), %eax\n  pushl %eax\n{}  jmp *{}\n7: pushl %eax",
                setbase(b),
                got_op("lfn", b)
            ),
            "  movl $0x1111, %eax",
        );
    }
    // X86-24: the slot of `weak0` holds 0 (a `cmpl $imm8` is plain `GOT32`).
    g.chk(
        "GOT32 cmpl $0, wund@GOT(ebx)",
        "  xorl %eax, %eax\n  cmpl $0, wund@GOT(%ebx)\n  sete %al\n  pushl %eax",
        "  movl $1, %eax",
    );
    // X86-26: plain `GOT32` (a `mov` and a `push`) is never relaxed.
    for sym in ["ldata", "absv", "wund"] {
        for &b in few {
            let modrm = if b == "none" { 0x05 } else { 0x80 | rm(b) };
            g.chk(
                &format!("GOT32 mov {sym}@GOT({b}) -> eax"),
                &format!(
                    "{}  .byte 0x8b, {modrm:#04x}\n  .reloc ., R_386_GOT32, {sym}\n  .long 0\n  pushl %eax",
                    setbase(b)
                ),
                &format!("{}\n  movl %ecx, %eax", addr_ecx(sym)),
            );
            g.chk(
                &format!("GOT32 push {sym}@GOT({b})"),
                &format!("{}  pushl {}", setbase(b), got_op(sym, b)),
                &format!("{}\n  movl %ecx, %eax", addr_ecx(sym)),
            );
        }
    }
}

/// I1–I5 (X86-32).
fn ie_sites(g: &mut Gen, x: &str, ie_abs: bool) {
    let regs = ["eax", "ecx", "edx", "ebx", "ebp", "esi", "edi"];
    for r in regs {
        if ie_abs {
            let form = if r == "eax" { "I1" } else { "I2" };
            g.chk(
                &format!("{form} movl {x}@indntpoff, {r}"),
                &format!("  movl {x}@indntpoff, %{r}\n  addl %gs:0, %{r}\n  pushl %{r}"),
                &tls_want(x),
            );
            g.chk2(
                &format!("I3 addl {x}@indntpoff, {r}"),
                &format!("  movl %gs:0, %{r}\n  addl {x}@indntpoff, %{r}\n  pushfl\n  pushl %{r}"),
                &format!("{}\n  movl %gs:0, %ecx\n  movl %eax, %edx\n  subl %ecx, %edx\n  addl %edx, %ecx\n  pushfl\n  popl %edx", tls_want(x)),
            );
        }
        let bases: &[&str] = if r == "eax" { &BASES } else { &["ebx", "esi"] };
        for &b in bases {
            if b == r {
                continue;
            }
            g.chk(
                &format!("I4 movl {x}@gotntpoff({b}), {r}"),
                &format!(
                    "{}  movl {x}@gotntpoff(%{b}), %{r}\n  addl %gs:0, %{r}\n  pushl %{r}",
                    setbase(b)
                ),
                &tls_want(x),
            );
            g.chk2(
                &format!("I5 addl {x}@gotntpoff({b}), {r}"),
                &format!(
                    "{}  movl %gs:0, %{r}\n  addl {x}@gotntpoff(%{b}), %{r}\n  pushfl\n  pushl %{r}",
                    setbase(b)
                ),
                &format!("{}\n  movl %gs:0, %ecx\n  movl %eax, %edx\n  subl %ecx, %edx\n  addl %edx, %ecx\n  pushfl\n  popl %edx", tls_want(x)),
            );
        }
    }
}

/// D1–D3 (X86-34), each call relocation the form admits.
/// D1–D3 (X86-34): `d3_bases` are D3's bases.
fn gd_sites(g: &mut Gen, x: &str, d3_bases: &[&str]) {
    for &(call, name) in g.calls {
        g.chk(
            &format!("D1 {x} {name}"),
            &format!("  leal {x}@tlsgd(,%ebx,1), %eax\n  call {call}\n  pushl %eax"),
            &tls_want(x),
        );
        for &b in d3_bases {
            g.chk(
                &format!("D3 {x} base {b} {name} nop"),
                &format!(
                    "{}  leal {x}@tlsgd(%{b}), %eax\n  call {call}\n  nop\n  pushl %eax",
                    setbase(b)
                ),
                &tls_want(x),
            );
        }
    }
    for b in ["ebx", "ecx", "edx", "ebp", "esi", "edi"] {
        g.chk(
            &format!("D2 {x} base {b} GOT32X"),
            &format!(
                "{}  leal {x}@tlsgd(%{b}), %eax\n  call *___tls_get_addr@GOT(%{b})\n  pushl %eax",
                setbase(b)
            ),
            &tls_want(x),
        );
        g.chk(
            &format!("D2 {x} base {b} GOT32"),
            &format!("{}  leal {x}@tlsgd(%{b}), %eax\n{}\n  pushl %eax", setbase(b), call_got32(b)),
            &tls_want(x),
        );
    }
}

/// D3 alone (X86-34), with bases `bases`.
fn d3_sites(g: &mut Gen, x: &str, bases: &[&str]) {
    for &(call, name) in g.calls {
        for &b in bases {
            g.chk(
                &format!("D3 {x} base {b} {name} nop"),
                &format!(
                    "{}  leal {x}@tlsgd(%{b}), %eax\n  call {call}\n  nop\n  pushl %eax",
                    setbase(b)
                ),
                &tls_want(x),
            );
        }
    }
}

/// L1–L2 (X86-35), with `TLS_LDO_32` in code (X86-61). `l1_bases`: the
/// bases of L1.
fn ld_sites(g: &mut Gen, x: &str, l1_bases: &[&str]) {
    let dtp = format!("  leal {x}@dtpoff(%eax), %eax\n  pushl %eax");
    for &b in l1_bases {
        for &(call, name) in g.calls {
            g.chk(
                &format!("L1 {x} base {b} {name}"),
                &format!("{}  leal {x}@tlsldm(%{b}), %eax\n  call {call}\n{dtp}", setbase(b)),
                &tls_want(x),
            );
        }
    }
    if l1_bases.contains(&"ebx") {
        for b in ["ebx", "ecx", "ebp", "esi", "edi"] {
            g.chk(
                &format!("L2 {x} base {b} GOT32X"),
                &format!(
                    "{}  leal {x}@tlsldm(%{b}), %eax\n  call *___tls_get_addr@GOT(%{b})\n{dtp}",
                    setbase(b)
                ),
                &tls_want(x),
            );
            g.chk(
                &format!("L2 {x} base {b} GOT32"),
                &format!("{}  leal {x}@tlsldm(%{b}), %eax\n{}\n{dtp}", setbase(b), call_got32(b)),
                &tls_want(x),
            );
        }
    }
}

/// T1–T2 (X86-37, X86-63): `regs` are T1's destinations.
fn desc_sites(g: &mut Gen, x: &str, regs: &[&str], module_base: bool) {
    for &r in regs {
        let mv = if r == "eax" { String::new() } else { format!("  movl %{r}, %eax\n") };
        g.chk(
            &format!("T1 T2 {x} -> {r}"),
            &format!(
                "  leal {x}@tlsdesc(%ebx), %{r}\n{mv}  call *{x}@tlscall(%eax)\n  addl %gs:0, %eax\n  pushl %eax"
            ),
            &tls_want(x),
        );
        if module_base {
            g.chk(
                &format!("T1 T2 _TLS_MODULE_BASE_ -> {r} + {x}@dtpoff"),
                &format!(
                    "  leal _TLS_MODULE_BASE_@tlsdesc(%ebx), %{r}\n{mv}  call *_TLS_MODULE_BASE_@tlscall(%eax)\n  \
                     addl %gs:0, %eax\n  leal {x}@dtpoff(%eax), %eax\n  pushl %eax"
                ),
                &tls_want(x),
            );
        }
    }
}

/// The sites of `set`, as `(assembly, number of checks)`; `pic` leaves out
/// the no-base GOT forms and the absolute `TLS_IE` forms (static only).
pub fn sites_s(set: Set, pic: bool) -> (String, usize) {
    let mut g = Gen { out: String::new(), names: Vec::new(), calls: BOTH };
    match set {
        Set::Common => {
            if pic {
                g.calls = PLT_ONLY;
            }
            got_sites(&mut g, pic);
            for x in ["tv", "tb"] {
                ie_sites(&mut g, x, !pic);
                gd_sites(&mut g, x, &["ebx"]);
                ld_sites(&mut g, x, &["ebx"]);
                desc_sites(&mut g, x, &["eax"], true);
            }
        }
        Set::Lld => {
            if pic {
                // The `PC32` partners Common leaves out of static-pie.
                g.calls = PC32_ONLY;
                for x in ["tv", "tb"] {
                    gd_sites(&mut g, x, &["ebx"]);
                    ld_sites(&mut g, x, &["ebx"]);
                }
                g.calls = BOTH;
            }
            for x in ["tv", "tb"] {
                ld_sites(&mut g, x, &["esi", "edi", "ecx"]);
                d3_sites(&mut g, x, &["esi", "edi", "ecx"]);
            }
            // X86-23: thread-local `weak0` takes T = 0 in every form.
            ie_sites(&mut g, "tw", !pic);
            gd_sites(&mut g, "tw", &["ebx"]);
            ld_sites(&mut g, "tw", &["ebx"]);
            desc_sites(&mut g, "tw", &["eax"], false);
        }
        Set::Bfd => {
            for x in ["tv", "tb"] {
                desc_sites(&mut g, x, &["ecx", "edx", "ebp", "esi", "edi"], true);
            }
        }
    }
    let k = g.names.len();
    let mut s = String::from(
        "  .text\n  .globl run_checks\n  .type run_checks,@function\nrun_checks:\n  \
         pushl %ebp\n  pushl %ebx\n  pushl %esi\n  pushl %edi\n",
    );
    s.push_str(&g.out);
    let _ = write!(
        s,
        "  movl ${k}, %eax\n  popl %edi\n  popl %esi\n  popl %ebx\n  popl %ebp\n  ret\n  \
         .size run_checks, .-run_checks\n  \
         .globl lfn\n  .hidden lfn\n  .type lfn,@function\nlfn: movl $0x1111, %eax\n  ret\n  \
         .globl absv\n  .set absv, 0x1234\n  .weak wund\n  .weak tw\n  .type tw, @tls_object\n  \
         .data\n  .p2align 2\n  .globl ldata\n  .hidden ldata\nldata: .long 1, 2\n  \
         .section .tdata,\"awT\",@progbits\n  .p2align 2\npad0: .long 1, 2, 3\n  .globl tv\ntv: .long 5\n  \
         .section .tbss,\"awT\",@nobits\n  .p2align 6\n  .zero 8\ntb: .zero 4\n  \
         .bss\n  .p2align 2\n  .globl res\nres: .zero {}\n  \
         .section .data.rel.ro,\"aw\"\n  .p2align 2\n  .globl names\nnames:\n",
        8 * k
    );
    for i in 0..k {
        let _ = writeln!(s, "  .long n{i}");
    }
    // Not `SHF_MERGE`: the true-input pass then places the names linearly.
    s.push_str("  .section .rodata,\"a\"\n");
    for (i, n) in g.names.iter().enumerate() {
        let _ = writeln!(s, "n{i}: .asciz \"{n}\"");
    }
    s.push_str("  .section .note.GNU-stack,\"\",@progbits\n");
    (s, k)
}
