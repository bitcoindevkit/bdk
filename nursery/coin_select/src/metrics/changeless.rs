use super::change_lower_bound;
use crate::{bnb::BnbMetric, float::Ordf32, CoinSelector, Drain, Target};

/// Metric for finding changeless solutions only.
pub struct Changeless<'c, C> {
    /// The target parameters for the resultant selection.
    pub target: Target,
    /// Policy to determine whether a selection requires a change output.
    pub change_policy: &'c C,
}

impl<'c, C> BnbMetric for Changeless<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Ordf32> {
        let drain = (self.change_policy)(cs, self.target);
        if cs.is_target_met(self.target, drain) && (*self.change_policy)(cs, self.target).is_none()
        {
            Some(Ordf32(0.0))
        } else {
            None
        }
        // if !cs.is_target_met(self.target, drain) {
        //     None
        // } else {
        //     Some(Ordf32(0.0))
        // }
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Ordf32> {
        if change_lower_bound(cs, self.target, &self.change_policy).is_some() {
            None
        } else {
            Some(Ordf32(0.0))
        }
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}
