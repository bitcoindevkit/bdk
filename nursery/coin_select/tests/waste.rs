use bdk_coin_select::{
    change_policy, float::Ordf32, metrics::Waste, Candidate, CoinSelector, Drain, DrainWeights,
    FeeRate, Target,
};
use proptest::{
    prelude::*,
    test_runner::{RngAlgorithm, TestRng},
};
use rand::prelude::IteratorRandom;

#[test]
fn waste_all_selected_except_one_is_optimal_and_awkward() {
    let num_inputs = 40;
    let target = 15578;
    let feerate = 8.190512;
    let min_fee = 0;
    let base_weight = 453;
    let long_term_feerate_diff = -3.630499;
    let change_weight = 1;
    let change_spend_weight = 41;
    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let long_term_feerate =
        FeeRate::from_sat_per_vb((0.0f32).max(feerate - long_term_feerate_diff));
    let feerate = FeeRate::from_sat_per_vb(feerate);
    let drain_weights = DrainWeights {
        output_weight: change_weight,
        spend_weight: change_spend_weight,
    };

    let change_policy = change_policy::min_waste(drain_weights, long_term_feerate);
    let wv = test_wv(&mut rng);
    let candidates = wv.take(num_inputs).collect::<Vec<_>>();

    let cs = CoinSelector::new(&candidates, base_weight);
    let target = Target {
        value: target,
        feerate,
        min_fee,
    };

    let solutions = cs.bnb_solutions(Waste {
        target,
        long_term_feerate,
        change_policy: &change_policy,
    });

    let (_i, (best, score)) = solutions
        .enumerate()
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("it should have found solution");

    let mut all_selected = cs.clone();
    all_selected.select_all();
    let target_waste = all_selected.waste(
        target,
        long_term_feerate,
        change_policy(&all_selected, target),
        1.0,
    );
    assert!(score.0 < target_waste);
    assert_eq!(best.selected().len(), 39);
}

#[test]
fn waste_naive_effective_value_shouldnt_be_better() {
    let num_inputs = 23;
    let target = 1475;
    let feerate = 1.0;
    let min_fee = 989;
    let base_weight = 0;
    let long_term_feerate_diff = 3.8413858;
    let change_weight = 1;
    let change_spend_weight = 1;
    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let long_term_feerate =
        FeeRate::from_sat_per_vb((0.0f32).max(feerate - long_term_feerate_diff));
    let feerate = FeeRate::from_sat_per_vb(feerate);
    let drain_weights = DrainWeights {
        output_weight: change_weight,
        spend_weight: change_spend_weight,
    };
    let drain = Drain {
        weights: drain_weights,
        value: 0,
    };

    let change_policy = change_policy::min_waste(drain_weights, long_term_feerate);
    let wv = test_wv(&mut rng);
    let candidates = wv.take(num_inputs).collect::<Vec<_>>();

    let cs = CoinSelector::new(&candidates, base_weight);

    let target = Target {
        value: target,
        feerate,
        min_fee,
    };

    let solutions = cs.bnb_solutions(Waste {
        target,
        long_term_feerate,
        change_policy: &change_policy,
    });

    let (_i, (_best, score)) = solutions
        .enumerate()
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("should find solution");

    let mut naive_select = cs.clone();
    naive_select.sort_candidates_by_key(|(_, wv)| core::cmp::Reverse(wv.value_pwu()));
    // we filter out failing onces below
    let _ = naive_select.select_until_target_met(target, drain);

    let bench_waste = naive_select.waste(
        target,
        long_term_feerate,
        change_policy(&naive_select, target),
        1.0,
    );

    assert!(score < Ordf32(bench_waste));
}

#[test]
fn waste_doesnt_take_too_long_to_finish() {
    let start = std::time::Instant::now();
    let num_inputs = 22;
    let target = 0;
    let feerate = 4.9522414;
    let min_fee = 0;
    let base_weight = 2;
    let long_term_feerate_diff = -0.17994404;
    let change_weight = 1;
    let change_spend_weight = 34;

    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let long_term_feerate =
        FeeRate::from_sat_per_vb((0.0f32).max(feerate - long_term_feerate_diff));
    let feerate = FeeRate::from_sat_per_vb(feerate);
    let drain_weights = DrainWeights {
        output_weight: change_weight,
        spend_weight: change_spend_weight,
    };

    let change_policy = change_policy::min_waste(drain_weights, long_term_feerate);
    let wv = test_wv(&mut rng);
    let candidates = wv.take(num_inputs).collect::<Vec<_>>();

    let cs = CoinSelector::new(&candidates, base_weight);

    let target = Target {
        value: target,
        feerate,
        min_fee,
    };

    let solutions = cs.bnb_solutions(Waste {
        target,
        long_term_feerate,
        change_policy: &change_policy,
    });

    solutions
        .enumerate()
        .inspect(|_| {
            if start.elapsed().as_millis() > 1_000 {
                panic!("took too long to finish")
            }
        })
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("should find solution");
}

/// When long term feerate is lower than current adding new inputs should in general make things
/// worse except in the case that we can get rid of the change output with negative effective
/// value inputs. In this case the right answer to select everything.
#[test]
fn waste_lower_long_term_feerate_but_still_need_to_select_all() {
    let num_inputs = 16;
    let target = 5586;
    let feerate = 9.397041;
    let min_fee = 0;
    let base_weight = 91;
    let long_term_feerate_diff = 0.22074795;
    let change_weight = 1;
    let change_spend_weight = 27;

    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let long_term_feerate = FeeRate::from_sat_per_vb(0.0f32.max(feerate - long_term_feerate_diff));
    let feerate = FeeRate::from_sat_per_vb(feerate);
    let drain_weights = DrainWeights {
        output_weight: change_weight,
        spend_weight: change_spend_weight,
    };

    let change_policy = change_policy::min_waste(drain_weights, long_term_feerate);
    let wv = test_wv(&mut rng);
    let candidates = wv.take(num_inputs).collect::<Vec<_>>();

    let cs = CoinSelector::new(&candidates, base_weight);

    let target = Target {
        value: target,
        feerate,
        min_fee,
    };

    let solutions = cs.bnb_solutions(Waste {
        target,
        long_term_feerate,
        change_policy: &change_policy,
    });
    let bench = {
        let mut all_selected = cs.clone();
        all_selected.select_all();
        all_selected
    };

    let (_i, (_sol, waste)) = solutions
        .enumerate()
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("should find solution");

    let bench_waste = bench.waste(
        target,
        long_term_feerate,
        change_policy(&bench, target),
        1.0,
    );

    assert!(waste <= Ordf32(bench_waste));
}

#[test]
fn waste_low_but_non_negative_rate_diff_means_adding_more_inputs_might_reduce_excess() {
    let num_inputs = 22;
    let target = 7620;
    let feerate = 8.173157;
    let min_fee = 0;
    let base_weight = 35;
    let long_term_feerate_diff = 0.0;
    let change_weight = 1;
    let change_spend_weight = 47;

    let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
    let long_term_feerate = FeeRate::from_sat_per_vb(0.0f32.max(feerate - long_term_feerate_diff));
    let feerate = FeeRate::from_sat_per_vb(feerate);
    let drain_weights = DrainWeights {
        output_weight: change_weight,
        spend_weight: change_spend_weight,
    };

    let change_policy = change_policy::min_waste(drain_weights, long_term_feerate);
    let wv = test_wv(&mut rng);
    let mut candidates = wv.take(num_inputs).collect::<Vec<_>>();
    // HACK: for this test had to set segwit true to keep it working once we
    // started properly accounting for legacy weight variations
    candidates
        .iter_mut()
        .for_each(|candidate| candidate.is_segwit = true);

    let cs = CoinSelector::new(&candidates, base_weight);

    let target = Target {
        value: target,
        feerate,
        min_fee,
    };

    let solutions = cs.bnb_solutions(Waste {
        target,
        long_term_feerate,
        change_policy: &change_policy,
    });
    let bench = {
        let mut all_selected = cs.clone();
        all_selected.select_all();
        all_selected
    };

    let (_i, (_sol, waste)) = solutions
        .enumerate()
        .filter_map(|(i, sol)| Some((i, sol?)))
        .last()
        .expect("should find solution");

    let bench_waste = bench.waste(
        target,
        long_term_feerate,
        change_policy(&bench, target),
        1.0,
    );

    assert!(waste <= Ordf32(bench_waste));
}

proptest! {
    #![proptest_config(ProptestConfig {
        timeout: 6_000,
        cases: 1_000,
        ..Default::default()
    })]
    #[test]
    #[cfg(not(debug_assertions))] // too slow if compiling for debug
    fn waste_prop_waste(
        num_inputs in 0usize..50,
        target in 0u64..25_000,
        feerate in 1.0f32..10.0,
        min_fee in 0u64..1_000,
        base_weight in 0u32..500,
        long_term_feerate_diff in -5.0f32..5.0,
        change_weight in 1u32..100,
        change_spend_weight in 1u32..100,
    ) {
        println!("=======================================");
        let start = std::time::Instant::now();
        let mut rng = TestRng::deterministic_rng(RngAlgorithm::ChaCha);
        let long_term_feerate = FeeRate::from_sat_per_vb(0.0f32.max(feerate - long_term_feerate_diff));
        let feerate = FeeRate::from_sat_per_vb(feerate);
        let drain = Drain {
            weight: change_weight,
            spend_weight: change_spend_weight,
            value: 0
        };

        let change_policy = crate::change_policy::min_waste(drain, long_term_feerate);
        let wv = test_wv(&mut rng);
        let candidates = wv.take(num_inputs).collect::<Vec<_>>();

        let cs = CoinSelector::new(&candidates, base_weight);

        let target = Target {
            value: target,
            feerate,
            min_fee
        };

        let solutions = cs.branch_and_bound(Waste {
            target,
            long_term_feerate,
            change_policy: &change_policy
        });


        let best = solutions
            .enumerate()
            .filter_map(|(i, sol)| Some((i, sol?)))
            .last();

        match best {
            Some((_i, (sol, _score))) => {

                let mut cmp_benchmarks = vec![
                    {
                        let mut naive_select = cs.clone();
                        naive_select.sort_candidates_by_key(|(_, wv)| core::cmp::Reverse(wv.effective_value(target.feerate)));
                        // we filter out failing onces below
                        let _ = naive_select.select_until_target_met(target, drain);
                        naive_select
                    },
                    {
                        let mut all_selected = cs.clone();
                        all_selected.select_all();
                        all_selected
                    },
                    {
                        let mut all_effective_selected = cs.clone();
                        all_effective_selected.select_all_effective(target.feerate);
                        all_effective_selected
                    }
                ];

                // add some random selections -- technically it's possible that one of these is better but it's very unlikely if our algorithm is working correctly.
                cmp_benchmarks.extend((0..10).map(|_|randomly_satisfy_target_with_low_waste(&cs, target, long_term_feerate, &change_policy, &mut rng)));

                let cmp_benchmarks = cmp_benchmarks.into_iter().filter(|cs| cs.is_target_met(target, change_policy(&cs, target)));
                let sol_waste = sol.waste(target, long_term_feerate, change_policy(&sol, target), 1.0);

                for (_bench_id, mut bench) in cmp_benchmarks.enumerate() {
                    let bench_waste = bench.waste(target, long_term_feerate, change_policy(&bench, target), 1.0);
                    if sol_waste > bench_waste {
                        dbg!(_bench_id);
                        println!("bnb solution: {}", sol);
                        bench.sort_candidates_by_descending_value_pwu();
                        println!("found better: {}", bench);
                    }
                    prop_assert!(sol_waste <= bench_waste);
                }
            },
            None => {
                dbg!(feerate - long_term_feerate);
                prop_assert!(!cs.is_selection_plausible_with_change_policy(target, &change_policy));
            }
        }

        dbg!(start.elapsed());
    }
}

fn test_wv(mut rng: impl RngCore) -> impl Iterator<Item = Candidate> {
    core::iter::repeat_with(move || {
        let value = rng.gen_range(0..1_000);
        Candidate {
            value,
            weight: rng.gen_range(0..100),
            input_count: rng.gen_range(1..2),
            is_segwit: rng.gen_bool(0.5),
        }
    })
}

// this is probably a useful thing to have on CoinSelector but I don't want to design it yet
#[allow(unused)]
fn randomly_satisfy_target_with_low_waste<'a>(
    cs: &CoinSelector<'a>,
    target: Target,
    long_term_feerate: FeeRate,
    change_policy: &impl Fn(&CoinSelector, Target) -> Drain,
    rng: &mut impl RngCore,
) -> CoinSelector<'a> {
    let mut cs = cs.clone();

    let mut last_waste: Option<f32> = None;
    while let Some(next) = cs.unselected_indices().choose(rng) {
        cs.select(next);
        let change = change_policy(&cs, target);
        if cs.is_target_met(target, change) {
            let curr_waste = cs.waste(target, long_term_feerate, change, 1.0);
            if let Some(last_waste) = last_waste {
                if curr_waste > last_waste {
                    break;
                }
            }
            last_waste = Some(curr_waste);
        }
    }
    cs
}
