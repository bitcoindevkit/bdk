#![allow(unused)]
use bdk_coin_select::{float::Ordf32, metrics, Candidate, CoinSelector, Drain, FeeRate, Target};
use proptest::{
    prelude::*,
    test_runner::{RngAlgorithm, TestRng},
};
use rand::prelude::IteratorRandom;

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

proptest! {
    #![proptest_config(ProptestConfig {
        timeout: 1_000,
        cases: 1_000,
        ..Default::default()
    })]
    #[test]
    #[cfg(not(debug_assertions))] // too slow if compiling for debug
    fn changeless_prop(
        num_inputs in 0usize..15,
        target in 0u64..15_000,
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

        let change_policy = bdk_coin_select::change_policy::min_waste(drain, long_term_feerate);
        let wv = test_wv(&mut rng);
        let candidates = wv.take(num_inputs).collect::<Vec<_>>();

        let cs = CoinSelector::new(&candidates, base_weight);

        let target = Target {
            value: target,
            feerate,
            min_fee
        };

        let solutions = cs.branch_and_bound(metrics::Changeless {
            target,
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
                ];

                cmp_benchmarks.extend((0..10).map(|_|random_minimal_selection(&cs, target, long_term_feerate, &change_policy, &mut rng)));

                let cmp_benchmarks = cmp_benchmarks.into_iter().filter(|cs| cs.is_target_met(target, change_policy(&cs, target)));
                for (_bench_id, bench) in cmp_benchmarks.enumerate() {
                    prop_assert!(change_policy(&bench, target).is_some() >=  change_policy(&sol, target).is_some());
                }
            }
            None => {
                prop_assert!(!cs.is_selection_plausible_with_change_policy(target, &change_policy));
            }
        }
        dbg!(start.elapsed());
    }
}

// this is probably a useful thing to have on CoinSelector but I don't want to design it yet
#[allow(unused)]
fn random_minimal_selection<'a>(
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
        if cs.is_target_met(target, change_policy(&cs, target)) {
            break;
        }
    }
    cs
}
