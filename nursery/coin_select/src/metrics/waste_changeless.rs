use crate::{float::Ordf32, BnbMetric, Candidate, CoinSelector, Drain, FeeRate, Target};

const NO_EXCESS: f32 = 0.0;
const WITH_EXCESS: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
pub struct WasteChangeless {
    target: Target,
    long_term_feerate: FeeRate,

    /// Contains the sorted index of the first candidate with a negative effective value.
    ///
    /// NOTE: This is the SORTED index, not the original index.
    negative_ev_index: Option<usize>,
}

impl WasteChangeless {
    pub fn new(target: Target, long_term_feerate: FeeRate) -> Self {
        Self {
            target,
            long_term_feerate,
            negative_ev_index: None,
        }
    }

    /// Private as this depends on `cs` being sorted.
    fn remaining_negative_evs<'a>(
        &mut self,
        cs: &'a CoinSelector<'_>,
    ) -> impl Iterator<Item = (usize, Candidate)> + DoubleEndedIterator + 'a {
        let sorted_start_index = match self.negative_ev_index {
            Some(v) => v,
            None => {
                let index = cs
                    .candidates()
                    .position(|(_, c)| c.effective_value(self.target.feerate).0 < 0.0)
                    .unwrap_or(cs.candidates().len());
                self.negative_ev_index = Some(index);
                index
            }
        };

        cs.candidates()
            .skip(sorted_start_index)
            .filter(move |(i, _)| !cs.banned().contains(i))
    }
}

impl BnbMetric for WasteChangeless {
    type Score = Ordf32;

    fn score(&mut self, cs: &crate::CoinSelector<'_>) -> Option<Self::Score> {
        let no_drain = Drain::none();

        if !cs.is_target_met(self.target, no_drain) {
            return None;
        }

        Some(Ordf32(cs.waste(
            self.target,
            self.long_term_feerate,
            no_drain,
            WITH_EXCESS,
        )))
    }

    fn bound(&mut self, cs: &crate::CoinSelector<'_>) -> Option<Self::Score> {
        let no_drain = Drain::none();

        // input_waste = input_weight * (feerate - long_term_feerate)
        // total_waste = sum(input_waste..) + excess
        let rate_diff = self.target.feerate.spwu() - self.long_term_feerate.spwu();

        let mut cs = cs.clone();

        // select until target met
        let prev_count = cs.selected().len();
        cs.select_until_target_met(self.target, no_drain).ok()?;
        let newly_selected = cs.selected().len() - prev_count;

        // initial lower bound is just the selection
        let mut lb = Ordf32(cs.waste(self.target, self.long_term_feerate, no_drain, WITH_EXCESS));

        if rate_diff >= 0.0 {
            let can_slurp = newly_selected > 0;
            if can_slurp {
                let mut slurp_cs = cs.clone();

                let (slurp_index, candidate_to_slurp) =
                    slurp_cs.selected().last().expect("must have selection");
                slurp_cs.deselect(slurp_index);

                let perfect_excess = Ord::max(
                    slurp_cs.rate_excess(self.target, no_drain),
                    slurp_cs.absolute_excess(self.target, no_drain),
                );

                let old_input_weight = candidate_to_slurp.weight as f32;
                let perfect_input_weight = slurp(self.target, perfect_excess, candidate_to_slurp);

                let slurped_lb = Ordf32(
                    slurp_cs.waste(self.target, self.long_term_feerate, no_drain, NO_EXCESS)
                        + (perfect_input_weight - old_input_weight) * self.target.feerate.spwu(),
                );

                lb = lb.min(slurped_lb);
                return Some(lb);
            }

            // try adding candidates with (-ev) to minimize excess!
            // select until target is no longer met, then slurp!
            for (index, candidate) in self.remaining_negative_evs(&cs.clone()).rev() {
                cs.select(index);

                if cs.is_target_met(self.target, no_drain) {
                    // target is still met
                    lb = lb.min(Ordf32(cs.waste(
                        self.target,
                        self.long_term_feerate,
                        no_drain,
                        WITH_EXCESS,
                    )));
                    continue;
                }

                cs.deselect(index);
                let perfect_excess = Ord::max(
                    cs.rate_excess(self.target, no_drain),
                    cs.absolute_excess(self.target, no_drain),
                );
                let perfect_input_weight = slurp(self.target, -perfect_excess, candidate);
                lb = lb.min(Ordf32(
                    cs.waste(self.target, self.long_term_feerate, no_drain, NO_EXCESS)
                        + (perfect_input_weight - candidate.weight as f32)
                            * self.target.feerate.spwu(),
                ));
                break;
            }

            return Some(lb);
        }

        // [todo] the bound for -ve rate-diff is very loose, fix this!

        cs.select_all();
        lb = lb.min(Ordf32(cs.waste(
            self.target,
            self.long_term_feerate,
            no_drain,
            NO_EXCESS,
        )));
        Some(lb)
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}

fn slurp(target: Target, excess: i64, candidate: Candidate) -> f32 {
    let vpw = candidate.value_pwu().0;
    let perfect_weight = -excess as f32 / (vpw - target.feerate.spwu());

    #[cfg(debug_assertions)]
    {
        let perfect_value = (candidate.value as f32 * perfect_weight) / candidate.weight as f32;
        let perfect_vpw = perfect_value / perfect_weight;
        if perfect_vpw.is_nan() {
            assert_eq!(perfect_value, 0.0);
            assert_eq!(perfect_weight, 0.0);
        } else {
            assert!(
                (vpw - perfect_vpw).abs() < 0.01,
                "value:weight ratio must stay the same: vpw={} perfect_vpw={} perfect_value={} perfect_weight={}",
                vpw,
                perfect_vpw,
                perfect_value,
                perfect_weight,
            );
        }
    }

    perfect_weight.max(0.0)
}
