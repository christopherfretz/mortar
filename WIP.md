# WP-030 stage 1 WIP (deleted in the final commit)

- Written: skeleton of spec/x86.md. Sections are being filled in order from §1.
- Verified, measured and in harness logs (/Volumes/AgentWork/mortar-helper/.build/review-x86/logs-*.txt):
  census (logs-census-table.txt), GOT32/GOT32X forms (gotform.sh, relaxsum.py over relaxcmp.sh),
  TLS relaxation bytes per linker (logs-sum-*.txt), IFUNC (ifunc.sh), TLS layout, header/segments
  (layout.sh), split-dwarf TLS (dbgtls.sh), per-type values (typeprobe.py), undefined weak, merge
  strings, COMDAT debug refs (misc.sh), -fno-plt (noplt.sh).
- Learned, not yet written into the spec: everything (this summary is the source for §2-§19).
  Key choices: GOT32X relaxed only in static non-PIE, GNU ld forms (c7 c0+r / 81 /n / f7 c0+r /
  67 e8 / e9 rel 90), only for the unambiguous plain-ModRM operand; GOT32 never relaxed;
  ambiguous [P-2,P) operands refused; GD/LD/IE/GOTIE/DESC to LE with GNU/lld bytes, GOTIE add as
  81 c0+r (keeps flags, not lld's lea); IFUNC static: ebx-free IPLT entry is canonical; static-pie:
  GOTOFF and PC32 to IFUNC refused, PLT32 through ff a3 entry; SHT_RELA in EM_386 refused;
  dynamic types in input refused; 16/8 read signed with su checks.
- Next: write §1-§7, commit; then §8-§14, commit; §15-§19 + appendix, commit; run-all.sh;
  README row; delete this file; rm -rf /tmp/mortar-wp030-1592004.
