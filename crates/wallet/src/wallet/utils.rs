// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::{absolute, relative, Amount, Script, Sequence};

use miniscript::{MiniscriptKey, Satisfier, ToPublicKey};

use rand_core::RngCore;

/// Trait to check if a value is below the dust limit.
/// We are performing dust value calculation for a given script public key using rust-bitcoin to
/// keep it compatible with network dust rate
// we implement this trait to make sure we don't mess up the comparison with off-by-one like a <
// instead of a <= etc.
pub trait IsDust {
    /// Check whether or not a value is below dust limit
    fn is_dust(&self, script: &Script) -> bool;
}

impl IsDust for Amount {
    fn is_dust(&self, script: &Script) -> bool {
        *self < script.minimal_non_dust()
    }
}

impl IsDust for u64 {
    fn is_dust(&self, script: &Script) -> bool {
        Amount::from_sat(*self).is_dust(script)
    }
}

pub struct After {
    pub current_height: Option<u32>,
    pub assume_height_reached: bool,
}

impl After {
    pub(crate) fn new(current_height: Option<u32>, assume_height_reached: bool) -> After {
        After {
            current_height,
            assume_height_reached,
        }
    }
}

pub(crate) fn check_nsequence_rbf(sequence: Sequence, csv: Sequence) -> bool {
    // The nSequence value must enable relative timelocks
    if !sequence.is_relative_lock_time() {
        return false;
    }

    // Both values should be represented in the same unit (either time-based or
    // block-height based)
    if sequence.is_time_locked() != csv.is_time_locked() {
        return false;
    }

    // The value should be at least `csv`
    if sequence < csv {
        return false;
    }

    true
}

impl<Pk: MiniscriptKey + ToPublicKey> Satisfier<Pk> for After {
    fn check_after(&self, n: absolute::LockTime) -> bool {
        if let Some(current_height) = self.current_height {
            current_height >= n.to_consensus_u32()
        } else {
            self.assume_height_reached
        }
    }
}

pub struct Older {
    pub current_height: Option<u32>,
    pub create_height: Option<u32>,
    pub assume_height_reached: bool,
}

impl Older {
    pub(crate) fn new(
        current_height: Option<u32>,
        create_height: Option<u32>,
        assume_height_reached: bool,
    ) -> Older {
        Older {
            current_height,
            create_height,
            assume_height_reached,
        }
    }
}

impl<Pk: MiniscriptKey + ToPublicKey> Satisfier<Pk> for Older {
    fn check_older(&self, n: relative::LockTime) -> bool {
        if let Some(current_height) = self.current_height {
            // TODO: test >= / >
            current_height
                >= self
                    .create_height
                    .unwrap_or(0)
                    .checked_add(n.to_consensus_u32())
                    .expect("Overflowing addition")
        } else {
            self.assume_height_reached
        }
    }
}
/// Trait to perform the calculation of the transaction amount spent.
/// Use the spend() method pass the three tuples of`Amount` values to perform the calc of total.
pub trait TxAmountSpend {
    /// Perfome the calc to get total of the transaction
    fn spend(&self) -> Amount;
}
impl TxAmountSpend for (Amount, Amount, Amount) {
    fn spend(&self) -> Amount {
        // Pass the tree tuples of `Amount` to performe calculate
        let (sent, received, fee) = *self;
        sent - received + fee
    }
}

// The Knuth shuffling algorithm based on the original [Fisher-Yates method](https://en.wikipedia.org/wiki/Fisher%E2%80%93Yates_shuffle)
pub(crate) fn shuffle_slice<T>(list: &mut [T], rng: &mut impl RngCore) {
    if list.is_empty() {
        return;
    }
    let mut current_index = list.len() - 1;
    while current_index > 0 {
        let random_index = rng.next_u32() as usize % (current_index + 1);
        list.swap(current_index, random_index);
        current_index -= 1;
    }
}

pub(crate) type SecpCtx = Secp256k1<All>;

#[cfg(test)]
mod test {
    // When nSequence is lower than this flag the timelock is interpreted as block-height-based,
    // otherwise it's time-based
    pub(crate) const SEQUENCE_LOCKTIME_TYPE_FLAG: u32 = 1 << 22;

    use super::{check_nsequence_rbf, shuffle_slice, IsDust, TxAmountSpend};
    use crate::bitcoin::{Address, Amount, Network, Sequence};
    use alloc::vec::Vec;
    use core::str::FromStr;
    use rand::{rngs::StdRng, thread_rng, SeedableRng};

    #[test]
    fn test_is_dust() {
        let script_p2pkh = Address::from_str("1GNgwA8JfG7Kc8akJ8opdNWJUihqUztfPe")
            .unwrap()
            .require_network(Network::Bitcoin)
            .unwrap()
            .script_pubkey();
        assert!(script_p2pkh.is_p2pkh());
        assert!(545.is_dust(&script_p2pkh));
        assert!(!546.is_dust(&script_p2pkh));

        let script_p2wpkh = Address::from_str("bc1qxlh2mnc0yqwas76gqq665qkggee5m98t8yskd8")
            .unwrap()
            .require_network(Network::Bitcoin)
            .unwrap()
            .script_pubkey();
        assert!(script_p2wpkh.is_p2wpkh());
        assert!(293.is_dust(&script_p2wpkh));
        assert!(!294.is_dust(&script_p2wpkh));
    }

    #[test]
    fn test_check_nsequence_rbf_msb_set() {
        let result = check_nsequence_rbf(Sequence(0x80000000), Sequence(5000));
        assert!(!result);
    }

    #[test]
    fn test_check_nsequence_rbf_lt_csv() {
        let result = check_nsequence_rbf(Sequence(4000), Sequence(5000));
        assert!(!result);
    }

    #[test]
    fn test_check_nsequence_rbf_different_unit() {
        let result =
            check_nsequence_rbf(Sequence(SEQUENCE_LOCKTIME_TYPE_FLAG + 5000), Sequence(5000));
        assert!(!result);
    }

    #[test]
    fn test_check_nsequence_rbf_mask() {
        let result = check_nsequence_rbf(Sequence(0x3f + 10_000), Sequence(5000));
        assert!(result);
    }

    #[test]
    fn test_check_nsequence_rbf_same_unit_blocks() {
        let result = check_nsequence_rbf(Sequence(10_000), Sequence(5000));
        assert!(result);
    }

    #[test]
    fn test_check_nsequence_rbf_same_unit_time() {
        let result = check_nsequence_rbf(
            Sequence(SEQUENCE_LOCKTIME_TYPE_FLAG + 10_000),
            Sequence(SEQUENCE_LOCKTIME_TYPE_FLAG + 5000),
        );
        assert!(result);
    }

    #[test]
    #[cfg(feature = "std")]
    fn test_shuffle_slice_empty_vec() {
        let mut test: Vec<u8> = vec![];
        shuffle_slice(&mut test, &mut thread_rng());
    }

    #[test]
    #[cfg(feature = "std")]
    fn test_shuffle_slice_single_vec() {
        let mut test: Vec<u8> = vec![0];
        shuffle_slice(&mut test, &mut thread_rng());
    }

    #[test]
    fn test_shuffle_slice_duple_vec() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut test: Vec<u8> = vec![0, 1];
        shuffle_slice(&mut test, &mut rng);
        assert_eq!(test, &[0, 1]);
        let seed = [6; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut test: Vec<u8> = vec![0, 1];
        shuffle_slice(&mut test, &mut rng);
        assert_eq!(test, &[1, 0]);
    }

    #[test]
    fn test_shuffle_slice_multi_vec() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut test: Vec<u8> = vec![0, 1, 2, 4, 5];
        shuffle_slice(&mut test, &mut rng);
        assert_eq!(test, &[2, 1, 0, 4, 5]);
        let seed = [25; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut test: Vec<u8> = vec![0, 1, 2, 4, 5];
        shuffle_slice(&mut test, &mut rng);
        assert_eq!(test, &[0, 4, 1, 2, 5]);
    }
    #[test]
    fn test_txamountspend() {
        let sent: Amount = Amount::from_sat(30);
        let received: Amount = Amount::from_sat(30);
        let fee: Amount = Amount::from_sat(30);
        let result = (sent, received, fee).spend();
        assert_eq!(result, Amount::from_sat(30));
    }
}
