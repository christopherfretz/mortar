# WIP — WP-084 (x86-64 large sections)

Done (uncommitted -> this commit): layout (assign.rs large_output, rank.rs rank 8 within 16-19,
chunk.rs .lbss commons, address.rs separate-code for LargeRx, symbols.rs edata/etext/bss_start
exclude large), core LoadClass::Large*, resolve SHN_X86_64_LCOMMON -> large common (view.rs place_of).
Measured: /tmp/mortar-wp084-2664338/e1 (scan.out, link.out, linkm.out); mortar matches GNU ld on 51 links.

Next: fix old refusal tests (x64.rs x64_large_data_is_refused_by_name, resolve tests/lcommon.rs,
layout tests/stages.rs ~225), new tests, xfail removal, specs, record, CI.
