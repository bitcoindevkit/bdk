//! This module contains a collection of change policies.
//!
//! A change policy determines whether a given coin selection (presented by [`CoinSelector`]) should
//! construct a transaction with a change output. A change policy is represented as a function of
//! type `Fn(&CoinSelector, Target) -> Drain`.

#[allow(unused)] // some bug in <= 1.48.0 sees this as unused when it isn't
use crate::float::FloatExt;
use crate::{CoinSelector, Drain, DrainWeights, FeeRate, Target};
use core::convert::TryInto;

/// Construct a change policy that creates change when the change value is greater than `min_value`.
pub fn min_value(
    drain_weights: DrainWeights,
    min_value: u64,
) -> impl Fn(&CoinSelector, Target) -> Drain {
    let min_value: i64 = min_value
        .try_into()
        .expect("min_value is ridiculously large");

    move |cs, target| {
        let mut drain = Drain {
            weights: drain_weights,
            ..Default::default()
        };

        let excess = cs.excess(target, drain);
        if excess < min_value {
            return Drain::none();
        }

        drain.value = excess
            .try_into()
            .expect("must be positive since it is greater than min_value (which is positive)");
        drain
    }
}

/// Construct a change policy that creates change when it would reduce the transaction waste.
///
/// **WARNING:** This may result in a change value that is below dust limit. [`min_value_and_waste`]
/// is a more sensible default.
pub fn min_waste(
    drain_weights: DrainWeights,
    long_term_feerate: FeeRate,
) -> impl Fn(&CoinSelector, Target) -> Drain {
    move |cs, target| {
        // The output waste of a changeless solution is the excess.
        let waste_changeless = cs.excess(target, Drain::none());
        let waste_with_change = drain_weights
            .waste(target.feerate, long_term_feerate)
            .ceil() as i64;

        if waste_changeless <= waste_with_change {
            return Drain::none();
        }

        let mut drain = Drain {
            weights: drain_weights,
            value: 0,
        };
        drain.value = cs
            .excess(target, drain)
            .try_into()
            .expect("the excess must be positive because drain free excess was > waste");
        drain
    }
}

/// Construct a change policy that creates change when it would reduce the transaction waste given
/// that `min_value` is respected.
///
/// This is equivalent to combining [`min_value`] with [`min_waste`], and including change when both
/// policies have change.
pub fn min_value_and_waste(
    drain_weights: DrainWeights,
    min_value: u64,
    long_term_feerate: FeeRate,
) -> impl Fn(&CoinSelector, Target) -> Drain {
    let min_waste_policy = crate::change_policy::min_waste(drain_weights, long_term_feerate);

    move |cs, target| {
        let drain = min_waste_policy(cs, target);
        if drain.value < min_value {
            return Drain::none();
        }
        drain
    }
}
