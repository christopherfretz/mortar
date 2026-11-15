# WP-032e WIP

- done: timers (5130a38); fix 1 second placement reuses fixed point + sites + symtab plan.
- measured fix 1 (protoc, 9 interleaved): layout 62.0->40.0 ms (1T), 46.1->31.1 (8T); CPU 8T 862->806; RSS 8T 235->229.
- correctness fix (sites.rs erratum_windows): windows were [q,q+16) per 0xFF8 inside the section; missed W4 at 0x1008 (W1 at 0xFFC) and windows whose head is in the previous section. base mis-linked sections workload: adrp at 0x93dffc (fn at page 0xffc) relaxed to `adr x1,0x93d000` from the unrelocated word (should be page 0xaf8000; lld agrees). golden: sections 8710efd1af24fba4 -> 8b7835917569b6f6 (one more veneer, +8 shift); others unchanged. plumb did not catch it (spec finding).
- merged main f0f1dcc once (a8b60f6; conflict in front.rs sites block). golden vs main build: identical except sections (the fix); x64 (CXX_THREADS, x86_64-linux-gnu-g++ -static) f37821dc24b320ad.
- steps unified: MORTAR_STEPS only; input/resolve print `mortar: steps: <phase> <step>: X ms` and `... <phase> count <name>: n`.
- MORTAR_HUGEPAGES=1 opt-in (main.rs re-exec via /proc/self/exe, arg0 kept, GLIBC_TUNABLES appended, guard MORTAR_HUGEPAGES_EXEC, skips if tunable named). data: hp1.txt (4 wl x 1/4/8/12), hp2.txt (off/tun/reexec), thp.txt (PR_SET_THP_DISABLE: no effect), faults 63% input store copies. exec cost ~0.2 ms.
- fix: code() skips sections whose room touches no erratum window. protoc 8T layout 28.2->22.3 (cpu 90->40), total 156.9->150.3, cpu 672->633; 1T layout 35.8->31.0. nog 8T 96.0->89.7, cpu 286->246. golden identical.
- next: CPU growth with threads (layout 100 ms CPU at 8T for 31 wall), sites (12 ms), erratum code(), validate.
