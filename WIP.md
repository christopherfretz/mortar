# WP-075 work in progress

## Done
- Scope item 1, the prototype: `crates/mortar_odr/tests/oracle/o24/prototype.sh` (shapes),
  `o24.py` (O-50/O-51 rows written into a linked-in placeholder `.note.mason`, read back by a
  PT_NOTE walk; O-24 (a)/(b) as written, `--literal`, and with the amended identity,
  `--amended`). Results: `results/{aarch64,x86_64}-{bfd,lld}.log` (the four agree line for
  line on findings and on what ran).
- Table placement measured: a prepared `.note.mason` object linked in gets its own PT_NOTE
  (align 8) inside the first PT_LOAD on both targets with GNU ld and lld; `objcopy
  --add-section` on a linked `.so` leaves the section outside every segment ("allocated
  section not in segment", address 0, alignment 1 although 8 was asked): the loader does not
  map it and a PT_NOTE reader does not find it (`readelf -n` still prints it, by section).

## STOP (scope item 1's condition): interim hand-back sent
Rows where O-24 as written is wrong or silent (program runs another module's copy):
- unversioned vs versioned (exe exports unversioned, libv1 @LIB1_1: libv1 runs the exe's copy;
  liba unversioned + libv2 @@LIB2_1, both orders: one module runs the other's copy): the rule
  says "different version nodes: never compared" -> silent. glibc check_match binds an
  unversioned definition to a versioned reference and an unversioned reference to the default
  version.
- (b) "a (name, version) that the output does not define": an output with a HIDDEN copy still
  lets libb bind liba's copy -> silent. Should read "does not export".
Waiting for the supervisor's ruling before writing the engine (items 2-5).

## Next
- After the ruling: item 2 (the table in mortar's dynamic executables), item 3 (the check in
  mortar_odr), item 4 (cost), item 5 (specs).
