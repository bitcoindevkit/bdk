// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Transaction builder
//!
//! ## Example
//!
//! ```
//! # use std::str::FromStr;
//! # use bitcoin::*;
//! # use bdk::*;
//! # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
//! // Create a transaction with one output to `to_address` of 50_000 satoshi, with a custom fee rate
//! // of 5.0 satoshi/vbyte, only spending non-change outputs and with RBF signaling
//! // enabled
//! let builder = TxBuilder::with_recipients(vec![(to_address.script_pubkey(), 50_000)])
//!     .fee_rate(FeeRate::from_sat_per_vb(5.0))
//!     .do_not_spend_change()
//!     .enable_rbf();
//! # let builder: TxBuilder<bdk::database::MemoryDatabase, _> = builder;
//! ```

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::default::Default;
use std::marker::PhantomData;

use bitcoin::{OutPoint, Script, SigHashType, Transaction};

use super::coin_selection::{CoinSelectionAlgorithm, DefaultCoinSelectionAlgorithm};
use crate::database::Database;
use crate::types::{FeeRate, UTXO};

/// A transaction builder
///
/// This structure contains the configuration that the wallet must follow to build a transaction.
///
/// For an example see [this module](super::tx_builder)'s documentation;
#[derive(Debug)]
pub struct TxBuilder<D: Database, Cs: CoinSelectionAlgorithm<D>> {
    pub(crate) recipients: Vec<(Script, u64)>,
    pub(crate) send_all: bool,
    pub(crate) fee_policy: Option<FeePolicy>,
    pub(crate) policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) utxos: Vec<OutPoint>,
    pub(crate) unspendable: HashSet<OutPoint>,
    pub(crate) manually_selected_only: bool,
    pub(crate) sighash: Option<SigHashType>,
    pub(crate) ordering: TxOrdering,
    pub(crate) locktime: Option<u32>,
    pub(crate) rbf: Option<u32>,
    pub(crate) version: Option<Version>,
    pub(crate) change_policy: ChangeSpendPolicy,
    pub(crate) force_non_witness_utxo: bool,
    pub(crate) coin_selection: Cs,

    phantom: PhantomData<D>,
}

#[derive(Debug)]
pub enum FeePolicy {
    FeeRate(FeeRate),
    FeeAmount(u64),
}

impl std::default::Default for FeePolicy {
    fn default() -> Self {
        FeePolicy::FeeRate(FeeRate::default_min_relay_fee())
    }
}

// Unfortunately derive doesn't work with `PhantomData`: https://github.com/rust-lang/rust/issues/26925
impl<D: Database, Cs: CoinSelectionAlgorithm<D>> Default for TxBuilder<D, Cs>
where
    Cs: Default,
{
    fn default() -> Self {
        TxBuilder {
            recipients: Default::default(),
            send_all: Default::default(),
            fee_policy: Default::default(),
            policy_path: Default::default(),
            utxos: Default::default(),
            unspendable: Default::default(),
            manually_selected_only: Default::default(),
            sighash: Default::default(),
            ordering: Default::default(),
            locktime: Default::default(),
            rbf: Default::default(),
            version: Default::default(),
            change_policy: Default::default(),
            force_non_witness_utxo: Default::default(),
            coin_selection: Default::default(),

            phantom: PhantomData,
        }
    }
}

impl<D: Database> TxBuilder<D, DefaultCoinSelectionAlgorithm> {
    /// Create an empty builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a builder starting from a list of recipients
    pub fn with_recipients(recipients: Vec<(Script, u64)>) -> Self {
        Self::default().set_recipients(recipients)
    }
}

impl<D: Database, Cs: CoinSelectionAlgorithm<D>> TxBuilder<D, Cs> {
    /// Replace the recipients already added with a new list
    pub fn set_recipients(mut self, recipients: Vec<(Script, u64)>) -> Self {
        self.recipients = recipients;
        self
    }

    /// Add a recipient to the internal list
    pub fn add_recipient(mut self, script_pubkey: Script, amount: u64) -> Self {
        self.recipients.push((script_pubkey, amount));
        self
    }

    /// Send all inputs to a single output.
    ///
    /// The semantics of `send_all` depend on whether you are using [`create_tx`] or [`bump_fee`].
    /// In `create_tx` it (by default) **selects all the wallets inputs** and sends them to a single
    /// output. In `bump_fee` it means to send the original inputs and any additional manually
    /// selected intputs to a single output.
    ///
    /// Adding more than one recipients with this option enabled will result in an error.
    ///
    /// The value associated with the only recipient is irrelevant and will be replaced by the wallet.
    ///
    /// [`bump_fee`]: crate::wallet::Wallet::bump_fee
    /// [`create_tx`]: crate::wallet::Wallet::create_tx
    pub fn send_all(mut self) -> Self {
        self.send_all = true;
        self
    }

    /// Set a custom fee rate
    pub fn fee_rate(mut self, fee_rate: FeeRate) -> Self {
        self.fee_policy = Some(FeePolicy::FeeRate(fee_rate));
        self
    }

    /// Set an absolute fee
    pub fn fee_absolute(mut self, fee_amount: u64) -> Self {
        self.fee_policy = Some(FeePolicy::FeeAmount(fee_amount));
        self
    }

    /// Set the policy path to use while creating the transaction
    ///
    /// This method accepts a map where the key is the policy node id (see
    /// [`Policy::id`](crate::descriptor::Policy::id)) and the value is the list of the indexes of
    /// the items that are intended to be satisfied from the policy node (see
    /// [`SatisfiableItem::Thresh::items`](crate::descriptor::policy::SatisfiableItem::Thresh::items)).
    pub fn policy_path(mut self, policy_path: BTreeMap<String, Vec<usize>>) -> Self {
        self.policy_path = Some(policy_path);
        self
    }

    /// Replace the internal list of utxos that **must** be spent with a new list
    ///
    /// These have priority over the "unspendable" utxos, meaning that if a utxo is present both in
    /// the "utxos" and the "unspendable" list, it will be spent.
    pub fn utxos(mut self, utxos: Vec<OutPoint>) -> Self {
        self.utxos = utxos;
        self
    }

    /// Add a utxo to the internal list of utxos that **must** be spent
    ///
    /// These have priority over the "unspendable" utxos, meaning that if a utxo is present both in
    /// the "utxos" and the "unspendable" list, it will be spent.
    pub fn add_utxo(mut self, utxo: OutPoint) -> Self {
        self.utxos.push(utxo);
        self
    }

    /// Only spend utxos added by [`add_utxo`] and [`utxos`].
    ///
    /// The wallet will **not** add additional utxos to the transaction even if they are needed to
    /// make the transaction valid.
    ///
    /// [`add_utxo`]: Self::add_utxo
    /// [`utxos`]: Self::utxos
    pub fn manually_selected_only(mut self) -> Self {
        self.manually_selected_only = true;
        self
    }

    /// Replace the internal list of unspendable utxos with a new list
    ///
    /// It's important to note that the "must-be-spent" utxos added with [`TxBuilder::utxos`] and
    /// [`TxBuilder::add_utxo`] have priority over these. See the docs of the two linked methods
    /// for more details.
    pub fn unspendable(mut self, unspendable: Vec<OutPoint>) -> Self {
        self.unspendable = unspendable.into_iter().collect();
        self
    }

    /// Add a utxo to the internal list of unspendable utxos
    ///
    /// It's important to note that the "must-be-spent" utxos added with [`TxBuilder::utxos`] and
    /// [`TxBuilder::add_utxo`] have priority over this. See the docs of the two linked methods
    /// for more details.
    pub fn add_unspendable(mut self, unspendable: OutPoint) -> Self {
        self.unspendable.insert(unspendable);
        self
    }

    /// Sign with a specific sig hash
    ///
    /// **Use this option very carefully**
    pub fn sighash(mut self, sighash: SigHashType) -> Self {
        self.sighash = Some(sighash);
        self
    }

    /// Choose the ordering for inputs and outputs of the transaction
    pub fn ordering(mut self, ordering: TxOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    /// Use a specific nLockTime while creating the transaction
    ///
    /// This can cause conflicts if the wallet's descriptors contain an "after" (OP_CLTV) operator.
    pub fn nlocktime(mut self, locktime: u32) -> Self {
        self.locktime = Some(locktime);
        self
    }

    /// Enable signaling RBF
    ///
    /// This will use the default nSequence value of `0xFFFFFFFD`.
    pub fn enable_rbf(self) -> Self {
        self.enable_rbf_with_sequence(0xFFFFFFFD)
    }

    /// Enable signaling RBF with a specific nSequence value
    ///
    /// This can cause conflicts if the wallet's descriptors contain an "older" (OP_CSV) operator
    /// and the given `nsequence` is lower than the CSV value.
    ///
    /// If the `nsequence` is higher than `0xFFFFFFFD` an error will be thrown, since it would not
    /// be a valid nSequence to signal RBF.
    pub fn enable_rbf_with_sequence(mut self, nsequence: u32) -> Self {
        self.rbf = Some(nsequence);
        self
    }

    /// Build a transaction with a specific version
    ///
    /// The `version` should always be greater than `0` and greater than `1` if the wallet's
    /// descriptors contain an "older" (OP_CSV) operator.
    pub fn version(mut self, version: i32) -> Self {
        self.version = Some(Version(version));
        self
    }

    /// Do not spend change outputs
    ///
    /// This effectively adds all the change outputs to the "unspendable" list. See
    /// [`TxBuilder::unspendable`].
    pub fn do_not_spend_change(mut self) -> Self {
        self.change_policy = ChangeSpendPolicy::ChangeForbidden;
        self
    }

    /// Only spend change outputs
    ///
    /// This effectively adds all the non-change outputs to the "unspendable" list. See
    /// [`TxBuilder::unspendable`].
    pub fn only_spend_change(mut self) -> Self {
        self.change_policy = ChangeSpendPolicy::OnlyChange;
        self
    }

    /// Set a specific [`ChangeSpendPolicy`]. See [`TxBuilder::do_not_spend_change`] and
    /// [`TxBuilder::only_spend_change`] for some shortcuts.
    pub fn change_policy(mut self, change_policy: ChangeSpendPolicy) -> Self {
        self.change_policy = change_policy;
        self
    }

    /// Fill-in the [`psbt::Input::non_witness_utxo`](bitcoin::util::psbt::Input::non_witness_utxo) field even if the wallet only has SegWit
    /// descriptors.
    ///
    /// This is useful for signers which always require it, like Trezor hardware wallets.
    pub fn force_non_witness_utxo(mut self) -> Self {
        self.force_non_witness_utxo = true;
        self
    }

    /// Choose the coin selection algorithm
    ///
    /// Overrides the [`DefaultCoinSelectionAlgorithm`](super::coin_selection::DefaultCoinSelectionAlgorithm).
    pub fn coin_selection<P: CoinSelectionAlgorithm<D>>(
        self,
        coin_selection: P,
    ) -> TxBuilder<D, P> {
        TxBuilder {
            recipients: self.recipients,
            send_all: self.send_all,
            fee_policy: self.fee_policy,
            policy_path: self.policy_path,
            utxos: self.utxos,
            unspendable: self.unspendable,
            manually_selected_only: self.manually_selected_only,
            sighash: self.sighash,
            ordering: self.ordering,
            locktime: self.locktime,
            rbf: self.rbf,
            version: self.version,
            change_policy: self.change_policy,
            force_non_witness_utxo: self.force_non_witness_utxo,
            coin_selection,

            phantom: PhantomData,
        }
    }
}

/// Ordering of the transaction's inputs and outputs
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub enum TxOrdering {
    /// Randomized (default)
    Shuffle,
    /// Unchanged
    Untouched,
    /// BIP69 / Lexicographic
    BIP69Lexicographic,
}

impl Default for TxOrdering {
    fn default() -> Self {
        TxOrdering::Shuffle
    }
}

impl TxOrdering {
    pub fn sort_tx(&self, tx: &mut Transaction) {
        match self {
            TxOrdering::Untouched => {}
            TxOrdering::Shuffle => {
                use rand::seq::SliceRandom;
                #[cfg(test)]
                use rand::SeedableRng;

                #[cfg(not(test))]
                let mut rng = rand::thread_rng();
                #[cfg(test)]
                let mut rng = rand::rngs::StdRng::seed_from_u64(0);

                tx.output.shuffle(&mut rng);
            }
            TxOrdering::BIP69Lexicographic => {
                tx.input.sort_unstable_by_key(|txin| {
                    (txin.previous_output.txid, txin.previous_output.vout)
                });
                tx.output
                    .sort_unstable_by_key(|txout| (txout.value, txout.script_pubkey.clone()));
            }
        }
    }
}

/// Transaction version
///
/// Has a default value of `1`
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub(crate) struct Version(pub(crate) i32);

impl Default for Version {
    fn default() -> Self {
        Version(1)
    }
}

/// Policy regarding the use of change outputs when creating a transaction
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub enum ChangeSpendPolicy {
    /// Use both change and non-change outputs (default)
    ChangeAllowed,
    /// Only use change outputs (see [`TxBuilder::only_spend_change`])
    OnlyChange,
    /// Only use non-change outputs (see [`TxBuilder::do_not_spend_change`])
    ChangeForbidden,
}

impl Default for ChangeSpendPolicy {
    fn default() -> Self {
        ChangeSpendPolicy::ChangeAllowed
    }
}

impl ChangeSpendPolicy {
    pub(crate) fn is_satisfied_by(&self, utxo: &UTXO) -> bool {
        match self {
            ChangeSpendPolicy::ChangeAllowed => true,
            ChangeSpendPolicy::OnlyChange => utxo.is_internal,
            ChangeSpendPolicy::ChangeForbidden => !utxo.is_internal,
        }
    }
}

#[cfg(test)]
mod test {
    const ORDERING_TEST_TX: &'static str = "0200000003c26f3eb7932f7acddc5ddd26602b77e7516079b03090a16e2c2f54\
                                            85d1fd600f0100000000ffffffffc26f3eb7932f7acddc5ddd26602b77e75160\
                                            79b03090a16e2c2f5485d1fd600f0000000000ffffffff571fb3e02278217852\
                                            dd5d299947e2b7354a639adc32ec1fa7b82cfb5dec530e0500000000ffffffff\
                                            03e80300000000000002aaeee80300000000000001aa200300000000000001ff\
                                            00000000";
    macro_rules! ordering_test_tx {
        () => {
            deserialize::<bitcoin::Transaction>(&Vec::<u8>::from_hex(ORDERING_TEST_TX).unwrap())
                .unwrap()
        };
    }

    use bitcoin::consensus::deserialize;
    use bitcoin::hashes::hex::FromHex;

    use super::*;

    #[test]
    fn test_output_ordering_default_shuffle() {
        assert_eq!(TxOrdering::default(), TxOrdering::Shuffle);
    }

    #[test]
    fn test_output_ordering_untouched() {
        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::Untouched.sort_tx(&mut tx);

        assert_eq!(original_tx, tx);
    }

    #[test]
    fn test_output_ordering_shuffle() {
        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::Shuffle.sort_tx(&mut tx);

        assert_eq!(original_tx.input, tx.input);
        assert_ne!(original_tx.output, tx.output);
    }

    #[test]
    fn test_output_ordering_bip69() {
        use std::str::FromStr;

        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::BIP69Lexicographic.sort_tx(&mut tx);

        assert_eq!(
            tx.input[0].previous_output,
            bitcoin::OutPoint::from_str(
                "0e53ec5dfb2cb8a71fec32dc9a634a35b7e24799295ddd5278217822e0b31f57:5"
            )
            .unwrap()
        );
        assert_eq!(
            tx.input[1].previous_output,
            bitcoin::OutPoint::from_str(
                "0f60fdd185542f2c6ea19030b0796051e7772b6026dd5ddccd7a2f93b73e6fc2:0"
            )
            .unwrap()
        );
        assert_eq!(
            tx.input[2].previous_output,
            bitcoin::OutPoint::from_str(
                "0f60fdd185542f2c6ea19030b0796051e7772b6026dd5ddccd7a2f93b73e6fc2:1"
            )
            .unwrap()
        );

        assert_eq!(tx.output[0].value, 800);
        assert_eq!(tx.output[1].script_pubkey, From::from(vec![0xAA]));
        assert_eq!(tx.output[2].script_pubkey, From::from(vec![0xAA, 0xEE]));
    }

    fn get_test_utxos() -> Vec<UTXO> {
        vec![
            UTXO {
                outpoint: OutPoint {
                    txid: Default::default(),
                    vout: 0,
                },
                txout: Default::default(),
                is_internal: false,
            },
            UTXO {
                outpoint: OutPoint {
                    txid: Default::default(),
                    vout: 1,
                },
                txout: Default::default(),
                is_internal: true,
            },
        ]
    }

    #[test]
    fn test_change_spend_policy_default() {
        let change_spend_policy = ChangeSpendPolicy::default();
        let filtered = get_test_utxos()
            .into_iter()
            .filter(|u| change_spend_policy.is_satisfied_by(u))
            .collect::<Vec<_>>();

        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_change_spend_policy_no_internal() {
        let change_spend_policy = ChangeSpendPolicy::ChangeForbidden;
        let filtered = get_test_utxos()
            .into_iter()
            .filter(|u| change_spend_policy.is_satisfied_by(u))
            .collect::<Vec<_>>();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].is_internal, false);
    }

    #[test]
    fn test_change_spend_policy_only_internal() {
        let change_spend_policy = ChangeSpendPolicy::OnlyChange;
        let filtered = get_test_utxos()
            .into_iter()
            .filter(|u| change_spend_policy.is_satisfied_by(u))
            .collect::<Vec<_>>();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].is_internal, true);
    }

    #[test]
    fn test_default_tx_version_1() {
        let version = Version::default();
        assert_eq!(version.0, 1);
    }
}
