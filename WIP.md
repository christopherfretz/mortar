# WIP: WP-029 stage 1 (`spec/x64.md`), paused for host migration

State: **`spec/x64.md` is not written yet.** Every experiment is done; the
results are below. Scripts, inputs and a re-run driver are in
`/Volumes/AgentWork/mortar-helper/.build/review-x64/` (not committed, by the
brief). `/tmp/mortar-wp029-88016` has been removed, so re-run what you need
(see "Re-running"). Delete this file before the branch is handed back.

## Take-over session 3 (2026-11-15): writing `spec/x64.md`

- `spec/x64.md` exists (skeleton committed; sections filled one by one,
  see the commit log). The full run `run-all.sh /tmp/mortar-x64-40671` has
  finished; its log is `.build/review-x64/run-all.log`. Two script bugs
  fixed in `.build/review-x64/run-all.sh`: `corpus.sh` is bash (E1 had
  compiled nothing) and E12's PC16 probe now fixes `.data` with
  `-Tdata=0x500000`. E1 is being re-run alone by `run-e1.sh` (log
  `run-e1.log`) into the same work dir.
- Already found wrong in the notes below: "No library carries a note" —
  libgcc.a, libgcov.a, libgfortran.a, libstdc++.a, libatomic/gomp/ssp and
  9 crt files carry `FEATURE_1_AND = 3` (E1 props); libc.a does not.
- **Next step:** fill x64.md sections in order, check each against
  run-all.log; then E1 census table from run-e1.log; then requests, open
  questions, Tests, README row; delete this file in the final commit.

## Take-over session 2 (2026-11-15), paused for a second host restart

- Read the brief, this file, a64.md, arch.md §2-§7 and Appendix A, and the
  synth/image/layout/diag/cli rules the spec cites. **No spec text written
  yet.**
- Tool versions re-checked on the new (aarch64) host: all identical to the
  table below (GCC/gfortran 14.2.0-19, binutils 2.44, lld 19.1.7/22.1.8,
  mold 2.37.1, qemu 10.0.13, glibc 2.41-11cross1, clang 19.1.7/22.1.8,
  gdb 16.3, lldb 19.1.7; hod 7e30d7c8, binary is native aarch64).
- `.build/review-x64/run-all.sh` gained **E12** (re-checks of every
  wrong-code claim: bfd narrow `16`/`8` and `PC16` values, mold's IBT
  IPLT without `endbr64` (`asm/iplt-ibt.s`), bfd's IFUNC `.got.plt` initial
  words vs S-69). The first full run was killed before finishing; no
  results were recorded. The briefs/ copy is unchanged.
- Found so far: request 5 (the "relocation truncated to fit" message)
  belongs to `spec/diag.md` (`relocation-out-of-range`, its message row),
  not arch.md §7, which does not define the text.
- **Next step:** `sh .build/review-x64/run-all.sh /tmp/mortar-x64-<pid>` in
  the background (log outside /tmp), check E2/E3/E7/E8/E12 against the
  notes below, then write `spec/x64.md` per "Next steps".

## Next steps (in order)

1. Write `spec/x64.md` in `spec/a64.md`'s structure: 1 scope, 2 census,
   3 terms and fields, 4 targets, 5 demand classes, 6 the table, 7 undefined
   weak, 8 relaxations (exact bytes, windows, `sequence_extent`,
   `max_reach`), 9 range, 10 shapes (GOT, `.got.plt`, IPLT plain and IBT,
   IRELATIVE, `-z` options), 11 properties, 12 TLS, 13 other sections and
   facts, 14 where the tools differ (with the wrong-code list), 15 the
   differential method for stage 2, 16 requests (numbered, with exact
   text), 17 open questions (each with a recommendation), 18 tests, and an
   appendix of commands. Number the rules `X-1`, `X-2`, … and tag them [M1]
   or [M2]. Make every claim measured (cite the experiment Ex below) or
   cited (psABI, the TLS ABI, Oliva's TLSDESC paper, binutils).
2. Change the `spec/README.md` row `x64.md, x86.md` to: `x64.md` "draft
   (WP-029 stage 1; awaiting review)" and `x86.md` "planned (WP-030)".
3. Commit in the worktree (do not push or merge, and do not edit the root
   `Cargo.toml` or `docs/BACKLOG.md`). Run `cargo xtask ci --quick` only if
   you touch something outside `spec/`.
4. Hand back with: the head hash; the spec's size and section list; the
   census table; where the references disagree and the choices; every
   request (id, target, one line); the open questions with their
   recommendations; what could not be verified; where the scripts are and
   how to re-run them; confirmation that the `/tmp` directory is removed.

## Re-running

`sh /Volumes/AgentWork/mortar-helper/.build/review-x64/run-all.sh /tmp/mortar-x64-$$`
runs E1 to E11. The workspace mount is noexec, so it builds and runs
everything under the given `/tmp` directory; remove that directory by
explicit path afterwards. Tools:

- `link4.sh` links with bfd, lld 19, lld 22 and mold, as static and as
  static-pie.
- `cmpdis.py` compares disassembly per label.
- `mkprop.sh` writes a property-note object.
- `typeprobe.py` gives a per-type verdict.
- `runprogs.sh` runs the qemu programs.
- `corpus.sh`, `x64scan.py` and `patsum.py` build and scan the census.

Inputs are in `asm/` (`relax.s`, `apx-*.s`, `iplt.s`, `iplt-ibt.s`, `weak.s`,
`range*.s`, `size-pc16*.s`, `fill.s`) and `src/`.

Versions:

| tool | version |
|---|---|
| GCC, gfortran | 14.2.0 (Debian 14.2.0-19) |
| GNU ld, as | 2.44 |
| LLD | 19.1.7 and 22.1.8 |
| mold | 2.37.1 |
| qemu-x86_64 | 10.0.13 |
| glibc | 2.41 (Debian, x86_64 cross) |
| hod | 7e30d7c8 |

## Census (E1)

The corpus is 522 compile jobs, of which 498 objects succeeded:

- GCC, Clang 19 and Clang 22: 135 objects each (a.c, d.c, b.cc). The flag
  sets are {-fno-pic, -fPIE, -fPIC} × 15 option sets: -O0; -O2;
  -g -ffunction-sections -fdata-sections; -mcmodel=medium; -mcmodel=large;
  -fno-plt; -mtls-dialect=gnu2; each -ftls-model; -fcf-protection=full;
  -Wa,-mrelax-relocations=no; -mapxf; large+gnu2; medium+-fno-plt.
- gfortran: 45 objects.
- hod: 48 of 72. It was given static/pic × small/medium/large × tlsdesc
  on/off × O0/O2, and all 24 medium-model jobs fail with hod's own
  "unsupported construct 'x64:code-model'".

| type | gcc | clang-19 | clang-22 | gfortran | hod | crt | libc.a | libc_nonshared.a | libm.a | libpthread+rt+dl+util+resolv | libstdc++.a | libgcc.a | libgcov.a | libgfortran.a | libatomic+gomp+ssp |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 `64` | 4993 | 1716 | 1772 | 121 | 1284 | 28 | 1536 | - | - | 133 | 5436 | 5 | - | 177 | 96 |
| 2 `PC32` | 3029 | 2156 | 2156 | 440 | 1102 | 31 | 18956 | 7 | 14528 | 367 | 18616 | 2223 | 131 | 17220 | 3958 |
| 4 `PLT32` | 2837 | 3034 | 3016 | 268 | 1492 | 7 | 12006 | 4 | 6833 | 215 | 26735 | 1864 | 190 | 16308 | 2712 |
| 9 `GOTPCREL` | 47 | 80 | 78 | 3 | - | 1 | 83 | - | - | - | 6 | 5 | - | - | - |
| 10 `32` | 12802 | 4843 | 5302 | 205 | - | 72 | - | - | - | - | - | - | - | - | - |
| 11 `32S` | 87 | 85 | 85 | 39 | - | 4 | - | - | - | - | - | - | - | - | - |
| 17 `DTPOFF64` | - | 24 | 24 | - | - | - | - | - | - | - | - | - | - | - | - |
| 19 `TLSGD` | 130 | 113 | 113 | - | 46 | - | - | - | - | - | 1 | 898 | 4 | - | - |
| 20 `TLSLD` | 41 | 42 | 42 | - | 18 | - | - | - | - | - | 2 | 3 | - | - | - |
| 21 `DTPOFF32` | 100 | 79 | 79 | - | 36 | - | - | - | - | - | 2 | 5 | - | - | - |
| 22 `GOTTPOFF` | 136 | 125 | 125 | - | 48 | - | 1732 | - | 365 | 25 | - | - | - | 1 | - |
| 23 `TPOFF32` | 813 | 650 | 650 | - | 144 | - | 36 | - | - | - | - | - | - | 1 | 701 |
| 24 `PC64` | 60 | 158 | 158 | - | 840 | - | - | - | - | - | - | - | - | - | - |
| 25 `GOTOFF64` | 110 | 120 | 120 | 32 | 126 | - | - | - | - | - | - | - | - | - | - |
| 27 `GOT64` | 52 | 252 | 252 | 6 | 758 | - | - | - | - | - | - | - | - | - | - |
| 29 `GOTPC64` | 45 | 56 | 56 | 12 | 884 | - | - | - | - | - | - | - | - | - | - |
| 31 `PLTOFF64` | 188 | - | - | 30 | - | - | - | - | - | - | - | - | - | - | - |
| 34 `GOTPC32_TLSDESC` | 22 | 28 | 28 | - | 64 | - | - | - | - | - | - | - | - | - | - |
| 35 `TLSDESC_CALL` | 28 | 28 | 28 | - | 64 | - | - | - | - | - | - | - | - | - | - |
| 41 `GOTPCRELX` | 338 | 268 | 286 | 44 | - | 5 | - | - | - | - | - | - | - | - | - |
| 42 `REX_GOTPCRELX` | 242 | 342 | 344 | 39 | 78 | 11 | 70 | - | 456 | - | 6131 | 1667 | 53 | 18 | 6 |
| objects | 135 | 135 | 135 | 45 | 48 | 17 | 2215 | 4 | 1402 | 21 | 217 | 304 | 28 | 919 | 147 |

What else the census found:

- 21 types appear, and none is a CODE_* type: `-mapxf` emits none.
- `SHT_REL` count is 0.
- The follower of a sequence is always the next record
  (`seq_follower_not_next` = 0).
- GCC and the libraries have unsorted relocation sections (84 of GCC's
  objects and 245 of libc.a's, for example).
- `.eh_frame` section type:
  - `SHT_X86_64_UNWIND` (0x70000001) in every Clang and hod object;
  - PROGBITS for GCC, gfortran and all the libraries.
- FDE encoding:
  - `0x1b` with PC32 everywhere;
  - also `0x1c` with PC64 in Clang (126) and hod (732), from the large model.
- Personality encoding:
  - `0x9b` with PC32 in every producer and library;
  - `0x9c` with PC64 in GCC, Clang and hod;
  - `0x03` with `32` (13) and `0x00` with `64` (2) in GCC and Clang.
- CIE augmentations: zR, zPLR, and one zRS in libc.a.
- `.l*` / `SHF_X86_64_LARGE` sections:
  - Clang 19: 148; Clang 22: 168; hod: 770, all from the large model;
  - GCC: 0; the libraries: 0.
- Property notes appear only in the `-fcf-protection` jobs (0xc0000002=3,
  9 objects per C compiler). No library carries a note.

Byte patterns:

- `GOTTPOFF`: only `REX(48|4c) 8b` and `48 03`, with modrm `00 reg 101`.
  libc.a has 1661 `48 8b 05`.
- `REX_GOTPCRELX`: `48|4c 8b` (mov), `48|4c 03` (add) and `48|4c 3b` (cmp)
  only.
- `GOTPCRELX`: only `ff 15` (call) and `ff 25` (jmp).
- Plain `GOTPCREL`: `48 83 3d` (cmpq $0, A = −5; glibc's weak checks), the
  `-mrelax-relocations=no` movs, and `ff 35` / `ff 16` / `ff 7e`.
- `GOTPC32_TLSDESC`: `48 8d 05`, and in GCC also `48 8d 2d` and `48 8d 1d`.
  `TLSDESC_CALL` is always `ff 10` at P.
- `TLSGD`:
  - 16-byte form `66 48 8d 3d` with `66 66 48 e8` + PLT32 at P+8 (1010+);
  - 16-byte form with `66 48 ff 15` + GOTPCRELX at P+8 (24+);
  - 22-byte form, GCC `-fPIC -mcmodel=large` only:
    `48 8d 3d [P] 48 b8 [PLTOFF64 @P+6] 48 01 d8 ff d0`.
- `TLSLD`:
  - 12-byte `48 8d 3d [P] e8` + PLT32 at P+5;
  - 13-byte `… ff 15` + GOTPCRELX at P+6;
  - 22-byte form with PLTOFF64 at P+6.
- `DTPOFF32`: 277 records in exec sections, 24 in non-alloc. `DTPOFF64`:
  non-alloc only.
- IFUNC symbols:
  - in the corpus: 45 per C compiler, 16 in hod;
  - in the libraries: libc.a 76, libm.a 286, atomic+gomp+ssp 17.

## Decisions taken (write them into the spec)

### Statuses

- **M1:** 0, 1, 2, 4, 9, 10, 11, 12, 13, 14, 15, 17, 18, 19, 20, 21, 22, 23,
  24, 25, 26, 27, 29, 31, 32, 33, 34, 35, 41, 42.
- **Later:** 3 `GOT32`, 28 `GOTPCREL64`, 30 `GOTPLT64`, and 43–51 (APX
  `CODE_4/5/6_*`). The open question is to accept the GOTPCRELX and
  GOTTPOFF variants unrelaxed later.
- **Refused:**
  - the dynamic-only types in an input: 5, 6, 7, 8, 16, 36, 37, 38;
  - the MPX types 39 and 40;
  - unknown types, ≥ 52.

### Values and checks

- `GOT` is the start of `.got.plt` (`_GLOBAL_OFFSET_TABLE_`).
- TpRel(x) = x − tls.start − align_up(tls.size, tls.align) (variant II).
- DtpRel(x) = x − tls.start.

Checks, with lld-style ranges:

| type | check |
|---|---|
| PC32, PLT32, 32S, GOTPCREL*, GOTPC32 | s(32) |
| 32 | u(32) |
| 16 | su(16) = [−32768, 65535], as lld |
| 8 | su(8) |
| PC16 | s(16) |
| PC8 | s(8) |
| SIZE32 | u(32) (bfd and mold refuse −1; lld does not check) |
| 64-bit types | none |

### Classes (arch.md names)

| class | types |
|---|---|
| AbsWord | 1 |
| AbsNarrow | 10, 11, 12, 14 |
| Pc | 2, 13, 15, 24 |
| Branch | 4; also 31 `PLTOFF64` = L + A − GOT, Branch with `Minus::GotBase` |
| Got | 9, 41, 42, 27 (GOT64 = G(Addr) − GOT + A) |
| GotRel | 25 (S + A − GOT) |
| GotBaseRef | 26 and 29 (GOT + A − P) |
| Size | 32, 33 |
| TlsGd | 19 |
| TlsLd | 20 |
| TlsDtpRel | 17, 21 |
| TlsIe | 22 |
| TlsLe | 23, 18 |
| TlsDesc | 34, 35 |

- In alloc sections of an exec output, DTPOFF32 and DTPOFF64 are
  TpRel(S + A), as bfd and lld do. In non-alloc sections they are
  DtpRel(S + A), and all three references agree.

### GOTPCRELX family (relaxation; kind 2, so the GOT slot stays demanded)

The conditions for every form below:

- the target is local and defined in a live section: not ABS, not weak0,
  not ifunc;
- A = −4;
- the new field fits s(32).

The forms (REX' moves REX.R into REX.B):

| input | output | X | where |
|---|---|---|---|
| `REX_GOTPCRELX` `48\|4c 8b modrm(00 reg 101)` | `8d` at P−2 (lea) | S + A − P | static and static-pie |
| `GOTPCRELX` `ff 15` | `67 e8`, rel32 stays at P | S + A − P | |
| `GOTPCRELX` `ff 25` | `e9 <rel32 at P−1> 90` | S + A − P + 1 | |
| `REX_GOTPCRELX` binop, op ∈ {03,0b,13,1b,23,2b,33,3b} | `REX' 81 (c0\|op&0x38\|reg) imm32` | imm = S | static non-PIE only, S in u(31) |
| `REX_GOTPCRELX` test `85` | `REX' f7 (c0\|reg)` | imm = S | static non-PIE only, S in u(31) |

- Plain `GOTPCREL` is never relaxed.
- Any other opcode keeps the GOT; it is never refused.
- Windows: before 3 (the binops), before 2 (call and jmp).

### GOTTPOFF (kind 1, decided from the bytes at scan)

- The input must be `48|4c 8b` or `48|4c 03` with modrm `00 reg 101` and
  A = −4.
- mov → `REX' c7 (c0|reg) T`; add → `REX' 81 (c0|reg) T` (arch A.5's form;
  bfd and lld use lea for the add).
- T = TpRel(S), check s(32). For weak0, T = 0.
- Any other instruction keeps IE (`GOT_TPREL` slot = TpRel(S)). This is
  mold's behavior. It is never refused.
- **Correction of an earlier note:** mold does NOT write wrong code on an
  odd GOTTPOFF. The earlier test placed the `.reloc` one byte too early.
  With it fixed (`asm/relax.s` ODD, now at `.+4`), mold keeps the GOT
  correctly. bfd refuses ("TLS transition from R_X86_64_GOTTPOFF to
  R_X86_64_TPOFF32 against `ie' … failed"), and so does lld
  ("R_X86_64_GOTTPOFF must be used in MOVQ or ADDQ instructions only",
  also with `--no-relax`).

### TLSGD and TLSLD → LE (kind 1; no IE in M1, because static has no TLS-dynamic target)

- `seq_leads` = [19, 20], look-back 1.
- `sequence_extent` = 2 when the next record is 4, 41 or 31 at
  r_offset + 8, +5 or +6; otherwise 1.
- The follower (symbol `__tls_get_addr`) is consumed: no PLT or GOT demand,
  and no undefined-symbol error.

| form | input | LE output | window |
|---|---|---|---|
| GD16 | `66 48 8d 3d [P] 66 66 48 e8 [PLT32@P+8]` or `… 66 48 ff 15 [GOTPCRELX@P+8]` | `64 48 8b 04 25 00 00 00 00 48 8d 80 <T>` over [P−4, P+12), T = TpRel(S) at P+8 | (4, 8) |
| GD22 | `48 8d 3d [P] 48 b8 [PLTOFF64@P+6] (48\|4c) 01 (c0\|reg<<3) ff d0` | `64 48 8b 04 25 00 00 00 00 48 8d 80 <T> 66 0f 1f 44 00 00` over [P−3, P+19) (bfd's) | (3, 15) |
| LD12 | `48 8d 3d [P] e8 [PLT32@P+5]` | `66 66 66 64 48 8b 04 25 00 00 00 00` over [P−3, P+9) | (3, 5) |
| LD13 | `48 8d 3d [P] ff 15 [GOTPCRELX@P+6]` | `66 66 66 66 64 48 8b 04 25 00 00 00 00` over [P−3, P+10) | (3, 6) |
| LD22 | as GD22 but TLSLD | bfd's `66 66 66 66 2e 0f 1f 84 00 00 00 00 00 64 48 8b 04 25 00 00 00 00` over [P−3, P+19) | (3, 15) |

- The other windows:
  - call and jmp: before 2;
  - mov, binop, GOTTPOFF and the TLSDESC lea: before 3;
  - TLSDESC_CALL: after 2.
- **`max_reach` = (4, 15).** image.md I-111 says (4, 8); a request is needed.
- A GD or LD that does not match is refused by name ("unrecognized
  general-dynamic TLS sequence" / "… local-dynamic …"). Static libc.a has
  no `__tls_get_addr`; only `_dl_tlsdesc_return` and
  `_dl_tls_get_addr_soft` are there. In M2, an unmatched GD is kept with a
  GOT pair.
- This answers image.md R-14: the 22-byte forms are added.

### TLSDESC (each member independently)

- The lea is `48|4c 8d modrm(00 reg 101)`; it becomes `REX' c7 (c0|reg) T`.
  `ff 10` at P becomes `66 90`. All three references write this.
- T = TpRel(S). For `_TLS_MODULE_BASE_`, T = 0, and its DTPOFF32 fields
  are TpRel. This matches bfd and lld; mold writes TpRel(tls.start) with
  DTP-relative offsets, and both run. The linker defines
  `_TLS_MODULE_BASE_` at tls.start (hidden).
- Both GCC and Clang use `_TLS_MODULE_BASE_` with
  `-mtls-dialect=gnu2 -ftls-model=local-dynamic`. This answers arch.md Q7.
- Mismatched bytes are refused (static has no TLSDESC runtime).
- IE forms are M2.

### Undefined weak (E9; `weak.s`)

In static non-PIE, all three references use S = 0:

- call, jmp and lea go to 0;
- `.long wd-.` = −P;
- `32` and `64` are 0;
- GOTOFF64 and PLTOFF64 are −GOT;
- SIZE is 0;
- the GOT slot is 0 (bfd relaxes it to `mov $0`).

Mortar's rules:

- S = 0 everywhere.
- The Pc class against weak0 at an alloc place in pie is **E**. mold does
  the same ("can not be used; recompile with -fPIC"). lld 22 writes the
  link-time 0, so at run time the value is the image base: wrong code
  (seen in `w-lld22-spie`: `lea -0x127b(%rip)`). mold linked every glibc
  static-pie program, so the rule does not fire on real code.
- The Branch class keeps S = 0 in pie: libc.a's calls to weak symbols are
  guarded through a GOT slot that holds 0.
- AbsNarrow and AbsWord against weak0 in pie give 0 with no relocation
  (lld). bfd refuses `32` in PIE.
- GotRel against weak0 is E (arch.md §4.3).
- Thread-local weak0:
  - LE: X = A (lld's rule);
  - IE slot: 0;
  - relaxed forms: T = 0.
  - glibc's `_nl_current_LC_*` is accessed only after
    `cmpq $0, _nl_current_LC_X_used@GOTPCREL(%rip)` (A = −5), which keeps
    a GOT slot holding 0.

### Shapes (E3)

**Plain IPLT** (bfd's):

- 8-byte entry `ff 25 <rel32> 66 90`, where rel32 = word − (entry + 6).

**IBT IPLT** (bfd with `-z ibtplt` or `-z ibt`, lld with `-z force-ibt`):

- 16-byte entry `f3 0f 1e fa ff 25 <rel32> 66 0f 1f 44 00 00`, where
  rel32 = word − (entry + 10).
- Used when the merged IBT bit is set (`-z ibt` forces it) or with
  `-z ibtplt`.
- `.plt` is aligned to 16 (mortar's choice).

**`.got.plt` and IRELATIVE** (static):

- `.got.plt` has 3 reserved zero words, then one word per IFUNC, written 0.
- IRELATIVE goes in `.rela.plt`, with r_offset = the word and addend = the
  resolver.
- `__rela_iplt_*` bracket the IRELATIVE entries.
- Under static-pie, S-70 applies: `.rela.dyn`, and the symbols are not
  defined.

**GOT slots:**

| slot | static | static-pie |
|---|---|---|
| GDAT, local | S | S + RELATIVE |
| GDAT, abs | S | S |
| GDAT, ifunc | the IPLT entry | the IPLT entry (+ RELATIVE in pie) |
| GDAT, weak0 | 0 | 0 |
| GTPREL | TpRel(S) | TpRel(S) |

**What the references write:**

| linker | IPLT section and entry | IFUNC words | relocation section | notes |
|---|---|---|---|---|
| bfd | `.plt`, 8-byte entries, 8-aligned | `.got.plt`, initially **entry+6** (E3: 0x401006, 0x40100e) | `.rela.plt` | |
| lld | `.iplt`, 16-byte `ff 25; 68 n; e9` | 0 | `.rela.dyn` | |
| mold | `.plt.got`, 8-byte `ff 25 … cc cc` | in `.got` | | `.got.plt` has only 3 words |

- bfd's entry+6 contradicts synth.md S-69 ("GNU writes … 0 on x86-64"):
  correction request.
- Other observations:
  - bfd static-pie has 27 IRELATIVE + 1 NONE in `.rela.plt`;
  - mold static-pie has 2 NONE and defines `__rela_iplt_*` as absolute 0;
  - bfd sets `EI_OSABI` = GNU with IFUNC (mortar writes 0);
  - `e_flags` is 0 everywhere.
- **mold wrong code (confirmed again with every input IBT-marked,
  `asm/iplt-ibt.s`):** the output is marked IBT, but the IPLT entries have
  no `endbr64`. bfd and lld write `endbr64`.
- lld does not know `-z ibt` or `-z ibtplt` (it warns "unknown -z value").
- All three write `PT_GNU_PROPERTY` when there is a note.

### Property merge (E4)

| property | rule |
|---|---|
| FEATURE_1_AND (0xc0000002) | And |
| ISA_1_NEEDED, 0xc0008000..**0xc000ffff**, FEATURE_2_NEEDED (0xc0008001) | Or (bfd and mold) |
| ISA_1_USED, FEATURE_2_USED (0xc0010001), 0xc0010000..0xc0017fff | OrAll |
| unknown 0xc0000001 | bfd keeps it; mold drops it; mortar drops it with `unknown-property` |

- lld writes only FEATURE_1_AND.
- synth.md S-95's Or range 0xc0008000..0xc0008fff is **wrong**: request
  to change it to ..0xc000ffff (binutils `GNU_PROPERTY_X86_UINT32_OR_HI`).
- `-z cet-report=error|warning` messages:
  - bfd: "an.o: error: missing IBT and SHSTK properties";
  - lld: "an.o: -z cet-report: file does not have
    GNU_PROPERTY_X86_FEATURE_1_IBT property";
  - mold: "an.o: -cet-report=error: missing GNU_PROPERTY_X86_FEATURE_1_IBT".
- `-z ibt` and `-z shstk` force the bits in bfd and mold.

### Range (E7)

There are no thunks; the reach is ±2 GiB. The messages:

- bfd: "(.text+0x3): relocation truncated to fit: R_X86_64_PC32 against
  symbol `far' defined in *ABS* section in ob".
- lld: "relocation R_X86_64_PC32 out of range: 4297060001 is not in
  [-2147483648, 2147483647]; references 'far'".
- mold: "… is not in [-2147483648, 2147483648)".

Request: arch.md §7's message becomes
`relocation truncated to fit: {r_type} against '{name}': {value} is not in [{min}, {max}]`.
The brief requires the phrase.

**New bfd wrong-data finding** (`asm/range-bitfield.s`, `asm/range-pc16.s`):

- bfd accepts `R_X86_64_16` = −32769 and writes 0x7fff, and accepts
  `R_X86_64_8` = −129 and writes 0x7f. Its bitfield check allows
  [−65536, 65535]: −65537 is refused.
- bfd accepts `PC16` = +32768 and writes 0x8000, which reads as −32768.
- lld refuses all of these. mold refuses PC16; its `--defsym` cannot take
  negative values.

### Facts (E10, E11)

- Header: `EM_X86_64` (62), ELFCLASS64. x32 is refused already (elf.md
  §3.1 item 7). `e_flags` is 0, and `EI_OSABI` is 0.
- Base and pages:
  - base 0x400000 for mortar (bfd uses 0x400000; lld and mold use 0x200000);
  - max and common page are 4 KiB (p_align is 0x1000 for all three).
- Segments:
  - bfd uses separate-code (R/RX/R/RW); lld and mold do not;
  - layout L-71 makes noseparate-code the default.
- Fill: mortar uses `code_fill` 0xCC and `code_nop` 0x90. bfd fills with
  multi-byte NOPs; lld and mold use 0xCC.
- Relocation sections: RELA only, and `SHT_REL` is refused. x86 has no
  mapping symbols, so nothing without a relocation is ever decoded. The
  patterns read only bytes inside their record's window.
- Large sections: `SHF_X86_64_LARGE` stays refused (L-36). Large-model
  relocation *types* are M1, because GCC's large model uses `.text`.
- Tombstones (`tomb.c`, `--gc-sections`, DWARF 4 and 5):
  - all three write 0 in low_pc and aranges;
  - llvm-dwarfdump-22 `--verify` reports 3 errors for DWARF 4 and 0 for
    DWARF 5;
  - lld 22 with `-z dead-reloc-in-nonalloc=<.debug_info|line|rnglists|loclists|addr|frame>=0xffffffffffffffff`
    (I-47's all-ones) gives 0 errors for both, with the same gdb-multiarch
    output (`info line live`, `break live`; `dead` is undefined);
  - lldb-19 showed no "unable to resolve" for any of the eight;
  - the low layout was not tested.

### Runs under qemu (E5, E6)

- TLS layout:
  - alignments 16, 64 and 4096, with and without `.tdata`, under all four
    linkers, static and static-pie, give identical TP offsets;
  - for example, A = 4096 and memsz 0x20a8 gives d1 = −0x3000.
- Programs with bfd, lld 19, lld 22 and mold, static and static-pie, OK:
  - `p.c` (memcpy IFUNC, setlocale, pthread, TLS including 64-aligned, a
    local IFUNC);
  - `q.cc` (exceptions, `thread_local` string, threads);
  - `f.f90`.
- Variants: gnu2 [LD] and `-fno-plt` [LD] are OK for GCC and Clang 22.
- `-fPIC -mcmodel=large -DNOIFUNC`:
  - bfd and mold OK;
  - lld refuses the LD form ("expected R_X86_64_PLT32 or
    R_X86_64_GOTPCRELX after R_X86_64_TLSLD");
  - **lld 19 and 22 corrupt the 22-byte GD**: `gd.c` segfaults, static
    and static-pie, while bfd and mold print "GD ok 4 7". lld writes 16
    bytes from P−4, so the fs prefix is lost and `add`/`call *%rax`
    remain.
- bfd refuses PLTOFF64 and GOT64 against an IFUNC (clang large model with
  libc strlen); lld and mold accept them. mortar accepts them, with S = the
  IPLT entry.
- bfd asserts at `elf64-x86-64.c:4534` on DTPOFF64 in `.text`.

### Per-type verdicts (E8, `typeprobe.py`, in `.data`)

- All three accept 0, 13, 15, 18, 26, 32 and 33.
- 3 `GOT32`: bfd and lld write G − GOT + A, signed; mold refuses its own
  negative value.
- 28 `GOTPCREL64`: bfd and mold accept; lld refuses.
- 30 `GOTPLT64`: only bfd accepts.
- **bfd silently accepts** 5–8, 16 and 36–38, writing S + A and no dynamic
  relocation. That is wrong code for DTPMOD64 and IRELATIVE. bfd treats 39
  and 40 as PC32 and PLT32. lld and mold refuse all of these.
- Types ≥ 52 are refused by all three.

### APX (E2, later)

- GNU as 2.44 emits:
  - `CODE_4_GOTPCRELX` for REX2 mov/add GOTPCREL;
  - `CODE_4_GOTTPOFF` and `CODE_4_GOTPC32_TLSDESC`;
  - `CODE_6_GOTTPOFF` for NDD add;
  - plain GOTPCREL for the NDD and NF forms.
- lld 19 refuses 43, 44 and 50.
- bfd and lld 22 relax them (`d5 18 c7 c0`, `62 f4 e4 18 81 c0`, …).
- mold relaxes only the movs, to lea.

### Stage-2 differential method

- lld `--emit-relocs` keeps the original types; bfd converts relaxed
  records to 32/32S/PC32.
- Steps:
  1. Link the corpus and the glibc programs with lld 22 `--emit-relocs`,
     with and without `--no-relax`.
  2. Re-apply each record in lld's layout and compare, with a list of
     known differences:
     - GOTTPOFF add: mortar writes add, lld writes lea;
     - the GOTPCRELX mov in non-PIE: mortar writes lea, bfd writes
       `mov $imm`;
     - mold's `40 e8`.
  3. For GNU, classify the converted types.
  4. Run mortar's outputs under qemu.

## Requests to write in §16 (with exact text in the spec)

1. image.md I-111: the x86-64 bullet becomes (4, 15); R-14 is answered,
   since the 22-byte GD/LD forms are added.
2. arch.md §3.1:
   - "x86-64 `(4, 8)` for the A.3 forms" becomes "(4, 15), spec/x64.md §8";
   - x86-64 `seq_leads` = [19, 20].
3. arch.md A.2: precise wording. The jmp's rel32 moves one byte left; the
   call's rel32 stays at P. Also: mov → lea in every exec output; mov
   `$imm` is not taken; binops only in static non-PIE.
4. arch.md A.3/A.4: add GD22 and LD22. Q7 is answered.
5. arch.md §7: the "relocation truncated to fit" message (all
   architectures).
6. arch.md §4.3: the Pc row in Pie against Weak0 becomes "— (x86-64: `E`,
   spec/x64.md §7)".
7. synth.md S-95: the Or range becomes 0xc0008000..0xc000ffff.
8. synth.md S-69: GNU writes the entry's address + 6 on x86-64, not 0.
9. ROADMAP, synth S-70, layout, synth `[M3]` x86 tags: static-pie and the
   x86-64 static rules are M1 for x86-64, per this WP's brief. ROADMAP
   lists `-static-pie` in M4. Also write this as an open question with a
   recommendation: mortar_x64 implements and tests the static-pie rows at
   arch level, and the end-to-end static-pie gate waits for the generic
   `.dynamic`/RELATIVE machinery.
10. cli.md: `-z ibt`, `-z shstk`, `-z ibtplt` and `-z cet-report=` become
    M1 on x86-64. lld's `-z force-ibt` spelling: accept as an alias or
    refuse (open question).
11. diag.md: a new kind `cet-report` (a warning or error) with the text
    `{path}: missing {IBT|SHSTK} property (-z cet-report)`.
12. plumb.md: the reference forms its reference mode must accept:
    - bfd's `mov $imm`, `mov $0` and lea-for-add;
    - mold's `40 e8`/`40 e9` and its unrelaxed binops;
    - lld's 32-bit lea.

## Open questions to write in §17 (each with a recommendation)

| question | recommendation |
|---|---|
| libc++ and kiln objects are not in the census | re-run the census when they exist |
| APX CODE_* types | later; accept unrelaxed once a producer emits them |
| `SHF_X86_64_LARGE` | refused in M1; place the sections after the others, as lld does, in M3/M4 |
| static-pie milestone (request 9) | see request 9 |
| the `-z force-ibt` alias | accept as an alias of `-z ibt` |
| GOTPCRELX relaxation against abs/ifunc | keep the GOT |
| base 0x400000 | keep (a64 Q12) |
| `EI_OSABI` | 0 |
| IBT enforcement and APX execution | not run (no emulator support); M2 link tests |
| the `CODE_5` meaning | taken from the psABI as recalled; verify against the psABI text |

## Could not verify

- APX execution.
- IBT enforcement at run time.
- The lldb tombstone check in the low layout on x86-64.
- static-pie with mortar's own outputs (none exist yet).
- SIZE32 in lld beyond the one case.
- The `CODE_5` semantics.
- libc++ and kiln objects.
