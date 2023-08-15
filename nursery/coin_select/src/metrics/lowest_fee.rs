use crate::{
    float::Ordf32, metrics::change_lower_bound, BnbMetric, Candidate, CoinSelector, Drain,
    DrainWeights, FeeRate, Target,
};

pub struct LowestFee<'c, C> {
    pub target: Target,
    pub long_term_feerate: FeeRate,
    pub change_policy: &'c C,
}

impl<'c, C> LowestFee<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    fn calculate_metric(&self, cs: &CoinSelector<'_>, drain_weights: Option<DrainWeights>) -> f32 {
        match drain_weights {
            // with change
            Some(drain_weights) => {
                (cs.input_weight() + drain_weights.output_weight) as f32
                    * self.target.feerate.spwu()
                    + drain_weights.spend_weight as f32 * self.long_term_feerate.spwu()
            }
            // changeless
            None => {
                cs.input_weight() as f32 * self.target.feerate.spwu()
                    + (cs.selected_value() - self.target.value) as f32
            }
        }
    }
}

impl<'c, C> BnbMetric for LowestFee<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    type Score = Ordf32;

    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score> {
        let drain = (self.change_policy)(cs, self.target);
        if !cs.is_target_met(self.target, drain) {
            return None;
        }

        let drain_weights = if drain.is_some() {
            Some(drain.weights)
        } else {
            None
        };

        Some(Ordf32(self.calculate_metric(cs, drain_weights)))
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score> {
        // this either returns:
        // * None: change output may or may not exist
        // * Some: change output must exist from this branch onwards
        let change_lb = change_lower_bound(cs, self.target, &self.change_policy);
        let change_lb_weights = if change_lb.is_some() {
            Some(change_lb.weights)
        } else {
            None
        };

        if cs.is_target_met(self.target, change_lb) {
            // Target is met, is it possible to add further inputs to remove drain output?
            // If we do, can we get a better score?

            // First lower bound candidate is just the selection itself.
            let mut lower_bound = self.calculate_metric(cs, change_lb_weights);

            // Since a changeless solution may exist, we should try reduce the excess
            if change_lb.is_none() {
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
                            let perfect_input_weight =
                                slurp_candidate(finishing_input, excess, self.target.feerate);

                            (cs.input_weight() as f32 + perfect_input_weight)
                                * self.target.feerate.spwu()
                        }
                        None => self.calculate_metric(&cs, None),
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

        let perfect_excess = i64::min(
            cs.rate_excess(self.target, Drain::none()),
            cs.absolute_excess(self.target, Drain::none()),
        );

        match change_lb_weights {
            // must have change!
            Some(change_weights) => {
                // [todo] This will not be perfect, just a placeholder for now
                let lowest_fee = (cs.input_weight() + change_weights.output_weight) as f32
                    * self.target.feerate.spwu()
                    + change_weights.spend_weight as f32 * self.long_term_feerate.spwu();

                Some(Ordf32(lowest_fee))
            }
            // can be changeless!
            None => {
                // use the lowest excess to find "perfect candidate weight"
                let perfect_input_weight =
                    slurp_candidate(candidate_to_slurp, perfect_excess, self.target.feerate);

                // the perfect input weight canned the excess and we assume no change
                let lowest_fee =
                    (cs.input_weight() as f32 + perfect_input_weight) * self.target.feerate.spwu();

                Some(Ordf32(lowest_fee))
            }
        }
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}

fn slurp_candidate(candidate: Candidate, excess: i64, feerate: FeeRate) -> f32 {
    let candidate_weight = candidate.weight as f32;

    // this equation is dervied from:
    // * `input_effective_value = input_value - input_weight * feerate`
    // * `input_value * new_input_weight = new_input_value * input_weight`
    //      (ensure we have the same value:weight ratio)
    // where we want `input_effective_value` to match `-excess`.
    let perfect_weight = -(candidate_weight * excess as f32)
        / (candidate.value as f32 - candidate_weight * feerate.spwu());

    debug_assert!(perfect_weight <= candidate_weight);

    // we can't allow the weight to go negative
    perfect_weight.min(0.0)
}
