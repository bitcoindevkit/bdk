//! Branch and bound metrics that can be passed to [`CoinSelector::bnb_solutions`] or
//! [`CoinSelector::run_bnb`].
use crate::{bnb::BnbMetric, float::Ordf32, CoinSelector, Drain, Target};
mod waste;
pub use waste::*;
mod lowest_fee;
pub use lowest_fee::*;
mod changeless;
pub use changeless::*;

// Returns a drain if the current selection and every possible future selection would have a change
// output (otherwise Drain::none()) by using the heurisitic that if it has change with the current
// selection and it has one when we select every negative effective value candidate then it will
// always have change. We are essentially assuming that the change_policy is monotone with respect
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
        impl<$($a),*> BnbMetric for ($(($a, f32)),*)
            where $($a: BnbMetric),*
        {
            #[allow(unused)]
            fn score(&mut self, cs: &CoinSelector<'_>) -> Option<crate::float::Ordf32> {
                let mut acc = Option::<f32>::None;
                for (score, ratio) in [$((self.$b.0.score(cs)?, self.$b.1)),*] {
                    let score: Ordf32 = score;
                    let ratio: f32 = ratio;
                    match &mut acc {
                        Some(acc) => *acc += score.0 * ratio,
                        acc => *acc = Some(score.0 * ratio),
                    }
                }
                acc.map(Ordf32)
            }
            #[allow(unused)]
            fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<crate::float::Ordf32> {
                let mut acc = Option::<f32>::None;
                for (score, ratio) in [$((self.$b.0.bound(cs)?, self.$b.1)),*] {
                    let score: Ordf32 = score;
                    let ratio: f32 = ratio;
                    match &mut acc {
                        Some(acc) => *acc += score.0 * ratio,
                        acc => *acc = Some(score.0 * ratio),
                    }
                }
                acc.map(Ordf32)
            }
            #[allow(unused)]
            fn requires_ordering_by_descending_value_pwu(&self) -> bool {
                [$(self.$b.0.requires_ordering_by_descending_value_pwu()),*].iter().all(|x| *x)
            }
        }
    };
}

impl_for_tuple!();
impl_for_tuple!(A 0 B 1);
impl_for_tuple!(A 0 B 1 C 2);
impl_for_tuple!(A 0 B 1 C 2 D 3);
impl_for_tuple!(A 0 B 1 C 2 D 3 E 4);
