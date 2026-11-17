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
R Eq gnu --sort-common --sort_common
R Next gnu --sort-section
R Next gnu --export-dynamic-symbol
R Next gnu -init
R Next gnu -fini
R No gnu -E --export-dynamic
R No gnu --no-export-dynamic
R No gnu -Bsymbolic
R No gnu -Bsymbolic-functions
R No - --Bsymbolic-non-weak
R No - --Bsymbolic-non-weak-functions
R No gnu -Bno-symbolic
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
struct Row<'t> {
    /// The row's first spelling: its name in [`Options::read`].
    id: &'t str,
    value: Val,
    gnu: bool,
}

/// Each spelling's name, without its dashes, and its row.
type Names = Vec<(String, Row<'static>)>;
/// Each `-z` keyword and whether it takes `=VALUE`.
type Keywords = Vec<(&'static str, bool)>;

/// The accepted rows: the spellings' names and the `-z` keywords.
fn rows() -> (Names, Keywords) {
    let mut names = Vec::new();
    let mut z = Vec::new();
    for line in ACCEPTED.lines() {
        let t: Vec<&'static str> = line.split(' ').collect();
        let val = |s: &str| match s {
            "Next" => Val::Next,
            "Eq" => Val::Eq,
            _ => Val::No,
        };
        match t.as_slice() {
            ["Z", kw, v] => z.push((*kw, val(v) != Val::No)),
            ["R", v, gnu, spellings @ ..] if !spellings.is_empty() => {
                let row = Row { id: spellings[0], value: val(v), gnu: *gnu == "gnu" };
                for s in spellings {
                    names.push((s.trim_start_matches('-').to_string(), row));
                }
            }
            _ => {}
        }
    }
    (names, z)
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
fn dashes_allow(name: &str, dashes: usize) -> bool {
    if name.chars().count() == 1 {
        return dashes == 1;
    }
    if dashes == 1 && name.starts_with(['l', 'm', 'o']) {
        return false;
    }
    true
}

impl Options {
    /// The options of the link's `words` (the program name left out).
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)] // cli.md §2.2's lookup in its order
    pub fn read(words: &[Vec<u8>]) -> Options {
        let (names, z) = rows();
        let mut o = Options::default();
        let mut i = 0;
        while i < words.len() {
            let w = &words[i];
            let at = i;
            i += 1;
            if !w.starts_with(b"-") || w.as_slice() == b"-" {
                continue; // an input (cli.md §2.1)
            }
            let Ok(text) = std::str::from_utf8(w) else {
                // An option's name is ASCII: a word starting with `-` that
                // is not UTF-8 spells no option.
                unread(&mut o, at, w);
                continue;
            };
            // `-z keyword` and `-zkeyword` (cli.md §2.7).
            let kw = if text == "-z" {
                let Some(next) = words.get(i) else {
                    unread(&mut o, at, w);
                    continue;
                };
                i += 1;
                Some(String::from_utf8_lossy(next).into_owned())
            } else {
                text.strip_prefix("-z").filter(|k| !k.is_empty()).map(str::to_string)
            };
            if let Some(kw) = kw {
                let (k, v) = match kw.split_once('=') {
                    Some((k, v)) => (k, Some(v)),
                    None => (kw.as_str(), None),
                };
                match z.iter().find(|(name, _)| *name == k) {
                    Some((_, takes)) if *takes == v.is_some() => o.z(k),
                    _ => unread(&mut o, at, w),
                }
                continue;
            }
            let dashes = if text.starts_with("--") { 2 } else { 1 };
            let body = &text[dashes..];
            let (name, joined) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (body, None),
            };
            // Step 1: an exact name; 3: a short option (cli.md §2.2).
            let exact = names.iter().find(|(n, _)| n == name && dashes_allow(n, dashes));
            // Step 4: an abbreviation of a GNU long name: the line was
            // accepted, so it names exactly one (cli.md §2.4).
            let abbrev = || {
                if name.chars().count() < 2 || !dashes_allow(name, dashes) {
                    return None;
                }
                let mut hits = names.iter().filter(|(n, r)| {
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
            let found = exact.or_else(abbrev);
            let (row, value) = if let Some((_, row)) = found {
                let value = match (row.value, joined) {
                    (Val::No, Some(_)) => {
                        unread(&mut o, at, w);
                        continue;
                    }
                    (Val::No | Val::Eq, None) => None,
                    (_, Some(v)) => Some(v.to_string()),
                    (Val::Next, None) => {
                        let Some(next) = words.get(i) else {
                            unread(&mut o, at, w);
                            continue;
                        };
                        i += 1;
                        Some(String::from_utf8_lossy(next).into_owned())
                    }
                };
                (*row, value)
            } else {
                // Step 5: one dash, a short option, its value joined.
                let short = (dashes == 1).then(|| body.chars().next()).flatten().and_then(|c| {
                    names
                        .iter()
                        .find(|(n, r)| r.value == Val::Next && n.chars().eq(std::iter::once(c)))
                });
                match short {
                    Some((n, row)) if body.len() > n.len() => {
                        (*row, Some(body[n.len()..].to_string()))
                    }
                    _ => {
                        unread(&mut o, at, w);
                        continue;
                    }
                }
            };
            o.apply(row.id, value.as_deref());
        }
        o
    }

    fn z(&mut self, kw: &str) {
        match kw {
            "execstack" => self.execstack = Some(true),
            "noexecstack" => self.execstack = Some(false),
            _ => {}
        }
    }

    /// One option, by its row's first spelling.
    fn apply(&mut self, id: &str, value: Option<&str>) {
        match id {
            "--fix-cortex-a53-843419" => self.fix_843419 = true,
            "--gdb-index" => self.gdb_index = true,
            "--no-gdb-index" => self.gdb_index = false,
            "-s" | "-S" => self.strip_debug = true,
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
    // refused, in the table's order.
    #[test]
    fn s3_2a_accepted_is_the_options_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/status/options.md");
        let text = std::fs::read_to_string(path).unwrap();
        let mut want = String::new();
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
            if cells[4].starts_with("refused") {
                continue;
            }
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
