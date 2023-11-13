use crate::{
    float::Ordf32, metrics::change_lower_bound, BnbMetric, Candidate, CoinSelector, Drain,
    DrainWeights, FeeRate, Target,
};

/// Metric that aims to minimize transaction fees. The future fee for spending the change output is
/// included in this calculation.
///
/// The scoring function for changeless solutions is:
/// > input_weight * feerate + excess
///
/// The scoring function for solutions with change:
/// > (input_weight + change_output_weight) * feerate + change_spend_weight * long_term_feerate
pub struct LowestFee<'c, C> {
    /// The target parameters for the resultant selection.
    pub target: Target,
    /// The estimated feerate needed to spend our change output later.
    pub long_term_feerate: FeeRate,
    /// Policy to determine the change output (if any) of a given selection.
    pub change_policy: &'c C,
}

impl<'c, C> Clone for LowestFee<'c, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'c, C> Copy for LowestFee<'c, C> {}

impl<'c, C> LowestFee<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    fn calc_metric(&self, cs: &CoinSelector<'_>, drain_weights: Option<DrainWeights>) -> f32 {
        self.calc_metric_lb(cs, drain_weights)
            + match drain_weights {
                Some(_) => {
                    let selected_value = cs.selected_value();
                    assert!(selected_value >= self.target.value);
                    (cs.selected_value() - self.target.value) as f32
                }
                None => 0.0,
            }
    }

    fn calc_metric_lb(&self, cs: &CoinSelector<'_>, drain_weights: Option<DrainWeights>) -> f32 {
        match drain_weights {
            // with change
            Some(drain_weights) => {
                (cs.input_weight() + drain_weights.output_weight) as f32
                    * self.target.feerate.spwu()
                    + drain_weights.spend_weight as f32 * self.long_term_feerate.spwu()
            }
            // changeless
            None => cs.input_weight() as f32 * self.target.feerate.spwu(),
        }
    }
}

impl<'c, C> BnbMetric for LowestFee<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Ordf32> {
        let drain = (self.change_policy)(cs, self.target);
        if !cs.is_target_met(self.target, drain) {
            return None;
        }

        let drain_weights = if drain.is_some() {
            Some(drain.weights)
        } else {
            None
        };

        Some(Ordf32(self.calc_metric(cs, drain_weights)))
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Ordf32> {
        // this either returns:
        // * None: change output may or may not exist
        // * Some: change output must exist from this branch onwards
        let change_lb = change_lower_bound(cs, self.target, &self.change_policy);
        let change_lb_weights = if change_lb.is_some() {
            Some(change_lb.weights)
        } else {
            None
        };
        // println!("\tchange lb: {:?}", change_lb_weights);

        if cs.is_target_met(self.target, change_lb) {
            // Target is met, is it possible to add further inputs to remove drain output?
            // If we do, can we get a better score?

            // First lower bound candidate is just the selection itself (include excess).
            let mut lower_bound = self.calc_metric(cs, change_lb_weights);

            if change_lb_weights.is_none() {
                // Since a changeless solution may exist, we should try minimize the excess with by
                // adding as much -ev candidates as possible
                let selection_with_as_much_negative_ev_as_possible = cs
                    .clone()
                    .select_iter()
                    .rev()
                    .take_while(|(cs, _, candidate)| {
                        candidate.effective_value(self.target.feerate).0 < 0.0
                            && cs.is_target_met(self.target, Drain::none())
                    })
                    .last()
                    .map(|(cs, _, _)| cs);

                if let Some(cs) = selection_with_as_much_negative_ev_as_possible {
                    // we have selected as much "real" inputs as possible, is it possible to select
                    // one more with the perfect weight?
                    let can_do_better_by_slurping =
                        cs.unselected().next_back().and_then(|(_, candidate)| {
                            if candidate.effective_value(self.target.feerate).0 < 0.0 {
                                Some(candidate)
                            } else {
                                None
                            }
                        });
                    let lower_bound_changeless = match can_do_better_by_slurping {
                        Some(finishing_input) => {
                            let excess = cs.rate_excess(self.target, Drain::none());

                            // change the input's weight to make it's effective value match the excess
                            let perfect_input_weight = slurp(self.target, excess, finishing_input);

                            (cs.input_weight() as f32 + perfect_input_weight)
                                * self.target.feerate.spwu()
                        }
                        None => self.calc_metric(&cs, None),
                    };

                    lower_bound = lower_bound.min(lower_bound_changeless)
                }
            }

            return Some(Ordf32(lower_bound));
        }

        // target is not met yet
        // select until we just exceed target, then we slurp the last selection
        let (mut cs, slurp_index, candidate_to_slurp) = cs
            .clone()
            .select_iter()
            .find(|(cs, _, _)| cs.is_target_met(self.target, change_lb))?;
        cs.deselect(slurp_index);

        let mut lower_bound = self.calc_metric_lb(&cs, change_lb_weights);

        // find the max excess we need to rid of
        let perfect_excess = i64::max(
            cs.rate_excess(self.target, Drain::none()),
            cs.absolute_excess(self.target, Drain::none()),
        );
        // use the highest excess to find "perfect candidate weight"
        let perfect_input_weight = slurp(self.target, perfect_excess, candidate_to_slurp);
        lower_bound += perfect_input_weight * self.target.feerate.spwu();

        Some(Ordf32(lower_bound))
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}

fn slurp(target: Target, excess: i64, candidate: Candidate) -> f32 {
    let vpw = candidate.value_pwu().0;
    let perfect_weight = -excess as f32 / (vpw - target.feerate.spwu());
    perfect_weight.max(0.0)
}
