# WIP: WP-200 (strong reference extracts past a dropped COMDAT copy's definition)

Done: reading; design (the closure iterated: the demoted mentions of the
previous iteration's live files are undefined mentions with their binding;
the targets exclude them; a new iteration when D1 finds a demoted mention
that feeds back).

Next: implement in crates/mortar_resolve/src/live.rs + resolve.rs; unit
tests; lane tests; measurements; record spec/landed/resolve/WP-200.md.

Resume: branch wp/200-strong-ref-extracts; scratch .build/target-wp-140/wp200/.
