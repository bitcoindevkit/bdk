use crate::{bnb::BnBMetric, ord_float::Ordf32, CoinSelector, Drain, Target};
mod waste;
pub use waste::*;

pub struct Changeless<'c, C> {
    target: Target,
    change_policy: &'c C,
}

impl<'c, C> BnBMetric for Changeless<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> Drain,
{
    type Score = bool;

    fn score<'a>(&mut self, cs: &CoinSelector<'a>) -> Option<Self::Score> {
        let drain = (self.change_policy)(cs, self.target);
        if cs.is_target_met(self.target, drain) {
            let has_drain = !drain.is_none();
            Some(has_drain)
        } else {
            None
        }
    }

    fn bound<'a>(&mut self, cs: &CoinSelector<'a>) -> Option<Self::Score> {
        Some(change_lower_bound(cs, self.target, &self.change_policy).is_some())
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        true
    }
}

// Returns a drain if the current selection and every possible future selection would have a change
// output (otherwise Drain::none()) by using the heurisitic that if it has change with the current
// selection and it has one when we select every negative effective value candidate then it will
// always have a drain. We are essentially assuming that the change_policy is monotone with respect
// to the excess of the selection.
//
// NOTE: this should stay private because it requires cs to be sorted such that all negative
// effective value candidates are next to each other.
fn change_lower_bound<'a>(
    cs: &CoinSelector<'a>,
    target: Target,
    change_policy: &impl Fn(&CoinSelector<'a>, Target) -> Drain,
) -> Drain {
    let has_change_now = change_policy(cs, target).is_some();

    if has_change_now {
        let mut least_excess = cs.clone();
        cs.unselected()
            .rev()
            .take_while(|(_, wv)| wv.effective_value(target.feerate) < Ordf32(0.0))
            .for_each(|(index, _)| {
                least_excess.select(index);
            });

        change_policy(&least_excess, target)
    } else {
        Drain::none()
    }
}

macro_rules! impl_for_tuple {
    ($($a:ident $b:tt)*) => {
        impl<$($a),*> BnBMetric for ($($a),*)
            where $($a: BnBMetric),*
        {
            type Score=($(<$a>::Score),*);

            #[allow(unused)]
            fn score<'a>(&mut self, cs: &CoinSelector<'a>) -> Option<Self::Score> {
                Some(($(self.$b.score(cs)?),*))
            }
            #[allow(unused)]
            fn bound<'a>(&mut self, cs: &CoinSelector<'a>) -> Option<Self::Score> {
                Some(($(self.$b.bound(cs)?),*))
            }
            #[allow(unused)]
            fn requires_ordering_by_descending_value_pwu(&self) -> bool {
                [$(self.$b.requires_ordering_by_descending_value_pwu()),*].iter().all(|x| *x)

            }
        }
    };
}

impl_for_tuple!();
impl_for_tuple!(A 0 B 1);
impl_for_tuple!(A 0 B 1 C 2);
impl_for_tuple!(A 0 B 1 C 2 D 3);
impl_for_tuple!(A 0 B 1 C 2 D 3 E 4);
