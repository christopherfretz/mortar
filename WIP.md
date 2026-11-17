# WP-151 WIP

State: reading done; coding part A (records.rs: layout scheme, named rows).

Plan:
- A1 Info.layout_scheme; by_scheme per kind; kind3 layout by layout scheme,
  hash by odr.scheme; shape exception keyed on layout scheme; stats
  `layout schemes N, layout pairs not compared M` appended after
  `pairs not compared`.
- A2 named kind 2 rows: symbol of that name defined in file -> anchored
  (sym set); else strict only (inlined_only switch), counted unanchored.
- A3 PARTIAL: records already strict-only + warning; archives (opt-in,
  any mode): PARTIAL rows compared under strict only.
- B: details.rs nesting via width, slot help sentence, signed enums,
  enum default-level note, equal-details enum note; K14 relations
  DeclaredIn/DeclaredDifferentlyIn (mortar_diag + diag.md); C/C++ pair
  summary notes only ENFORCE-in-both keys; W4 std note (records gives
  per-file std, lib.rs runs records before foreign).
