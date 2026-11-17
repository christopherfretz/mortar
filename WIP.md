# WP-154 WIP (deleted in the final commit)

Written: nothing in spec/ yet; scripts being copied to tests/relocatable-design/.

Verified (scratch .build/target-wp-154/work/*.log): census (census2/lines.txt),
BusyBox lines, final matrix (178 same; mold COMMON->ABS and IFUNC crash),
facts 1-8 + supp (CREL: lld keeps, bfd/mold refuse; GOTPCRELX kept; nested -r),
options, -O0 C++ debug (gdb same; lldb same as mortar's direct link, which
differs from GNU ld only in __libc_start_main alias naming), meta (MT-30:
units concatenated, SHF_EXCLUDE align 8, --odr=strict same finding through
all three -r), bench (400 objs: bfd 8.5-11.5 s 225 MB; lld 0.9 s 770 MB;
mold 1.2 s 930 MB), more (prelink idiom, split-dwarf, Fortran, -i, LTO,
--gdb-index, -T).

Reported to main: (1) NONE-overlap refusal of bfd -r .eh_frame on AArch64;
(2) --localize-hidden C++ abort = resolve §6.3 case (b) not enforced for a
weak demoted mention.

Next: copy scripts; write spec rules (model, layout, image, resolve, arch,
cli, meta MT-30/MT-16); the record; delete WIP.md.
