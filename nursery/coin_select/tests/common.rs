use std::any::type_name;

use bdk_coin_select::{
    float::Ordf32, BnbMetric, Candidate, CoinSelector, Drain, DrainWeights, FeeRate, NoBnbSolution,
    Target,
};
use proptest::{
    prop_assert, prop_assert_eq,
    test_runner::{RngAlgorithm, TestRng},
};
use rand::Rng;

pub fn can_eventually_find_best_solution<M, P, GM, GC, GP>(
    gen_candidates: GC,
    gen_change_policy: GP,
    gen_metric: GM,
    params: StrategyParams,
) -> Result<(), proptest::test_runner::TestCaseError>
where
    M: BnbMetric<Score = Ordf32>,
    P: Fn(&CoinSelector, Target) -> Drain,
    GM: Fn(&StrategyParams, P) -> M,
    GC: Fn(usize) -> Vec<Candidate>,
    GP: Fn(&StrategyParams) -> P,
{
    println!("== TEST ==");
    println!("{}", type_name::<M>());

    let candidates = gen_candidates(params.n_candidates);
    {
        println!("\tcandidates:");
        for (i, candidate) in candidates.iter().enumerate() {
            println!(
                "\t\t[{}] {:?} ev={}",
                i,
                candidate,
                candidate.effective_value(params.feerate())
            );
        }
    }

    let target = params.target();

    let mut metric = gen_metric(&params, gen_change_policy(&params));
    let change_policy = gen_change_policy(&params);

    let mut selection = CoinSelector::new(&candidates, params.base_weight);
    let mut exp_selection = selection.clone();

    println!("\texhaustive search:");
    let now = std::time::Instant::now();
    let exp_result = exhaustive_search(&mut exp_selection, &mut metric);
    let exp_change = change_policy(&exp_selection, target);
    let exp_result_str = result_string(&exp_result.ok_or("no possible solution"), exp_change);
    println!(
        "\t\telapsed={:8}s result={}",
        now.elapsed().as_secs_f64(),
        exp_result_str
    );

    println!("\tbranch and bound:");
    let now = std::time::Instant::now();
    let result = bnb_search(&mut selection, metric);
    let change = change_policy(&selection, target);
    let result_str = result_string(&result, change);
    println!(
        "\t\telapsed={:8}s result={}",
        now.elapsed().as_secs_f64(),
        result_str
    );

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
            )
        }
        _ => prop_assert!(result.is_err(), "should not find solution"),
    }

    Ok(())
}

pub fn ensure_bound_is_not_too_tight<M, P, GM, GC, GP>(
    gen_candidates: GC,
    gen_change_policy: GP,
    gen_metric: GM,
    params: StrategyParams,
) -> Result<(), proptest::test_runner::TestCaseError>
where
    M: BnbMetric<Score = Ordf32>,
    P: Fn(&CoinSelector, Target) -> Drain,
    GM: Fn(&StrategyParams, P) -> M,
    GC: Fn(usize) -> Vec<Candidate>,
    GP: Fn(&StrategyParams) -> P,
{
    println!("== TEST ==");
    println!("{}", type_name::<M>());

    let candidates = gen_candidates(params.n_candidates);
    {
        println!("\tcandidates:");
        for (i, candidate) in candidates.iter().enumerate() {
            println!(
                "\t\t[{}] {:?} ev={}",
                i,
                candidate,
                candidate.effective_value(params.feerate())
            );
        }
    }

    let mut metric = gen_metric(&params, gen_change_policy(&params));

    let init_cs = {
        let mut cs = CoinSelector::new(&candidates, params.base_weight);
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
                    "selection={} score={} lb={}",
                    cs,
                    score,
                    lb_score
                );
            }

            for descendant_cs in ExhaustiveIter::new(&cs).into_iter().flatten() {
                if let Some(descendant_score) = metric.score(&descendant_cs) {
                    prop_assert!(
                        descendant_score >= lb_score,
                        "this: {} (score={}), parent: {} (lb={})",
                        descendant_cs,
                        descendant_score,
                        cs,
                        lb_score
                    );
                }
            }
        }
    }
    Ok(())
}

pub struct StrategyParams {
    pub n_candidates: usize,
    pub target_value: u64,
    pub base_weight: u32,
    pub min_fee: u64,
    pub feerate: f32,
    pub feerate_lt_diff: f32,
    pub drain_weight: u32,
    pub drain_spend_weight: u32,
    pub drain_dust: u64,
}

impl StrategyParams {
    pub fn target(&self) -> Target {
        Target {
            feerate: self.feerate(),
            min_fee: self.min_fee,
            value: self.target_value,
        }
    }

    pub fn feerate(&self) -> FeeRate {
        FeeRate::from_sat_per_vb(self.feerate)
    }

    pub fn long_term_feerate(&self) -> FeeRate {
        FeeRate::from_sat_per_vb(((self.feerate + self.feerate_lt_diff) as f32).max(1.0))
    }

    pub fn drain_weights(&self) -> DrainWeights {
        DrainWeights {
            output_weight: self.drain_weight,
            spend_weight: self.drain_spend_weight,
        }
    }
}

pub fn gen_candidates(n: usize) -> Vec<Candidate> {
    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
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
    .take(n)
    .collect()
}

pub struct ExhaustiveIter<'a> {
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

pub fn exhaustive_search<M>(cs: &mut CoinSelector, metric: &mut M) -> Option<(Ordf32, usize)>
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

pub fn bnb_search<M>(cs: &mut CoinSelector, metric: M) -> Result<(Ordf32, usize), NoBnbSolution>
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

pub fn result_string<E>(res: &Result<(Ordf32, usize), E>, change: Drain) -> String
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
