use bdk_coin_select::metrics::{LowestFee, Waste};
use bdk_coin_select::Drain;
use bdk_coin_select::{
    change_policy::min_value_and_waste, float::Ordf32, BnbMetric, Candidate, CoinSelector,
    DrainWeights, FeeRate, NoBnbSolution, Target,
};
use proptest::prelude::*;
use proptest::test_runner::{FileFailurePersistence, RngAlgorithm, TestRng};
use rand::{Rng, RngCore};

fn gen_candidate(mut rng: impl RngCore) -> impl Iterator<Item = Candidate> {
    core::iter::repeat_with(move || {
        let value = rng.gen_range(1..=500_000);
        let weight = rng.gen_range(1..=2000);
        let input_count = rng.gen_range(1..=2);
        let is_segwit = rng.gen_bool(0.01);

        Candidate {
            value,
            weight,
            input_count,
            is_segwit,
        }
    })
}

struct DynMetric(&'static mut dyn BnbMetric<Score = Ordf32>);

impl DynMetric {
    fn new(metric: impl BnbMetric<Score = Ordf32> + 'static) -> Self {
        Self(Box::leak(Box::new(metric)))
    }
}

impl BnbMetric for DynMetric {
    type Score = Ordf32;

    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score> {
        self.0.score(cs)
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score> {
        self.0.bound(cs)
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        self.0.requires_ordering_by_descending_value_pwu()
    }
}

struct ExhaustiveIter<'a> {
    stack: Vec<(CoinSelector<'a>, bool)>, // for branches: (cs, this_index, include?)
}

impl<'a> ExhaustiveIter<'a> {
    fn new(cs: &CoinSelector<'a>) -> Option<Self> {
        let mut iter = Self { stack: Vec::new() };
        iter.push_branches(cs);
        Some(iter)
    }

    fn push_branches(&mut self, cs: &CoinSelector<'a>) {
        let next_index = match cs.unselected_indices().next() {
            Some(next_index) => next_index,
            None => return,
        };

        let inclusion_cs = {
            let mut cs = cs.clone();
            assert!(cs.select(next_index));
            cs
        };
        self.stack.push((inclusion_cs, true));

        let exclusion_cs = {
            let mut cs = cs.clone();
            cs.ban(next_index);
            cs
        };
        self.stack.push((exclusion_cs, false));
    }
}

impl<'a> Iterator for ExhaustiveIter<'a> {
    type Item = CoinSelector<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (cs, inclusion) = self.stack.pop()?;
            let _more = self.push_branches(&cs);
            if inclusion {
                return Some(cs);
            }
        }
    }
}

fn exhaustive_search<M>(cs: &mut CoinSelector, metric: &mut M) -> Option<(Ordf32, usize)>
where
    M: BnbMetric<Score = Ordf32>,
{
    if metric.requires_ordering_by_descending_value_pwu() {
        cs.sort_candidates_by_descending_value_pwu();
    }

    let mut best = Option::<(CoinSelector, Ordf32)>::None;
    let mut rounds = 0;

    let iter = ExhaustiveIter::new(cs)?
        .enumerate()
        .inspect(|(i, _)| rounds = *i)
        .filter_map(|(_, cs)| metric.score(&cs).map(|score| (cs, score)));

    for (child_cs, score) in iter {
        match &mut best {
            Some((best_cs, best_score)) => {
                if score < *best_score {
                    *best_cs = child_cs;
                    *best_score = score;
                }
            }
            best => *best = Some((child_cs, score)),
        }
    }

    if let Some((best_cs, score)) = &best {
        println!("\t\tsolution={}, score={}", best_cs, score);
        *cs = best_cs.clone();
    }

    best.map(|(_, score)| (score, rounds))
}

fn bnb_search<M>(cs: &mut CoinSelector, metric: M) -> Result<(Ordf32, usize), NoBnbSolution>
where
    M: BnbMetric<Score = Ordf32>,
{
    let mut rounds = 0_usize;
    let (selection, score) = cs
        .bnb_solutions(metric)
        .inspect(|_| rounds += 1)
        .flatten()
        .last()
        .ok_or(NoBnbSolution {
            max_rounds: usize::MAX,
            rounds,
        })?;
    println!("\t\tsolution={}, score={}", selection, score);
    *cs = selection;

    Ok((score, rounds))
}

fn result_string<E>(res: &Result<(Ordf32, usize), E>, change: Drain) -> String
where
    E: std::fmt::Debug,
{
    match res {
        Ok((score, rounds)) => {
            let drain = if change.is_some() {
                format!("{:?}", change)
            } else {
                "None".to_string()
            };
            format!("Ok(score={} rounds={} drain={})", score, rounds, drain)
        }
        err => format!("{:?}", err),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        source_file: Some(file!()),
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("proptest-regressions"))),
        // cases: u32::MAX,
        ..Default::default()
    })]

    #[test]
    fn can_eventually_find_best_solution(
        n_candidates in 1..20_usize,        // candidates (n)
        target_value in 500..500_000_u64,   // target value (sats)
        base_weight in 0..1000_u32,         // base weight (wu)
        min_fee in 0..1_000_u64,            // min fee (sats)
        feerate in 1.0..100.0_f32,          // feerate (sats/vb)
        feerate_lt_diff in -5.0..50.0_f32,  // longterm feerate diff (sats/vb)
        drain_weight in 100..=500_u32,      // drain weight (wu)
        drain_spend_weight in 1..=2000_u32, // drain spend weight (wu)
        drain_dust in 100..=1000_u64,       // drain dust (sats)
    ) {
        println!("== TEST ==");
        let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);

        let candidates = gen_candidate(&mut rng)
            .take(n_candidates)
            .collect::<Vec<_>>();

        let feerate_lt = FeeRate::from_sat_per_vb(((feerate + feerate_lt_diff) as f32).max(1.0));
        let feerate = FeeRate::from_sat_per_vb(feerate);

        {
            println!("\tcandidates:");
            for (i, candidate) in candidates.iter().enumerate() {
                println!("\t\t[{}] {:?} ev={}", i, candidate, candidate.effective_value(feerate));
            }
        }

        let target = Target {
            feerate,
            min_fee,
            value: target_value,
        };
        let drain_weights = DrainWeights {
            output_weight: drain_weight,
            spend_weight: drain_spend_weight,
        };
        let change_policy = min_value_and_waste(drain_weights, drain_dust, feerate_lt);

        let metric_factories: [(&str, &dyn Fn() -> DynMetric); 2] = [
            ("lowest_fee", &|| DynMetric::new(LowestFee {
                target,
                long_term_feerate: feerate_lt,
                change_policy: Box::leak(Box::new(min_value_and_waste(drain_weights, drain_dust, feerate_lt))),
            })),
            ("waste", &|| DynMetric::new(Waste {
                target,
                long_term_feerate: feerate_lt,
                change_policy: Box::leak(Box::new(min_value_and_waste(drain_weights, drain_dust, feerate_lt))),
            })),
        ];

        for (metric_name, metric_factory) in metric_factories {
            let mut selection = CoinSelector::new(&candidates, base_weight);
            let mut exp_selection = selection.clone();
            println!("\t{}:", metric_name);

            let now = std::time::Instant::now();
            let result = bnb_search(&mut selection, metric_factory());
            let change = change_policy(&selection, target);
            let result_str = result_string(&result, change);
            println!("\t\t{:8}s for bnb: {}", now.elapsed().as_secs_f64(), result_str);

            let now = std::time::Instant::now();
            let exp_result = exhaustive_search(&mut exp_selection, &mut metric_factory());
            let exp_change = change_policy(&exp_selection, target);
            let exp_result_str = result_string(&exp_result.ok_or("no possible solution"), exp_change);
            println!("\t\t{:8}s for exh: {}", now.elapsed().as_secs_f64(), exp_result_str);

            match exp_result {
                Some((score_to_match, _max_rounds)) => {
                    let (score, _rounds) = result.expect("must find solution");
                    // [todo] how do we check that `_rounds` is less than `_max_rounds` MOST of the time?
                    prop_assert_eq!(
                        score,
                        score_to_match,
                        "score: got={} exp={}",
                        result_str,
                        exp_result_str
                    );
                }
                _ => prop_assert!(result.is_err(), "should not find solution"),
            }
        }
    }

    #[test]
    fn ensure_bound_does_not_undershoot(
        n_candidates in 0..15_usize,        // candidates (n)
        target_value in 500..500_000_u64,   // target value (sats)
        base_weight in 0..641_u32,          // base weight (wu)
        min_fee in 0..1_000_u64,            // min fee (sats)
        feerate in 1.0..100.0_f32,          // feerate (sats/vb)
        feerate_lt_diff in -5.0..50.0_f32,  // longterm feerate diff (sats/vb)
        drain_weight in 100..=500_u32,      // drain weight (wu)
        drain_spend_weight in 1..=1000_u32, // drain spend weight (wu)
        drain_dust in 100..=1000_u64,       // drain dust (sats)
    ) {
        println!("== TEST ==");

        let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);

        let candidates = gen_candidate(&mut rng)
            .take(n_candidates)
            .collect::<Vec<_>>();

        let feerate_lt = FeeRate::from_sat_per_vb(((feerate + feerate_lt_diff) as f32).max(1.0));
        assert!(feerate_lt >= FeeRate::zero());
        let feerate = FeeRate::from_sat_per_vb(feerate);

        {
            println!("\tcandidates:");
            for (i, candidate) in candidates.iter().enumerate() {
                println!("\t\t[{}] {:?} ev={}", i, candidate, candidate.effective_value(feerate));
            }
        }

        let target = Target {
            feerate,
            min_fee,
            value: target_value,
        };
        let drain_weights = DrainWeights {
            output_weight: drain_weight,
            spend_weight: drain_spend_weight,
        };

        let metric_factories: &[(&str, &dyn Fn() -> DynMetric)] = &[
            ("lowest_fee", &|| DynMetric::new(LowestFee {
                target,
                long_term_feerate: feerate_lt,
                change_policy: Box::leak(Box::new(min_value_and_waste(drain_weights, drain_dust, feerate_lt))),
            })),
            ("waste", &|| DynMetric::new(Waste {
                target,
                long_term_feerate: feerate_lt,
                change_policy: Box::leak(Box::new(min_value_and_waste(drain_weights, drain_dust, feerate_lt))),
            })),
        ];

        for (metric_name, metric_factory) in metric_factories {
            let mut metric = metric_factory();
            let init_cs = {
                let mut cs = CoinSelector::new(&candidates, base_weight);
                if metric.requires_ordering_by_descending_value_pwu() {
                    cs.sort_candidates_by_descending_value_pwu();
                }
                cs
            };

            for cs in ExhaustiveIter::new(&init_cs).into_iter().flatten() {
                if let Some(lb_score) = metric.bound(&cs) {
                    // This is the branch's lower bound. In other words, this is the BEST selection
                    // possible (can overshoot) traversing down this branch. Let's check that!

                    if let Some(score) = metric.score(&cs) {
                        prop_assert!(
                            score >= lb_score,
                            "[{}] selection={} score={} lb={}",
                            metric_name, cs, score, lb_score,
                        );
                    }

                    for descendant_cs in ExhaustiveIter::new(&cs).into_iter().flatten() {
                        if let Some(descendant_score) = metric.score(&descendant_cs) {
                            prop_assert!(
                                descendant_score >= lb_score,
                                "[{}] this: {} (score={}), parent: {} (lb={})",
                                metric_name, descendant_cs, descendant_score, cs, lb_score,
                            );
                        }
                    }
                }
            }
        }
    }
}
