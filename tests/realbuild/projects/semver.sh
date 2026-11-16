# semver (a Rust crate without dependencies, from crates.io): its unit
# tests and its four integration tests, each a test binary rustc links
# through the row's C compiler (-C linker=$CC), the way cargo has rustc
# link; built with rustc directly because cargo would want the network to
# resolve the crate's optional dependency. The Rust is the workspace's
# (rust-toolchain.toml).
rb_build() {
  cd "$RB_SRC"
  local flags=(--edition 2018 -g -C opt-level=2 -C codegen-units="$RB_JOBS" -C linker="$CC")
  rustc "${flags[@]}" --crate-type rlib --crate-name semver src/lib.rs -o libsemver.rlib
  rustc "${flags[@]}" --test --crate-name semver src/lib.rs -o test_lib
  for t in test_autotrait test_identifier test_version test_version_req; do
    rustc "${flags[@]}" --test --extern semver=libsemver.rlib "tests/$t.rs" -o "$t"
  done
}

rb_test() {
  cd "$RB_SRC"
  for t in test_lib test_autotrait test_identifier test_version test_version_req; do
    rb_run "$t" "./$t" --test-threads=4
  done
}
