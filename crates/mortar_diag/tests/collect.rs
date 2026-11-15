//! The collector (`spec/diag.md` §6, §7).

use std::sync::Barrier;

use mortar_diag::{
    Collector, DEFAULT_ERROR_LIMIT, Diagnostic, FileRef, Key, Kind, Place, Policy, RefusalClass,
    Relation, Report, Severity, Style,
};

const LIMIT_LINE: &str =
    "ld: error: too many errors emitted, stopping now (use --error-limit=0 to see all errors)\n";

fn undefined(name: &str) -> Diagnostic {
    Diagnostic::error(Kind::UndefinedSymbol { name: name.to_owned() })
}

fn odd(what: &str) -> Diagnostic {
    Diagnostic::warning(Kind::Malformed { what: what.to_owned() })
}

fn text(report: &Report) -> String {
    report.render(&Style::plain("ld"))
}

fn unlimited() -> Policy {
    Policy { error_limit: 0, ..Policy::default() }
}

fn collect(policy: Policy, items: &[(Key, Diagnostic)]) -> Report {
    let collector = Collector::new(policy);
    for (key, diagnostic) in items {
        collector.emit(*key, diagnostic.clone());
    }
    collector.finish()
}

/// A deterministic shuffle (a linear congruential generator): the tests
/// need many orders, not randomness.
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    for i in (1..items.len()).rev() {
        state =
            state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let j = (state >> 33) as usize % (i + 1);
        items.swap(i, j);
    }
}

// spec: diag.md §6.1 (keys compare by phase, then unit, then seq), §6.2
#[test]
fn report_order_is_by_key() {
    assert!(Key::new(0, 9, 9) < Key::new(1, 0, 0));
    assert!(Key::new(1, 0, 9) < Key::new(1, 1, 0));
    assert!(Key::new(1, 1, 0) < Key::new(1, 1, 1));
    assert_eq!(Key::new(1, 2, 3), Key { phase: 1, unit: 2, seq: 3 });
    let items = [
        (Key::new(2, 0, 0), undefined("f")),
        (Key::new(1, 7, 0), undefined("e")),
        (Key::new(1, 3, 9), undefined("d")),
        (Key::new(1, 3, 2), odd("c")),
        (Key::new(0, u32::MAX, u64::MAX), undefined("b")),
        (Key::new(0, 0, 0), undefined("a")),
    ];
    let report = collect(Policy::default(), &items);
    assert_eq!(
        text(&report),
        "ld: error: undefined symbol: a (undefined reference to `a')\n\
         ld: error: undefined symbol: b (undefined reference to `b')\n\
         ld: warning: c\n\
         ld: error: undefined symbol: d (undefined reference to `d')\n\
         ld: error: undefined symbol: e (undefined reference to `e')\n\
         ld: error: undefined symbol: f (undefined reference to `f')\n"
    );
    let ids: Vec<&str> = report.diagnostics().iter().map(Diagnostic::id).collect();
    assert_eq!(ids[2], "malformed-input");
    assert_eq!(ids.iter().filter(|id| **id == "undefined-symbol").count(), 5);
}

// spec: diag.md §6.2 (2: the same key orders by the plain text, as bytes)
#[test]
fn report_order_breaks_ties_by_text() {
    let key = Key::new(3, 3, 3);
    let mut items = vec![
        (key, undefined("b")),
        (key, undefined("a").with(Relation::ReferencedBy, Place::file("z.o"))),
        (key, undefined("a").with(Relation::ReferencedBy, Place::file("a.o"))),
        (key, undefined("a")),
        (key, odd("undefined symbol: a (undefined reference to `a')")),
        (key, undefined("B")),
        (key, Diagnostic::error(Kind::NoInputFiles)),
    ];
    let expected = "ld: error: no input files\n\
                    ld: error: undefined symbol: B (undefined reference to `B')\n\
                    ld: error: undefined symbol: a (undefined reference to `a')\n\
                    ld: error: undefined symbol: a (undefined reference to `a')\n\
                    >>> referenced by a.o\n\
                    ld: error: undefined symbol: a (undefined reference to `a')\n\
                    >>> referenced by z.o\n\
                    ld: error: undefined symbol: b (undefined reference to `b')\n\
                    ld: warning: undefined symbol: a (undefined reference to `a')\n";
    for seed in 0..20 {
        shuffle(&mut items, seed);
        assert_eq!(text(&collect(Policy::default(), &items)), expected);
    }
}

// spec: diag.md §6.2 (3: the same key and text, different fields: one
// order whatever the order of emission; equal values are both kept)
#[test]
fn report_order_breaks_full_ties_by_fields_and_keeps_duplicates() {
    let key = Key::new(0, 0, 0);
    // Two values with one text.
    let by_relation = odd("x").with(Relation::In, Place::file("a.o"));
    let by_note = odd("x").note("in a.o");
    assert_ne!(by_relation, by_note);
    assert_eq!(by_relation.render(&Style::plain("ld")), by_note.render(&Style::plain("ld")));

    let forward = collect(Policy::default(), &[(key, by_relation.clone()), (key, by_note.clone())]);
    let backward =
        collect(Policy::default(), &[(key, by_note.clone()), (key, by_relation.clone())]);
    assert_eq!(forward, backward);
    assert_eq!(forward.diagnostics().len(), 2);
    assert!(forward.diagnostics().contains(&by_relation));
    assert!(forward.diagnostics().contains(&by_note));

    let twice = collect(Policy::default(), &[(key, by_note.clone()), (key, by_note.clone())]);
    assert_eq!(twice.diagnostics(), [by_note.clone(), by_note]);
    assert_eq!(twice.warning_count(), 2);
    assert_eq!(text(&twice), "ld: warning: x\n>>> in a.o\nld: warning: x\n>>> in a.o\n");
}

/// A link's worth of diagnostics: several phases and units, errors and
/// warnings, shared keys, duplicates.
fn many() -> Vec<(Key, Diagnostic)> {
    let mut items = Vec::new();
    for phase in 0..3u16 {
        for unit in 0..12u32 {
            let file = FileRef::in_archive("libx.a", format!("m{unit}.o"));
            for seq in 0..9u64 {
                let name = format!("sym_{phase}_{unit}_{seq}");
                let place = Place::section(file.clone(), ".text", seq * 4);
                let diagnostic = match seq % 3 {
                    0 => undefined(&name).with(Relation::ReferencedBy, place),
                    1 => odd(&name).at(place),
                    _ => Diagnostic::refused(RefusalClass::RelocationType, name).at(place),
                };
                // Three diagnostics share each key.
                items.push((Key::new(phase, unit, seq / 3), diagnostic));
            }
        }
    }
    // Exact duplicates, and one key for a whole phase.
    for _ in 0..3 {
        items.push((Key::new(1, 5, 1), undefined("again")));
        items.push((Key::new(9, 0, 0), odd("again")));
    }
    items
}

/// The report order, computed the long way round from §6.2: by key, then
/// by text.
fn expected_text(items: &[(Key, Diagnostic)]) -> String {
    let mut lines: Vec<(Key, String)> =
        items.iter().map(|(key, d)| (*key, d.render(&Style::plain("ld")))).collect();
    lines.sort();
    lines.into_iter().map(|(_, text)| text).collect()
}

// spec: diag.md §6.2 (the order depends only on what was emitted), §6.6
// (shared by reference between scoped threads)
#[test]
fn order_does_not_depend_on_threads() {
    let mut items = many();
    let expected = expected_text(&items);
    let sequential = collect(unlimited(), &items);
    assert_eq!(text(&sequential), expected);
    assert_eq!(sequential.diagnostics().len(), items.len());

    for seed in 0..12 {
        shuffle(&mut items, seed);
        let threads = 2 + (seed as usize % 7);
        let collector = Collector::new(unlimited());
        let barrier = Barrier::new(threads);
        std::thread::scope(|scope| {
            for chunk in items.chunks(items.len().div_ceil(threads)) {
                let (collector, barrier) = (&collector, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for (key, diagnostic) in chunk {
                        collector.emit(*key, diagnostic.clone());
                        // Let the other threads in between.
                        std::thread::yield_now();
                    }
                });
            }
        });
        assert!(collector.has_errors());
        let report = collector.finish();
        assert_eq!(report, sequential, "seed {seed}, {threads} threads");
        assert_eq!(text(&report), expected);
    }
}

// spec: diag.md §6.3 (`no_warnings`: dropped, not counted, not shown)
#[test]
fn no_warnings_drops_warnings() {
    let items = [
        (Key::new(0, 0, 0), odd("w1")),
        (Key::new(0, 0, 1), undefined("e1")),
        (Key::new(0, 0, 2), odd("w2")),
    ];
    let report = collect(Policy { no_warnings: true, ..Policy::default() }, &items);
    assert_eq!(text(&report), "ld: error: undefined symbol: e1 (undefined reference to `e1')\n");
    assert_eq!((report.error_count(), report.warning_count()), (1, 0));

    let only_warnings = collect(Policy { no_warnings: true, ..Policy::default() }, &items[..1]);
    assert!(!only_warnings.failed());
    assert!(only_warnings.diagnostics().is_empty());
    assert_eq!(text(&only_warnings), "");
}

// spec: diag.md §6.3 (`fatal_warnings`: an error in text, count and
// outcome; the kind and id stay)
#[test]
fn fatal_warnings_makes_warnings_errors() {
    let collector = Collector::new(Policy { fatal_warnings: true, ..Policy::default() });
    assert!(!collector.has_errors());
    collector.emit(Key::new(0, 0, 0), odd("w1").at(Place::file("a.o")));
    assert!(collector.has_errors());
    let report = collector.finish();
    assert!(report.failed());
    assert_eq!((report.error_count(), report.warning_count()), (1, 0));
    assert_eq!(text(&report), "ld: error: a.o: w1\n");
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.severity(), Severity::Error);
    assert_eq!(diagnostic.id(), "malformed-input");
    assert_eq!(diagnostic.kind(), &Kind::Malformed { what: "w1".to_owned() });

    // Without the switch the same warning does not fail the link.
    let report = collect(Policy::default(), &[(Key::new(0, 0, 0), odd("w1"))]);
    assert!(!report.failed());
    assert_eq!(report.diagnostics()[0].severity(), Severity::Warning);
    assert_eq!(text(&report), "ld: warning: w1\n");
}

// spec: diag.md §6.3 (both set: `no_warnings` wins)
#[test]
fn no_warnings_wins_over_fatal_warnings() {
    let collector =
        Collector::new(Policy { no_warnings: true, fatal_warnings: true, ..Policy::default() });
    collector.emit(Key::new(0, 0, 0), odd("w1"));
    assert!(!collector.has_errors());
    let report = collector.finish();
    assert!(!report.failed());
    assert_eq!((report.error_count(), report.warning_count()), (0, 0));
    assert_eq!(text(&report), "");
}

/// `count` errors `u0`, `u1`, ... with one warning after each, keyed in
/// that order.
fn errors_and_warnings(count: usize) -> Vec<(Key, Diagnostic)> {
    (0..count)
        .flat_map(|i| {
            [
                (Key::new(0, 0, 2 * i as u64), undefined(&format!("u{i}"))),
                (Key::new(0, 0, 2 * i as u64 + 1), odd(&format!("w{i}"))),
            ]
        })
        .collect()
}

// spec: diag.md §6.4 (`E < N` and `E == N`: everything, no limit line)
#[test]
fn limit_below_and_at() {
    for count in [0, 1, 2, 3] {
        let report =
            collect(Policy { error_limit: 3, ..Policy::default() }, &errors_and_warnings(count));
        assert!(!report.truncated());
        assert_eq!(report.diagnostics().len(), 2 * count);
        assert_eq!((report.error_count(), report.warning_count()), (count, count));
        assert!(!text(&report).contains("too many errors"));
        assert_eq!(text(&report).lines().count(), 2 * count);
    }
    let at = collect(Policy { error_limit: 3, ..Policy::default() }, &errors_and_warnings(3));
    assert_eq!(
        text(&at),
        "ld: error: undefined symbol: u0 (undefined reference to `u0')\nld: warning: w0\n\
         ld: error: undefined symbol: u1 (undefined reference to `u1')\nld: warning: w1\n\
         ld: error: undefined symbol: u2 (undefined reference to `u2')\nld: warning: w2\n"
    );
}

// spec: diag.md §6.4 (`E > N`: up to and including the Nth error, later
// warnings cut too, then the limit line; the count is still `E`)
#[test]
fn limit_above() {
    let mut items = errors_and_warnings(4);
    for seed in 0..10 {
        shuffle(&mut items, seed);
        let report = collect(Policy { error_limit: 3, ..Policy::default() }, &items);
        assert!(report.truncated());
        assert!(report.failed());
        assert_eq!((report.error_count(), report.warning_count()), (4, 4));
        assert_eq!(report.diagnostics().len(), 5);
        assert_eq!(
            text(&report),
            format!(
                "ld: error: undefined symbol: u0 (undefined reference to `u0')\nld: warning: w0\n\
                 ld: error: undefined symbol: u1 (undefined reference to `u1')\nld: warning: w1\n\
                 ld: error: undefined symbol: u2 (undefined reference to `u2')\n{LIMIT_LINE}"
            )
        );
    }
    let one = collect(Policy { error_limit: 1, ..Policy::default() }, &errors_and_warnings(25));
    assert_eq!(
        text(&one),
        format!("ld: error: undefined symbol: u0 (undefined reference to `u0')\n{LIMIT_LINE}")
    );
    assert_eq!(one.error_count(), 25);
}

// spec: diag.md §6.4 (`0` means all)
#[test]
fn limit_zero_is_unlimited() {
    let report = collect(unlimited(), &errors_and_warnings(500));
    assert!(!report.truncated());
    assert_eq!(report.diagnostics().len(), 1000);
    assert_eq!(report.error_count(), 500);
    assert!(!text(&report).contains("too many errors"));
}

// spec: diag.md §6.4 (the default is 20, as lld)
#[test]
fn limit_default_is_20() {
    assert_eq!(DEFAULT_ERROR_LIMIT, 20);
    assert_eq!(
        Policy::default(),
        Policy { error_limit: 20, fatal_warnings: false, no_warnings: false }
    );
    assert_eq!(Collector::default().policy(), Policy::default());
    let errors: Vec<_> =
        (0..25).map(|i| (Key::new(0, 0, i), undefined(&format!("u{i}")))).collect();
    let exactly = collect(Policy::default(), &errors[..20]);
    assert!(!exactly.truncated());
    assert_eq!(text(&exactly).lines().count(), 20);
    let more = collect(Policy::default(), &errors[..21]);
    assert!(more.truncated());
    let out = text(&more);
    assert_eq!(out.lines().count(), 21);
    assert!(out.ends_with(&format!(
        "ld: error: undefined symbol: u19 (undefined reference to `u19')\n{LIMIT_LINE}"
    )));
    assert_eq!(text(&collect(Policy::default(), &errors)), out);
}

// spec: diag.md §6.4 (warnings do not count and are not limited; a
// promoted warning counts)
#[test]
fn limit_counts_errors_only_and_promoted_warnings() {
    let warnings: Vec<_> =
        (0..50).map(|i| (Key::new(0, 0, 2 * i), odd(&format!("w{i}")))).collect();
    let report = collect(Policy { error_limit: 2, ..Policy::default() }, &warnings);
    assert!(!report.truncated() && !report.failed());
    assert_eq!(report.diagnostics().len(), 50);

    let fatal = Policy { error_limit: 2, fatal_warnings: true, no_warnings: false };
    let report = collect(fatal, &warnings);
    assert!(report.truncated() && report.failed());
    assert_eq!(report.error_count(), 50);
    assert_eq!(text(&report), format!("ld: error: w0\nld: error: w1\n{LIMIT_LINE}"));

    // Warnings before the cut stay, however many; warnings after it go.
    let mut items = warnings;
    items.push((Key::new(0, 0, 81), undefined("first")));
    items.push((Key::new(0, 0, 91), undefined("second")));
    items.push((Key::new(0, 0, 95), undefined("third")));
    let report = collect(Policy { error_limit: 2, ..Policy::default() }, &items);
    let out = text(&report);
    assert!(out.ends_with(&format!(
        "ld: warning: w45\nld: error: undefined symbol: second (undefined reference to `second')\n{LIMIT_LINE}"
    )));
    assert_eq!(report.diagnostics().len(), 46 + 2);
    assert_eq!((report.error_count(), report.warning_count()), (3, 50));
}

// spec: diag.md §6.4 (the errors shown are the first N in report order,
// whatever the order of emission and however many are dropped on the way),
// §6.6
#[test]
fn limit_shows_the_first_errors_under_threads() {
    const ERRORS: u64 = 5000;
    let limit = 7;
    let mut items: Vec<(Key, Diagnostic)> = (0..ERRORS)
        .map(|i| {
            // Every key is shared by four errors, so the cut falls inside
            // a group of ties.
            (Key::new(0, (i / 4) as u32, 0), undefined(&format!("u{i:05}")))
        })
        .collect();
    items.push((Key::new(0, 0, 0), odd("early")));
    items.push((Key::new(0, 1, 0), odd("at the cut")));
    items.push((Key::new(0, 2, 0), odd("late")));
    let expected = format!(
        "ld: error: undefined symbol: u00000 (undefined reference to `u00000')\nld: error: undefined symbol: u00001 (undefined reference to `u00001')\n\
         ld: error: undefined symbol: u00002 (undefined reference to `u00002')\nld: error: undefined symbol: u00003 (undefined reference to `u00003')\n\
         ld: warning: early\n\
         ld: error: undefined symbol: u00004 (undefined reference to `u00004')\nld: error: undefined symbol: u00005 (undefined reference to `u00005')\n\
         ld: error: undefined symbol: u00006 (undefined reference to `u00006')\n{LIMIT_LINE}"
    );
    let policy = Policy { error_limit: limit, ..Policy::default() };
    // Ascending, descending, shuffled: one thread.
    assert_eq!(text(&collect(policy, &items)), expected);
    items.reverse();
    assert_eq!(text(&collect(policy, &items)), expected);
    for seed in 0..6 {
        shuffle(&mut items, seed);
        let sequential = collect(policy, &items);
        assert_eq!(text(&sequential), expected);
        let collector = Collector::new(policy);
        std::thread::scope(|scope| {
            for chunk in items.chunks(items.len().div_ceil(8)) {
                let collector = &collector;
                scope.spawn(move || {
                    for (key, diagnostic) in chunk {
                        collector.emit(*key, diagnostic.clone());
                    }
                });
            }
        });
        let report = collector.finish();
        assert_eq!(report, sequential);
        assert_eq!(report.error_count(), ERRORS as usize);
        assert_eq!(report.warning_count(), 3);
        assert!(report.truncated());
    }
}

// spec: diag.md §6.5
#[test]
fn report_of_nothing_and_of_warnings_only() {
    let empty = Collector::default().finish();
    assert!(!empty.failed() && !empty.truncated());
    assert!(empty.diagnostics().is_empty());
    assert_eq!((empty.error_count(), empty.warning_count()), (0, 0));
    assert_eq!(text(&empty), "");

    let warned = collect(Policy::default(), &[(Key::new(0, 0, 0), odd("w"))]);
    assert!(!warned.failed());
    assert_eq!((warned.error_count(), warned.warning_count()), (0, 1));
    assert_eq!(text(&warned), "ld: warning: w\n");
}

// spec: diag.md §6.5 (the report's text is its diagnostics' texts in
// order; the style is applied to each)
#[test]
fn report_renders_each_diagnostic_with_the_style() {
    let items = [
        (Key::new(0, 0, 1), undefined("_Z1fv").with(Relation::ReferencedBy, Place::file("a.o"))),
        (Key::new(0, 0, 0), odd("w")),
    ];
    let report = collect(Policy::default(), &items);
    let demangle = |name: &str| (name == "_Z1fv").then(|| "f()".to_owned());
    let style = Style { prog: "mortar", color: true, demangle: &demangle };
    let each: String = report.diagnostics().iter().map(|d| d.render(&style)).collect();
    assert_eq!(report.render(&style), each);
    assert_eq!(
        each,
        "mortar: \x1b[0;35mwarning:\x1b[0m w\n\
         mortar: \x1b[0;31merror:\x1b[0m undefined symbol: f() (undefined reference to `f()')\n\
         >>> mangled name: _Z1fv\n\
         >>> referenced by a.o\n"
    );
}

// spec: diag.md §7 (true from the first error on, false for warnings)
#[test]
fn has_errors_follows_emission() {
    let collector = Collector::default();
    assert!(!collector.has_errors());
    collector.emit(Key::new(0, 0, 0), odd("w"));
    assert!(!collector.has_errors());
    collector.emit(Key::new(5, 0, 0), undefined("x"));
    assert!(collector.has_errors());
    collector.emit(Key::new(0, 0, 1), odd("w"));
    assert!(collector.has_errors());
    let report = collector.finish();
    assert!(report.failed());
    assert_eq!((report.error_count(), report.warning_count()), (1, 2));

    let refused = Collector::default();
    refused.emit(Key::new(0, 0, 0), Diagnostic::refused(RefusalClass::Feature, "linking"));
    assert!(refused.has_errors());
}

// spec: diag.md §7 (`has_errors` stays true when the limit drops errors;
// `failed` is true exactly when an error was emitted)
#[test]
fn has_errors_is_seen_by_every_thread_after_the_phase() {
    let collector = Collector::new(Policy { error_limit: 1, ..Policy::default() });
    std::thread::scope(|scope| {
        for unit in 0..8u32 {
            let collector = &collector;
            scope.spawn(move || {
                for seq in 0..200 {
                    collector.emit(Key::new(0, unit, seq), undefined("x"));
                    assert!(collector.has_errors());
                }
            });
        }
    });
    assert!(collector.has_errors());
    let report = collector.finish();
    assert!(report.failed());
    assert_eq!(report.error_count(), 1600);
    assert_eq!(report.diagnostics().len(), 1);
}
