# WIP WP-255 (design step: the update's lazy Link and per-file front)

Done: reading list (brief, u-common, WP-225 Step 0, rulings).
Next: instrumentation patch (mortar_core Column probe behind feature wp255probe),
throwaway test crates/mortar/tests/wp255_readset.rs on protoc, record spec/landed/incr/WP-255.md.
Resume: patches in tests/incr-design/wp255/*.patch; `git apply` them, run tests/incr-design/wp255/run-readset.sh.
Never commit crates/: `git checkout -- crates` and delete the throwaway test before committing.
