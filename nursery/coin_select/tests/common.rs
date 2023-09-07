#![allow(dead_code)]

use std::any::type_name;

use bdk_coin_select::{
    float::Ordf32, BnbMetric, Candidate, CoinSelector, Drain, DrainWeights, FeeRate, NoBnbSolution,
    Target,
};
use proptest::{
    prelude::*,
    prop_assert, prop_assert_eq,
    test_runner::{RngAlgorithm, TestRng},
};

pub fn can_eventually_find_best_solution<P, M>(
    params: StrategyParams,
    candidates: Vec<Candidate>,
    change_policy: &P,
    mut metric: M,
) -> Result<(), proptest::test_runner::TestCaseError>
where
    M: BnbMetric<Score = Ordf32>,
    P: Fn(&CoinSelector, Target) -> Drain,
{
    println!("== TEST ==");
    println!("{}", type_name::<M>());
    println!("{:?}", params);

    let target = params.target();

    let mut selection = CoinSelector::new(&candidates, params.base_weight);
    let mut exp_selection = selection.clone();

    if metric.requires_ordering_by_descending_value_pwu() {
        exp_selection.sort_candidates_by_descending_value_pwu();
    }
    print_candidates(&params, &exp_selection);

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
    let result = bnb_search(&mut selection, metric, usize::MAX);
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
                "score:
                    got={}
                    exp={}",
                result_str,
                exp_result_str
            )
        }
        _ => prop_assert!(result.is_err(), "should not find solution"),
    }

    Ok(())
}

pub fn ensure_bound_is_not_too_tight<P, M>(
    params: StrategyParams,
    candidates: Vec<Candidate>,
    change_policy: &P,
    mut metric: M,
) -> Result<(), proptest::test_runner::TestCaseError>
where
    M: BnbMetric<Score = Ordf32>,
    P: Fn(&CoinSelector, Target) -> Drain,
{
    println!("== TEST ==");
    println!("{}", type_name::<M>());
    println!("{:?}", params);

    let target = params.target();

    let init_cs = {
        let mut cs = CoinSelector::new(&candidates, params.base_weight);
        if metric.requires_ordering_by_descending_value_pwu() {
            cs.sort_candidates_by_descending_value_pwu();
        }
        cs
    };
    print_candidates(&params, &init_cs);

    for (cs, _) in ExhaustiveIter::new(&init_cs).into_iter().flatten() {
        if let Some(lb_score) = metric.bound(&cs) {
            // This is the branch's lower bound. In other words, this is the BEST selection
            // possible (can overshoot) traversing down this branch. Let's check that!

            if let Some(score) = metric.score(&cs) {
                prop_assert!(
                    score >= lb_score,
                    "checking branch: selection={} score={} change={} lb={}",
                    cs,
                    score,
                    change_policy(&cs, target).is_some(),
                    lb_score
                );
            }

            for (descendant_cs, _) in ExhaustiveIter::new(&cs)
                .into_iter()
                .flatten()
                .filter(|(_, inc)| *inc)
            {
                if let Some(descendant_score) = metric.score(&descendant_cs) {
                    prop_assert!(
                        descendant_score >= lb_score,
                        "
                            parent={:8} change={} lb={} target_met={}
                        descendant={:8} change={} score={}
                        ",
                        cs,
                        change_policy(&cs, target).is_some(),
                        lb_score,
                        cs.is_target_met(target, Drain::none()),
                        descendant_cs,
                        change_policy(&descendant_cs, target).is_some(),
                        descendant_score,
                    );
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
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
        FeeRate::from_sat_per_vb((self.feerate + self.feerate_lt_diff).max(1.0))
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
        let value = rng.gen_range(1, 500_001);
        let weight = rng.gen_range(1, 2001);
        let input_count = rng.gen_range(1, 3);
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

pub fn print_candidates(params: &StrategyParams, cs: &CoinSelector<'_>) {
    println!("\tcandidates:");
    for (i, candidate) in cs.candidates() {
        println!(
            "\t\t{:3} | ev:{:10.2} | vpw:{:10.2} | waste:{:10.2} | {:?}",
            i,
            candidate.effective_value(params.feerate()),
            candidate.value_pwu(),
            candidate.weight as f32 * (params.feerate().spwu() - params.long_term_feerate().spwu()),
            candidate,
        );
    }
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
    type Item = (CoinSelector<'a>, bool);

    fn next(&mut self) -> Option<Self::Item> {
        let (cs, inclusion) = self.stack.pop()?;
        self.push_branches(&cs);
        Some((cs, inclusion))
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
        .filter(|(_, (_, inclusion))| *inclusion)
        .filter_map(|(_, (cs, _))| metric.score(&cs).map(|score| (cs, score)));

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

pub fn bnb_search<M>(
    cs: &mut CoinSelector,
    metric: M,
    max_rounds: usize,
) -> Result<(Ordf32, usize), NoBnbSolution>
where
    M: BnbMetric<Score = Ordf32>,
{
    let mut rounds = 0_usize;
    let (selection, score) = cs
        .bnb_solutions(metric)
        .inspect(|_| rounds += 1)
        .take(max_rounds)
        .flatten()
        .last()
        .ok_or(NoBnbSolution { max_rounds, rounds })?;
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
