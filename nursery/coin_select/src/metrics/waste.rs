use super::change_lower_bound;
use crate::{bnb::BnbMetric, float::Ordf32, Candidate, CoinSelector, Drain, FeeRate, Target};

/// The "waste" metric used by bitcoin core.
///
/// See this [great explanation](https://bitcoin.stackexchange.com/questions/113622/what-does-waste-metric-mean-in-the-context-of-coin-selection)
/// for an understanding of the waste metric.
///
/// ## WARNING: Waste metric considered wasteful
///
/// Note that bitcoin core at the time of writing use the waste metric to
///
/// 1. minimise the waste while searching for changeless solutions.
/// 2. It tiebreaks multiple valid selections from different algorithms (which do not try and
///    minimise waste) with waste.
///
/// This is **very** different from minimising waste in general which is what this metric will do
/// when used in [`CoinSelector::bnb_solutions`]. The waste metric tends to over consolidate funds.
/// If the `long_term_feerate` is even slightly higher than the current feerate (specified in
/// `target`) it will select all your coins!
pub struct Waste<'c, C> {
    /// The target parameters of the resultant selection.
    pub target: Target,
    /// The longterm feerate as part of the waste metric.
    pub long_term_feerate: FeeRate,
    /// Policy to determine the change output (if any) of a given selection.
    pub change_policy: &'c C,
}

impl<'c, C> Clone for Waste<'c, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'c, C> Copy for Waste<'c, C> {}

impl<'c, C> BnbMetric for Waste<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Ordf32> {
        let drain = (self.change_policy)(cs, self.target);
        if !cs.is_target_met(self.target, drain) {
            return None;
        }
        let score = cs.waste(self.target, self.long_term_feerate, drain, 1.0);
        Some(Ordf32(score))
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Ordf32> {
        // Welcome my bretheren. This dungeon was authored by Lloyd Fournier A.K.A "LLFourn" with
        // the assistance of chat GPT and the developers of the IOTA cryptocurrency. There are
        // comments trying to make sense of the logic here but it's really just me pretending I know
        // what's going on. I have tried to simplify the logic here many times but always end up
        // making it fail proptests.
        //
        // Don't be afraid. This function is a "heuristic" lower bound. It doesn't need to be super
        // duper correct. In testing it seems to come up with pretty good results pretty fast.
        let rate_diff = self.target.feerate.spwu() - self.long_term_feerate.spwu();
        // whether from this coin selection it's possible to avoid change
        let change_lower_bound = change_lower_bound(cs, self.target, &self.change_policy);
        const IGNORE_EXCESS: f32 = 0.0;
        const INCLUDE_EXCESS: f32 = 1.0;

        if rate_diff >= 0.0 {
            // Our lower bound algorithms differ depending on whether we have already met the target or not.
            if cs.is_target_met(self.target, change_lower_bound) {
                let current_change = (self.change_policy)(cs, self.target);

                // first lower bound candidate is just the selection itself
                let mut lower_bound = cs.waste(
                    self.target,
                    self.long_term_feerate,
                    current_change,
                    INCLUDE_EXCESS,
                );

                // But don't stop there we might be able to select negative value inputs which might
                // lower excess and reduce waste either by:
                // - removing the need for a change output
                // - reducing the excess if the current selection is changeless (only possible when rate_diff is small).
                let should_explore_changeless = change_lower_bound.is_none();

                if should_explore_changeless {
                    let selection_with_as_much_negative_ev_as_possible = cs
                        .clone()
                        .select_iter()
                        .rev()
                        .take_while(|(cs, _, wv)| {
                            wv.effective_value(self.target.feerate).0 < 0.0
                                && cs.is_target_met(self.target, Drain::none())
                        })
                        .last();

                    if let Some((cs, _, _)) = selection_with_as_much_negative_ev_as_possible {
                        let can_do_better_by_slurping =
                            cs.unselected().next_back().and_then(|(_, wv)| {
                                if wv.effective_value(self.target.feerate).0 < 0.0 {
                                    Some(wv)
                                } else {
                                    None
                                }
                            });
                        let lower_bound_without_change = match can_do_better_by_slurping {
                            Some(finishing_input) => {
                                // NOTE we are slurping negative value here to try and reduce excess in
                                // the hopes of getting rid of the change output
                                let value_to_slurp = -cs.rate_excess(self.target, Drain::none());
                                let weight_to_extinguish_excess =
                                    slurp_wv(finishing_input, value_to_slurp, self.target.feerate);
                                let waste_to_extinguish_excess =
                                    weight_to_extinguish_excess * rate_diff;
                                // return: waste after excess reduction
                                cs.waste(
                                    self.target,
                                    self.long_term_feerate,
                                    Drain::none(),
                                    IGNORE_EXCESS,
                                ) + waste_to_extinguish_excess
                            }
                            None => cs.waste(
                                self.target,
                                self.long_term_feerate,
                                Drain::none(),
                                INCLUDE_EXCESS,
                            ),
                        };

                        lower_bound = lower_bound.min(lower_bound_without_change);
                    }
                }

                Some(Ordf32(lower_bound))
            } else {
                // If feerate >= long_term_feerate, You *might* think that the waste lower bound
                // here is just the fewest number of inputs we need to meet the target but **no**.
                // Consider if there is 1 sat remaining to reach target. Should you add all the
                // weight of the next input for the waste calculation? *No* this leaads to a
                // pesimistic lower bound even if we ignore the excess because it adds too much
                // weight.
                //
                // Step 1: select everything up until the input that hits the target.
                let (mut cs, slurp_index, to_slurp) = cs
                    .clone()
                    .select_iter()
                    .find(|(cs, _, _)| cs.is_target_met(self.target, change_lower_bound))?;

                cs.deselect(slurp_index);

                // Step 2: We pretend that the final input exactly cancels out the remaining excess
                // by taking whatever value we want from it but at the value per weight of the real
                // input.
                let ideal_next_weight = {
                    // satisfying absolute and feerate constraints requires different calculations so we do them
                    // both independently and find which requires the most weight of the next input.
                    let remaining_rate = cs.rate_excess(self.target, change_lower_bound);
                    let remaining_abs = cs.absolute_excess(self.target, change_lower_bound);

                    let weight_to_satisfy_abs =
                        remaining_abs.min(0) as f32 / to_slurp.value_pwu().0;

                    let weight_to_satisfy_rate =
                        slurp_wv(to_slurp, remaining_rate.min(0), self.target.feerate);

                    let weight_to_satisfy = weight_to_satisfy_abs.max(weight_to_satisfy_rate);
                    debug_assert!(weight_to_satisfy <= to_slurp.weight as f32);
                    weight_to_satisfy
                };
                let weight_lower_bound = cs.input_weight() as f32 + ideal_next_weight;
                let mut waste = weight_lower_bound * rate_diff;
                waste += change_lower_bound.waste(self.target.feerate, self.long_term_feerate);

                Some(Ordf32(waste))
            }
        } else {
            // When long_term_feerate > current feerate each input by itself has negative waste.
            // This doesn't mean that waste monotonically decreases as you add inputs because
            // somewhere along the line adding an input might cause the change policy to add a
            // change ouput which could increase waste.
            //
            // So we have to try two things and we which one is best to find the lower bound:
            // 1. try selecting everything regardless of change
            let mut lower_bound = {
                let mut cs = cs.clone();
                // ... but first check that by selecting all effective we can actually reach target
                cs.select_all_effective(self.target.feerate);
                if !cs.is_target_met(self.target, Drain::none()) {
                    return None;
                }
                let change_at_value_optimum = (self.change_policy)(&cs, self.target);
                cs.select_all();
                // NOTE: we use the change from our "all effective" selection for min waste since
                // selecting all might not have change but in that case we'll catch it below.
                cs.waste(
                    self.target,
                    self.long_term_feerate,
                    change_at_value_optimum,
                    IGNORE_EXCESS,
                )
            };

            let look_for_changeless_solution = change_lower_bound.is_none();

            if look_for_changeless_solution {
                // 2. select the highest weight solution with no change
                let highest_weight_selection_without_change = cs
                    .clone()
                    .select_iter()
                    .rev()
                    .take_while(|(cs, _, wv)| {
                        wv.effective_value(self.target.feerate).0 < 0.0
                            || (self.change_policy)(cs, self.target).is_none()
                    })
                    .last();

                if let Some((cs, _, _)) = highest_weight_selection_without_change {
                    let no_change_waste = cs.waste(
                        self.target,
                        self.long_term_feerate,
                        Drain::none(),
                        IGNORE_EXCESS,
                    );

                    lower_bound = lower_bound.min(no_change_waste)
                }
            }

            Some(Ordf32(lower_bound))
        }
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}

/// Returns the "perfect weight" for this candidate to slurp up a given value with `feerate` while
/// not changing the candidate's value/weight ratio.
///
/// Used to pretend that a candidate had precisely `value_to_slurp` + fee needed to include it. It
/// tells you how much weight such a perfect candidate would have if it had the same value per
/// weight unit as `candidate`. This is useful for estimating a lower weight bound for a perfect
/// match.
fn slurp_wv(candidate: Candidate, value_to_slurp: i64, feerate: FeeRate) -> f32 {
    // the value per weight unit this candidate offers at feerate
    let value_per_wu = (candidate.value as f32 / candidate.weight as f32) - feerate.spwu();
    // return how much weight we need
    let weight_needed = value_to_slurp as f32 / value_per_wu;
    debug_assert!(weight_needed <= candidate.weight as f32);
    weight_needed.min(0.0)
}
