# Reproducers found by the Mason lane

Each says how to run it and what mortar should do.

## Refusals (fixed)

Neither was wrong output: both were refusals by name that kept every
libc++-22 link (Debian's static archives, built by Clang 22) from
linking. Build each with `clang-22 -c <file>.s` and link
`mortar -static <file>.o -o out`. Both link now; each is a link test.

| file | mortar | what the references do | spec, test |
|---|---|---|---|
| `aarch64-attributes.s` | links; the section is consumed (WP-027d; refused before as `unsupported section type: SHT_AARCH64_ATTRIBUTES`) | GNU ld 2.44 and mold drop it; lld 22 merges the `aeabi_feature_and_bits` subsection into the output's `.ARM.attributes` and writes its BTI/PAC bits as a GNU property note | layout.md L-35, resolve.md §10; `tests/link/resolve/aarch64-attributes.test` |
| `deplibs.s` | links; `.deplibs` is consumed and its specifiers not honoured (WP-034; refused before as `unsupported section type: 0x6fff4c04`) | GNU ld 2.44 and mold 2.37 ignore the specifiers and copy the section; lld 19 and 22 load `-lpthread` (and fail when they cannot find it) | resolve.md §10 "Dependent libraries"; `tests/link/resolve/deplibs.test`, `deplibs-undefined.test` |

In the libc++ archives: `.ARM.attributes` is in 71/71 members of
`libc++.a`, 18/18 of `libc++abi.a`, 7/9 of `libunwind.a`, 6/6 of
`libc++experimental.a`; `.deplibs` (`pthread`) in 11, 3 and 1 members of
the first three. With both sections removed (`objcopy -R`) every lane
program and libc++ test that GNU ld links, mortar links, and runs the same.
