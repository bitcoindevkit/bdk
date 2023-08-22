mod common;
use bdk_coin_select::change_policy::min_value_and_waste;
use bdk_coin_select::metrics::LowestFee;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig {
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
        common::can_eventually_find_best_solution(
            common::gen_candidates,
            |p| min_value_and_waste(
                p.drain_weights(),
                p.drain_dust,
                p.long_term_feerate(),
            ),
            |p, cp| LowestFee {
                target: p.target(),
                long_term_feerate: p.long_term_feerate(),
                // [TODO]: Remove this memory leak hack
                change_policy: Box::leak(Box::new(cp)),
            },
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
    fn ensure_bound_is_not_too_tight(
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
        common::ensure_bound_is_not_too_tight(
            common::gen_candidates,
            |p| min_value_and_waste(
                p.drain_weights(),
                p.drain_dust,
                p.long_term_feerate(),
            ),
            |p, cp| LowestFee {
                target: p.target(),
                long_term_feerate: p.long_term_feerate(),
                // [TODO]: Remove this memory leak hack
                change_policy: Box::leak(Box::new(cp)),
            },
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
}
