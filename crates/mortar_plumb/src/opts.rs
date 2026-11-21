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
/// `R <value> <gnu> <at-r> <spelling>...` with `<value>` `No`, `Next` (a
/// required value: `=` or the next word) or `Eq` (optional, `=` only),
/// `<gnu>` `gnu` for a row GNU ld has (its long names abbreviate,
/// cli.md §2.4) and `<at-r>` the row's class at `-r` (its "At `-r`"
/// column, `spec/cli.md` C-R2b; [`AtR`], WP-210); `Z <keyword> <value>
/// <at-r>` for a `-z` keyword.
pub const ACCEPTED: &str = "\
R Next gnu acts -o --output
R No gnu conflict -shared --Bshareable
R No gnu conflict -pie --pic-executable
R No gnu noop -no-pie --no-pic-executable
R No gnu acts -r -i --relocatable
R Next gnu noop -m
R Next gnu acts -e --entry
R No gnu acts -Ur
R No gnu acts -s --strip-all
R No gnu acts -S --strip-debug
R No gnu noop --strip-discarded
R No gnu acts -x --discard-all
R No gnu noop -X --discard-locals
R No gnu noop --discard-none
R Eq gnu acts --build-id
R No - noop --no-build-id
R Next gnu refused --compress-debug-sections
R No gnu noop --eh-frame-hdr
R No gnu noop --no-eh-frame-hdr
R No gnu refused --gc-sections
R No gnu noop --no-gc-sections
R No gnu noop --print-gc-sections
R No gnu noop --no-print-gc-sections
R No gnu noop --gc-keep-exported
R No - refused --gdb-index
R No - noop --no-gdb-index
R No - noop --no-debug-names
R Next gnu noop -l --library
R No gnu noop -( --start-group
R No gnu noop -) --end-group
R No - final --start-lib
R No - final --end-lib
R No gnu noop --as-needed
R No gnu noop --no-as-needed
R No gnu noop --whole-archive
R No gnu noop --no-whole-archive
R No gnu noop -Bstatic --dn --non_shared --static
R No gnu noop -Bdynamic --dy --call_shared
R No gnu noop --no-copy-dt-needed-entries --no-add-needed
R No gnu noop --push-state
R No gnu noop --pop-state
R No - noop --no-dependent-libraries
R No - final --warn-backrefs
R No - final --no-warn-backrefs
R Next gnu noop -L --library-path
R Next gnu final --sysroot
R Next gnu noop -rpath-link
R Next gnu noop --version-script
R Next gnu conflict --dynamic-list
R Eq gnu noop --sort-common --sort_common
R Next gnu noop --sort-section
R Next gnu acts -u --undefined
R Next gnu acts --require-defined
R Next gnu acts --defsym
R Next gnu refused --wrap
R Next gnu conflict --export-dynamic-symbol
R Next gnu noop --exclude-libs
R Next gnu conflict -init
R Next gnu conflict -fini
R No gnu noop --undefined-version
R No gnu noop --no-undefined-version
R No gnu conflict -E --export-dynamic
R No gnu noop --no-export-dynamic
R No gnu conflict -Bsymbolic
R No gnu conflict -Bsymbolic-functions
R No gnu noop -Bno-symbolic
R Next gnu conflict -h -soname
R Next gnu noop -rpath
R Next gnu noop -I --dynamic-linker
R No gnu noop --no-dynamic-linker
R No gnu conflict --enable-new-dtags
R No gnu noop --disable-new-dtags
R Next gnu noop --hash-style
R No gnu noop --rosegment
R Next - final --error-limit
R No gnu final --fatal-warnings
R No gnu final --no-fatal-warnings
R No gnu final -w --no-warnings
R Eq - final --color-diagnostics
R No - final --no-color-diagnostics
R No gnu noop --no-undefined
R No gnu final --allow-multiple-definition
R No - final --no-allow-multiple-definition
R No gnu noop --warn-common
R No - final --no-warn-common
R No gnu final --warn-execstack
R Next - final --odr
R No - final --no-odr-check
R No - final --odr-stats
R No - final --odr-check-archives
R Next - noop --odr-table
R Next - noop --odr-runtime
R No - noop --keep-mason-meta
R No - noop --detect-odr-violations
R No gnu final --version
R No gnu final -v
R No gnu final -V
R No gnu final --help
R Next gnu noop -Map
R No gnu final -M --print-map
R Eq gnu final --verbose
R Next gnu final --dependency-file
R Eq - noop --threads
R Next - noop --mortar-map
R No - final --time-phases
R Eq - acts --incremental
R No - noop --no-incremental
R Next - acts --incremental-state
R Eq - acts --incremental-report
R Next gnu noop -plugin
R Next gnu noop -plugin-opt
R No gnu noop -plugin-save-temps
R No gnu noop -flto
R Next gnu noop -flto-partition
R Next gnu noop -fuse-ld
R Eq gnu noop --fix-cortex-a53-843419
Z now No noop
Z lazy No noop
Z relro No noop
Z norelro No noop
Z execstack No acts
Z noexecstack No acts
Z text No noop
Z separate-code No noop
Z noseparate-code No noop
Z defs No noop
Z max-page-size Next noop
Z common-page-size Next noop
Z sectionheader No noop
Z muldefs No final
Z nostart-stop-gc No noop
Z initfirst No noop
Z interpose No noop
Z nodelete No noop
Z nodlopen No noop
Z origin No noop
Z memory-seal No refused
Z nomemory-seal No noop
R No gnu noop -EL
R No gnu noop -g
R Next gnu noop -O
R No gnu noop -Qy
R No gnu noop -qmagic
R Next gnu noop -assert
R No gnu noop --map-whole-files
R No gnu noop --no-map-whole-files
R No gnu noop --no-keep-memory
R No gnu noop --reduce-memory-overheads
R Next gnu noop --hash-size
R Next gnu noop --max-cache-size
";

/// The rows of `docs/status/options.md` whose status is `refused`, in
/// the same form as [`ACCEPTED`] (a test holds it equal to the table).
/// No word of mortar's map spells one (M9), but a reference linker's line
/// may (the caller's words, §12.1; `realbuild` judges the control
/// linker's output of a line mortar refused): whether such an option
/// takes the next word is the table's, as for every row (WP-131, V6).
pub const REFUSED: &str = "\
R Next gnu final --oformat
R No gnu acts -d --dc --dp
R No gnu final --no-define-common
R No gnu noop -q --emit-relocs
R No gnu final --no-strip-discarded
R No gnu final -n --nmagic
R No gnu final -N --omagic
R No gnu final --no-omagic
R No - final --no-nmagic
R Eq gnu final --package-metadata
R Next - refused --icf
R No - final --no-icf
R No - final --print-icf-sections
R No - final --no-print-icf-sections
R Next - final --keep-unique
R No - final --ignore-data-address-equality
R No - final --ignore-function-address-equality
R No - final --debug-names
R No gnu refused --relax
R No gnu noop --no-relax
R No gnu acts --force-group-allocation
R No gnu final --traditional-format
R No gnu final --embedded-relocs
R No gnu final --force-exe-suffix
R Next gnu final --out-implib
R No - final --execute-only
R No - final --no-execute-only
R Next - final --compress-sections
R Eq - final --separate-debug-file
R No - final --no-separate-debug-file
R Next gnu final -R --just-symbols
R Next gnu acts -b --format
R No gnu final --copy-dt-needed-entries --add-needed
R No gnu final --accept-unknown-input-arch
R No gnu final --no-accept-unknown-input-arch
R No gnu final --no-warn-mismatch
R No gnu final --no-warn-search-mismatch
R Next gnu final --remap-inputs-file
R Next gnu final --remap-inputs
R No - final --dependent-libraries
R No - final --fortran-common
R No - final --no-fortran-common
R Next - final --warn-backrefs-exclude
R Next - final --why-extract
R Next - final --print-archive-stats
R No - final --fat-lto-objects
R No - final --no-fat-lto-objects
R No gnu final -nostdlib
R Next gnu final -Y
R Next - final -C --directory
R Next - final --chroot
R Next gnu acts -T --script
R Next gnu final --default-script --dT
R Next gnu final -c --mri-script
R Next gnu conflict --export-dynamic-symbol-list
R Next gnu final --retain-symbols-file
R Next gnu final --section-ordering-file
R Next - final --symbol-ordering-file
R No - final --no-warn-symbol-ordering
R No - final --warn-symbol-ordering
R Next - final --call-graph-ordering-file
R Next - noop --call-graph-profile-sort
R Next - final --print-symbol-order
R Next - final --irpgo-profile
R Next - noop --bp-startup-sort
R Next - noop --bp-compression-sort
R No - noop --bp-compression-sort-startup-functions
R No - final --verbose-bp-section-orderer
R Next gnu noop --orphan-handling
R Eq gnu acts --unique
R No gnu final --enable-non-contiguous-regions
R No gnu final --enable-non-contiguous-regions-warnings
R No gnu final --disable-linker-version
R No gnu final --enable-linker-version
R No gnu final --check-sections
R No gnu final --no-check-sections
R Eq - final --shuffle-sections
R No - final --reverse-sections
R Next - final --randomize-section-padding
R Next - final --undefined-glob
R Next gnu final -y --trace-symbol
R Next gnu final --ignore-unresolved-symbol
R Eq gnu final --demangle
R No gnu final --no-demangle
R No gnu final --default-symver
R No gnu final --default-imported-symver
R Next gnu final --version-exports-section
R No gnu final --disable-multiple-abs-defs
R No - final --gnu-unique
R No - final --no-gnu-unique
R Next gnu final --task-link
R No - conflict --Bsymbolic-non-weak
R No - conflict --Bsymbolic-non-weak-functions
R Next - conflict --pack-dyn-relocs
R Next gnu final -f --auxiliary
R Next gnu final -F --filter
R Next gnu final --audit
R Next gnu final -P --depaudit
R No gnu final -Bgroup
R No gnu conflict --dynamic-list-data
R No gnu conflict --dynamic-list-cpp-new
R No gnu conflict --dynamic-list-cpp-typeinfo
R No gnu final --allow-shlib-undefined
R No gnu final --no-allow-shlib-undefined
R Next gnu final --spare-dynamic-tags
R Next - final --spare-program-headers
R No - final --apply-dynamic-relocs
R No - final --use-android-relr-tags
R No - final --no-use-android-relr-tags
R No - final --android-memtag-heap
R No - final --android-memtag-stack
R Next - conflict --android-memtag-mode
R Next gnu final -Ttext
R Next gnu final -Tdata
R Next gnu final -Tbss
R Next gnu final -Ttext-segment
R Next gnu final -Trodata-segment
R Next gnu final -Tldata-segment
R Next gnu final --section-start
R Next gnu final --image-base
R No gnu final --no-rosegment
R No - final --xosegment
R No - final --no-xosegment
R Eq gnu final --split-by-file
R Eq gnu final --split-by-reloc
R Next gnu final -G --gpsize
R No - final --optimize-bb-jumps
R No - final --no-optimize-bb-jumps
R No - final --branch-to-branch
R No - final --no-branch-to-branch
R Next gnu noop --unresolved-symbols
R No gnu final --warn-unresolved-symbols
R No gnu final --error-unresolved-symbols
R No gnu final --noinhibit-exec --noinhibit_exec
R Next gnu final --error-handling-script
R No gnu final --warn-constructors
R No gnu final --warn-multiple-gp
R No gnu final --warn-once
R No gnu final --warn-section-align
R No gnu final --warn-textrel --warn-shared-textrel
R No gnu final --warn-alternate-em
R No gnu final --warn-execstack-objects
R No gnu final --no-warn-execstack
R No gnu final --error-execstack
R No gnu final --no-error-execstack
R No gnu final --warn-rwx-segments
R No gnu final --no-warn-rwx-segments
R No gnu final --error-rwx-segments
R No gnu final --no-error-rwx-segments
R No - final --warn-ifunc-textrel
R No - final --no-warn-ifunc-textrel
R No - final --vs-diagnostics
R Next - final --why-live
R No gnu final --print-map-discarded
R No gnu final --no-print-map-discarded
R No gnu final --print-map-locals
R No gnu final --no-print-map-locals
R No gnu final --cref
R No gnu final -t --trace
R Next - final --reproduce
R No - final --repro
R Next - final --thread-count
R No - final --no-threads
R No - final --verify
R Next - acts --incremental-write
R Next - acts --incremental-sync
R No - acts --incremental-verify
R Next - acts --incremental-changed
R No gnu final --stats
R No - final --perf
R Eq - final --time-trace
R Next - final --time-trace-granularity
R No gnu final --print-memory-usage
R No gnu final --print-output-format
R No gnu final --print-sysroot
R No gnu final --target-help
R No - final --fork
R No - final --no-fork
R No - final --quick-exit
R No - final --no-quick-exit
R No - final --detach
R No - final --no-detach
R No - final --mmap-output-file
R No - final --no-mmap-output-file
R Next - final --rsp-quoting
R Next - final --run
R Next - noop --lto
R Next - noop --lto-O
R Next - noop --lto-CGO
R Next - noop --lto-partitions
R Next - noop --lto-aa-pipeline
R Next - noop --lto-newpm-passes
R No - noop --lto-debug-pass-manager
R No - noop --lto-emit-asm
R No - noop --lto-emit-llvm
R Next - noop --lto-sample-profile
R Next - noop --lto-cs-profile-file
R No - noop --lto-cs-profile-generate
R No - noop --lto-whole-program-visibility
R No - final --no-lto-whole-program-visibility
R No - noop --lto-pgo-warn-mismatch
R No - final --no-lto-pgo-warn-mismatch
R Next - noop --lto-basic-block-sections
R No - noop --lto-basic-block-address-map
R No - final --no-lto-basic-block-address-map
R No - noop --lto-unique-basic-block-section-names
R No - final --no-lto-unique-basic-block-section-names
R No - noop --lto-validate-all-vtables-have-type-infos
R No - final --no-lto-validate-all-vtables-have-type-infos
R Next - noop --lto-known-safe-vtables
R Next - final --load-pass-plugin
R Next - final --mllvm
R Eq - final --save-temps
R Next - final --opt-remarks-filename
R Next - final --opt-remarks-format
R Next - final --opt-remarks-passes
R Next - final --opt-remarks-hotness-threshold
R No - final --opt-remarks-with-hotness
R Next - final --thinlto-cache-dir
R Next - final --thinlto-cache-policy
R Next - final --thinlto-jobs
R Next - final --thinlto-single-module
R Next - final --thinlto-distributor
R Next - final --thinlto-distributor-arg
R Next - final --thinlto-remote-compiler
R Next - final --thinlto-remote-compiler-arg
R Next - final --thinlto-remote-compiler-prepend-arg
R No gnu final --fix-cortex-a53-835769
R No gnu final --no-apply-dynamic-relocs
R No gnu final --pic-veneer
R Next gnu final --stub-group-size
R No gnu final --no-enum-size-warning
R No gnu final --no-wchar-size-warning
R No gnu final --no-pipeline-knowledge
R No gnu final --ld-generated-unwind-info
R No gnu final --no-ld-generated-unwind-info
Z force-bti No refused
Z bti-report Eq noop
Z pac-plt No noop
Z gcs Next refused
Z gcs-report Eq noop
Z gcs-report-dynamic Next noop
Z noextern-protected-data No noop
Z indirect-extern-access No refused
Z noindirect-extern-access No noop
Z dynamic-undefined-weak No noop
Z nodynamic-undefined-weak No noop
Z call-nop Next noop
Z ibtplt No noop
Z ibt No refused
Z shstk No refused
Z cet-report Next final
Z report-relative-reloc No noop
Z x86-64-baseline No final
Z x86-64-v2 No refused
Z x86-64-v3 No refused
Z x86-64-v4 No refused
Z isa-level-report Next noop
Z noreloc-overflow No noop
Z lam-u48 No refused
Z lam-u48-report Next final
Z lam-u57 No refused
Z lam-u57-report Next final
Z lam-report Next final
Z mark-plt No noop
Z nomark-plt No noop
Z notext No noop
Z undefs No noop
Z stack-size Next refused
Z pack-relative-relocs No noop
Z nopack-relative-relocs No noop
Z copyreloc No final
Z nocopyreloc No noop
Z combreloc No noop
Z nocombreloc No noop
Z nosectionheader No conflict
Z unique-symbol No refused
Z nounique-symbol No noop
Z globalaudit No noop
Z start-stop-gc No noop
Z start-stop-visibility Next noop
Z global No noop
Z unique No noop
Z nounique No noop
Z loadfltr No noop
Z nodefaultlib No noop
Z nodump No noop
Z common No noop
Z nocommon No noop
Z dead-reloc-in-nonalloc Next final
Z execute-only-report Next noop
Z force-ibt No final
Z hazardplt No final
Z ifunc-noplt No final
Z keep-text-section-prefix No final
Z nokeep-text-section-prefix No final
Z nognustack No final
Z rel No final
Z rela No final
Z retpolineplt No final
Z rodynamic No final
Z separate-loadable-segments No final
Z wxneeded No final
Z lrodata-after-bss No final
Z nolrodata-after-bss No final
Z zicfilp Next final
Z zicfiss Next final
Z zicfilp-unlabeled-report Next final
Z zicfilp-func-sig-report Next final
Z zicfiss-report Next final
Z execstack-if-needed No final
Z rewrite-endbr No final
Z norewrite-endbr No final
R Next gnu final -a
R Next gnu final -A --architecture
R No gnu final -EB
R No gnu final --dll-verbose
R No gnu final --ctf-variables
R No gnu final --no-ctf-variables
R Next gnu noop --ctf-share-types
R No - final --be8
R No - final --fix-cortex-a8
R No - final --merge-exidx-entries
R No - final --no-merge-exidx-entries
R No - final --target1-abs
R No - final --target1-rel
R Next - final --target2
R No - final --cmse-implib
R Next - final --in-implib
R No - final --relax-gp
R No - final --no-relax-gp
R No - final --pcrel-optimize
R No - final --no-pcrel-optimize
R No - final --toc-optimize
R No - final --no-toc-optimize
R Eq - final --power10-stubs
R No - final --no-power10-stubs
R Next - final --split-stack-adjust-size
";

/// A row's class at `-r` (`docs/status/options.md`'s "At `-r`" column,
/// `spec/cli.md` C-R2b; plumb.md §14.6, WP-210).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtR {
    /// `acts`: the option changes the `-r` output (C-R3), or is refused
    /// with `-r` until its package lands.
    Acts,
    /// `noop`: accepted, no effect (C-R2).
    Noop,
    /// `as in a final link`.
    Final,
    /// `conflicting-options with -r` (C-R1).
    Conflict,
    /// `refused` with `-r` (C-R4, C-R2a).
    Refused,
}

fn at_r(s: &str) -> AtR {
    match s {
        "acts" => AtR::Acts,
        "noop" => AtR::Noop,
        "conflict" => AtR::Conflict,
        "refused" => AtR::Refused,
        _ => AtR::Final,
    }
}

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
    at_r: AtR,
}

/// The whole table (WP-131): each spelling's name, without its dashes,
/// with its row, and each `-z` keyword with whether it takes `=VALUE` and
/// whether it is accepted.
#[derive(Debug)]
pub struct Table {
    names: Vec<(&'static str, Row)>,
    z: Vec<(&'static str, bool, bool)>,
    /// Each `-z` keyword's class at `-r`, in `z`'s order.
    z_at_r: Vec<AtR>,
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
            let mut t = Table { names: Vec::new(), z: Vec::new(), z_at_r: Vec::new() };
            for (text, accepted) in [(ACCEPTED, true), (REFUSED, false)] {
                for line in text.lines() {
                    let w: Vec<&'static str> = line.split(' ').collect();
                    match w.as_slice() {
                        ["Z", kw, v, r] => {
                            t.z.push((*kw, val(v) != Val::No, accepted));
                            t.z_at_r.push(at_r(r));
                        }
                        ["R", v, gnu, r, spellings @ ..] if !spellings.is_empty() => {
                            let row = Row {
                                id: spellings[0],
                                value: val(v),
                                gnu: *gnu == "gnu",
                                accepted,
                                at_r: at_r(r),
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

    /// The class at `-r` of the row whose first spelling is `id` (a
    /// [`Word::Opt`]'s), or of `-z` keyword `kw` when `id` is `-z kw`
    /// ([`Word::Z`]'s, spelled so); `None` for no row (WP-210).
    pub fn at_r(&self, id: &str) -> Option<AtR> {
        if let Some(kw) = id.strip_prefix("-z ") {
            return self.z.iter().position(|z| z.0 == kw).map(|i| self.z_at_r[i]);
        }
        self.names.iter().find(|(_, r)| r.id == id).map(|(_, r)| r.at_r)
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

/// `--odr-runtime`'s value (§13.1, WP-197; default `off`). `Other`: a
/// value mortar refuses (`spec/cli.md` C-ODR-RT), so no output of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OdrRuntime {
    Off,
    Report,
    Fatal,
    Other,
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
    /// The last `-e`/`--entry` (M10's roots of the line, WP-147).
    pub entry: Option<String>,
    /// `-shared` (M10: every export is a root, `spec/resolve.md` §4.7).
    pub shared: bool,
    /// The last of `-E` and `--no-export-dynamic`.
    pub export_dynamic: bool,
    /// The names of `-init`, `-fini` and `--export-dynamic-symbol` (M10's
    /// roots of the line, `spec/resolve.md` §4.7, WP-147).
    pub named_roots: Vec<String>,
    /// The names of `-u`/`--undefined` and `--require-defined`: names the
    /// line mentions (§6.5 Y5's undefined entry, WP-246).
    pub undefined: Vec<String>,
    /// The last `-z max-page-size=N` (layout.md L-74; `N = 0` means the
    /// default), when its value reads as a number.
    pub max_page: Option<u64>,
    /// The `--wrap=SYM` names, in order (`spec/resolve.md` §7.1; the
    /// derivation is `wrap.rs`'s, WP-170).
    pub wrap: Vec<String>,
    /// The `--defsym=SYM=EXPR` operands, in order (`spec/resolve.md`
    /// §7.2; the derivation is `defsym.rs`'s, WP-170 part 2).
    pub defsym: Vec<String>,
    /// Words that no accepted option spells, in order, with their index
    /// in the line (M9).
    pub unread: Vec<(usize, String)>,
    /// The last `--odr-runtime=MODE` (§13.1, WP-197; default `off`).
    pub odr_runtime: OdrRuntime,
    /// Whether the line spells `--odr-table` at all (§13.1: the runtime's
    /// implied table stands only when the line names none; an explicit
    /// last `=off` beside the runtime is a line mortar refuses).
    pub odr_table_given: bool,
    /// The last of `-z now` (true) and `-z lazy` (false), when the line
    /// has either (§13.1: the runtime's `-z now` unless `-z lazy`).
    pub z_now: Option<bool>,
    /// The last of `-x`, `-X` and `--discard-none` (`spec/synth.md` S-78:
    /// which input locals `.symtab` keeps; Y6, WP-246).
    pub discard: Discard,
    /// The last of `--incremental[=MODE]` (true) and `--no-incremental`
    /// (`spec/incr.md` IN-10): the line asks for incremental mode.
    pub incremental: bool,
    /// The options of `spec/incr.md` IN-12 (and IN-12a's `-shared`) that
    /// make an `--incremental` link a plain one, by the spelling §3.3 M10
    /// names, each with the last of the option and its negation (WP-251;
    /// [`Options::incr_plain`]).
    pub in12: Vec<(&'static str, bool)>,
}

/// `spec/synth.md` S-78: the input locals the `-x`/`-X` rule discards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Discard {
    /// None of the three: the `.L` locals of split `Merge` sections.
    #[default]
    Default,
    /// `-X`/`--discard-locals`: every `.L` local.
    Locals,
    /// `-x`/`--discard-all`: every input local, `STT_FILE` included.
    All,
    /// `--discard-none`: none.
    None,
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
            entry: None,
            shared: false,
            export_dynamic: false,
            named_roots: Vec::new(),
            undefined: Vec::new(),
            max_page: None,
            odr_runtime: OdrRuntime::Off,
            odr_table_given: false,
            z_now: None,
            discard: Discard::Default,
            incremental: false,
            in12: Vec::new(),
            wrap: Vec::new(),
            defsym: Vec::new(),
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

    /// The last state of one of IN-12's options (WP-251).
    fn in12(&mut self, name: &'static str, on: bool) {
        match self.in12.iter_mut().find(|(n, _)| *n == name) {
            Some(e) => e.1 = on,
            None => self.in12.push((name, on)),
        }
    }

    /// IN-12's options (and IN-12a's `-shared`) that make an
    /// `--incremental` link plain and that the words alone show, in the
    /// order of their first word: `-r`, `--emit-relocs`, a
    /// `--compress-debug-sections` other than `none`, `--gdb-index`,
    /// `--debug-names`, an `--icf` other than `none`, `--sort-common`,
    /// `--sort-section`, `--start-lib`, `-shared`. A `SECTIONS` script
    /// and the output kind are not read from the words (WP-251).
    pub fn incr_plain(&self) -> Vec<&'static str> {
        self.in12.iter().filter(|&&(_, on)| on).map(|&(n, _)| n).collect()
    }

    /// Whether the line puts the link in incremental mode as far as its
    /// words show (`spec/plumb.md` §3.3 M10, WP-251): `--incremental`
    /// last, and none of IN-12's options the words show.
    pub fn incremental_by_words(&self) -> bool {
        self.incremental && self.in12.iter().all(|&(_, on)| !on)
    }

    fn z(&mut self, kw: &str, value: Option<&str>) {
        match kw {
            "execstack" => self.execstack = Some(true),
            "noexecstack" => self.execstack = Some(false),
            "now" => self.z_now = Some(true),
            "lazy" => self.z_now = Some(false),
            "max-page-size" => self.max_page = value.and_then(number),
            _ => {}
        }
    }

    /// One option, by its row's first spelling.
    fn apply(&mut self, id: &str, value: Option<&str>) {
        match id {
            "--fix-cortex-a53-843419" => self.fix_843419 = true,
            "--gdb-index" => {
                self.gdb_index = true;
                self.in12("--gdb-index", true);
            }
            "--no-gdb-index" => {
                self.gdb_index = false;
                self.in12("--gdb-index", false);
            }
            "-s" => {
                self.strip_debug = true;
                self.strip_all = true;
            }
            "-S" => self.strip_debug = true,
            "-x" => self.discard = Discard::All,
            "-X" => self.discard = Discard::Locals,
            "--discard-none" => self.discard = Discard::None,
            "--gc-sections" => self.gc_sections = true,
            "--no-gc-sections" => self.gc_sections = false,
            "-T" | "--default-script" => self.script = true,
            "-e" => self.entry = value.map(str::to_string),
            "-shared" => {
                self.shared = true;
                self.in12("-shared", true);
            }
            "-E" => self.export_dynamic = true,
            "--no-export-dynamic" => self.export_dynamic = false,
            "-init" | "-fini" | "--export-dynamic-symbol" => {
                self.named_roots.extend(value.map(str::to_string));
            }
            "-u" | "--require-defined" => self.undefined.extend(value.map(str::to_string)),
            "--keep-mason-meta" => self.keep_mason_meta = true,
            "--wrap" => self.wrap.extend(value.map(str::to_string)),
            "--defsym" => self.defsym.extend(value.map(str::to_string)),
            "--odr-table" => {
                self.odr_table_given = true;
                self.odr_table = match value {
                    Some("exported") => OdrTable::Exported,
                    Some("full") => OdrTable::Full,
                    _ => OdrTable::Off,
                };
            }
            "--odr" => self.odr_off = value == Some("off"),
            "--odr-runtime" => {
                self.odr_runtime = match value {
                    Some("report") => OdrRuntime::Report,
                    Some("fatal") => OdrRuntime::Fatal,
                    Some("off") => OdrRuntime::Off,
                    // mortar refuses any other value: the line is not one
                    // it linked (§13.1).
                    _ => OdrRuntime::Other,
                };
            }
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
            "--incremental" => self.incremental = true,
            "--no-incremental" => self.incremental = false,
            "-r" => self.in12("-r", true),
            "-q" => self.in12("--emit-relocs", true),
            "--compress-debug-sections" => {
                self.in12("--compress-debug-sections", value.is_some_and(|v| v != "none"));
            }
            "--debug-names" => self.in12("--debug-names", true),
            "--no-debug-names" => self.in12("--debug-names", false),
            "--icf" => self.in12("--icf", value.is_some_and(|v| v != "none")),
            "--no-icf" => self.in12("--icf", false),
            "--sort-common" => self.in12("--sort-common", true),
            "--sort-section" => self.in12("--sort-section", true),
            "--start-lib" => self.in12("--start-lib", true),
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

/// The options of a mortar output's line (§3.2a): the caller's `line`
/// when it gives one (WP-201: the same rule as §12.1a's), else the map's
/// `arg` records. `None` for a reference's map, whose rules read the
/// caller's line where they read one.
pub(crate) fn of(map: &crate::Map, line: Option<&[String]>) -> Option<Options> {
    (map.format == crate::Format::Mortar).then(|| match line {
        Some(w) => Options::read(&w.iter().map(|x| x.as_bytes().to_vec()).collect::<Vec<_>>()),
        None => Options::read(&map.args),
    })
}

/// M9 (§3.2a): each word of a mortar map's `arg` records that no option
/// mortar accepts spells (the map's own record of its line, whoever gives
/// the options).
pub(crate) fn unread_findings(map: &crate::Map) -> Vec<crate::Finding> {
    if map.format != crate::Format::Mortar {
        return Vec::new();
    }
    Options::read(&map.args)
        .unread
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
            // The "At `-r`" column's class (WP-210).
            let r = [
                ("acts", "acts"),
                ("noop", "noop"),
                ("as in", "final"),
                ("conflicting", "conflict"),
                ("refused", "refused"),
            ]
            .iter()
            .find_map(|(p, k)| cells[5].starts_with(p).then_some(*k))
            .unwrap_or_else(|| panic!("{}: At -r '{}' has no class", spellings[0], cells[5]));
            if let Some(kw) = spellings[0].strip_prefix("-z ") {
                let _ = writeln!(want, "Z {kw} {v} {r}");
            } else {
                let gnu = if cells[2].contains("GNU") { "gnu" } else { "-" };
                let _ = writeln!(want, "R {v} {gnu} {r} {}", spellings.join(" "));
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

    // §14.6 (WP-210): the mirror's class at -r, by first spelling and
    // `-z kw`; every option stage R reads as acting is an `acts` row.
    #[test]
    fn s14_6_the_class_at_r() {
        let t = Table::get();
        for (id, want) in [
            ("-T", AtR::Acts),
            ("--defsym", AtR::Acts),
            ("-z execstack", AtR::Acts),
            ("-no-pie", AtR::Noop),
            ("-pie", AtR::Conflict),
            ("--gc-sections", AtR::Refused),
            ("-z gcs", AtR::Refused),
            ("--warn-backrefs", AtR::Final),
        ] {
            assert_eq!(t.at_r(id), Some(want), "{id}");
        }
        assert_eq!(t.at_r("--no-such-option"), None);
        for id in crate::relocatable::READ_AT_R {
            assert_eq!(t.at_r(id), Some(AtR::Acts), "READ_AT_R's {id} is not an acts row");
        }
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
        // §13.1 (WP-197): --odr-runtime, the last one; the table spelled;
        // the last of -z now and -z lazy.
        assert_eq!(read(&["--odr-runtime=report"]).odr_runtime, OdrRuntime::Report);
        assert_eq!(read(&["--odr-runtime", "fatal"]).odr_runtime, OdrRuntime::Fatal);
        assert_eq!(
            read(&["--odr-runtime=fatal", "--odr-runtime=off"]).odr_runtime,
            OdrRuntime::Off
        );
        assert_eq!(read(&["--odr-runtime=loud"]).odr_runtime, OdrRuntime::Other);
        assert_eq!(read(&[]).odr_runtime, OdrRuntime::Off);
        assert!(!read(&[]).odr_table_given && read(&["--odr-table=off"]).odr_table_given);
        assert_eq!(read(&["-z", "now", "-zlazy"]).z_now, Some(false));
        assert_eq!(read(&["-z", "lazy", "-z", "now"]).z_now, Some(true));
        assert_eq!(read(&["-z", "relro"]).z_now, None);
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
