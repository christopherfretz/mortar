# WP-090 work in progress (removed in the final commit)

Done:
- Measurement of GNU ld 2.44, lld 19/22, mold 2.37.1 (scratch
  /tmp/mortar-wp090-4711): GNU ld writes R4's LE words in -static and
  -static-pie (also with --no-relax, -z now, -z norelro); lld refuses 513/514;
  mold keeps GD (pair 1, DTPREL; static __tls_get_addr). Beyond 32 bits GNU ld
  truncates silently (wrong code); nop missing: GNU ld W15 (wrong code).
- Code: 513/514 rows M1 (table.rs); scan refuses an unmatched GD member under
  the static tables (`UNRECOGNIZED_GD`, apply.rs), `m2` passed to scan.

Next:
- unit tests (relax.rs static column; a64.rs counts; scan rules i19)
- link tests tests/link/tls/gd-trad-*.test; verifier verdicts
- specs a64.md §1, §6.1 status, §8.4 static column; record
  spec/landed/a64/WP-090.md; static-identical; gate; hosted run
