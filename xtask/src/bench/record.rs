//! The results of a bench run and the file they are kept in
//! (`spec/harness.md` "bench", "Results file", "Machine"). A baseline is a
//! results file.

use std::fmt::Write as _;
use std::path::Path;

/// The first line of a results file.
pub const MAGIC: &str = "# mortar-bench 1";

/// The fields of a `workload` line after its first.
pub const WORKLOAD_FIELDS: [&str; 10] = [
    "name",
    "class",
    "objects",
    "archives",
    "shared",
    "others",
    "input_bytes",
    "symbols",
    "output_bytes",
    "input_hash",
];

/// The fields of a `link` line after its first.
pub const LINK_FIELDS: [&str; 15] = [
    "status",
    "workload",
    "linker",
    "threads",
    "runs",
    "wall_us_min",
    "wall_us",
    "wall_us_max",
    "cpu_us",
    "rss_kib_min",
    "rss_kib",
    "rss_kib_max",
    "output_bytes",
    "spread_permille",
    "detail",
];

/// How the links of one linker configuration on one workload ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Every run exited with status 0 and left an output file.
    Ok,
    /// mortar said that it does not link this, and left no output.
    Refused,
    /// A run exceeded a limit of the harness (`spec/harness.md` "Limits"):
    /// not an answer of the linker's, and not a time.
    Killed,
    /// Anything else. Not a time.
    Failed,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Killed => "killed",
            Self::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Ok, Self::Refused, Self::Killed, Self::Failed].into_iter().find(|s| s.text() == text)
    }
}

/// The number of threads a linker was allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Threads {
    /// What the linker chooses by itself.
    Default,
    /// One: `--threads=1`, or a linker that has one thread.
    One,
}

impl Threads {
    pub fn text(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::One => "1",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Default, Self::One].into_iter().find(|t| t.text() == text)
    }
}

/// What the runs of a link that succeeded measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Figures {
    /// Wall-clock time, microseconds: the fastest run, the middle one (the
    /// reported figure) and the slowest.
    pub wall_us_min: u64,
    pub wall_us: u64,
    pub wall_us_max: u64,
    /// User plus system CPU time of the middle run by CPU time,
    /// microseconds; absent without the memory probe.
    pub cpu_us: Option<u64>,
    /// Peak resident set size, KiB: smallest, middle (the reported
    /// figure), largest; absent without the memory probe.
    pub rss_kib: Option<[u64; 3]>,
    /// The size of the output file of the last run.
    pub output_bytes: u64,
}

impl Figures {
    /// How far the middle run is above the fastest, in thousandths of the
    /// fastest.
    pub fn spread_permille(&self) -> u64 {
        permille_above(self.wall_us, self.wall_us_min)
    }
}

/// `(value - base) / base` in thousandths, 0 when `value` is not above.
pub fn permille_above(value: u64, base: u64) -> u64 {
    let above = u128::from(value.saturating_sub(base)) * 1000;
    u64::try_from(above / u128::from(base.max(1))).unwrap_or(u64::MAX)
}

/// One linker configuration on one workload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub status: Status,
    pub workload: String,
    /// `bfd`, `lld`, `mold` or `mortar`.
    pub linker: String,
    pub threads: Threads,
    /// The measured runs the figures are from; 0 unless the status is `ok`.
    pub runs: u32,
    /// Present exactly when the status is `ok`.
    pub figures: Option<Figures>,
    /// What a refusal or a failure said.
    pub detail: String,
}

/// A workload as it was measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkloadInfo {
    pub name: String,
    pub class: String,
    /// The files the link reads, by kind (`spec/harness.md` "Census").
    pub objects: u64,
    pub archives: u64,
    pub shared: u64,
    pub others: u64,
    pub input_bytes: u64,
    /// The `.symtab` entries and the size of GNU ld's output of the
    /// census link.
    pub symbols: u64,
    pub output_bytes: u64,
    pub input_hash: String,
}

/// A results file: a header, the workloads, and their links.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    /// `key`, `value`, in the order of the file; a key appears once.
    pub header: Vec<(String, String)>,
    pub workloads: Vec<WorkloadInfo>,
    pub links: Vec<Link>,
}

impl Results {
    /// The value of a header key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A header value that is a number.
    pub fn number(&self, key: &str) -> Option<u64> {
        self.get(key)?.parse().ok()
    }

    /// Sets a header key, in place if it is there, else at the end.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.header.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.header.push((key.to_owned(), value)),
        }
    }

    pub fn workload(&self, name: &str) -> Option<&WorkloadInfo> {
        self.workloads.iter().find(|w| w.name == name)
    }

    pub fn link(&self, workload: &str, linker: &str, threads: Threads) -> Option<&Link> {
        self.links
            .iter()
            .find(|l| l.workload == workload && l.linker == linker && l.threads == threads)
    }

    /// The names of the linkers that have a link, in the order of their
    /// first.
    pub fn linkers(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for link in &self.links {
            if !names.contains(&link.linker.as_str()) {
                names.push(&link.linker);
            }
        }
        names
    }

    /// The file's text.
    pub fn to_text(&self) -> String {
        let mut text = format!("{MAGIC}\n");
        for (key, value) in &self.header {
            let _ = writeln!(text, "# {key}\t{}", escape(value));
        }
        let _ = writeln!(text, "# workload-fields\t{}", WORKLOAD_FIELDS.join("\t"));
        let _ = writeln!(text, "# link-fields\t{}", LINK_FIELDS.join("\t"));
        for w in &self.workloads {
            let _ = writeln!(
                text,
                "workload\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                escape(&w.name),
                escape(&w.class),
                w.objects,
                w.archives,
                w.shared,
                w.others,
                w.input_bytes,
                w.symbols,
                w.output_bytes,
                escape(&w.input_hash)
            );
        }
        for link in &self.links {
            let number = |n: Option<u64>| n.map_or_else(|| "-".to_owned(), |n| n.to_string());
            let f = link.figures;
            let rss = |at: usize| number(f.and_then(|f| f.rss_kib).map(|rss| rss[at]));
            let _ = writeln!(
                text,
                "link\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                link.status.text(),
                escape(&link.workload),
                escape(&link.linker),
                link.threads.text(),
                link.runs,
                number(f.map(|f| f.wall_us_min)),
                number(f.map(|f| f.wall_us)),
                number(f.map(|f| f.wall_us_max)),
                number(f.and_then(|f| f.cpu_us)),
                rss(0),
                rss(1),
                rss(2),
                number(f.map(|f| f.output_bytes)),
                number(f.map(|f| f.spread_permille())),
                escape(&link.detail)
            );
        }
        text
    }

    /// Reads a file's text. Every error names its line.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines().enumerate().map(|(at, line)| (at + 1, line));
        if lines.next().map(|(_, line)| line) != Some(MAGIC) {
            return Err(format!("line 1: not a results file (expected `{MAGIC}`)"));
        }
        let mut results = Self::default();
        let mut seen_record = false;
        for (at, line) in lines {
            let fail = |what: String| format!("line {at}: {what}");
            if let Some(header) = line.strip_prefix("# ") {
                let (key, value) = header
                    .split_once('\t')
                    .ok_or_else(|| fail("a header line without a value".into()))?;
                let expected = match key {
                    "workload-fields" => Some(WORKLOAD_FIELDS.join("\t")),
                    "link-fields" => Some(LINK_FIELDS.join("\t")),
                    _ => None,
                };
                if let Some(expected) = expected {
                    if value != expected {
                        return Err(fail(format!("the `{key}` are not this reader's")));
                    }
                } else if seen_record {
                    return Err(fail(format!("header key `{key}` after a record")));
                } else if results.get(key).is_some() {
                    return Err(fail(format!("header key `{key}` appears twice")));
                } else {
                    results.header.push((key.to_owned(), unescape(value).map_err(&fail)?));
                }
                continue;
            }
            seen_record = true;
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[0] {
                "workload" => results.workloads.push(parse_workload(&fields[1..]).map_err(&fail)?),
                "link" => results.links.push(parse_link(&fields[1..]).map_err(&fail)?),
                other => return Err(fail(format!("unknown record `{other}`"))),
            }
        }
        Ok(results)
    }

    /// Reads the file at `path`.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Writes the file at `path`, under a temporary name first.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let cannot = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(cannot)?;
        }
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temporary, self.to_text()).map_err(cannot)?;
        std::fs::rename(&temporary, path).map_err(cannot)
    }
}

fn parse_number(field: &str, name: &str) -> Result<u64, String> {
    let digits = !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit());
    field
        .parse()
        .ok()
        .filter(|_| digits)
        .ok_or_else(|| format!("`{name}` is not a number: `{field}`"))
}

fn parse_optional(field: &str, name: &str) -> Result<Option<u64>, String> {
    if field == "-" { Ok(None) } else { parse_number(field, name).map(Some) }
}

fn parse_workload(fields: &[&str]) -> Result<WorkloadInfo, String> {
    let &[
        name,
        class,
        objects,
        archives,
        shared,
        others,
        input_bytes,
        symbols,
        output_bytes,
        input_hash,
    ] = fields
    else {
        return Err(format!("a workload line has {} fields", WORKLOAD_FIELDS.len()));
    };
    Ok(WorkloadInfo {
        name: unescape(name)?,
        class: unescape(class)?,
        objects: parse_number(objects, "objects")?,
        archives: parse_number(archives, "archives")?,
        shared: parse_number(shared, "shared")?,
        others: parse_number(others, "others")?,
        input_bytes: parse_number(input_bytes, "input_bytes")?,
        symbols: parse_number(symbols, "symbols")?,
        output_bytes: parse_number(output_bytes, "output_bytes")?,
        input_hash: unescape(input_hash)?,
    })
}

fn parse_link(fields: &[&str]) -> Result<Link, String> {
    let &[
        status,
        workload,
        linker,
        threads,
        runs,
        min,
        wall,
        max,
        cpu,
        rss_min,
        rss,
        rss_max,
        output,
        spread,
        detail,
    ] = fields
    else {
        return Err(format!("a link line has {} fields", LINK_FIELDS.len()));
    };
    let status = Status::parse(status).ok_or_else(|| format!("unknown status `{status}`"))?;
    let threads = Threads::parse(threads).ok_or_else(|| format!("unknown threads `{threads}`"))?;
    let runs =
        u32::try_from(parse_number(runs, "runs")?).map_err(|_| "too many runs".to_owned())?;
    let walls = [
        parse_optional(min, "wall_us_min")?,
        parse_optional(wall, "wall_us")?,
        parse_optional(max, "wall_us_max")?,
    ];
    let rss = [
        parse_optional(rss_min, "rss_kib_min")?,
        parse_optional(rss, "rss_kib")?,
        parse_optional(rss_max, "rss_kib_max")?,
    ];
    let cpu_us = parse_optional(cpu, "cpu_us")?;
    let output = parse_optional(output, "output_bytes")?;
    // Derived from the times: checked for its form, then ignored.
    parse_optional(spread, "spread_permille")?;
    let figures = match (walls, output) {
        ([Some(wall_us_min), Some(wall_us), Some(wall_us_max)], Some(output_bytes)) => {
            let rss_kib = match rss {
                [Some(a), Some(b), Some(c)] => Some([a, b, c]),
                [None, None, None] => None,
                _ => return Err("the memory figures are incomplete".into()),
            };
            Some(Figures { wall_us_min, wall_us, wall_us_max, cpu_us, rss_kib, output_bytes })
        }
        ([None, None, None], None) if cpu_us.is_none() && rss == [None; 3] => None,
        _ => return Err("the figures are incomplete".into()),
    };
    if (status == Status::Ok) != figures.is_some() || (status == Status::Ok) != (runs > 0) {
        return Err(format!(
            "a link that is `{}` has figures exactly when it is `ok`",
            status.text()
        ));
    }
    Ok(Link {
        status,
        workload: unescape(workload)?,
        linker: unescape(linker)?,
        threads,
        runs,
        figures,
        detail: unescape(detail)?,
    })
}

/// Free text as one field: `\\`, `\t`, `\n`, `\r`; an empty value is `-`,
/// and a value that is exactly `-` is `\-`.
pub fn escape(text: &str) -> String {
    match text {
        "" => "-".to_owned(),
        "-" => "\\-".to_owned(),
        _ => text
            .replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\n', "\\n")
            .replace('\r', "\\r"),
    }
}

/// The inverse of [`escape`].
pub fn unescape(field: &str) -> Result<String, String> {
    match field {
        "-" => return Ok(String::new()),
        "\\-" => return Ok("-".to_owned()),
        _ => {}
    }
    let mut text = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        text.push(match chars.next() {
            Some('\\') => '\\',
            Some('t') => '\t',
            Some('n') => '\n',
            Some('r') => '\r',
            other => {
                return Err(format!(
                    "bad escape `\\{}`",
                    other.map(String::from).unwrap_or_default()
                ));
            }
        });
    }
    Ok(text)
}

/// 64-bit FNV-1a, fed in pieces.
#[derive(Clone, Copy, Debug)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    /// Feeds `bytes` and a NUL, so that two fields never run together.
    pub fn field(&mut self, bytes: &[u8]) {
        self.update(bytes);
        self.update(&[0]);
    }

    /// `fnv1a64:` and 16 lower-case hexadecimal digits.
    pub fn text(self) -> String {
        format!("fnv1a64:{:016x}", self.0)
    }
}

/// What identifies the machine a run was made on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub arch: String,
    pub cpu: String,
    pub cores: usize,
}

impl Machine {
    /// This machine: the architecture the harness was built for, the
    /// processor `/proc/cpuinfo` names, the parallelism available.
    pub fn detect() -> Self {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        Self {
            arch: std::env::consts::ARCH.to_owned(),
            cpu: cpu_name(&cpuinfo),
            cores: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        }
    }

    /// `<arch>-<cpu>-<cores>c` in lower-case letters, digits and single
    /// dashes. It names no host and no user.
    pub fn id(&self) -> String {
        let raw = format!("{}-{}-{}c", self.arch, self.cpu, self.cores);
        let mut id = String::new();
        for c in raw.chars() {
            if c.is_ascii_alphanumeric() {
                id.push(c.to_ascii_lowercase());
            } else if !id.ends_with('-') {
                id.push('-');
            }
        }
        id.trim_matches('-').to_owned()
    }
}

/// The processor's name from the text of `/proc/cpuinfo`: the first `model
/// name`, else `implementer <CPU implementer> part <CPU part>` (Arm), else
/// `unknown`.
pub fn cpu_name(cpuinfo: &str) -> String {
    let value = |key: &str| {
        cpuinfo.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim() == key && !value.trim().is_empty()).then(|| value.trim().to_owned())
        })
    };
    if let Some(model) = value("model name") {
        return model;
    }
    match (value("CPU implementer"), value("CPU part")) {
        (Some(implementer), Some(part)) => format!("implementer {implementer} part {part}"),
        _ => "unknown".to_owned(),
    }
}

/// Seconds since the epoch as `YYYY-MM-DDThh:mm:ssZ`.
pub fn utc_date(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil date from a day count (days since 0000-03-01, 400-year eras).
    let z = days + 719_468;
    let (era, day_of_era) = (z / 146_097, z % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

// spec/harness.md "bench", "Results file" and "Machine".
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn ok(workload: &str, linker: &str, threads: Threads, wall: u64, rss: u64) -> Link {
        Link {
            status: Status::Ok,
            workload: workload.to_owned(),
            linker: linker.to_owned(),
            threads,
            runs: 5,
            figures: Some(Figures {
                wall_us_min: wall - wall / 50,
                wall_us: wall,
                wall_us_max: wall + wall / 10,
                cpu_us: Some(wall * 2),
                rss_kib: Some([rss - 100, rss, rss + 100]),
                output_bytes: 70_432,
            }),
            detail: String::new(),
        }
    }

    pub(crate) fn not_ok(workload: &str, linker: &str, status: Status, detail: &str) -> Link {
        Link {
            status,
            workload: workload.to_owned(),
            linker: linker.to_owned(),
            threads: Threads::Default,
            runs: 0,
            figures: None,
            detail: detail.to_owned(),
        }
    }

    pub(crate) fn info(name: &str, class: &str) -> WorkloadInfo {
        WorkloadInfo {
            name: name.to_owned(),
            class: class.to_owned(),
            objects: 4,
            archives: 2,
            shared: 3,
            others: 1,
            input_bytes: 2_345_678,
            symbols: 1_234,
            output_bytes: 70_000,
            input_hash: "fnv1a64:0123456789abcdef".to_owned(),
        }
    }

    pub(crate) fn sample() -> Results {
        let mut results = Results::default();
        for (key, value) in [
            ("date", "2026-11-14T20:00:00Z"),
            ("machine", "aarch64-implementer-0x61-part-0x000-24c"),
            ("runs", "5"),
            ("calibration", "lld on fnv1a64:00000000000000aa"),
            ("calibration_before_us", "40000"),
            ("calibration_after_us", "41000"),
            ("noisy", "no"),
            ("version.bfd", "GNU ld (GNU Binutils for Debian) 2.44"),
            ("note", "a\ttab, a \\ and\na line"),
            ("empty", ""),
            ("dash", "-"),
        ] {
            results.set(key, value);
        }
        results.workloads = vec![info("hello-c", "small"), info("cxx-medium-g", "medium")];
        results.links = vec![
            ok("hello-c", "bfd", Threads::One, 9_000, 5_000),
            ok("hello-c", "lld", Threads::Default, 6_000, 9_000),
            ok("hello-c", "lld", Threads::One, 5_000, 8_000),
            not_ok(
                "hello-c",
                "mortar",
                Status::Refused,
                "mortar: error: linking is not implemented yet",
            ),
            ok("cxx-medium-g", "bfd", Threads::One, 4_000_000, 900_000),
            not_ok("cxx-medium-g", "lld", Status::Failed, "lld: signal 11"),
        ];
        results
    }

    // "Results file": a file read and written again is the same bytes, and
    // what was written is what is read.
    #[test]
    fn results_file_round_trip() {
        let results = sample();
        let text = results.to_text();
        assert!(text.starts_with("# mortar-bench 1\n# date\t2026-11-14T20:00:00Z\n"), "{text}");
        assert!(text.contains("# note\ta\\ttab, a \\\\ and\\na line\n# empty\t-\n# dash\t\\-\n"));
        assert!(text.contains(
            "workload\thello-c\tsmall\t4\t2\t3\t1\t2345678\t1234\t70000\tfnv1a64:0123456789abcdef\n"
        ));
        assert!(text.contains(
            "link\tok\thello-c\tbfd\t1\t5\t8820\t9000\t9900\t18000\t4900\t5000\t5100\t70432\t20\t-\n"
        ));
        assert!(text.contains(
            "link\trefused\thello-c\tmortar\tdefault\t0\t-\t-\t-\t-\t-\t-\t-\t-\t-\t\
             mortar: error: linking is not implemented yet\n"
        ));
        let read = Results::parse(&text).unwrap();
        assert_eq!(read, results);
        assert_eq!(read.to_text(), text);
        assert_eq!(read.get("runs"), Some("5"));
        assert_eq!(read.number("calibration_before_us"), Some(40_000));
        assert_eq!(read.number("date"), None);
        assert_eq!(read.linkers(), ["bfd", "lld", "mortar"]);
        assert!(read.link("hello-c", "lld", Threads::One).is_some());
        assert!(read.link("hello-c", "mold", Threads::One).is_none());
        assert_eq!(read.workload("cxx-medium-g").map(|w| w.class.as_str()), Some("medium"));

        // Without the memory probe there is a time and no memory.
        let mut bare = results.clone();
        let figures = bare.links[0].figures.as_mut().unwrap();
        (figures.cpu_us, figures.rss_kib) = (None, None);
        assert_eq!(Results::parse(&bare.to_text()).unwrap(), bare);
    }

    // "Results file": files are written under a temporary name and read
    // back; a file that is not there is an error with its name.
    #[test]
    fn results_are_written_and_read() {
        let scratch = mortar_testutil::Scratch::new("bench-record");
        let path = scratch.join("deep/results.tsv");
        sample().write(&path).unwrap();
        assert_eq!(Results::read(&path).unwrap(), sample());
        let names: Vec<_> = std::fs::read_dir(scratch.join("deep"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["results.tsv"]);
        let missing = Results::read(&scratch.join("absent.tsv")).unwrap_err();
        assert!(missing.contains("absent.tsv"), "{missing}");
    }

    // "Results file": what a reader refuses, with the line.
    #[test]
    fn malformed_files() {
        let good = sample().to_text();
        let broken = |from: &str, to: &str| {
            assert!(good.contains(from), "{from}");
            Results::parse(&good.replacen(from, to, 1)).unwrap_err()
        };
        assert!(Results::parse("").unwrap_err().starts_with("line 1: not a results file"));
        assert!(Results::parse("# hod-bench 1\n").is_err());
        assert!(broken("# runs\t5", "# runs").contains("line 4: a header line without a value"));
        assert!(broken("# runs\t5", "# date\t5").contains("`date` appears twice"));
        assert!(
            broken("\tstatus\tworkload", "\tworkload\tstatus")
                .contains("`link-fields` are not this reader's")
        );
        assert!(
            broken("link\tok\thello-c\tbfd", "lnk\tok\thello-c\tbfd")
                .contains("unknown record `lnk`")
        );
        assert!(
            broken("link\tok\thello-c\tbfd", "link\tfine\thello-c\tbfd")
                .contains("unknown status `fine`")
        );
        assert!(broken("\tbfd\t1\t5\t", "\tbfd\t2\t5\t").contains("unknown threads `2`"));
        assert!(broken("\t8820\t9000", "\t8820\t9e3").contains("`wall_us` is not a number"));
        assert!(broken("\t8820\t9000", "\t8820\t-").contains("figures are incomplete"));
        assert!(
            broken("\t4900\t5000\t5100", "\t4900\t-\t5100")
                .contains("memory figures are incomplete")
        );
        assert!(broken("\t70432\t20\t-\n", "\t70432\t20\n").contains("a link line has 15 fields"));
        assert!(broken("\t2345678\t", "\t").contains("a workload line has 10 fields"));
        // A failed or refused link has no figures, and an `ok` one has them.
        assert!(
            broken("link\tok\thello-c\tbfd", "link\tfailed\thello-c\tbfd").contains("exactly when")
        );
        assert!(broken("link\trefused\thello-c", "link\tok\thello-c").contains("exactly when"));
        assert!(
            broken("link\tok\thello-c\tbfd", "link\tkilled\thello-c\tbfd").contains("exactly when")
        );
        assert!(broken("mortar: error", "mortar\\x error").contains("bad escape `\\x`"));
        let late = format!("{good}# late\tkey\n");
        assert!(Results::parse(&late).unwrap_err().contains("header key `late` after a record"));
    }

    // "Results file": the spread is the middle run above the fastest.
    #[test]
    fn spread() {
        assert_eq!(permille_above(1050, 1000), 50);
        assert_eq!(permille_above(1000, 1000), 0);
        assert_eq!(permille_above(900, 1000), 0);
        assert_eq!(permille_above(3, 2), 500);
        assert_eq!(permille_above(5, 0), 5000);
        assert_eq!(
            ok("w", "bfd", Threads::One, 10_000, 5_000).figures.unwrap().spread_permille(),
            20
        );
    }

    // "Census": the hash of a workload's inputs is FNV-1a over fields.
    #[test]
    fn hashes() {
        assert_eq!(Fnv::default().text(), "fnv1a64:cbf29ce484222325");
        let mut hash = Fnv::default();
        hash.update(b"a");
        assert_eq!(hash.text(), "fnv1a64:af63dc4c8601ec8c");
        let (mut joined, mut split) = (Fnv::default(), Fnv::default());
        joined.field(b"ab");
        joined.field(b"");
        split.field(b"a");
        split.field(b"b");
        assert_ne!(joined.text(), split.text());
    }

    // "Machine": the id is made of the architecture, the processor and the
    // parallelism, and names no host.
    #[test]
    fn machine_id() {
        let arm = "processor\t: 0\nBogoMIPS\t: 48.00\nCPU implementer\t: 0x61\n\
                   CPU architecture: 8\nCPU variant\t: 0x1\nCPU part\t: 0x000\n";
        assert_eq!(cpu_name(arm), "implementer 0x61 part 0x000");
        let machine = Machine { arch: "aarch64".into(), cpu: cpu_name(arm), cores: 24 };
        assert_eq!(machine.id(), "aarch64-implementer-0x61-part-0x000-24c");
        let x86 = "processor\t: 0\nmodel name\t: AMD EPYC 7B13 (R) @ 2.2GHz\nmodel name\t: other\n";
        let machine = Machine { arch: "x86_64".into(), cpu: cpu_name(x86), cores: 8 };
        assert_eq!(machine.id(), "x86-64-amd-epyc-7b13-r-2-2ghz-8c");
        assert_eq!(cpu_name("model name\t:\n"), "unknown");
        let here = Machine::detect();
        assert!(here.cores >= 1 && here.id().ends_with(&format!("-{}c", here.cores)));
    }

    // "Results file": the date is UTC.
    #[test]
    fn utc_dates() {
        assert_eq!(utc_date(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_date(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_date(1_794_700_799), "2026-11-14T23:59:59Z");
        assert_eq!(utc_date(1_709_251_200), "2024-03-01T00:00:00Z");
    }
}
