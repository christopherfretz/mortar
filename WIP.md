# WP-030 stage 1 WIP (deleted in the final commit)

- Written: skeleton of spec/x86.md.
- Verified: tool versions (gcc/g++ 14.2 i686 cross, NO i686 gfortran: x86_64-linux-gnu-gfortran -m32 compiles
  i386 objects but there is no i386 libgfortran.a; ld 2.44, lld 19.1.7/22.1.8, mold 2.37.1, qemu-i386 10.0.13,
  clang 19.1.7/22.1.8, gdb 16.3).
- Harness: /Volumes/AgentWork/mortar-helper/.build/review-x86/ ; scratch /tmp/mortar-wp030-1592004.
- Next: write scan32.py (ELF32 REL scanner), corpus.sh, run census.
