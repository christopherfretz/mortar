# WP-047 (M2-A64) — work in progress

Done (committed): lazy_target, dyn_tags, weak_use (src/weak.rs), the a64-local
dynamic_outcome override (TX → text-relocation; Pie PC/LO vs shlib → COPY /
canonical-plt-in-pie), symbolic ABS64 place = 0, large TLSDESC vs TLS-dynamic
refused by name (LARGE_TLSDESC_DYNAMIC); tests/dynamic.rs (unit cells).

Next: tests/dynamic_ref.rs (PLT/.got.plt and TLS bytes vs GNU ld and lld);
static-identical; spec/landed/dyn/WP-047.md; gate; delete this file.
