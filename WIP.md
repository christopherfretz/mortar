# WP-060 final round: work in progress

Done (this checkpoint):
1. Split DWARF measured (R14): §6 "Split DWARF", §1.5, IN-55, record decision 9.
2. N2: scenario g (`gc-edge-kinds`); IN-29 table; record row.
3. By-value oracle: 56 of 56 pass; the 6 old failures were oracle bugs
   (piece lookup; S-50 rule 1); IN-34a/IN-27 `.eh_frame`-reference rule.
   Results: `.build/review-incr/results/final/`.

Left: the final commit (delete this file, README row, spec_tables test,
remove `.build/target-wp-060`).
