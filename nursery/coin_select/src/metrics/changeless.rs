use super::change_lower_bound;
use crate::{bnb::BnbMetric, CoinSelector, Drain, Target};

pub struct Changeless<'c, C> {
    pub target: Target,
    pub change_policy: &'c C,
}

impl<'c, C> BnbMetric for Changeless<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    type Score = bool;

    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score> {
        let drain = (self.change_policy)(cs, self.target);
        if cs.is_target_met(self.target, drain) {
            let has_drain = !drain.is_none();
            Some(has_drain)
        } else {
            None
        }
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<Self::Score> {
        Some(change_lower_bound(cs, self.target, &self.change_policy).is_some())
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}
