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
use bitcoin::{LockTime, Script, Sequence};

use miniscript::{MiniscriptKey, Satisfier, ToPublicKey};

/// Trait to check if a value is below the dust limit.
/// We are performing dust value calculation for a given script public key using rust-bitcoin to
/// keep it compatible with network dust rate
// we implement this trait to make sure we don't mess up the comparison with off-by-one like a <
// instead of a <= etc.
pub trait IsDust {
    /// Check whether or not a value is below dust limit
    fn is_dust(&self, script: &Script) -> bool;
}

impl IsDust for u64 {
    fn is_dust(&self, script: &Script) -> bool {
        *self < script.dust_value().to_sat()
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

pub(crate) fn check_nsequence_rbf(rbf: Sequence, csv: Sequence) -> bool {
    // The RBF value must enable relative timelocks
    if !rbf.is_relative_lock_time() {
        return false;
    }

    // Both values should be represented in the same unit (either time-based or
    // block-height based)
    if rbf.is_time_locked() != csv.is_time_locked() {
        return false;
    }

    // The value should be at least `csv`
    if rbf < csv {
        return false;
    }

    true
}

impl<Pk: MiniscriptKey + ToPublicKey> Satisfier<Pk> for After {
    fn check_after(&self, n: LockTime) -> bool {
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
    fn check_older(&self, n: Sequence) -> bool {
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

pub(crate) type SecpCtx = Secp256k1<All>;

#[cfg(test)]
mod test {
    // When nSequence is lower than this flag the timelock is interpreted as block-height-based,
    // otherwise it's time-based
    pub(crate) const SEQUENCE_LOCKTIME_TYPE_FLAG: u32 = 1 << 22;

    use super::{check_nsequence_rbf, IsDust};
    use crate::bitcoin::{Address, Sequence};
    use std::str::FromStr;

    #[test]
    fn test_is_dust() {
        let script_p2pkh = Address::from_str("1GNgwA8JfG7Kc8akJ8opdNWJUihqUztfPe")
            .unwrap()
            .script_pubkey();
        assert!(script_p2pkh.is_p2pkh());
        assert!(545.is_dust(&script_p2pkh));
        assert!(!546.is_dust(&script_p2pkh));

        let script_p2wpkh = Address::from_str("bc1qxlh2mnc0yqwas76gqq665qkggee5m98t8yskd8")
            .unwrap()
            .script_pubkey();
        assert!(script_p2wpkh.is_v0_p2wpkh());
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
}
