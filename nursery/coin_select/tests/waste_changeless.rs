#![allow(unused_imports)]

mod common;
use bdk_coin_select::metrics::WasteChangeless;
use bdk_coin_select::{Candidate, CoinSelector, Drain};
use proptest::{prelude::*, proptest};

proptest! {
    #[test]
    #[cfg(not(debug_assertions))] // too slow if compiling for debug
    fn can_eventually_find_best_solution(
        n_candidates in 1..20_usize,        // candidates (n)
        target_value in 500..500_000_u64,   // target value (sats)
        base_weight in 0..1000_u32,         // base weight (wu)
        min_fee in 0..1000_u64,             // min fee (sats)
        feerate in 1.0..100.0_f32,          // feerate (sats/vb)
        feerate_lt_diff in -50.0..0.0_f32,  // longterm feerate diff (sats/vb)
        drain_weight in 100..=500_u32,      // drain weight (wu)
        drain_spend_weight in 1..=2000_u32, // drain spend weight (wu)
        drain_dust in 100..=1000_u64,       // drain dust (sats)
    ) {
        common::can_eventually_find_best_solution(
            common::gen_candidates,
            |_| |_, _| Drain::none(),
            |p, _| WasteChangeless::new(p.target(), p.long_term_feerate()),
            common::StrategyParams {
                n_candidates,
                target_value,
                base_weight,
                min_fee,
                feerate,
                feerate_lt_diff,
                drain_weight,
                drain_spend_weight,
                drain_dust,
            },
        )?;
    }

    /// This is extremely non-optimal right now
    #[test]
    #[cfg(not(debug_assertions))] // too slow if compiling for debug
    fn find_best_solution_with_negative_diff(
        n_candidates in 5..15_usize,        // candidates (n)
        target_value in 500..500_000_u64,   // target value (sats)
        base_weight in 0..1000_u32,         // base weight (wu)
        min_fee in 0..1000_u64,             // min fee (sats)
        feerate in 1.0..100.0_f32,          // feerate (sats/vb)
        feerate_lt_diff in 0.0..50.0_f32,  // longterm feerate diff (sats/vb)
        drain_weight in 100..=500_u32,      // drain weight (wu)
        drain_spend_weight in 1..=2000_u32, // drain spend weight (wu)
        drain_dust in 100..=1000_u64,       // drain dust (sats)
    ) {
        common::can_eventually_find_best_solution(
            common::gen_candidates,
            |_| |_, _| Drain::none(),
            |p, _| WasteChangeless::new(p.target(), p.long_term_feerate()),
            common::StrategyParams {
                n_candidates,
                target_value,
                base_weight,
                min_fee,
                feerate,
                feerate_lt_diff,
                drain_weight,
                drain_spend_weight,
                drain_dust,
            },
        )?;
    }

    #[test]
    #[cfg(not(debug_assertions))] // too slow if compiling for debug
    fn ensure_bound_is_not_too_tight(
        n_candidates in 0..15_usize,        // candidates (n)
        target_value in 500..500_000_u64,   // target value (sats)
        base_weight in 0..1000_u32,         // base weight (wu)
        min_fee in 0..1000_u64,             // min fee (sats)
        feerate in 1.0..100.0_f32,          // feerate (sats/vb)
        feerate_lt_diff in -10.0..0.0_f32,  // longterm feerate diff (sats/vb)
        drain_weight in 100..=500_u32,      // drain weight (wu)
        drain_spend_weight in 1..=1000_u32, // drain spend weight (wu)
        drain_dust in 100..=1000_u64,       // drain dust (sats)
    ) {
        common::ensure_bound_is_not_too_tight(
            common::gen_candidates,
            |_| |_, _| Drain::none(),
            |p, _| WasteChangeless::new(p.target(), p.long_term_feerate()),
            common::StrategyParams {
                n_candidates,
                target_value,
                base_weight,
                min_fee,
                feerate,
                feerate_lt_diff,
                drain_weight,
                drain_spend_weight,
                drain_dust,
            },
        )?;
    }

    #[test]
    #[cfg(not(debug_assertions))] // too slow if compiling for debug
    fn identical_candidates(
        n_candidates in 30..300_usize,
        target_value in 50_000..500_000_u64,   // target value (sats)
        base_weight in 0..641_u32,          // base weight (wu)
        min_fee in 0..1_000_u64,            // min fee (sats)
        feerate in 1.0..10.0_f32,          // feerate (sats/vb)
        feerate_lt_diff in -5.0..5.0_f32,  // longterm feerate diff (sats/vb)
        drain_weight in 100..=500_u32,      // drain weight (wu)
        drain_spend_weight in 1..=1000_u32, // drain spend weight (wu)
        drain_dust in 100..=1000_u64,       // drain dust (sats)
    ) {
        println!("== TEST ==");

        let params = common::StrategyParams {
            n_candidates,
            target_value,
            base_weight,
            min_fee,
            feerate,
            feerate_lt_diff,
            drain_weight,
            drain_spend_weight,
            drain_dust,
        };
        println!("{:?}", params);

        let candidates = core::iter::repeat(Candidate {
                value: 20_000,
                weight: (32 + 4 + 4 + 1) * 4 + 64 + 32,
                input_count: 1,
                is_segwit: true,
            })
            .take(params.n_candidates)
            .collect::<Vec<_>>();

        let mut cs = CoinSelector::new(&candidates, params.base_weight);

        let metric = WasteChangeless::new(params.target(), params.long_term_feerate());

        let (score, rounds) = common::bnb_search(&mut cs, metric, params.n_candidates)?;
        println!("\t\tscore={} rounds={}", score, rounds);
        prop_assert!(rounds <= params.n_candidates);
    }

}
