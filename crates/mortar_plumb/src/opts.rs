//! The link's options that decide a rule, read from its words
//! (`spec/plumb.md` §3.2a): the one reader of a mortar map's `arg`
//! records. Every option-dependent rule of the verifier (§3.2a's table)
//! takes its option from here.
//!
//! The words are read as `spec/cli.md` §2 reads a line: one or two
//! dashes (§2.2's forms, a long name starting with `l`, `m` or `o` with
//! two only), a required value joined by `=` or the next word whatever it
//! is (§2.3), an optional value by `=` only, `-z keyword` and
//! `-zkeyword` (§2.7), a short option with its value joined (§2.2 step
//! 5), an abbreviation of a GNU long name (§2.4), and the last of an
//! option and its negation winning (§2.10). Which spellings exist, which
//! take a value and which are GNU's come from `docs/status/options.md`,
//! the rows mortar does not refuse ([`ACCEPTED`]; a test holds the two
//! equal): a line mortar linked has no other word, so a word this reader
//! cannot place is a map that no link of this mortar wrote (M9).

/// The rows of `docs/status/options.md` whose status is not `refused`:
/// `R <value> <gnu> <spelling>...` with `<value>` `No`, `Next` (a
/// required value: `=` or the next word) or `Eq` (optional, `=` only)
/// and `<gnu>` `gnu` for a row GNU ld has (its long names abbreviate,
/// cli.md §2.4); `Z <keyword> <value>` for a `-z` keyword.
pub const ACCEPTED: &str = "\
R Next gnu -o --output
R No gnu -shared --Bshareable
R No gnu -pie --pic-executable
R No gnu -no-pie --no-pic-executable
R Next gnu -m
R Next gnu -e --entry
R No gnu -s --strip-all
R No gnu -S --strip-debug
R No gnu --strip-discarded
R No gnu -x --discard-all
R No gnu -X --discard-locals
R No gnu --discard-none
R Eq gnu --build-id
R No - --no-build-id
R Next gnu --compress-debug-sections
R No gnu --eh-frame-hdr
R No gnu --no-eh-frame-hdr
R No gnu --gc-sections
R No gnu --no-gc-sections
R No gnu --print-gc-sections
R No gnu --no-print-gc-sections
R No gnu --gc-keep-exported
R No - --gdb-index
R No - --no-gdb-index
R No - --no-debug-names
R Next gnu -l --library
R No gnu -( --start-group
R No gnu -) --end-group
R No gnu --as-needed
R No gnu --no-as-needed
R No gnu -Bstatic --dn --non_shared --static
R No gnu -Bdynamic --dy --call_shared
R No gnu --no-copy-dt-needed-entries --no-add-needed
R No gnu --push-state
R No gnu --pop-state
R No - --no-dependent-libraries
R No - --warn-backrefs
R No - --no-warn-backrefs
R Next gnu -L --library-path
R Next gnu --sysroot
R Next gnu -rpath-link
R Next gnu --version-script
R Eq gnu --sort-common --sort_common
R Next gnu --sort-section
R Next gnu --export-dynamic-symbol
R Next gnu -init
R Next gnu -fini
R No gnu --undefined-version
R No gnu --no-undefined-version
R No gnu -E --export-dynamic
R No gnu --no-export-dynamic
R No gnu -Bsymbolic
R No gnu -Bsymbolic-functions
R No gnu -Bno-symbolic
R Next gnu -h -soname
R Next gnu -rpath
R Next gnu -I --dynamic-linker
R No gnu --no-dynamic-linker
R No gnu --enable-new-dtags
R No gnu --disable-new-dtags
R Next gnu --hash-style
R No gnu --rosegment
R Next - --error-limit
R No gnu --fatal-warnings
R No gnu --no-fatal-warnings
R No gnu -w --no-warnings
R Eq - --color-diagnostics
R No - --no-color-diagnostics
R No gnu --no-undefined
R No gnu --warn-common
R No - --no-warn-common
R No gnu --warn-execstack
R Next - --odr
R No - --no-odr-check
R No - --odr-stats
R No - --odr-check-archives
R Next - --odr-table
R No - --keep-mason-meta
R No - --detect-odr-violations
R No gnu --version
R No gnu -v
R No gnu -V
R No gnu --help
R Next gnu -Map
R No gnu -M --print-map
R Eq gnu --verbose
R Next gnu --dependency-file
R Eq - --threads
R Next - --mortar-map
R No - --time-phases
R Eq - --incremental
R No - --no-incremental
R Next - --incremental-state
R Eq - --incremental-report
R Next gnu -plugin
R Next gnu -plugin-opt
R No gnu -plugin-save-temps
R No gnu -flto
R Next gnu -flto-partition
R Next gnu -fuse-ld
R Eq gnu --fix-cortex-a53-843419
Z now No
Z lazy No
Z relro No
Z norelro No
Z execstack No
Z noexecstack No
Z text No
Z separate-code No
Z noseparate-code No
Z defs No
Z max-page-size Next
Z common-page-size Next
Z sectionheader No
Z nostart-stop-gc No
Z memory-seal No
Z nomemory-seal No
R No gnu -EL
R No gnu -g
R Next gnu -O
R No gnu -Qy
R No gnu -qmagic
R Next gnu -assert
R No gnu --map-whole-files
R No gnu --no-map-whole-files
R No gnu --no-keep-memory
R No gnu --reduce-memory-overheads
R Next gnu --hash-size
R Next gnu --max-cache-size
";

/// The rows of `docs/status/options.md` whose status is `refused`, in
/// the same form as [`ACCEPTED`] (a test holds it equal to the table).
/// No word of mortar's map spells one (M9), but a reference linker's line
/// may (the caller's words, §12.1; `realbuild` judges the control
/// linker's output of a line mortar refused): whether such an option
/// takes the next word is the table's, as for every row (WP-131, V6).
pub const REFUSED: &str = "\
R No gnu -r -i --relocatable
R Next gnu --oformat
R No gnu -Ur
R No gnu -d --dc --dp
R No gnu --no-define-common
R No gnu -q --emit-relocs
R No gnu --no-strip-discarded
R No gnu -n --nmagic
R No gnu -N --omagic
R No gnu --no-omagic
R No - --no-nmagic
R Eq gnu --package-metadata
R Next - --icf
R No - --no-icf
R No - --print-icf-sections
R No - --no-print-icf-sections
R Next - --keep-unique
R No - --ignore-data-address-equality
R No - --ignore-function-address-equality
R No - --debug-names
R No gnu --relax
R No gnu --no-relax
R No gnu --force-group-allocation
R No gnu --traditional-format
R No gnu --embedded-relocs
R No gnu --force-exe-suffix
R Next gnu --out-implib
R No - --execute-only
R No - --no-execute-only
R Next - --compress-sections
R Eq - --separate-debug-file
R No - --no-separate-debug-file
R Next gnu -R --just-symbols
R Next gnu -b --format
R No - --start-lib
R No - --end-lib
R No gnu --whole-archive
R No gnu --no-whole-archive
R No gnu --copy-dt-needed-entries --add-needed
R No gnu --accept-unknown-input-arch
R No gnu --no-accept-unknown-input-arch
R No gnu --no-warn-mismatch
R No gnu --no-warn-search-mismatch
R Next gnu --remap-inputs-file
R Next gnu --remap-inputs
R No - --dependent-libraries
R No - --fortran-common
R No - --no-fortran-common
R Next - --warn-backrefs-exclude
R Next - --why-extract
R Next - --print-archive-stats
R No - --fat-lto-objects
R No - --no-fat-lto-objects
R No gnu -nostdlib
R Next gnu -Y
R Next - -C --directory
R Next - --chroot
R Next gnu -T --script
R Next gnu --default-script --dT
R Next gnu -c --mri-script
R Next gnu --dynamic-list
R Next gnu --export-dynamic-symbol-list
R Next gnu --retain-symbols-file
R Next gnu --section-ordering-file
R Next - --symbol-ordering-file
R No - --no-warn-symbol-ordering
R No - --warn-symbol-ordering
R Next - --call-graph-ordering-file
R Next - --call-graph-profile-sort
R Next - --print-symbol-order
R Next - --irpgo-profile
R Next - --bp-startup-sort
R Next - --bp-compression-sort
R No - --bp-compression-sort-startup-functions
R No - --verbose-bp-section-orderer
R Next gnu --orphan-handling
R Eq gnu --unique
R No gnu --enable-non-contiguous-regions
R No gnu --enable-non-contiguous-regions-warnings
R No gnu --disable-linker-version
R No gnu --enable-linker-version
R No gnu --check-sections
R No gnu --no-check-sections
R Eq - --shuffle-sections
R No - --reverse-sections
R Next - --randomize-section-padding
R Next gnu -u --undefined
R Next gnu --require-defined
R Next - --undefined-glob
R Next gnu --defsym
R Next gnu --wrap
R Next gnu --exclude-libs
R Next gnu -y --trace-symbol
R Next gnu --ignore-unresolved-symbol
R Eq gnu --demangle
R No gnu --no-demangle
R No gnu --default-symver
R No gnu --default-imported-symver
R Next gnu --version-exports-section
R No gnu --disable-multiple-abs-defs
R No - --gnu-unique
R No - --no-gnu-unique
R Next gnu --task-link
R No - --Bsymbolic-non-weak
R No - --Bsymbolic-non-weak-functions
R Next - --pack-dyn-relocs
R Next gnu -f --auxiliary
R Next gnu -F --filter
R Next gnu --audit
R Next gnu -P --depaudit
R No gnu -Bgroup
R No gnu --dynamic-list-data
R No gnu --dynamic-list-cpp-new
R No gnu --dynamic-list-cpp-typeinfo
R No gnu --allow-shlib-undefined
R No gnu --no-allow-shlib-undefined
R Next gnu --spare-dynamic-tags
R Next - --spare-program-headers
R No - --apply-dynamic-relocs
R No - --use-android-relr-tags
R No - --no-use-android-relr-tags
R No - --android-memtag-heap
R No - --android-memtag-stack
R Next - --android-memtag-mode
R Next gnu -Ttext
R Next gnu -Tdata
R Next gnu -Tbss
R Next gnu -Ttext-segment
R Next gnu -Trodata-segment
R Next gnu -Tldata-segment
R Next gnu --section-start
R Next gnu --image-base
R No gnu --no-rosegment
R No - --xosegment
R No - --no-xosegment
R Eq gnu --split-by-file
R Eq gnu --split-by-reloc
R Next gnu -G --gpsize
R No - --optimize-bb-jumps
R No - --no-optimize-bb-jumps
R No - --branch-to-branch
R No - --no-branch-to-branch
R Next gnu --unresolved-symbols
R No gnu --warn-unresolved-symbols
R No gnu --error-unresolved-symbols
R No gnu --allow-multiple-definition
R No - --no-allow-multiple-definition
R No gnu --noinhibit-exec --noinhibit_exec
R Next gnu --error-handling-script
R No gnu --warn-constructors
R No gnu --warn-multiple-gp
R No gnu --warn-once
R No gnu --warn-section-align
R No gnu --warn-textrel --warn-shared-textrel
R No gnu --warn-alternate-em
R No gnu --warn-execstack-objects
R No gnu --no-warn-execstack
R No gnu --error-execstack
R No gnu --no-error-execstack
R No gnu --warn-rwx-segments
R No gnu --no-warn-rwx-segments
R No gnu --error-rwx-segments
R No gnu --no-error-rwx-segments
R No - --warn-ifunc-textrel
R No - --no-warn-ifunc-textrel
R No - --vs-diagnostics
R Next - --why-live
R No gnu --print-map-discarded
R No gnu --no-print-map-discarded
R No gnu --print-map-locals
R No gnu --no-print-map-locals
R No gnu --cref
R No gnu -t --trace
R Next - --reproduce
R No - --repro
R Next - --thread-count
R No - --no-threads
R No - --verify
R Next - --incremental-write
R Next - --incremental-sync
R No - --incremental-verify
R Next - --incremental-changed
R No gnu --stats
R No - --perf
R Eq - --time-trace
R Next - --time-trace-granularity
R No gnu --print-memory-usage
R No gnu --print-output-format
R No gnu --print-sysroot
R No gnu --target-help
R No - --fork
R No - --no-fork
R No - --quick-exit
R No - --no-quick-exit
R No - --detach
R No - --no-detach
R No - --mmap-output-file
R No - --no-mmap-output-file
R Next - --rsp-quoting
R Next - --run
R Next - --lto
R Next - --lto-O
R Next - --lto-CGO
R Next - --lto-partitions
R Next - --lto-aa-pipeline
R Next - --lto-newpm-passes
R No - --lto-debug-pass-manager
R No - --lto-emit-asm
R No - --lto-emit-llvm
R Next - --lto-sample-profile
R Next - --lto-cs-profile-file
R No - --lto-cs-profile-generate
R No - --lto-whole-program-visibility
R No - --no-lto-whole-program-visibility
R No - --lto-pgo-warn-mismatch
R No - --no-lto-pgo-warn-mismatch
R Next - --lto-basic-block-sections
R No - --lto-basic-block-address-map
R No - --no-lto-basic-block-address-map
R No - --lto-unique-basic-block-section-names
R No - --no-lto-unique-basic-block-section-names
R No - --lto-validate-all-vtables-have-type-infos
R No - --no-lto-validate-all-vtables-have-type-infos
R Next - --lto-known-safe-vtables
R Next - --load-pass-plugin
R Next - --mllvm
R Eq - --save-temps
R Next - --opt-remarks-filename
R Next - --opt-remarks-format
R Next - --opt-remarks-passes
R Next - --opt-remarks-hotness-threshold
R No - --opt-remarks-with-hotness
R Next - --thinlto-cache-dir
R Next - --thinlto-cache-policy
R Next - --thinlto-jobs
R Next - --thinlto-single-module
R Next - --thinlto-distributor
R Next - --thinlto-distributor-arg
R Next - --thinlto-remote-compiler
R Next - --thinlto-remote-compiler-arg
R Next - --thinlto-remote-compiler-prepend-arg
R No gnu --fix-cortex-a53-835769
R No gnu --no-apply-dynamic-relocs
R No gnu --pic-veneer
R Next gnu --stub-group-size
R No gnu --no-enum-size-warning
R No gnu --no-wchar-size-warning
R No gnu --no-pipeline-knowledge
R No gnu --ld-generated-unwind-info
R No gnu --no-ld-generated-unwind-info
Z force-bti No
Z bti-report Eq
Z pac-plt No
Z gcs Next
Z gcs-report Eq
Z gcs-report-dynamic Next
Z noextern-protected-data No
Z indirect-extern-access No
Z noindirect-extern-access No
Z dynamic-undefined-weak No
Z nodynamic-undefined-weak No
Z call-nop Next
Z ibtplt No
Z ibt No
Z shstk No
Z cet-report Next
Z report-relative-reloc No
Z x86-64-baseline No
Z x86-64-v2 No
Z x86-64-v3 No
Z x86-64-v4 No
Z isa-level-report Next
Z noreloc-overflow No
Z lam-u48 No
Z lam-u48-report Next
Z lam-u57 No
Z lam-u57-report Next
Z lam-report Next
Z mark-plt No
Z nomark-plt No
Z notext No
Z undefs No
Z stack-size Next
Z pack-relative-relocs No
Z nopack-relative-relocs No
Z copyreloc No
Z nocopyreloc No
Z combreloc No
Z nocombreloc No
Z nosectionheader No
Z muldefs No
Z unique-symbol No
Z nounique-symbol No
Z globalaudit No
Z start-stop-gc No
Z start-stop-visibility Next
Z global No
Z initfirst No
Z interpose No
Z unique No
Z nounique No
Z loadfltr No
Z nodefaultlib No
Z nodelete No
Z nodlopen No
Z nodump No
Z origin No
Z common No
Z nocommon No
Z dead-reloc-in-nonalloc Next
Z execute-only-report Next
Z force-ibt No
Z hazardplt No
Z ifunc-noplt No
Z keep-text-section-prefix No
Z nokeep-text-section-prefix No
Z nognustack No
Z rel No
Z rela No
Z retpolineplt No
Z rodynamic No
Z separate-loadable-segments No
Z wxneeded No
Z lrodata-after-bss No
Z nolrodata-after-bss No
Z zicfilp Next
Z zicfiss Next
Z zicfilp-unlabeled-report Next
Z zicfilp-func-sig-report Next
Z zicfiss-report Next
Z execstack-if-needed No
Z rewrite-endbr No
Z norewrite-endbr No
R Next gnu -a
R Next gnu -A --architecture
R No gnu -EB
R No gnu --dll-verbose
R No gnu --ctf-variables
R No gnu --no-ctf-variables
R Next gnu --ctf-share-types
R No - --be8
R No - --fix-cortex-a8
R No - --merge-exidx-entries
R No - --no-merge-exidx-entries
R No - --target1-abs
R No - --target1-rel
R Next - --target2
R No - --cmse-implib
R Next - --in-implib
R No - --relax-gp
R No - --no-relax-gp
R No - --pcrel-optimize
R No - --no-pcrel-optimize
R No - --toc-optimize
R No - --no-toc-optimize
R Eq - --power10-stubs
R No - --no-power10-stubs
R Next - --split-stack-adjust-size
";

/// How a row takes its value (cli.md §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Val {
    No,
    /// Required: joined by `=` (or, for a short option, by nothing) or
    /// the next word.
    Next,
    /// Optional, `=` only.
    Eq,
}

#[derive(Debug, Clone, Copy)]
struct Row {
    /// The row's first spelling: its name in [`Options::read`] and
    /// [`Word::Opt`].
    id: &'static str,
    value: Val,
    gnu: bool,
    /// A row of [`ACCEPTED`] (else of [`REFUSED`]).
    accepted: bool,
}

/// The whole table (WP-131): each spelling's name, without its dashes,
/// with its row, and each `-z` keyword with whether it takes `=VALUE` and
/// whether it is accepted.
#[derive(Debug)]
pub struct Table {
    names: Vec<(&'static str, Row)>,
    z: Vec<(&'static str, bool, bool)>,
}

/// What a word of a line is, read by [`Table::word`] (cli.md §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Word {
    /// An input: a word that does not start with `-`, or `-` alone
    /// (cli.md §2.1).
    Input,
    /// An option of a row, by the row's first spelling, with its value.
    Opt { id: &'static str, value: Option<String>, accepted: bool },
    /// `-z keyword[=value]` (§2.7).
    Z { kw: &'static str, value: Option<String>, accepted: bool },
    /// A word starting with `-` that no row spells, or spells wrongly (a
    /// value given to an option that takes none, a required value missing
    /// at the end of the line, a word that is not UTF-8): M9 in a mortar
    /// map.
    Unknown,
}

fn val(s: &str) -> Val {
    match s {
        "Next" => Val::Next,
        "Eq" => Val::Eq,
        _ => Val::No,
    }
}

impl Table {
    /// The table, read once.
    pub fn get() -> &'static Table {
        static T: std::sync::OnceLock<Table> = std::sync::OnceLock::new();
        T.get_or_init(|| {
            let mut t = Table { names: Vec::new(), z: Vec::new() };
            for (text, accepted) in [(ACCEPTED, true), (REFUSED, false)] {
                for line in text.lines() {
                    let w: Vec<&'static str> = line.split(' ').collect();
                    match w.as_slice() {
                        ["Z", kw, v] => t.z.push((*kw, val(v) != Val::No, accepted)),
                        ["R", v, gnu, spellings @ ..] if !spellings.is_empty() => {
                            let row = Row {
                                id: spellings[0],
                                value: val(v),
                                gnu: *gnu == "gnu",
                                accepted,
                            };
                            for s in spellings {
                                t.names.push((s.trim_start_matches('-'), row));
                            }
                        }
                        _ => {}
                    }
                }
            }
            t
        })
    }

    /// Every row: its first spelling, its spellings, whether it takes a
    /// value (`Some(true)` required, `Some(false)` optional by `=`) and
    /// whether it is accepted. For tests over the table.
    #[cfg(test)]
    pub(crate) fn rows(&self) -> Vec<(&'static str, Vec<&'static str>, Option<bool>, bool)> {
        let mut v: Vec<(&'static str, Vec<&'static str>, Option<bool>, bool)> = Vec::new();
        for (name, r) in &self.names {
            let takes = match r.value {
                Val::No => None,
                Val::Next => Some(true),
                Val::Eq => Some(false),
            };
            match v.iter_mut().find(|x| x.0 == r.id) {
                Some(x) => x.1.push(name),
                None => v.push((r.id, vec![name], takes, r.accepted)),
            }
        }
        v
    }

    /// The `-z` keywords: name, takes `=VALUE`, accepted. For tests.
    #[cfg(test)]
    pub(crate) fn keywords(&self) -> &[(&'static str, bool, bool)] {
        &self.z
    }

    /// Reads the word at `i` of `words` and the value it takes: what it
    /// is and how many words it used (1, or 2 with a value in the next
    /// word). cli.md §2.2's lookup in its order: an exact name, a short
    /// option, an abbreviation of a GNU long name (§2.4), then a short
    /// option with its value joined (step 5); §2.3's values; §2.7's `-z`.
    pub fn word<W: AsRef<[u8]>>(&self, words: &[W], i: usize) -> (Word, usize) {
        let Some(w) = words.get(i).map(AsRef::as_ref) else { return (Word::Unknown, 1) };
        if !w.starts_with(b"-") || w == b"-" {
            return (Word::Input, 1);
        }
        let next = || words.get(i + 1).map(|n| String::from_utf8_lossy(n.as_ref()).into_owned());
        let Ok(text) = std::str::from_utf8(w) else {
            // An option's name is ASCII: a word starting with `-` that
            // is not UTF-8 spells no option.
            return (Word::Unknown, 1);
        };
        // `-z keyword` and `-zkeyword` (cli.md §2.7).
        let (kw, used) = if text == "-z" {
            match next() {
                Some(k) => (Some(k), 2),
                None => return (Word::Unknown, 1),
            }
        } else {
            (text.strip_prefix("-z").filter(|k| !k.is_empty()).map(str::to_string), 1)
        };
        if let Some(kw) = kw {
            let (k, v) = match kw.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (kw.as_str(), None),
            };
            return match self.z.iter().find(|(name, _, _)| *name == k) {
                Some((name, takes, accepted)) if *takes == v.is_some() => {
                    (Word::Z { kw: name, value: v, accepted: *accepted }, used)
                }
                _ => (Word::Unknown, used),
            };
        }
        let dashes = if text.starts_with("--") { 2 } else { 1 };
        let body = &text[dashes..];
        let (name, joined) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (body, None),
        };
        // Step 1: an exact name; 3: a short option (cli.md §2.2).
        let exact = self.names.iter().find(|(n, _)| *n == name && dashes_allow(n, dashes));
        // Step 4: an abbreviation of a GNU long name: an accepted line
        // names exactly one (cli.md §2.4).
        let abbrev = || {
            if name.chars().count() < 2 || !dashes_allow(name, dashes) {
                return None;
            }
            let mut hits = self.names.iter().filter(|(n, r)| {
                r.gnu && n.len() > name.len() && n.starts_with(name) && n.chars().count() > 1
            });
            let first = hits.next()?;
            // Several spellings of one row are still one option here:
            // mortar refuses the abbreviation (cli.md §2.4), and an
            // accepted line never has one.
            if hits.any(|(_, r)| r.id != first.1.id) {
                return None;
            }
            Some(first)
        };
        if let Some((_, row)) = exact.or_else(abbrev) {
            let opt =
                |value: Option<String>| Word::Opt { id: row.id, value, accepted: row.accepted };
            return match (row.value, joined) {
                (Val::No, Some(_)) => (Word::Unknown, 1),
                (Val::No | Val::Eq, None) => (opt(None), 1),
                (_, Some(v)) => (opt(Some(v.to_string())), 1),
                (Val::Next, None) => match next() {
                    Some(v) => (opt(Some(v)), 2),
                    None => (Word::Unknown, 1),
                },
            };
        }
        // Step 5: one dash, a short option, its value joined.
        let short = (dashes == 1).then(|| body.chars().next()).flatten().and_then(|c| {
            self.names
                .iter()
                .find(|(n, r)| r.value == Val::Next && n.chars().eq(std::iter::once(c)))
        });
        match short {
            Some((n, row)) if body.len() > n.len() => (
                Word::Opt {
                    id: row.id,
                    value: Some(body[n.len()..].to_string()),
                    accepted: row.accepted,
                },
                1,
            ),
            _ => (Word::Unknown, 1),
        }
    }
}

/// `--odr-table`'s value (§13.1; default `off`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OdrTable {
    Off,
    Exported,
    Full,
}

/// The build id the line asks for (`spec/image.md` I-69; the last of
/// `--build-id[=STYLE]` and `--no-build-id` wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildId {
    /// No option, `--build-id=none` or `--no-build-id`: no note.
    None,
    /// A style by its I-69 name: `default` (bare `--build-id`), `fast`,
    /// `sha1`, `sha256`, or another value (`md5`, `uuid`, an unknown
    /// word), which mortar refuses.
    Style(String),
    /// `0xHEX`: the user's bytes, not a hash.
    Given,
}

/// The options §3.2a's rules read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent options of the line
pub struct Options {
    /// `--fix-cortex-a53-843419` (with or without a value) was given;
    /// `--no-fix-cortex-a53-843419`, which no linker of cli.md has, is
    /// not a word of an accepted line (M9).
    pub fix_843419: bool,
    /// The last of `--gdb-index` and `--no-gdb-index` (§6.10).
    pub gdb_index: bool,
    /// `-s`/`--strip-all` or `-S`/`--strip-debug` (synth.md S-G8: the
    /// debug information is stripped).
    pub strip_debug: bool,
    /// `--keep-mason-meta` (§13.1).
    pub keep_mason_meta: bool,
    /// The last `--odr-table` (§13.1).
    pub odr_table: OdrTable,
    /// The last of `--odr=M` and `--no-odr-check` is `off` (§13.1).
    pub odr_off: bool,
    /// The last of `--build-id[=STYLE]` and `--no-build-id` (§6.7).
    pub build_id: BuildId,
    /// The last of `-z execstack` (true) and `-z noexecstack` (false).
    pub execstack: Option<bool>,
    /// `-s`/`--strip-all` (synth.md S-84: no `.symtab`).
    pub strip_all: bool,
    /// The last of `--gc-sections` and `--no-gc-sections` (`spec/image.md`
    /// I-5, I-6): §3.3's M10 reads it (a `discard gc` record, WP-143).
    pub gc_sections: bool,
    /// `-T`/`--script`/`--default-script` (a `discard script` record's
    /// reason, M10).
    pub script: bool,
    /// The last `-z max-page-size=N` (layout.md L-74; `N = 0` means the
    /// default), when its value reads as a number.
    pub max_page: Option<u64>,
    /// Words that no accepted option spells, in order, with their index
    /// in the line (M9).
    pub unread: Vec<(usize, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fix_843419: false,
            gdb_index: false,
            strip_debug: false,
            keep_mason_meta: false,
            odr_table: OdrTable::Off,
            odr_off: false,
            build_id: BuildId::None,
            execstack: None,
            strip_all: false,
            gc_sections: false,
            script: false,
            max_page: None,
            unread: Vec::new(),
        }
    }
}

fn unread(o: &mut Options, at: usize, w: &[u8]) {
    o.unread.push((at, String::from_utf8_lossy(w).into_owned()));
}

/// Whether a spelling's name may be written with `dashes` (cli.md §2.2):
/// a one-letter name with one dash only; a long name starting with `l`,
/// `m` or `o` with two only; every other name with either.
pub(crate) fn dashes_allow(name: &str, dashes: usize) -> bool {
    if name.chars().count() == 1 {
        return dashes == 1;
    }
    if dashes == 1 && name.starts_with(['l', 'm', 'o']) {
        return false;
    }
    true
}

impl Options {
    /// The options of the link's `words` (the program name left out), by
    /// [`Table::word`]: a word no accepted row spells is M9's.
    pub fn read(words: &[Vec<u8>]) -> Options {
        let t = Table::get();
        let mut o = Options::default();
        let mut i = 0;
        while i < words.len() {
            let (w, used) = t.word(words, i);
            match w {
                Word::Input => {}
                Word::Opt { id, value, accepted: true } => o.apply(id, value.as_deref()),
                Word::Z { kw, value, accepted: true } => o.z(kw, value.as_deref()),
                Word::Opt { accepted: false, .. } | Word::Z { .. } | Word::Unknown => {
                    unread(&mut o, i, &words[i]);
                }
            }
            i += used;
        }
        o
    }

    fn z(&mut self, kw: &str, value: Option<&str>) {
        match kw {
            "execstack" => self.execstack = Some(true),
            "noexecstack" => self.execstack = Some(false),
            "max-page-size" => self.max_page = value.and_then(number),
            _ => {}
        }
    }

    /// One option, by its row's first spelling.
    fn apply(&mut self, id: &str, value: Option<&str>) {
        match id {
            "--fix-cortex-a53-843419" => self.fix_843419 = true,
            "--gdb-index" => self.gdb_index = true,
            "--no-gdb-index" => self.gdb_index = false,
            "-s" => {
                self.strip_debug = true;
                self.strip_all = true;
            }
            "-S" => self.strip_debug = true,
            "--gc-sections" => self.gc_sections = true,
            "--no-gc-sections" => self.gc_sections = false,
            "-T" | "--default-script" => self.script = true,
            "--keep-mason-meta" => self.keep_mason_meta = true,
            "--odr-table" => {
                self.odr_table = match value {
                    Some("exported") => OdrTable::Exported,
                    Some("full") => OdrTable::Full,
                    _ => OdrTable::Off,
                };
            }
            "--odr" => self.odr_off = value == Some("off"),
            "--no-odr-check" => self.odr_off = true,
            "--build-id" => {
                self.build_id = match value {
                    None => BuildId::Style("default".into()),
                    Some("none") => BuildId::None,
                    Some(v) if v.starts_with("0x") => BuildId::Given,
                    Some(v) => BuildId::Style(v.into()),
                };
            }
            "--no-build-id" => self.build_id = BuildId::None,
            _ => {}
        }
    }
}

/// A number as GNU's `-z` values spell it: `0x` hex, a leading `0`
/// octal, else decimal.
fn number(v: &str) -> Option<u64> {
    if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else if v.len() > 1 && v.starts_with('0') {
        u64::from_str_radix(&v[1..], 8).ok()
    } else {
        v.parse().ok()
    }
}

/// The options of a mortar map's line (§3.2a), when it has `arg`
/// records.
pub(crate) fn of(map: &crate::Map) -> Option<Options> {
    (map.format == crate::Format::Mortar).then(|| Options::read(&map.args))
}

/// M9 (§3.2a): each word of a mortar map's line that no option mortar
/// accepts spells.
pub(crate) fn unread_findings(opts: &Options) -> Vec<crate::Finding> {
    opts.unread
        .iter()
        .map(|(i, w)| {
            crate::Finding::new(
                crate::report::Rule::Map(9),
                format!(
                    "the map's line, word {i} '{w}': no option mortar accepts spells it \
                     (spec/cli.md §2, docs/status/options.md); the options are read without it"
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    fn words(s: &[&str]) -> Vec<Vec<u8>> {
        s.iter().map(|w| w.as_bytes().to_vec()).collect()
    }

    fn read(s: &[&str]) -> Options {
        Options::read(&words(s))
    }

    // §3.2a: ACCEPTED is docs/status/options.md's rows that are not
    // refused, REFUSED its refused rows, each in the table's order.
    #[test]
    fn s3_2a_accepted_is_the_options_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/status/options.md");
        let text = std::fs::read_to_string(path).unwrap();
        let mut want = String::new();
        let mut refused = String::new();
        for line in text.lines().filter(|l| l.starts_with("| `")) {
            // Cells split at `|` not escaped by a backslash.
            let mut cells = Vec::new();
            let mut cur = String::new();
            let mut prev = ' ';
            for c in line.chars() {
                if c == '|' && prev != '\\' {
                    cells.push(std::mem::take(&mut cur).trim().to_string());
                } else {
                    cur.push(c);
                }
                prev = c;
            }
            let cells = &cells[1..];
            let spellings: Vec<&str> = cells[0].split('`').skip(1).step_by(2).collect();
            let want = if cells[4].starts_with("refused") { &mut refused } else { &mut want };
            let v = if cells[1].is_empty() {
                "No"
            } else if cells[1].starts_with("optional") {
                "Eq"
            } else {
                "Next"
            };
            if let Some(kw) = spellings[0].strip_prefix("-z ") {
                let _ = writeln!(want, "Z {kw} {v}");
            } else {
                let gnu = if cells[2].contains("GNU") { "gnu" } else { "-" };
                let _ = writeln!(want, "R {v} {gnu} {}", spellings.join(" "));
            }
        }
        assert_eq!(
            ACCEPTED, want,
            "ACCEPTED is not the options table; replace it with the right side"
        );
        assert_eq!(
            REFUSED, refused,
            "REFUSED is not the options table's refused rows; replace it with the right side"
        );
    }

    // §3.2a: cli.md §2.4 lets an abbreviation of a GNU long name stand for
    // it. The options read here are reached by their abbreviations only
    // when no accepted spelling of another row is a prefix: the line was
    // accepted, so the abbreviation named one GNU spelling.
    #[test]
    fn s3_2a_abbreviations_name_one_row() {
        assert!(read(&["--fix-cortex-a53-84"]).fix_843419);
        assert!(read(&["-fix-cortex-a53-843"]).fix_843419);
        assert_eq!(read(&["--build"]).build_id, BuildId::Style("default".into()));
        assert_eq!(read(&["--build=sha1"]).build_id, BuildId::Style("sha1".into()));
        assert!(read(&["--strip-a"]).strip_debug);
        assert!(read(&["--strip-de"]).strip_debug);
        // `--strip-d` could be --strip-debug or --strip-discarded: no
        // accepted line has it.
        assert_eq!(read(&["--strip-d"]).unread.len(), 1);
        // lld's and mortar's own names do not abbreviate.
        assert_eq!(read(&["--gdb-ind"]).unread.len(), 1);
        assert_eq!(read(&["--keep-mason"]).unread.len(), 1);
    }

    // §3.2a with cli.md §2.2, §2.3, §2.7, §2.10: forms, values, last wins.
    #[test]
    fn s3_2a_spellings_values_and_last_wins() {
        let o = read(&["--fix-cortex-a53-843419", "-o", "out", "a.o"]);
        assert!(o.fix_843419 && o.unread.is_empty());
        assert!(read(&["--fix-cortex-a53-843419=full"]).fix_843419);
        assert!(!read(&["-o", "--fix-cortex-a53-843419"]).fix_843419, "a value");
        assert!(!read(&["-Map", "--gdb-index"]).gdb_index, "a value");
        assert!(read(&["-Map=x", "--gdb-index"]).gdb_index);
        assert!(read(&["-gdb-index"]).gdb_index);
        assert!(!read(&["--gdb-index", "--no-gdb-index"]).gdb_index);
        assert!(read(&["--no-gdb-index", "--gdb-index"]).gdb_index);
        assert!(read(&["-s"]).strip_debug && read(&["--strip-debug"]).strip_debug);
        assert!(read(&["-strip-all"]).strip_debug);
        // `-odr-table=full` is `-o dr-table=full` (cli.md §2.2).
        let o = read(&["-odr-table=full"]);
        assert_eq!(o.odr_table, OdrTable::Off);
        assert!(o.unread.is_empty());
        assert_eq!(read(&["--odr-table", "full"]).odr_table, OdrTable::Full);
        assert_eq!(read(&["--odr-table=exported"]).odr_table, OdrTable::Exported);
        assert_eq!(read(&["--odr-table=full", "--odr-table=off"]).odr_table, OdrTable::Off);
        assert!(read(&["--no-odr-check"]).odr_off);
        assert!(!read(&["--no-odr-check", "--odr=warn"]).odr_off);
        assert!(read(&["--odr", "off"]).odr_off);
        assert!(read(&["--keep-mason-meta"]).keep_mason_meta);
        assert_eq!(read(&["--build-id"]).build_id, BuildId::Style("default".into()));
        assert_eq!(read(&["--build-id=0xabcd"]).build_id, BuildId::Given);
        assert_eq!(read(&["--build-id", "--no-build-id"]).build_id, BuildId::None);
        assert_eq!(read(&["--build-id=sha1", "--build-id=none"]).build_id, BuildId::None);
        assert_eq!(read(&[]).build_id, BuildId::None);
        assert_eq!(read(&["-z", "execstack"]).execstack, Some(true));
        assert_eq!(read(&["-zexecstack", "-z", "noexecstack"]).execstack, Some(false));
        assert_eq!(read(&["-z", "max-page-size=0x10000"]).unread, Vec::new());
        // Short options with their value joined (step 5).
        let o = read(&["-L/usr/lib", "-lc", "-ofile", "-O2", "-maarch64linux", "-emain"]);
        assert!(o.unread.is_empty(), "{:?}", o.unread);
        // A value may be any bytes.
        let mut w = words(&["-o"]);
        w.push(b"\xff out\n".to_vec());
        w.push(b"--gdb-index".to_vec());
        let o = Options::read(&w);
        assert!(o.gdb_index && o.unread.is_empty());
    }

    // §3.2a, M9: words no accepted option spells.
    #[test]
    fn s3_2a_unread_words() {
        for bad in [
            &["--no-fix-cortex-a53-843419"][..],
            &["--frobnicate"],
            &["--gdb-index=yes"],
            &["-z", "frob"],
            &["-z"],
            &["-o"],
            &["--odr-table"],
            &["-z", "max-page-size"],
        ] {
            assert_eq!(read(bad).unread.len(), 1, "{bad:?}");
        }
        assert_eq!(Options::read(&[b"-\xff".to_vec()]).unread.len(), 1);
        // Inputs are not options.
        assert!(read(&["a.o", "-", "x y.o"]).unread.is_empty());
    }
}
