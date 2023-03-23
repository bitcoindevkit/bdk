#[allow(unused)] // some bug in <= 1.48.0 sees this as unused when it isn't
use crate::float::FloatExt;
use crate::{CoinSelector, Drain, FeeRate, Target};
use core::convert::TryInto;

/// Add a change output if the change value would be greater than or equal to `min_value`.
///
/// Note that the value field of the `drain` is ignored.
pub fn min_value(mut drain: Drain, min_value: u64) -> impl Fn(&CoinSelector, Target) -> Drain {
    debug_assert!(drain.is_some());
    let min_value: i64 = min_value
        .try_into()
        .expect("min_value is ridiculously large");
    drain.value = 0;
    move |cs, target| {
        let excess = cs.excess(target, drain);
        if excess >= min_value {
            let mut drain = drain;
            drain.value = excess.try_into().expect(
                "cannot be negative since we checked it against min_value which is positive",
            );
            drain
        } else {
            Drain::none()
        }
    }
}

/// Add a change output if it would reduce the overall waste of the transaction.
///
/// Note that the value field of the `drain` is ignored.
/// The `value` will be set to whatever needs to be to reach the given target.
pub fn min_waste(
    mut drain: Drain,
    long_term_feerate: FeeRate,
) -> impl Fn(&CoinSelector, Target) -> Drain {
    debug_assert!(drain.is_some());
    drain.value = 0;

    move |cs, target| {
        let excess = cs.excess(target, Drain::none());
        if excess > drain.waste(target.feerate, long_term_feerate).ceil() as i64 {
            let mut drain = drain;
            drain.value = cs
                .excess(target, drain)
                .try_into()
                .expect("the excess must be positive because drain free excess was > waste");
            drain
        } else {
            Drain::none()
        }
    }
}
