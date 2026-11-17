# WIP: WP-126 (C++ in -shared)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-126.md. Scratch: .build/target-wp-145/wp126/
(lt.sh runs the lane on given tests).

Done: tests/link/dyn/cxx/shared-eh.test, shared-rtti.test (pass, all four linkers, both targets).
Next: shared-unique, shared-thread-local, shared-ctor-order, shared-static-libstdcxx,
shared-cxx-options; then googletest BUILD_SHARED_LIBS (runner manifest row), survey of
zlib/lua/googletest with --work under scratch; record spec/landed/dyn/WP-126.md.
