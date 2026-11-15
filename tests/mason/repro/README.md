# Reproducers found by the Mason lane

Neither is wrong output: both are refusals by name that keep every
libc++-22 link (Debian's static archives, built by Clang 22) from
linking. Build each with `clang-22 -c <file>.s` and link
`mortar -static <file>.o -o out`; GNU ld and lld link both.

| file | mortar today | what the references do | spec |
|---|---|---|---|
| `aarch64-attributes.s` | `unsupported section type: SHT_AARCH64_ATTRIBUTES` | GNU ld 2.44 and lld 22 link; lld merges the `aeabi_feature_and_bits` subsection into the output's `.ARM.attributes` | layout.md L-35 says "accepted and consumed"; resolve.md "Section types" still refuses (WP-027d item 4) |
| `deplibs.s` | `unsupported section type: 0x6fff4c04` (named as a number) | GNU ld ignores `.deplibs`; lld adds `-lpthread` (and fails when it cannot find it) | resolve.md "Section types" refuses on purpose; the diagnostic should name `SHT_LLVM_DEPENDENT_LIBRARIES` |

In the libc++ archives: `.ARM.attributes` is in 71/71 members of
`libc++.a`, 18/18 of `libc++abi.a`, 7/9 of `libunwind.a`, 6/6 of
`libc++experimental.a`; `.deplibs` (`pthread`) in 11, 3 and 1 members of
the first three. With both sections removed (`objcopy -R`) every lane
program and libc++ test that GNU ld links, mortar links, and runs the same.
