use bdk_coin_select::{BnBMetric, Candidate, CoinSelector, Drain, FeeRate, Target};
#[macro_use]
extern crate alloc;

use alloc::vec::Vec;
use proptest::{
    prelude::*,
    test_runner::{RngAlgorithm, TestRng},
};
use rand::{Rng, RngCore};

fn test_wv(mut rng: impl RngCore) -> impl Iterator<Item = Candidate> {
    core::iter::repeat_with(move || {
        let value = rng.gen_range(0..1_000);
        Candidate {
            value,
            weight: 100,
            input_count: rng.gen_range(1..2),
            is_segwit: rng.gen_bool(0.5),
        }
    })
}

struct MinExcessThenWeight {
    target: Target,
}

impl BnBMetric for MinExcessThenWeight {
    type Score = (i64, u32);

    fn score<'a>(&mut self, cs: &CoinSelector<'a>) -> Option<Self::Score> {
        if cs.excess(self.target, Drain::none()) < 0 {
            None
        } else {
            Some((cs.excess(self.target, Drain::none()), cs.selected_weight()))
        }
    }

    fn bound<'a>(&mut self, cs: &CoinSelector<'a>) -> Option<Self::Score> {
        let lower_bound_excess = cs.excess(self.target, Drain::none()).max(0);
        let lower_bound_weight = {
            let mut cs = cs.clone();
            cs.select_until_target_met(self.target, Drain::none())
                .ok()?;
            cs.selected_weight()
        };
        Some((lower_bound_excess, lower_bound_weight))
    }
}

#[test]
/// Detect regressions/improvements by making sure it always finds the solution in the same
/// number of iterations.
fn bnb_finds_an_exact_solution_in_n_iter() {
    let solution_len = 8;
    let num_additional_canidates = 50;

    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let mut wv = test_wv(&mut rng);

    let solution: Vec<Candidate> = (0..solution_len).map(|_| wv.next().unwrap()).collect();
    let solution_weight = solution.iter().map(|sol| sol.weight).sum();
    let target = solution.iter().map(|c| c.value).sum();

    let mut candidates = solution.clone();
    candidates.extend(wv.take(num_additional_canidates));
    candidates.sort_unstable_by_key(|wv| core::cmp::Reverse(wv.value));

    let cs = CoinSelector::new(&candidates, 0);

    let target = Target {
        value: target,
        // we're trying to find an exact selection value so set fees to 0
        feerate: FeeRate::zero(),
        min_fee: 0,
    };

    let solutions = cs.branch_and_bound(MinExcessThenWeight { target });

    let (i, (best, _score)) = solutions
        .enumerate()
        .take(807)
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("it found a solution");

    assert_eq!(i, 806);

    assert!(best.selected_weight() <= solution_weight);
    assert_eq!(best.selected_value(), target.value);
}

#[test]
fn bnb_finds_solution_if_possible_in_n_iter() {
    let num_inputs = 18;
    let target = 8_314;
    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let wv = test_wv(&mut rng);
    let candidates = wv.take(num_inputs).collect::<Vec<_>>();
    let cs = CoinSelector::new(&candidates, 0);

    let target = Target {
        value: target,
        feerate: FeeRate::default_min_relay_fee(),
        min_fee: 0,
    };

    let solutions = cs.branch_and_bound(MinExcessThenWeight { target });

    let (i, (sol, _score)) = solutions
        .enumerate()
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("found a solution");

    assert_eq!(i, 176);
    let excess = sol.excess(target, Drain::none());
    assert_eq!(excess, 8);
}

proptest! {

    #[test]
    fn bnb_always_finds_solution_if_possible(num_inputs in 1usize..50, target in 0u64..10_000) {
        let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
        let wv = test_wv(&mut rng);
        let candidates = wv.take(num_inputs).collect::<Vec<_>>();
        let cs = CoinSelector::new(&candidates, 0);

        let target = Target {
            value: target,
            feerate: FeeRate::zero(),
            min_fee: 0,
        };

        let solutions = cs.branch_and_bound(MinExcessThenWeight { target });

        match solutions.enumerate().filter_map(|(i, sol)| Some((i, sol?))).last() {
            Some((_i, (sol, _score))) => assert!(sol.selected_value() >= target.value),
            _ => prop_assert!(!cs.is_selection_possible(target, Drain::none())),
        }
    }

    #[test]
    fn bnb_always_finds_exact_solution_eventually(
        solution_len in 1usize..10,
        num_additional_canidates in 0usize..100,
        num_preselected in 0usize..10
    ) {
        let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
        let mut wv = test_wv(&mut rng);

        let solution: Vec<Candidate> = (0..solution_len).map(|_| wv.next().unwrap()).collect();
        let target = solution.iter().map(|c| c.value).sum();
        let solution_weight = solution.iter().map(|sol| sol.weight).sum();

        let mut candidates = solution.clone();
        candidates.extend(wv.take(num_additional_canidates));

        let mut cs = CoinSelector::new(&candidates, 0);
        for i in 0..num_preselected.min(solution_len) {
            cs.select(i);
        }

        // sort in descending value
        cs.sort_candidates_by_key(|(_, wv)| core::cmp::Reverse(wv.value));

        let target = Target {
            value: target,
            // we're trying to find an exact selection value so set fees to 0
            feerate: FeeRate::zero(),
            min_fee: 0
        };

        let solutions = cs.branch_and_bound(MinExcessThenWeight { target });

        let (_i, (best, _score)) = solutions
            .enumerate()
            .filter_map(|(i, sol)| Some((i, sol?)))
            .last()
            .expect("it found a solution");



        prop_assert!(best.selected_weight() <= solution_weight);
        prop_assert_eq!(best.selected_value(), target.value);
    }
}
