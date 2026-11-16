- Second pass, unmeasured, each with the package that must measure it:
  - a 64 KiB-page AArch64 kernel (D-123's RELRO end, lld's unaligned
    end): no such host; S-H records it as a lane gap, S1b asserts the
    alignment by construction (P-R6);
  - the AArch64 `NEEDED_INDIRECT_EXTERN_ACCESS` property (W41, Q22):
    S1a (it keeps the property; GCC's AArch64 support of
    `-mno-direct-extern-access` to be checked there);
  - D-117's address rule on objects outside the corpus: kiln+hod objects
    (the Mason lane), gfortran `-fPIE` objects, Clang 19: S1a runs
    `relscan.py`'s successor over them before it merges;
  - the `+4+t` immediate adjustment of x86 PC-relative uses on
    instructions beyond the corpus (AVX-512 with `imm8`, `imm16`
    forms): S5;
  - i386 links of the rule (only the relocation scan was run): S8;
  - lld 19 for the D-112 table (only lld 22 ran): S-H's reference
    matrix;
  - W28's and W29's root causes: S8;
  - Q21's decoding option: its own package after S1j.
