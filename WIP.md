# WIP: WP-154 (-r design)

Written: nothing yet in spec/.
Verified (scratch .build/target-wp-154/exp, work/):
- census/: driver lines for -r (gcc adds -dynamic-linker, --eh-frame-hdr, --hash-style, --as-needed, -X, --fix-cortex-a53-843419, plugin; clang adds --build-id, --eh-frame-hdr, --hash-style). -Wl,-r without -nostdlib fails (-r and -pie).
- busybox/: the two lines (gcc and clang `-nostdlib -r` of applets.o); bfd/lld/mold -r then bfd/mortar final: busybox runs.
- final/: 178 cells same as direct; mold -r bugs (COMMON -> ABS, IFUNC crash), reported to supervisor.
Next: structural facts per reference (sections, groups, symbols, relocs, eh_frame, merge, debug, notes), rcheck.py oracle prototype, options tail, -b binary, meta, large bench, then the specs.
