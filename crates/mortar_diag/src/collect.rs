//! The collector (`spec/diag.md` §6): diagnostics come in from many
//! threads in any order and leave in one order that depends only on what
//! was emitted.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicUsize, Ordering as Atomic};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::model::{Diagnostic, Severity};
use crate::render::{Style, limit_line};

/// The error limit when nothing sets one (`--error-limit`, as lld).
pub const DEFAULT_ERROR_LIMIT: usize = 20;

/// Where a diagnostic belongs in the report (`spec/diag.md` §6.1). Keys
/// compare field by field in this order; the pipeline assigns the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The ordinal of the phase that found the problem.
    pub phase: u16,
    /// The unit of work inside the phase: an input file's ordinal, an
    /// argument's index.
    pub unit: u32,
    /// The position inside the unit: an offset, an index, a sequence
    /// number that the unit's own (sequential) work assigns.
    pub seq: u64,
}

impl Key {
    /// A key from its three parts.
    #[must_use]
    pub const fn new(phase: u16, unit: u32, seq: u64) -> Self {
        Self { phase, unit, seq }
    }
}

/// The switches of a link that change what is reported
/// (`spec/diag.md` §6.3, §6.4). The command line sets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// How many errors a report shows; `0` shows all (`--error-limit`).
    pub error_limit: usize,
    /// Warnings are errors (`--fatal-warnings`).
    pub fatal_warnings: bool,
    /// Warnings are dropped (`--no-warnings`, `-w`). Wins over
    /// `fatal_warnings`.
    pub no_warnings: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self { error_limit: DEFAULT_ERROR_LIMIT, fatal_warnings: false, no_warnings: false }
    }
}

#[derive(Debug)]
struct Entry {
    key: Key,
    diagnostic: Diagnostic,
}

/// The order of a report (`spec/diag.md` §6.2): the key, then the plain
/// text, then the structure. Entries that compare equal are equal in every
/// respect, so no order between them is observable.
fn order(a: &Entry, b: &Entry) -> Ordering {
    a.key.cmp(&b.key).then_with(|| {
        if a.diagnostic == b.diagnostic {
            return Ordering::Equal;
        }
        let plain = Style::plain("");
        a.diagnostic
            .render(&plain)
            .cmp(&b.diagnostic.render(&plain))
            .then_with(|| a.diagnostic.cmp(&b.diagnostic))
    })
}

#[derive(Debug, Default)]
struct Inner {
    errors: Vec<Entry>,
    warnings: Vec<Entry>,
    /// Once more errors than the limit have been seen: the key of the last
    /// error a report could still show. An error with a greater key is
    /// counted and dropped.
    cut: Option<Key>,
}

/// Collects the diagnostics of one link (`spec/diag.md` §6). Shared by
/// reference between the threads of a phase; nothing is printed until the
/// link ends and [`Collector::finish`] puts the diagnostics in order.
#[derive(Debug, Default)]
pub struct Collector {
    policy: Policy,
    /// Errors emitted so far, promoted warnings included.
    errors: AtomicUsize,
    inner: Mutex<Inner>,
}

/// How many errors beyond twice the limit are held before the surplus is
/// dropped: memory stays proportional to the limit, and the sort that
/// finds the surplus runs once per `limit + SLACK` errors.
const SLACK: usize = 64;

impl Collector {
    /// A collector with the given switches.
    #[must_use]
    pub fn new(policy: Policy) -> Self {
        Self { policy, errors: AtomicUsize::new(0), inner: Mutex::default() }
    }

    /// The switches this collector applies.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Reports a diagnostic. `key` says where it belongs in the report;
    /// when it is emitted, and by which thread, does not matter.
    pub fn emit(&self, key: Key, mut diagnostic: Diagnostic) {
        if diagnostic.severity() == Severity::Warning {
            if self.policy.no_warnings {
                return;
            }
            if self.policy.fatal_warnings {
                diagnostic.promote();
            }
        }
        let mut inner = self.lock();
        if diagnostic.severity() == Severity::Warning {
            inner.warnings.push(Entry { key, diagnostic });
            return;
        }
        // Counted under the lock: a reader that sees the diagnostic stored
        // also sees it counted.
        self.errors.fetch_add(1, Atomic::Relaxed);
        if inner.cut.is_some_and(|cut| key > cut) {
            return;
        }
        inner.errors.push(Entry { key, diagnostic });
        let limit = self.policy.error_limit;
        if limit != 0 && inner.errors.len() >= limit.saturating_mul(2).saturating_add(SLACK) {
            // Only the `limit` first errors can be shown, and an error that
            // arrives later can only move the cut earlier.
            inner.errors.sort_by(order);
            inner.errors.truncate(limit);
            inner.cut = inner.errors.last().map(|last| last.key);
        }
    }

    /// Whether an error has been emitted: the link has failed and no
    /// output file may be left (`spec/diag.md` §7). One atomic load; the
    /// phases poll it between their steps.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.errors.load(Atomic::Relaxed) != 0
    }

    /// The warnings emitted so far, with their keys, in report order
    /// (diag.md §6.2), without ending collection: an incremental link's
    /// capture stores them in its state before the link's report is
    /// finished (`spec/incr.md` IN-75a, WP-265). Under `--fatal-warnings`
    /// none (each was made an error), under `-w` none (each was dropped).
    #[must_use]
    pub fn warnings(&self) -> Vec<(Key, Diagnostic)> {
        let inner = self.lock();
        let mut held: Vec<&Entry> = inner.warnings.iter().collect();
        held.sort_by(|a, b| order(a, b));
        held.into_iter().map(|e| (e.key, e.diagnostic.clone())).collect()
    }

    /// Ends collection: the diagnostics in report order, cut at the error
    /// limit.
    #[must_use]
    pub fn finish(self) -> Report {
        let errors = self.errors.into_inner();
        let Inner { errors: mut all, warnings: mut held_warnings, .. } =
            self.inner.into_inner().unwrap_or_else(PoisonError::into_inner);
        let warnings = held_warnings.len();
        all.append(&mut held_warnings);
        all.sort_by(order);
        let limit = self.policy.error_limit;
        let truncated = limit != 0 && errors > limit;
        if truncated {
            let mut seen = 0;
            let end = all.iter().position(|entry| {
                seen += usize::from(entry.diagnostic.severity() == Severity::Error);
                seen == limit
            });
            all.truncate(end.map_or(all.len(), |index| index + 1));
        }
        let keys = all.iter().map(|entry| entry.key).collect();
        Report {
            diagnostics: all.into_iter().map(|entry| entry.diagnostic).collect(),
            keys,
            errors,
            warnings,
            truncated,
        }
    }

    /// A panic of a thread that held the lock is a bug of mortar's, and the
    /// list it left is still a list: the diagnostics collected so far are
    /// worth more to the user than a second panic.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a link reported, in order (`spec/diag.md` §6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    diagnostics: Vec<Diagnostic>,
    /// `diagnostics`' keys, index for index (an incremental link's capture
    /// stores a warning with its key: `spec/incr.md` IN-75a).
    keys: Vec<Key>,
    errors: usize,
    warnings: usize,
    truncated: bool,
}

impl Report {
    /// Whether the link failed: at least one error was emitted. A failed
    /// link leaves no output file (`spec/diag.md` §7).
    #[must_use]
    pub fn failed(&self) -> bool {
        self.errors != 0
    }

    /// The diagnostics to show, in report order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Every warning shown, with its key, in report order (an incremental
    /// link's capture, `spec/incr.md` IN-75a; WP-221 F-7).
    pub fn warnings(&self) -> impl Iterator<Item = (Key, &Diagnostic)> {
        self.keys
            .iter()
            .zip(&self.diagnostics)
            .filter(|(_, d)| d.severity() == Severity::Warning)
            .map(|(k, d)| (*k, d))
    }

    /// How many errors were emitted, warnings made errors included, shown
    /// or not.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.errors
    }

    /// How many warnings were emitted and stayed warnings, shown or not.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.warnings
    }

    /// Whether the error limit cut the report.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The whole report as text, for standard error: each diagnostic, then
    /// the limit line if the report was cut (`spec/diag.md` §5, §6.4).
    #[must_use]
    pub fn render(&self, style: &Style<'_>) -> String {
        let mut out = String::new();
        for diagnostic in &self.diagnostics {
            diagnostic.render_into(style, &mut out);
        }
        if self.truncated {
            limit_line(style, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    fn held(collector: &Collector) -> usize {
        collector.lock().errors.len()
    }

    // spec: diag.md §6.4 (the collector holds at most `2N + 64` errors,
    // whatever arrives and in whatever order; warnings are not limited)
    #[test]
    fn limit_bounds_the_errors_held() {
        for limit in [1usize, 5, 20] {
            let bound = 2 * limit + SLACK;
            let orders: [Box<dyn Fn(u64) -> u64>; 3] = [
                Box::new(|i| i),
                Box::new(|i| 100_000 - i),
                Box::new(|i| i.wrapping_mul(2_654_435_761) % 100_003),
            ];
            for seq_of in orders {
                let collector = Collector::new(Policy { error_limit: limit, ..Policy::default() });
                for i in 0..20_000u64 {
                    let name = format!("u{i}");
                    collector.emit(
                        Key::new(0, 0, seq_of(i)),
                        Diagnostic::error(Kind::UndefinedSymbol { name }),
                    );
                    assert!(held(&collector) <= bound, "{} held, limit {limit}", held(&collector));
                }
                let report = collector.finish();
                assert_eq!(report.error_count(), 20_000);
                assert_eq!(report.diagnostics().len(), limit);
            }
        }
        // One key for everything: nothing can be dropped by key alone, and
        // the bound still holds.
        let collector = Collector::new(Policy { error_limit: 3, ..Policy::default() });
        for i in 0..5_000u64 {
            let name = format!("u{:04}", 4_999 - i);
            collector.emit(Key::new(0, 0, 0), Diagnostic::error(Kind::UndefinedSymbol { name }));
            assert!(held(&collector) <= 2 * 3 + SLACK);
        }
        let report = collector.finish();
        let shown: Vec<_> = report.diagnostics().iter().map(|d| d.kind().symbol()).collect();
        assert_eq!(shown, [Some("u0000"), Some("u0001"), Some("u0002")]);
    }

    // spec: incr.md IN-75a, diag.md §6.2 (WP-265: the warnings read
    // before `finish`, in report order, equal the report's; errors and
    // the policy's dropped warnings are not among them)
    #[test]
    fn warnings_before_finish_are_the_reports() {
        let warn = |n: &str| Diagnostic::warning(Kind::UndefinedSymbol { name: n.to_owned() });
        let collector = Collector::new(Policy::default());
        collector.emit(Key::new(70, 2, 5), warn("b"));
        collector.emit(Key::new(70, 1, 9), warn("c"));
        collector.emit(Key::new(70, 2, 5), warn("a"));
        collector.emit(Key::new(10, 0, 0), Diagnostic::error(Kind::NoInputFiles));
        let early = collector.warnings();
        let names: Vec<_> = early.iter().map(|(_, d)| d.kind().symbol()).collect();
        assert_eq!(names, [Some("c"), Some("a"), Some("b")]);
        // Reading does not end collection: a later warning is still taken.
        collector.emit(Key::new(70, 3, 0), warn("d"));
        assert_eq!(collector.warnings().len(), 4);
        let report = collector.finish();
        let late: Vec<(Key, Diagnostic)> = report.warnings().map(|(k, d)| (k, d.clone())).collect();
        assert_eq!(late[..3], early[..]);
        assert_eq!(late.len(), 4);
        for policy in [
            Policy { fatal_warnings: true, ..Policy::default() },
            Policy { no_warnings: true, ..Policy::default() },
        ] {
            let c = Collector::new(policy);
            c.emit(Key::new(70, 0, 0), warn("x"));
            assert!(c.warnings().is_empty(), "{policy:?}");
        }
    }

    // spec: diag.md §6.4 (`0`: nothing is dropped)
    #[test]
    fn no_limit_holds_everything() {
        let collector = Collector::new(Policy { error_limit: 0, ..Policy::default() });
        for i in 0..1_000u64 {
            collector.emit(Key::new(0, 0, i), Diagnostic::error(Kind::NoInputFiles));
        }
        assert_eq!(held(&collector), 1_000);
    }
}
