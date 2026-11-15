//! The statistics of a bench run and the gate (`spec/harness.md` "bench",
//! "Measurement", "Noise" and "The gate"): the middle of N runs, when a
//! difference is a regression, and when the machine was too unsteady for a
//! time to say anything.

use super::record::{Link, Results, Status, Threads, permille_above};

/// The linker the gate judges.
pub const JUDGED: &str = "mortar";

/// The margins of the gate. All relative ones are in thousandths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// A time is slower when it is above the baseline's by more than this
    /// and by more than [`Thresholds::time_floor_us`].
    pub time_permille: u64,
    pub time_floor_us: u64,
    /// The same for peak memory.
    pub memory_permille: u64,
    pub memory_floor_kib: u64,
    /// A time whose middle run is further than this above its fastest is
    /// not steady enough to be called slower.
    pub spread_permille: u64,
    /// The calibration link before and after a run may differ by this
    /// much; beyond it the run is noisy.
    pub calibration_permille: u64,
    /// The calibration link may be this much slower than the baseline's;
    /// beyond it the machine is too busy for a time to be judged.
    pub drift_permille: u64,
}

impl Default for Thresholds {
    /// The contract: 5% and 2 ms of time, 2% and 1 MiB of memory; a
    /// spread of 5%, a calibration difference of 10%, a drift of 25%.
    fn default() -> Self {
        Self {
            time_permille: 50,
            time_floor_us: 2_000,
            memory_permille: 20,
            memory_floor_kib: 1_024,
            spread_permille: 50,
            calibration_permille: 100,
            drift_permille: 250,
        }
    }
}

/// The middle of `values`: of an even number, the lower of the two middle
/// values plus half their difference, rounded down. `None` of nothing.
pub fn median(values: &[u64]) -> Option<u64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let upper = *sorted.get(sorted.len() / 2)?;
    if sorted.len() % 2 == 1 {
        return Some(upper);
    }
    let lower = sorted[sorted.len() / 2 - 1];
    Some(lower + (upper - lower) / 2)
}

/// Is `now` above `base` by more than `permille` thousandths of `base`
/// scaled by `scale.0 / scale.1`, and by more than `floor`?
pub fn exceeds(now: u64, base: u64, permille: u64, floor: u64, scale: (u64, u64)) -> bool {
    // Exact in 128 bits; a product beyond them saturates, which only the
    // extremes reach.
    let allowed = u128::from(base)
        .saturating_mul(u128::from(1000 + permille))
        .saturating_mul(u128::from(scale.0));
    let measured = (u128::from(now) * 1000).saturating_mul(u128::from(scale.1.max(1)));
    measured > allowed && now.saturating_sub(base) > floor
}

/// Did the calibration link, measured before and after the run, differ by
/// more than `bound` thousandths of the faster of the two?
pub fn calibration_differs(before_us: u64, after_us: u64, bound: u64) -> bool {
    let (slow, fast) = (before_us.max(after_us), before_us.min(after_us));
    u128::from(slow) * 1000 > u128::from(fast) * u128::from(1000 + bound)
}

/// What the gate says about one link of the judged linker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Faster or smaller than the baseline by more than the margins.
    Improved,
    /// Slower, larger, or both, by more than the margins. Fails the gate.
    Regressed {
        time: bool,
        memory: bool,
    },
    /// Slower by more than the margin, on a measurement that cannot carry
    /// the claim. Neither a failure of the linker nor a pass.
    Noisy(String),
    /// The baseline linked this workload and the run does not. Fails.
    Lost,
    /// There is nothing to compare with, for the reason given.
    New(&'static str),
    /// Not compared, for the reason given.
    NotJudged(&'static str),
    /// Compared without consequence, for the reason given.
    Advisory(String),
}

impl Verdict {
    /// Fails the gate.
    pub fn fails(&self) -> bool {
        matches!(self, Self::Regressed { .. } | Self::Lost)
    }

    pub fn text(&self) -> String {
        match self {
            Self::Ok => "ok".to_owned(),
            Self::Improved => "improved".to_owned(),
            Self::Regressed { time: true, memory: true } => "REGRESSED (time, memory)".to_owned(),
            Self::Regressed { time: true, memory: false } => "REGRESSED (time)".to_owned(),
            Self::Regressed { .. } => "REGRESSED (memory)".to_owned(),
            Self::Noisy(why) => format!("NOISY: {why}"),
            Self::Lost => "LOST".to_owned(),
            Self::New(why) => format!("new: {why}"),
            Self::NotJudged(why) => format!("not judged: {why}"),
            Self::Advisory(why) => format!("advisory: {why}"),
        }
    }
}

/// What a run and a baseline have in common, for every link of the run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Common {
    /// Why the baseline does not bind the run, if it does not.
    pub advisory: Option<String>,
    /// Why no time of the run can be called slower, if none can.
    pub unsteady: Option<String>,
    /// The calibration link's time now and in the baseline: what the
    /// machine lost since scales the time margin. `None` when either is
    /// unknown or the calibration links are not the same.
    pub calibration: Option<(u64, u64)>,
}

impl Common {
    /// Compares the headers of a run and a baseline.
    pub fn of(run: &Results, base: &Results, thresholds: &Thresholds) -> Self {
        let advisory = match (run.get("machine"), base.get("machine")) {
            (Some(now), Some(then)) if now == then => None,
            (_, then) => Some(format!("another machine ({})", then.unwrap_or("unknown"))),
        };
        let same_link =
            run.get("calibration").is_some() && run.get("calibration") == base.get("calibration");
        let times = |results: &Results| {
            let before = results.number("calibration_before_us")?;
            Some(before.min(results.number("calibration_after_us")?))
        };
        let calibration = times(run).zip(times(base)).filter(|_| same_link);
        let unsteady = if run.get("noisy") == Some("yes") {
            Some("the run is noisy".to_owned())
        } else if base.get("noisy") == Some("yes") {
            Some("the baseline was recorded on a noisy machine".to_owned())
        } else {
            match calibration {
                None => Some("no common calibration link".to_owned()),
                Some((now, then)) if permille_above(now, then) > thresholds.drift_permille => {
                    Some(format!(
                        "the machine is {} slower than at the baseline",
                        percent(permille_above(now, then))
                    ))
                }
                Some(_) => None,
            }
        };
        Self { advisory, unsteady, calibration }
    }
}

/// `12.5%` of thousandths.
pub fn percent(permille: u64) -> String {
    format!("{}.{}%", permille / 10, permille % 10)
}

/// The gate's rule for one link of the judged linker: `now` against the
/// baseline's link of the same workload and threads, `changed` saying
/// whether the workload's inputs are the baseline's.
pub fn judge(
    now: &Link,
    base: Option<&Link>,
    changed: bool,
    common: &Common,
    thresholds: &Thresholds,
) -> Verdict {
    if now.status == Status::Failed {
        return Verdict::NotJudged("the link failed in this run");
    }
    // Killed by a limit where the baseline linked is a lost link (below);
    // anywhere else it is nothing to compare.
    if now.status == Status::Killed && base.is_none_or(|base| base.status != Status::Ok) {
        return Verdict::NotJudged("killed by a limit in this run");
    }
    let Some(base) = base.filter(|base| !matches!(base.status, Status::Failed | Status::Killed))
    else {
        return Verdict::New("the baseline does not have this link");
    };
    if changed {
        return Verdict::Advisory("the workload changed since the baseline".to_owned());
    }
    let (now_figures, base_figures) = match (now.figures, base.figures) {
        (None, None) => return Verdict::NotJudged("refused, as in the baseline"),
        (Some(_), None) => return Verdict::New("mortar links this workload now"),
        (None | Some(_), Some(_)) if common.advisory.is_some() => {
            return Verdict::Advisory(common.advisory.clone().unwrap_or_default());
        }
        (None, Some(_)) => return Verdict::Lost,
        (Some(now), Some(base)) => (now, base),
    };

    let t = thresholds;
    let rss = now_figures.rss_kib.zip(base_figures.rss_kib).map(|(now, base)| (now[1], base[1]));
    let memory = rss.is_some_and(|(now, base)| {
        exceeds(now, base, t.memory_permille, t.memory_floor_kib, (1, 1))
    });
    // A machine that got faster does not tighten the gate.
    let drift = common.calibration.map_or((1, 1), |(now, then)| (now.max(then), then));
    let (wall_now, wall_base) = (now_figures.wall_us, base_figures.wall_us);
    let slower = exceeds(wall_now, wall_base, t.time_permille, t.time_floor_us, drift);
    let spread = now_figures.spread_permille().max(base_figures.spread_permille());
    let unsteady = common.unsteady.clone().or_else(|| {
        (spread > t.spread_permille).then(|| format!("the runs spread by {}", percent(spread)))
    });
    match unsteady {
        // Memory is judged whatever the machine is doing.
        Some(why) if slower && !memory => Verdict::Noisy(why),
        unsteady if memory || (slower && unsteady.is_none()) => {
            Verdict::Regressed { time: slower && unsteady.is_none(), memory }
        }
        _ => {
            let faster = exceeds(wall_base, wall_now, t.time_permille, t.time_floor_us, (1, 1));
            let smaller = rss.is_some_and(|(now, base)| {
                exceeds(base, now, t.memory_permille, t.memory_floor_kib, (1, 1))
            });
            if faster || smaller { Verdict::Improved } else { Verdict::Ok }
        }
    }
}

/// One line of a check: a link of the judged linker and its verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    pub workload: String,
    pub threads: Threads,
    pub now: Link,
    pub base: Option<Link>,
    pub verdict: Verdict,
}

/// A run judged against a baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub common: Common,
    pub judged: Vec<Judged>,
}

impl Check {
    /// Judges every link of the judged linker in `run`, in the run's order.
    pub fn new(run: &Results, base: &Results, thresholds: &Thresholds) -> Self {
        let common = Common::of(run, base, thresholds);
        let judged = run
            .links
            .iter()
            .filter(|link| link.linker == JUDGED)
            .map(|now| {
                let then = base.link(&now.workload, JUDGED, now.threads);
                let hash = |results: &Results| {
                    results.workload(&now.workload).map(|w| w.input_hash.clone())
                };
                let changed = hash(run) != hash(base);
                Judged {
                    workload: now.workload.clone(),
                    threads: now.threads,
                    now: now.clone(),
                    base: then.cloned(),
                    verdict: judge(now, then, changed, &common, thresholds),
                }
            })
            .collect();
        Self { common, judged }
    }

    /// A link regressed or was lost.
    pub fn failed(&self) -> bool {
        self.judged.iter().any(|j| j.verdict.fails())
    }

    /// A link could not be judged for noise.
    pub fn noisy(&self) -> bool {
        self.judged.iter().any(|j| matches!(j.verdict, Verdict::Noisy(_)))
    }
}

// spec/harness.md "bench", "Measurement", "Noise" and "The gate".
#[cfg(test)]
mod tests {
    use super::super::record::tests::{info, not_ok, ok, sample};
    use super::*;

    const T: Thresholds = Thresholds {
        time_permille: 50,
        time_floor_us: 2_000,
        memory_permille: 20,
        memory_floor_kib: 1_024,
        spread_permille: 50,
        calibration_permille: 100,
        drift_permille: 250,
    };

    // "The gate": the margins are the contract's.
    #[test]
    fn the_contract() {
        assert_eq!(Thresholds::default(), T);
    }

    // "Measurement": the middle of N runs.
    #[test]
    fn medians() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[7]), Some(7));
        assert_eq!(median(&[9, 1, 5]), Some(5));
        assert_eq!(median(&[4, 1]), Some(2));
        assert_eq!(median(&[10, 1, 4, 3]), Some(3));
        assert_eq!(median(&[5, 3, 1, 2, 4]), Some(3));
        assert_eq!(median(&[u64::MAX, u64::MAX]), Some(u64::MAX));
        // One slow run does not move it.
        assert_eq!(median(&[100, 101, 99, 100, 5_000]), Some(100));
    }

    // "The gate": more than 5% and more than the floor; more than 2% and
    // more than the floor.
    #[test]
    fn threshold_decisions() {
        let time = |now, base| exceeds(now, base, T.time_permille, T.time_floor_us, (1, 1));
        assert!(!time(1_050_000, 1_000_000), "exactly 5% is within the margin");
        assert!(time(1_050_001, 1_000_000));
        assert!(!time(900_000, 1_000_000));
        // The absolute floor: a 5 ms link that takes 6.5 ms is 30% slower
        // and not judged slower; at 7.1 ms it is.
        assert!(!time(6_500, 5_000));
        assert!(!time(7_000, 5_000), "exactly the floor is within the margin");
        assert!(time(7_001, 5_000));
        // A link above the floor but within 5%.
        assert!(!time(102_500, 100_000));

        let memory = |now, base| exceeds(now, base, T.memory_permille, T.memory_floor_kib, (1, 1));
        assert!(!memory(102_000, 100_000));
        assert!(memory(102_001, 100_000));
        assert!(!memory(5_900, 5_000), "18% of 5 MiB is under the floor of 1 MiB");
        assert!(memory(6_025, 5_000));
        // No overflow at the extremes.
        assert!(!exceeds(u64::MAX, u64::MAX, 50, 0, (u64::MAX, 1)));
        assert!(exceeds(u64::MAX, 1, 50, 0, (1, 0)));
    }

    // "Noise": the calibration link before and after the run.
    #[test]
    fn noisy_run_detection() {
        assert!(!calibration_differs(40_000, 44_000, T.calibration_permille), "10% is the bound");
        assert!(calibration_differs(40_000, 44_001, T.calibration_permille));
        assert!(calibration_differs(44_001, 40_000, T.calibration_permille), "in either direction");
        assert!(!calibration_differs(40_000, 40_000, T.calibration_permille));
        assert!(calibration_differs(0, 1, T.calibration_permille));
    }

    fn steady() -> Common {
        Common { advisory: None, unsteady: None, calibration: Some((40_000, 40_000)) }
    }

    fn mortar(wall: u64, rss: u64) -> Link {
        ok("w", JUDGED, Threads::Default, wall, rss)
    }

    // "The gate": pass, improvement, and the regressions.
    #[test]
    fn check_pass_and_fail() {
        let base = mortar(100_000, 50_000);
        let verdict = |wall, rss| judge(&mortar(wall, rss), Some(&base), false, &steady(), &T);
        assert_eq!(verdict(100_000, 50_000), Verdict::Ok);
        assert_eq!(verdict(104_000, 50_900), Verdict::Ok);
        assert_eq!(verdict(106_000, 50_000), Verdict::Regressed { time: true, memory: false });
        assert_eq!(verdict(100_000, 52_000), Verdict::Regressed { time: false, memory: true });
        assert_eq!(verdict(120_000, 60_000), Verdict::Regressed { time: true, memory: true });
        assert_eq!(verdict(90_000, 50_000), Verdict::Improved);
        assert_eq!(verdict(100_000, 45_000), Verdict::Improved);
        assert_eq!(verdict(120_000, 45_000), Verdict::Regressed { time: true, memory: false });
        assert!(verdict(106_000, 50_000).fails() && !verdict(90_000, 50_000).fails());
        assert_eq!(verdict(120_000, 60_000).text(), "REGRESSED (time, memory)");
        assert_eq!(verdict(106_000, 50_000).text(), "REGRESSED (time)");
        assert_eq!(verdict(100_000, 52_000).text(), "REGRESSED (memory)");

        // Small links: the floors.
        let small = mortar(5_000, 5_000);
        let verdict = |wall, rss| judge(&mortar(wall, rss), Some(&small), false, &steady(), &T);
        assert_eq!(verdict(6_900, 5_900), Verdict::Ok);
        assert_eq!(verdict(7_100, 5_000), Verdict::Regressed { time: true, memory: false });
        assert_eq!(verdict(3_100, 4_100), Verdict::Ok, "an improvement is held to the floors too");
    }

    // "The gate": what is not a comparison of two measurements.
    #[test]
    fn links_without_a_comparison() {
        let base = mortar(100_000, 50_000);
        let refused = not_ok("w", JUDGED, Status::Refused, "not implemented");
        let failed = not_ok("w", JUDGED, Status::Failed, "signal 11");
        let judge =
            |now: &Link, base: Option<&Link>, changed| judge(now, base, changed, &steady(), &T);
        assert_eq!(
            judge(&refused, Some(&refused), false).text(),
            "not judged: refused, as in the baseline"
        );
        assert_eq!(
            judge(&refused, None, false).text(),
            "new: the baseline does not have this link"
        );
        assert_eq!(
            judge(&base, Some(&failed), false).text(),
            "new: the baseline does not have this link"
        );
        assert_eq!(
            judge(&base, Some(&refused), false).text(),
            "new: mortar links this workload now"
        );
        assert_eq!(judge(&refused, Some(&base), false), Verdict::Lost);
        assert!(Verdict::Lost.fails());
        assert_eq!(
            judge(&failed, Some(&base), false).text(),
            "not judged: the link failed in this run"
        );
        // A changed workload is compared without consequence, whatever happened.
        let slow = mortar(200_000, 90_000);
        for now in [&slow, &refused] {
            let verdict = judge(now, Some(&base), true);
            assert_eq!(verdict.text(), "advisory: the workload changed since the baseline");
            assert!(!verdict.fails());
        }
    }

    // "The gate", "Noise": a slower time is not a regression when the
    // measurement cannot carry it; memory is judged whatever the machine
    // is doing; a time that is not slower needs no such care.
    #[test]
    fn an_unsteady_measurement_is_not_a_regression() {
        let base = mortar(100_000, 50_000);
        let noisy = Common { unsteady: Some("the run is noisy".into()), ..steady() };
        let verdict = |wall, rss| judge(&mortar(wall, rss), Some(&base), false, &noisy, &T);
        assert_eq!(verdict(130_000, 50_000), Verdict::Noisy("the run is noisy".into()));
        assert_eq!(verdict(130_000, 60_000), Verdict::Regressed { time: false, memory: true });
        assert_eq!(verdict(100_000, 60_000), Verdict::Regressed { time: false, memory: true });
        assert_eq!(verdict(101_000, 50_000), Verdict::Ok);
        assert_eq!(verdict(80_000, 50_000), Verdict::Improved);
        assert!(!verdict(130_000, 50_000).fails());
        assert_eq!(verdict(130_000, 50_000).text(), "NOISY: the run is noisy");

        // The spread of either side's runs: the middle above the fastest.
        let mut wide = mortar(130_000, 50_000);
        wide.figures.as_mut().unwrap().wall_us_min = 110_000;
        assert_eq!(
            judge(&wide, Some(&base), false, &steady(), &T),
            Verdict::Noisy("the runs spread by 18.1%".into())
        );
        let mut wide_base = base.clone();
        wide_base.figures.as_mut().unwrap().wall_us_min = 90_000;
        assert!(matches!(
            judge(&mortar(130_000, 50_000), Some(&wide_base), false, &steady(), &T),
            Verdict::Noisy(_)
        ));
    }

    // "The gate", "Drift": what the machine lost since the baseline, by
    // the calibration link, scales the time margin and nothing else; a
    // faster machine does not tighten it.
    #[test]
    fn the_calibration_link_is_the_control() {
        let base = mortar(100_000, 50_000);
        let drifted = |now, then| Common { calibration: Some((now, then)), ..steady() };
        let verdict =
            |wall, common: &Common| judge(&mortar(wall, 50_000), Some(&base), false, common, &T);
        // The machine is 20% slower: 126% is within 5% of 120%, 127% is not.
        assert_eq!(verdict(126_000, &drifted(48_000, 40_000)), Verdict::Ok);
        assert_eq!(
            verdict(126_001, &drifted(48_000, 40_000)),
            Verdict::Regressed { time: true, memory: false }
        );
        // The machine is faster: the margin is the plain one.
        assert_eq!(verdict(105_000, &drifted(30_000, 40_000)), Verdict::Ok);
        assert_eq!(
            verdict(105_001, &drifted(30_000, 40_000)),
            Verdict::Regressed { time: true, memory: false }
        );
        // Memory is not scaled.
        let larger =
            judge(&mortar(100_000, 52_000), Some(&base), false, &drifted(48_000, 40_000), &T);
        assert_eq!(larger, Verdict::Regressed { time: false, memory: true });
    }

    // "The gate": what a run and a baseline have in common is read from
    // their headers.
    #[test]
    fn headers_decide_what_binds() {
        let base = sample();
        let mut run = sample();
        assert_eq!(
            Common::of(&run, &base, &T),
            Common { advisory: None, unsteady: None, calibration: Some((40_000, 40_000)) }
        );
        run.set("machine", "x86-64-some-cpu-8c");
        assert_eq!(
            Common::of(&run, &base, &T).advisory.as_deref(),
            Some("another machine (aarch64-implementer-0x61-part-0x000-24c)")
        );
        let mut run = sample();
        run.set("noisy", "yes");
        assert_eq!(Common::of(&run, &base, &T).unsteady.as_deref(), Some("the run is noisy"));
        assert_eq!(
            Common::of(&base, &run, &T).unsteady.as_deref(),
            Some("the baseline was recorded on a noisy machine")
        );
        let mut run = sample();
        run.set("calibration", "mold on fnv1a64:00000000000000aa");
        let common = Common::of(&run, &base, &T);
        assert_eq!(
            (common.calibration, common.unsteady.as_deref()),
            (None, Some("no common calibration link"))
        );
        // More than 25% slower than at the baseline: too busy to judge a time.
        let mut run = sample();
        run.set("calibration_before_us", "50400");
        run.set("calibration_after_us", "52000");
        let common = Common::of(&run, &base, &T);
        assert_eq!(common.calibration, Some((50_400, 40_000)));
        assert_eq!(
            common.unsteady.as_deref(),
            Some("the machine is 26.0% slower than at the baseline")
        );
        run.set("calibration_before_us", "50000");
        assert_eq!(Common::of(&run, &base, &T).unsteady, None);
    }

    // "The gate": a run is judged link by link of mortar's; the reference
    // linkers are never judged.
    #[test]
    fn judging_a_run() {
        let mut base = sample();
        base.workloads.push(info("big", "large"));
        base.links.push(mortar(100_000, 50_000));
        base.links.last_mut().unwrap().workload = "big".into();
        let mut run = base.clone();
        // A reference linker twice as slow is nobody's regression.
        run.links[0] = ok("hello-c", "bfd", Threads::One, 18_000, 9_000);
        let check = Check::new(&run, &base, &T);
        let texts: Vec<_> = check
            .judged
            .iter()
            .map(|j| format!("{} {} {}", j.workload, j.threads.text(), j.verdict.text()))
            .collect();
        assert_eq!(
            texts,
            ["hello-c default not judged: refused, as in the baseline", "big default ok"]
        );
        assert!(!check.failed() && !check.noisy());

        run.links.last_mut().unwrap().figures.as_mut().unwrap().wall_us = 150_000;
        let check = Check::new(&run, &base, &T);
        assert!(check.noisy() && !check.failed(), "the runs spread: {:?}", check.judged[1].verdict);
        let figures = run.links.last_mut().unwrap().figures.as_mut().unwrap();
        (figures.wall_us_min, figures.wall_us_max) = (149_000, 151_000);
        let check = Check::new(&run, &base, &T);
        assert!(check.failed());
        assert_eq!(check.judged[1].verdict, Verdict::Regressed { time: true, memory: false });
        assert_eq!(check.judged[1].base.as_ref().map(|b| b.workload.as_str()), Some("big"));

        // Another workload under the same name.
        run.workloads.last_mut().unwrap().input_hash = "fnv1a64:ffffffffffffffff".into();
        let check = Check::new(&run, &base, &T);
        assert!(!check.failed());
        assert_eq!(
            check.judged[1].verdict.text(),
            "advisory: the workload changed since the baseline"
        );
    }

    #[test]
    fn percents() {
        assert_eq!(percent(0), "0.0%");
        assert_eq!(percent(181), "18.1%");
        assert_eq!(percent(1000), "100.0%");
    }
}
