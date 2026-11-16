# WP-075 work in progress

## Done
- Scope item 1, the prototype: `spec/landed/meta/WP-075/{prototype.sh,o24.py}` and the four
  logs `prototype-{aarch64,x86_64}-{bfd,lld}.log` (identical on findings and on what ran).
- Interim hand-back sent; ruling received (row 3: (b) is "does not export"; unversioned vs
  versioned measured: p1/p2/p3 providers x libnv f@V1 + f@@V2 x libu on old f@V1, both
  orders; K18b c1/c2). The shapes do not contradict the amended rule.

## Now
- Amended O-24 / MT-13 text from the measured rows; O-51 `-Bsymbolic-functions` sentence.
- Then: item 2 (table), item 3 (check), xtask `meta write --into` fix (target objcopy),
  item 4 (cost), item 5 (specs), record, merge main, gate, hosted run.
