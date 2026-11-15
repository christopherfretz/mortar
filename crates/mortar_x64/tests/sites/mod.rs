//! Self-checking relaxation sites (the review harness's `genrelax.py`,
//! extended): every register × every relaxable form of x64.md §8, plus
//! the GD/LD sequences with every follower type, TLSDESC with
//! `_TLS_MODULE_BASE_`, `DTPOFF32`/`DTPOFF64` in code, and the GOT loads
//! of `abs` and `weak0`. Each site leaves its value in `%r11` and the
//! value an unrelaxed instruction would compute in `%rax`;
//! `relaxmain.c` compares and prints the failures.
#![allow(dead_code)]

use std::fmt::Write as _;

const R64: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];

struct Gen {
    out: String,
    names: Vec<String>,
}

impl Gen {
    fn chk(&mut self, desc: &str, body: &str, expect: &str) {
        let k = self.names.len();
        self.names.push(desc.to_string());
        let _ = write!(
            self.out,
            "# {desc}\n  mov %rsp, saved_rsp(%rip)\n{body}\n  mov saved_rsp(%rip), %rsp\n  \
             mov %r11, res+{}(%rip)\n{expect}\n  mov %rax, res+{}(%rip)\n",
            16 * k,
            16 * k + 8
        );
    }
}

fn mv(src: &str, dst: &str) -> String {
    if src == dst { String::new() } else { format!("  mov %{src}, %{dst}") }
}

const K: u32 = 0x10000;
const FLG: &str = "\n  lahf\n  seto %al\n  movzwl %ax, %eax\n";

/// `relax.s` and its number of checks; `ld22` adds the 22-byte LD form,
/// which lld refuses (§8.4).
pub fn relax_s(ld22: bool) -> (String, usize) {
    let mut g = Gen { out: String::new(), names: Vec::new() };
    for r in R64 {
        g.chk(
            &format!("G1 mov {r}"),
            &format!("  mov gfoo@GOTPCREL(%rip), %{r}\n{}", mv(r, "r11")),
            "  lea gfoo(%rip), %rax",
        );
        for op in ["add", "or", "adc", "sbb", "and", "sub", "xor"] {
            let pre = if matches!(op, "adc" | "sbb") { "  stc" } else { "  clc" };
            g.chk(
                &format!("G5 {op} {r}"),
                &format!(
                    "  mov ${K}, %{r}\n{pre}\n  {op} gfoo@GOTPCREL(%rip), %{r}\n{}{FLG}  xor %rax, %r11",
                    mv(r, "r11")
                ),
                &format!(
                    "  lea gfoo(%rip), %rcx\n  mov ${K}, %rdx\n{pre}\n  {op} %rcx, %rdx\n  mov %rdx, %r8{FLG}  xor %r8, %rax"
                ),
            );
        }
        for op in ["cmp", "test"] {
            g.chk(
                &format!("G5/6 {op} {r}"),
                &format!("  mov ${K}, %{r}\n  {op} gfoo@GOTPCREL(%rip), %{r}{FLG}  mov %rax, %r11"),
                &format!("  lea gfoo(%rip), %rcx\n  mov ${K}, %rdx\n  {op} %rcx, %rdx{FLG}"),
            );
        }
        g.chk(
            &format!("I1 mov {r}"),
            &format!("  movq tlsx@gottpoff(%rip), %{r}\n{}", mv(r, "r11")),
            "  mov $tlsx@tpoff, %rax",
        );
        // D1: `add $T` keeps the flags of the input `add`.
        g.chk(
            &format!("I2 add {r}"),
            &format!(
                "  mov ${K}, %{r}\n  clc\n  addq tlsx@gottpoff(%rip), %{r}\n{}{FLG}  xor %rax, %r11",
                mv(r, "r11")
            ),
            &format!(
                "  mov $tlsx@tpoff, %rcx\n  mov ${K}, %rdx\n  clc\n  add %rcx, %rdx\n  mov %rdx, %r8{FLG}  xor %r8, %rax"
            ),
        );
        if r != "rsp" {
            g.chk(
                &format!("T1 desc {r}"),
                &format!(
                    "  lea tlsx@tlsdesc(%rip), %{r}\n{}\n  call *tlsx@tlscall(%rax)\n  mov %rax, %r11",
                    mv(r, "rax")
                ),
                "  mov $tlsx@tpoff, %rax",
            );
        }
    }
    for r in ["eax", "ecx", "edx", "ebx", "esi", "edi", "ebp"] {
        g.chk(
            &format!("G2 movl {r}"),
            &format!("  movl gfoo@GOTPCREL(%rip), %{r}\n  mov %{r}, %r11d"),
            "  lea gfoo(%rip), %rax\n  mov %eax, %eax",
        );
    }
    g.chk("G3 call", "  call *gfn@GOTPCREL(%rip)\n  mov %rax, %r11", "  mov $77, %rax");
    g.chk(
        "G4 jmp",
        "  lea 1f(%rip), %rax\n  push %rax\n  jmp *gfn@GOTPCREL(%rip)\n1: mov %rax, %r11",
        "  mov $77, %rax",
    );
    // X-22, X-17: `abs` and `weak0` keep the GOT.
    g.chk("abs GOT", "  mov abs1@GOTPCREL(%rip), %r11", "  mov $0x1234, %rax");
    g.chk("weak0 GOT", "  mov wund@GOTPCREL(%rip), %r11", "  xor %eax, %eax");
    g.chk(
        "weak0 cmpq",
        "  xor %r11, %r11\n  cmpq $0, wund@GOTPCREL(%rip)\n  sete %r11b",
        "  mov $1, %eax",
    );
    // X-18: thread-local weak0, T = 0.
    g.chk("I1 weak0", "  movq wtls@gottpoff(%rip), %r11", "  xor %eax, %eax");
    let tp = "  mov %fs:0, %rax\n  lea tlsx@tpoff(%rax), %rax";
    // §8.3: GD16 with each follower, GD16x, GD22 (W4).
    g.chk(
        "GD16 PLT32",
        "  .byte 0x66\n  leaq tlsx@tlsgd(%rip), %rdi\n  .byte 0x66, 0x66\n  rex64 call __tls_get_addr@PLT\n  mov %rax, %r11",
        tp,
    );
    g.chk(
        "GD16 PC32",
        "  .byte 0x66\n  leaq tlsx@tlsgd(%rip), %rdi\n  .byte 0x66, 0x66, 0x48, 0xe8\n9: .long 0\n  .reloc 9b, R_X86_64_PC32, __tls_get_addr-4\n  mov %rax, %r11",
        tp,
    );
    g.chk(
        "GD16x GOTPCRELX",
        "  .byte 0x66\n  leaq tlsx@tlsgd(%rip), %rdi\n  .byte 0x66, 0x48, 0xff, 0x15\n9: .long 0\n  .reloc 9b, R_X86_64_GOTPCRELX, __tls_get_addr-4\n  mov %rax, %r11",
        tp,
    );
    g.chk(
        "GD16x GOTPCREL",
        "  .byte 0x66\n  leaq tlsx@tlsgd(%rip), %rdi\n  .byte 0x66, 0x48, 0xff, 0x15\n9: .long 0\n  .reloc 9b, R_X86_64_GOTPCREL, __tls_get_addr-4\n  mov %rax, %r11",
        tp,
    );
    for reg in ["r15", "rbx"] {
        g.chk(
            &format!("GD22 {reg}"),
            &format!(
                "  leaq tlsx@tlsgd(%rip), %rdi\n  movabsq $__tls_get_addr@pltoff, %rax\n  addq %{reg}, %rax\n  call *%rax\n  mov %rax, %r11"
            ),
            tp,
        );
    }
    // §8.4 with `DTPOFF32`/`DTPOFF64` in code (X-46: `TpRel`).
    g.chk(
        "LD12 PLT32",
        "  leaq tlsx@tlsld(%rip), %rdi\n  call __tls_get_addr@PLT\n  lea tlsx@dtpoff(%rax), %r11",
        tp,
    );
    g.chk(
        "LD12 PC32",
        "  leaq tlsx@tlsld(%rip), %rdi\n  .byte 0xe8\n9: .long 0\n  .reloc 9b, R_X86_64_PC32, __tls_get_addr-4\n  lea tlsx@dtpoff(%rax), %r11",
        tp,
    );
    g.chk(
        "LD13 GOTPCRELX",
        "  leaq tlsx@tlsld(%rip), %rdi\n  call *__tls_get_addr@GOTPCREL(%rip)\n  lea tlsx@dtpoff(%rax), %r11",
        tp,
    );
    g.chk(
        "LD13 GOTPCREL",
        "  leaq tlsx@tlsld(%rip), %rdi\n  .byte 0xff, 0x15\n9: .long 0\n  .reloc 9b, R_X86_64_GOTPCREL, __tls_get_addr-4\n  lea tlsx@dtpoff(%rax), %r11",
        tp,
    );
    if ld22 {
        // lld refuses LD22 (§8.4); GNU ld asserts on a `DTPOFF64` in code.
        g.chk(
            "LD22",
            "  leaq tlsx@tlsld(%rip), %rdi\n  movabsq $__tls_get_addr@pltoff, %rax\n  addq %r15, %rax\n  call *%rax\n  lea tlsx@dtpoff(%rax), %r11",
            tp,
        );
    } else {
        g.chk(
            "LD12 DTPOFF64",
            "  leaq tlsx@tlsld(%rip), %rdi\n  call __tls_get_addr@PLT\n  movabsq $tlsx@dtpoff, %r11\n  add %rax, %r11",
            tp,
        );
    }
    // X-47: `_TLS_MODULE_BASE_` (gnu2 local-dynamic).
    g.chk(
        "T1 _TLS_MODULE_BASE_",
        "  lea _TLS_MODULE_BASE_@tlsdesc(%rip), %rax\n  call *_TLS_MODULE_BASE_@tlscall(%rax)\n  add %fs:0, %rax\n  lea tlsx@dtpoff(%rax), %r11",
        tp,
    );
    let k = g.names.len();
    let mut s = String::from(
        "  .text\n  .globl site_base\nsite_base:\n  .globl run_checks\n  .type run_checks,@function\nrun_checks:\n  push %rbx; push %rbp; push %r12; push %r13; push %r14; push %r15\n",
    );
    s.push_str(&g.out);
    let _ = write!(
        s,
        "  pop %r15; pop %r14; pop %r13; pop %r12; pop %rbp; pop %rbx\n  mov ${k}, %eax\n  ret\n\
         \x20 .type gfn,@function\ngfn: mov $77, %eax\n  ret\n\
         \x20 .globl abs1\n  .set abs1, 0x1234\n  .weak wund\n  .weak wtls\n  .type wtls, @tls_object\n\
         \x20 .data\n  .globl gfoo\ngfoo: .quad 0x1234\n\
         \x20 .section .tbss,\"awT\",@nobits\n  .p2align 4\npad: .zero 40\ntlsx: .zero 8\n\
         \x20 .bss\nsaved_rsp: .quad 0\n  .globl res\nres: .zero {}\n\
         \x20 .section .data.rel.ro,\"aw\"\n  .globl names\nnames:\n",
        16 * k
    );
    for i in 0..k {
        let _ = writeln!(s, "  .quad n{i}");
    }
    s.push_str("  .section .rodata\n");
    for (i, n) in g.names.iter().enumerate() {
        let _ = writeln!(s, "n{i}: .asciz \"{n}\"");
    }
    s.push_str("  .section .note.GNU-stack,\"\",@progbits\n");
    (s, k)
}
