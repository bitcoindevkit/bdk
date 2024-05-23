use super::*;

/// Strategy in which we should branch.
pub enum BranchStrategy {
    /// We continue exploring subtrees of this node, starting with the inclusion branch.
    Continue,
    /// We continue exploring ONLY the omission branch of this node, skipping the inclusion branch.
    SkipInclusion,
    /// We skip both the inclusion and omission branches of this node.
    SkipBoth,
}

impl BranchStrategy {
    pub fn will_continue(&self) -> bool {
        matches!(self, Self::Continue | Self::SkipInclusion)
    }
}

/// Closure to decide the branching strategy, alongside a score (if the current selection is a
/// candidate solution).
pub type DecideStrategy<'c, S> = dyn Fn(&Bnb<'c, S>) -> (BranchStrategy, Option<S>);

/// [`Bnb`] represents the current state of the BnB algorithm.
pub struct Bnb<'c, S> {
    pub pool: Vec<(usize, &'c WeightedValue)>,
    pub pool_pos: usize,
    pub best_score: S,

    pub selection: CoinSelector<'c>,
    pub rem_abs: u64,
    pub rem_eff: i64,
}

impl<'c, S: Ord> Bnb<'c, S> {
    /// Creates a new [`Bnb`].
    pub fn new(selector: CoinSelector<'c>, pool: Vec<(usize, &'c WeightedValue)>, max: S) -> Self {
        let (rem_abs, rem_eff) = pool.iter().fold((0, 0), |(abs, eff), (_, c)| {
            (
                abs + c.value,
                eff + c.effective_value(selector.opts.target_feerate),
            )
        });

        Self {
            pool,
            pool_pos: 0,
            best_score: max,
            selection: selector,
            rem_abs,
            rem_eff,
        }
    }

    /// Turns our [`Bnb`] state into an iterator.
    ///
    /// `strategy` should assess our current selection/node and determine the branching strategy and
    /// whether this selection is a candidate solution (if so, return the selection score).
    pub fn into_iter<'f>(self, strategy: &'f DecideStrategy<'c, S>) -> BnbIter<'c, 'f, S> {
        BnbIter {
            state: self,
            done: false,
            strategy,
        }
    }

    /// Attempt to backtrack to the previously selected node's omission branch, return false
    /// otherwise (no more solutions).
    pub fn backtrack(&mut self) -> bool {
        (0..self.pool_pos).rev().any(|pos| {
            let (index, candidate) = self.pool[pos];

            if self.selection.is_selected(index) {
                // deselect the last `pos`, so the next round will check the omission branch
                self.pool_pos = pos;
                self.selection.deselect(index);
                true
            } else {
                self.rem_abs += candidate.value;
                self.rem_eff += candidate.effective_value(self.selection.opts.target_feerate);
                false
            }
        })
    }

    /// Continue down this branch and skip the inclusion branch if specified.
    pub fn forward(&mut self, skip: bool) {
        let (index, candidate) = self.pool[self.pool_pos];
        self.rem_abs -= candidate.value;
        self.rem_eff -= candidate.effective_value(self.selection.opts.target_feerate);

        if !skip {
            self.selection.select(index);
        }
    }

    /// Compare the advertised score with the current best. The new best will be the smaller value. Return true
    /// if best is replaced.
    pub fn advertise_new_score(&mut self, score: S) -> bool {
        if score <= self.best_score {
            self.best_score = score;
            return true;
        }
        false
    }
}

pub struct BnbIter<'c, 'f, S> {
    state: Bnb<'c, S>,
    done: bool,

    /// Check our current selection (node) and returns the branching strategy alongside a score
    /// (if the current selection is a candidate solution).
    strategy: &'f DecideStrategy<'c, S>,
}

impl<'c, 'f, S: Ord + Copy + Display> Iterator for BnbIter<'c, 'f, S> {
    type Item = Option<CoinSelector<'c>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let (strategy, score) = (self.strategy)(&self.state);

        let mut found_best = Option::<CoinSelector>::None;

        if let Some(score) = score {
            if self.state.advertise_new_score(score) {
                found_best = Some(self.state.selection.clone());
            }
        }

        debug_assert!(
            !strategy.will_continue() || self.state.pool_pos < self.state.pool.len(),
            "Faulty strategy implementation! Strategy suggested that we continue traversing, however, we have already reached the end of the candidates pool! pool_len={}, pool_pos={}",
            self.state.pool.len(), self.state.pool_pos,
        );

        match strategy {
            BranchStrategy::Continue => {
                self.state.forward(false);
            }
            BranchStrategy::SkipInclusion => {
                self.state.forward(true);
            }
            BranchStrategy::SkipBoth => {
                if !self.state.backtrack() {
                    self.done = true;
                }
            }
        };

        // increment selection pool position for next round
        self.state.pool_pos += 1;

        if found_best.is_some() || !self.done {
            Some(found_best)
        } else {
            // we have traversed all branches
            None
        }
    }
}

/// Determines how we should limit rounds of branch and bound.
pub enum BnbLimit {
    Rounds(usize),
    #[cfg(feature = "std")]
    Duration(core::time::Duration),
}

impl From<usize> for BnbLimit {
    fn from(v: usize) -> Self {
        Self::Rounds(v)
    }
}

#[cfg(feature = "std")]
impl From<core::time::Duration> for BnbLimit {
    fn from(v: core::time::Duration) -> Self {
        Self::Duration(v)
    }
}

/// This is a variation of the Branch and Bound Coin Selection algorithm designed by Murch (as seen
/// in Bitcoin Core).
///
/// The differences are as follows:
/// * In addition to working with effective values, we also work with absolute values.
///   This way, we can use bounds of the absolute values to enforce `min_absolute_fee` (which is used by
///   RBF), and `max_extra_target` (which can be used to increase the possible solution set, given
///   that the sender is okay with sending extra to the receiver).
///
/// Murch's Master Thesis: <https://murch.one/wp-content/uploads/2016/11/erhardt2016coinselection.pdf>
/// Bitcoin Core Implementation: <https://github.com/bitcoin/bitcoin/blob/23.x/src/wallet/coinselection.cpp#L65>
///
/// TODO: Another optimization we could do is figure out candidates with the smallest waste, and
/// if we find a result with waste equal to this, we can just break.
pub fn coin_select_bnb<L>(limit: L, selector: CoinSelector) -> Option<CoinSelector>
where
    L: Into<BnbLimit>,
{
    let opts = selector.opts;

    // prepare the pool of candidates to select from:
    // * filter out candidates with negative/zero effective values
    // * sort candidates by descending effective value
    let pool = {
        let mut pool = selector
            .unselected()
            .filter(|(_, c)| c.effective_value(opts.target_feerate) > 0)
            .collect::<Vec<_>>();
        pool.sort_unstable_by(|(_, a), (_, b)| {
            let a = a.effective_value(opts.target_feerate);
            let b = b.effective_value(opts.target_feerate);
            b.cmp(&a)
        });
        pool
    };

    let feerate_decreases = opts.target_feerate > opts.long_term_feerate();

    let target_abs = opts.target_value.unwrap_or(0) + opts.min_absolute_fee;
    let target_eff = selector.effective_target();

    let upper_bound_abs = target_abs + (opts.drain_weight as f32 * opts.target_feerate) as u64;
    let upper_bound_eff = target_eff + opts.drain_waste();

    let strategy = move |bnb: &Bnb<i64>| -> (BranchStrategy, Option<i64>) {
        let selected_abs = bnb.selection.selected_absolute_value();
        let selected_eff = bnb.selection.selected_effective_value();

        // backtrack if the remaining value is not enough to reach the target
        if selected_abs + bnb.rem_abs < target_abs || selected_eff + bnb.rem_eff < target_eff {
            return (BranchStrategy::SkipBoth, None);
        }

        // backtrack if the selected value has already surpassed upper bounds
        if selected_abs > upper_bound_abs && selected_eff > upper_bound_eff {
            return (BranchStrategy::SkipBoth, None);
        }

        let selected_waste = bnb.selection.selected_waste();

        // when feerate decreases, waste without excess is guaranteed to increase with each
        // selection. So if we have already surpassed the best score, we can backtrack.
        if feerate_decreases && selected_waste > bnb.best_score {
            return (BranchStrategy::SkipBoth, None);
        }

        // solution?
        if selected_abs >= target_abs && selected_eff >= target_eff {
            let waste = selected_waste + bnb.selection.current_excess();
            return (BranchStrategy::SkipBoth, Some(waste));
        }

        // early bailout optimization:
        // If the candidate at the previous position is NOT selected and has the same weight and
        // value as the current candidate, we can skip selecting the current candidate.
        if bnb.pool_pos > 0 && !bnb.selection.is_empty() {
            let (_, candidate) = bnb.pool[bnb.pool_pos];
            let (prev_index, prev_candidate) = bnb.pool[bnb.pool_pos - 1];

            if !bnb.selection.is_selected(prev_index)
                && candidate.value == prev_candidate.value
                && candidate.weight == prev_candidate.weight
            {
                return (BranchStrategy::SkipInclusion, None);
            }
        }

        // check out the inclusion branch first
        (BranchStrategy::Continue, None)
    };

    // determine the sum of absolute and effective values for the current selection
    let (selected_abs, selected_eff) = selector.selected().fold((0, 0), |(abs, eff), (_, c)| {
        (
            abs + c.value,
            eff + c.effective_value(selector.opts.target_feerate),
        )
    });

    let bnb = Bnb::new(selector, pool, i64::MAX);

    // not enough to select anyway
    if selected_abs + bnb.rem_abs < target_abs || selected_eff + bnb.rem_eff < target_eff {
        return None;
    }

    match limit.into() {
        BnbLimit::Rounds(rounds) => {
            bnb.into_iter(&strategy)
                .take(rounds)
                .reduce(|b, c| if c.is_some() { c } else { b })
        }
        #[cfg(feature = "std")]
        BnbLimit::Duration(duration) => {
            let start = std::time::SystemTime::now();
            bnb.into_iter(&strategy)
                .take_while(|_| start.elapsed().expect("failed to get system time") <= duration)
                .reduce(|b, c| if c.is_some() { c } else { b })
        }
    }?
}

// #[cfg(all(test, feature = "miniscript"))]
// mod test {
//     use bitcoin::secp256k1::Secp256k1;
//
//     use crate::coin_select::{evaluate_cs::evaluate, ExcessStrategyKind};
//
//     use super::{
//         coin_select_bnb,
//         evaluate_cs::{Evaluation, EvaluationError},
//         tester::Tester,
//         CoinSelector, CoinSelectorOpt, Vec, WeightedValue,
//     };
//
//     fn tester() -> Tester {
//         const DESC_STR: &str = "tr(xprv9uBuvtdjghkz8D1qzsSXS9Vs64mqrUnXqzNccj2xcvnCHPpXKYE1U2Gbh9CDHk8UPyF2VuXpVkDA7fk5ZP4Hd9KnhUmTscKmhee9Dp5sBMK)";
//         Tester::new(&Secp256k1::default(), DESC_STR)
//     }
//
//     fn evaluate_bnb(
//         initial_selector: CoinSelector,
//         max_tries: usize,
//     ) -> Result<Evaluation, EvaluationError> {
//         evaluate(initial_selector, |cs| {
//             coin_select_bnb(max_tries, cs.clone()).map_or(false, |new_cs| {
//                 *cs = new_cs;
//                 true
//             })
//         })
//     }
//
//     #[test]
//     fn not_enough_coins() {
//         let t = tester();
//         let candidates: Vec<WeightedValue> = vec![
//             t.gen_candidate(0, 100_000).into(),
//             t.gen_candidate(1, 100_000).into(),
//         ];
//         let opts = t.gen_opts(200_000);
//         let selector = CoinSelector::new(&candidates, &opts);
//         assert!(!coin_select_bnb(10_000, selector).is_some());
//     }
//
//     #[test]
//     fn exactly_enough_coins_preselected() {
//         let t = tester();
//         let candidates: Vec<WeightedValue> = vec![
//             t.gen_candidate(0, 100_000).into(), // to preselect
//             t.gen_candidate(1, 100_000).into(), // to preselect
//             t.gen_candidate(2, 100_000).into(),
//         ];
//         let opts = CoinSelectorOpt {
//             target_feerate: 0.0,
//             ..t.gen_opts(200_000)
//         };
//         let selector = {
//             let mut selector = CoinSelector::new(&candidates, &opts);
//             selector.select(0); // preselect
//             selector.select(1); // preselect
//             selector
//         };
//
//         let evaluation = evaluate_bnb(selector, 10_000).expect("eval failed");
//         println!("{}", evaluation);
//         assert_eq!(evaluation.solution.selected, (0..=1).collect());
//         assert_eq!(evaluation.solution.excess_strategies.len(), 1);
//         assert_eq!(
//             evaluation.feerate_offset(ExcessStrategyKind::ToFee).floor(),
//             0.0
//         );
//     }
//
//     /// `cost_of_change` acts as the upper-bound in Bnb; we check whether these boundaries are
//     /// enforced in code
//     #[test]
//     fn cost_of_change() {
//         let t = tester();
//         let candidates: Vec<WeightedValue> = vec![
//             t.gen_candidate(0, 200_000).into(),
//             t.gen_candidate(1, 200_000).into(),
//             t.gen_candidate(2, 200_000).into(),
//         ];
//
//         // lowest and highest possible `recipient_value` opts for derived `drain_waste`, assuming
//         // that we want 2 candidates selected
//         let (lowest_opts, highest_opts) = {
//             let opts = t.gen_opts(0);
//
//             let fee_from_inputs =
//                 (candidates[0].weight as f32 * opts.target_feerate).ceil() as u64 * 2;
//             let fee_from_template =
//                 ((opts.base_weight + 2) as f32 * opts.target_feerate).ceil() as u64;
//
//             let lowest_opts = CoinSelectorOpt {
//                 target_value: Some(
//                     400_000 - fee_from_inputs - fee_from_template - opts.drain_waste() as u64,
//                 ),
//                 ..opts
//             };
//
//             let highest_opts = CoinSelectorOpt {
//                 target_value: Some(400_000 - fee_from_inputs - fee_from_template),
//                 ..opts
//             };
//
//             (lowest_opts, highest_opts)
//         };
//
//         // test lowest possible target we can select
//         let lowest_eval = evaluate_bnb(CoinSelector::new(&candidates, &lowest_opts), 10_000);
//         assert!(lowest_eval.is_ok());
//         let lowest_eval = lowest_eval.unwrap();
//         println!("LB {}", lowest_eval);
//         assert_eq!(lowest_eval.solution.selected.len(), 2);
//         assert_eq!(lowest_eval.solution.excess_strategies.len(), 1);
//         assert_eq!(
//             lowest_eval
//                 .feerate_offset(ExcessStrategyKind::ToFee)
//                 .floor(),
//             0.0
//         );
//
//         // test the highest possible target we can select
//         let highest_eval = evaluate_bnb(CoinSelector::new(&candidates, &highest_opts), 10_000);
//         assert!(highest_eval.is_ok());
//         let highest_eval = highest_eval.unwrap();
//         println!("UB {}", highest_eval);
//         assert_eq!(highest_eval.solution.selected.len(), 2);
//         assert_eq!(highest_eval.solution.excess_strategies.len(), 1);
//         assert_eq!(
//             highest_eval
//                 .feerate_offset(ExcessStrategyKind::ToFee)
//                 .floor(),
//             0.0
//         );
//
//         // test lower out of bounds
//         let loob_opts = CoinSelectorOpt {
//             target_value: lowest_opts.target_value.map(|v| v - 1),
//             ..lowest_opts
//         };
//         let loob_eval = evaluate_bnb(CoinSelector::new(&candidates, &loob_opts), 10_000);
//         assert!(loob_eval.is_err());
//         println!("Lower OOB: {}", loob_eval.unwrap_err());
//
//         // test upper out of bounds
//         let uoob_opts = CoinSelectorOpt {
//             target_value: highest_opts.target_value.map(|v| v + 1),
//             ..highest_opts
//         };
//         let uoob_eval = evaluate_bnb(CoinSelector::new(&candidates, &uoob_opts), 10_000);
//         assert!(uoob_eval.is_err());
//         println!("Upper OOB: {}", uoob_eval.unwrap_err());
//     }
//
//     #[test]
//     fn try_select() {
//         let t = tester();
//         let candidates: Vec<WeightedValue> = vec![
//             t.gen_candidate(0, 300_000).into(),
//             t.gen_candidate(1, 300_000).into(),
//             t.gen_candidate(2, 300_000).into(),
//             t.gen_candidate(3, 200_000).into(),
//             t.gen_candidate(4, 200_000).into(),
//         ];
//         let make_opts = |v: u64| -> CoinSelectorOpt {
//             CoinSelectorOpt {
//                 target_feerate: 0.0,
//                 ..t.gen_opts(v)
//             }
//         };
//
//         let test_cases = vec![
//             (make_opts(100_000), false, 0),
//             (make_opts(200_000), true, 1),
//             (make_opts(300_000), true, 1),
//             (make_opts(500_000), true, 2),
//             (make_opts(1_000_000), true, 4),
//             (make_opts(1_200_000), false, 0),
//             (make_opts(1_300_000), true, 5),
//             (make_opts(1_400_000), false, 0),
//         ];
//
//         for (opts, expect_solution, expect_selected) in test_cases {
//             let res = evaluate_bnb(CoinSelector::new(&candidates, &opts), 10_000);
//             assert_eq!(res.is_ok(), expect_solution);
//
//             match res {
//                 Ok(eval) => {
//                     println!("{}", eval);
//                     assert_eq!(eval.feerate_offset(ExcessStrategyKind::ToFee), 0.0);
//                     assert_eq!(eval.solution.selected.len(), expect_selected as _);
//                 }
//                 Err(err) => println!("expected failure: {}", err),
//             }
//         }
//     }
//
//     #[test]
//     fn early_bailout_optimization() {
//         let t = tester();
//
//         // target: 300_000
//         // candidates: 2x of 125_000, 1000x of 100_000, 1x of 50_000
//         // expected solution: 2x 125_000, 1x 50_000
//         // set bnb max tries: 1100, should succeed
//         let candidates = {
//             let mut candidates: Vec<WeightedValue> = vec![
//                 t.gen_candidate(0, 125_000).into(),
//                 t.gen_candidate(1, 125_000).into(),
//                 t.gen_candidate(2, 50_000).into(),
//             ];
//             (3..3 + 1000_u32)
//                 .for_each(|index| candidates.push(t.gen_candidate(index, 100_000).into()));
//             candidates
//         };
//         let opts = CoinSelectorOpt {
//             target_feerate: 0.0,
//             ..t.gen_opts(300_000)
//         };
//
//         let result = evaluate_bnb(CoinSelector::new(&candidates, &opts), 1100);
//         assert!(result.is_ok());
//
//         let eval = result.unwrap();
//         println!("{}", eval);
//         assert_eq!(eval.solution.selected, (0..=2).collect());
//     }
//
//     #[test]
//     fn should_exhaust_iteration() {
//         static MAX_TRIES: usize = 1000;
//         let t = tester();
//         let candidates = (0..MAX_TRIES + 1)
//             .map(|index| t.gen_candidate(index as _, 10_000).into())
//             .collect::<Vec<WeightedValue>>();
//         let opts = t.gen_opts(10_001 * MAX_TRIES as u64);
//         let result = evaluate_bnb(CoinSelector::new(&candidates, &opts), MAX_TRIES);
//         assert!(result.is_err());
//         println!("error as expected: {}", result.unwrap_err());
//     }
//
//     /// Solution should have fee >= min_absolute_fee (or no solution at all)
//     #[test]
//     fn min_absolute_fee() {
//         let t = tester();
//         let candidates = {
//             let mut candidates = Vec::new();
//             t.gen_weighted_values(&mut candidates, 5, 10_000);
//             t.gen_weighted_values(&mut candidates, 5, 20_000);
//             t.gen_weighted_values(&mut candidates, 5, 30_000);
//             t.gen_weighted_values(&mut candidates, 10, 10_300);
//             t.gen_weighted_values(&mut candidates, 10, 10_500);
//             t.gen_weighted_values(&mut candidates, 10, 10_700);
//             t.gen_weighted_values(&mut candidates, 10, 10_900);
//             t.gen_weighted_values(&mut candidates, 10, 11_000);
//             t.gen_weighted_values(&mut candidates, 10, 12_000);
//             t.gen_weighted_values(&mut candidates, 10, 13_000);
//             candidates
//         };
//         let mut opts = CoinSelectorOpt {
//             min_absolute_fee: 1,
//             ..t.gen_opts(100_000)
//         };
//
//         (1..=120_u64).for_each(|fee_factor| {
//             opts.min_absolute_fee = fee_factor * 31;
//
//             let result = evaluate_bnb(CoinSelector::new(&candidates, &opts), 21_000);
//             match result {
//                 Ok(result) => {
//                     println!("Solution {}", result);
//                     let fee = result.solution.excess_strategies[&ExcessStrategyKind::ToFee].fee;
//                     assert!(fee >= opts.min_absolute_fee);
//                     assert_eq!(result.solution.excess_strategies.len(), 1);
//                 }
//                 Err(err) => {
//                     println!("No Solution: {}", err);
//                 }
//             }
//         });
//     }
//
//     /// For a decreasing feerate (long-term feerate is lower than effective feerate), we should
//     /// select less. For increasing feerate (long-term feerate is higher than effective feerate), we
//     /// should select more.
//     #[test]
//     fn feerate_difference() {
//         let t = tester();
//         let candidates = {
//             let mut candidates = Vec::new();
//             t.gen_weighted_values(&mut candidates, 10, 2_000);
//             t.gen_weighted_values(&mut candidates, 10, 5_000);
//             t.gen_weighted_values(&mut candidates, 10, 20_000);
//             candidates
//         };
//
//         let decreasing_feerate_opts = CoinSelectorOpt {
//             target_feerate: 1.25,
//             long_term_feerate: Some(0.25),
//             ..t.gen_opts(100_000)
//         };
//
//         let increasing_feerate_opts = CoinSelectorOpt {
//             target_feerate: 0.25,
//             long_term_feerate: Some(1.25),
//             ..t.gen_opts(100_000)
//         };
//
//         let decreasing_res = evaluate_bnb(
//             CoinSelector::new(&candidates, &decreasing_feerate_opts),
//             21_000,
//         )
//         .expect("no result");
//         let decreasing_len = decreasing_res.solution.selected.len();
//
//         let increasing_res = evaluate_bnb(
//             CoinSelector::new(&candidates, &increasing_feerate_opts),
//             21_000,
//         )
//         .expect("no result");
//         let increasing_len = increasing_res.solution.selected.len();
//
//         println!("decreasing_len: {}", decreasing_len);
//         println!("increasing_len: {}", increasing_len);
//         assert!(decreasing_len < increasing_len);
//     }
//
//     /// TODO: UNIMPLEMENTED TESTS:
//     /// * Excess strategies:
//     ///     * We should always have `ExcessStrategy::ToFee`.
//     ///     * We should only have `ExcessStrategy::ToRecipient` when `max_extra_target > 0`.
//     ///     * We should only have `ExcessStrategy::ToDrain` when `drain_value >= min_drain_value`.
//     /// * Fuzz
//     ///     * Solution feerate should never be lower than target feerate
//     ///     * Solution fee should never be lower than `min_absolute_fee`.
//     ///     * Preselected should always remain selected
//     fn _todo() {}
// }
