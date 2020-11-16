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
//! # use bdk::wallet::tx_builder::CreateTx;
//! # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
//! // Create a transaction with one output to `to_address` of 50_000 satoshi, with a custom fee rate
//! // of 5.0 satoshi/vbyte, only spending non-change outputs and with RBF signaling
//! // enabled
//! let builder = TxBuilder::with_recipients(vec![(to_address.script_pubkey(), 50_000)])
//!     .fee_rate(FeeRate::from_sat_per_vb(5.0))
//!     .do_not_spend_change()
//!     .enable_rbf();
//! # let builder: TxBuilder<bdk::database::MemoryDatabase, _, CreateTx> = builder;
//! ```

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::default::Default;
use std::marker::PhantomData;

use bitcoin::{OutPoint, Script, SigHashType, Transaction};

use super::coin_selection::{CoinSelectionAlgorithm, DefaultCoinSelectionAlgorithm};
use crate::database::Database;
use crate::types::{FeeRate, ScriptType, UTXO};

/// Context in which the [`TxBuilder`] is valid
pub trait TxBuilderContext: std::fmt::Debug + Default + Clone {}

/// [`Wallet::create_tx`](super::Wallet::create_tx) context
#[derive(Debug, Default, Clone)]
pub struct CreateTx;
impl TxBuilderContext for CreateTx {}

/// [`Wallet::bump_fee`](super::Wallet::bump_fee) context
#[derive(Debug, Default, Clone)]
pub struct BumpFee;
impl TxBuilderContext for BumpFee {}

/// A transaction builder
///
/// This structure contains the configuration that the wallet must follow to build a transaction.
///
/// For an example see [this module](super::tx_builder)'s documentation;
#[derive(Debug)]
pub struct TxBuilder<D: Database, Cs: CoinSelectionAlgorithm<D>, Ctx: TxBuilderContext> {
    pub(crate) recipients: Vec<(Script, u64)>,
    pub(crate) drain_wallet: bool,
    pub(crate) single_recipient: Option<Script>,
    pub(crate) fee_policy: Option<FeePolicy>,
    pub(crate) internal_policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) external_policy_path: Option<BTreeMap<String, Vec<usize>>>,
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
    pub(crate) include_output_redeem_witness_script: bool,

    phantom: PhantomData<(D, Ctx)>,
}

#[derive(Debug)]
pub(crate) enum FeePolicy {
    FeeRate(FeeRate),
    FeeAmount(u64),
}

impl std::default::Default for FeePolicy {
    fn default() -> Self {
        FeePolicy::FeeRate(FeeRate::default_min_relay_fee())
    }
}

// Unfortunately derive doesn't work with `PhantomData`: https://github.com/rust-lang/rust/issues/26925
impl<D: Database, Cs: CoinSelectionAlgorithm<D>, Ctx: TxBuilderContext> Default
    for TxBuilder<D, Cs, Ctx>
where
    Cs: Default,
{
    fn default() -> Self {
        TxBuilder {
            recipients: Default::default(),
            drain_wallet: Default::default(),
            single_recipient: Default::default(),
            fee_policy: Default::default(),
            internal_policy_path: Default::default(),
            external_policy_path: Default::default(),
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
            include_output_redeem_witness_script: Default::default(),

            phantom: PhantomData,
        }
    }
}

// methods supported by both contexts, but only for `DefaultCoinSelectionAlgorithm`
impl<D: Database, Ctx: TxBuilderContext> TxBuilder<D, DefaultCoinSelectionAlgorithm, Ctx> {
    /// Create an empty builder
    pub fn new() -> Self {
        Self::default()
    }
}

// methods supported by both contexts, for any CoinSelectionAlgorithm
impl<D: Database, Cs: CoinSelectionAlgorithm<D>, Ctx: TxBuilderContext> TxBuilder<D, Cs, Ctx> {
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

    /// Set the policy path to use while creating the transaction for a given script type
    ///
    /// This method accepts a map where the key is the policy node id (see
    /// [`Policy::id`](crate::descriptor::Policy::id)) and the value is the list of the indexes of
    /// the items that are intended to be satisfied from the policy node (see
    /// [`SatisfiableItem::Thresh::items`](crate::descriptor::policy::SatisfiableItem::Thresh::items)).
    ///
    /// ## Example
    ///
    /// An example of when the policy path is needed is the following descriptor:
    /// `wsh(thresh(2,pk(A),sj:and_v(v:pk(B),n:older(6)),snj:and_v(v:pk(C),after(630000))))`,
    /// derived from the miniscript policy `thresh(2,pk(A),and(pk(B),older(6)),and(pk(C),after(630000)))`.
    /// It declares three descriptor fragments, and at the top level it uses `thresh()` to
    /// ensure that at least two of them are satisfied. The individual fragments are:
    ///
    /// 1. `pk(A)`
    /// 2. `and(pk(B),older(6))`
    /// 3. `and(pk(C),after(630000))`
    ///
    /// When those conditions are combined in pairs, it's clear that the transaction needs to be created
    /// differently depending on how the user intends to satisfy the policy afterwards:
    ///
    /// * If fragments `1` and `2` are used, the transaction will need to use a specific
    ///   `n_sequence` in order to spend an `OP_CSV` branch.
    /// * If fragments `1` and `3` are used, the transaction will need to use a specific `locktime`
    ///   in order to spend an `OP_CLTV` branch.
    /// * If fragments `2` and `3` are used, the transaction will need both.
    ///
    /// When the spending policy is represented as a tree (see
    /// [`Wallet::policies`](super::Wallet::policies)), every node
    /// is assigned a unique identifier that can be used in the policy path to specify which of
    /// the node's children the user intends to satisfy: for instance, assuming the `thresh()`
    /// root node of this example has an id of `aabbccdd`, the policy path map would look like:
    ///
    /// `{ "aabbccdd" => [0, 1] }`
    ///
    /// where the key is the node's id, and the value is a list of the children that should be
    /// used, in no particular order.
    ///
    /// If a particularly complex descriptor has multiple ambiguous thresholds in its structure,
    /// multiple entries can be added to the map, one for each node that requires an explicit path.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use std::collections::BTreeMap;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// let mut path = BTreeMap::new();
    /// path.insert("aabbccdd".to_string(), vec![0, 1]);
    ///
    /// let builder = TxBuilder::with_recipients(vec![(to_address.script_pubkey(), 50_000)])
    ///     .policy_path(path, ScriptType::External);
    /// # let builder: TxBuilder<bdk::database::MemoryDatabase, _, _> = builder;
    /// ```
    pub fn policy_path(
        mut self,
        policy_path: BTreeMap<String, Vec<usize>>,
        script_type: ScriptType,
    ) -> Self {
        let to_update = match script_type {
            ScriptType::Internal => &mut self.internal_policy_path,
            ScriptType::External => &mut self.external_policy_path,
        };

        *to_update = Some(policy_path);
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

    /// Spend all the available inputs. This respects filters like [`unspendable`] and the change policy.
    pub fn drain_wallet(mut self) -> Self {
        self.drain_wallet = true;
        self
    }

    /// Choose the coin selection algorithm
    ///
    /// Overrides the [`DefaultCoinSelectionAlgorithm`](super::coin_selection::DefaultCoinSelectionAlgorithm).
    pub fn coin_selection<P: CoinSelectionAlgorithm<D>>(
        self,
        coin_selection: P,
    ) -> TxBuilder<D, P, Ctx> {
        TxBuilder {
            recipients: self.recipients,
            drain_wallet: self.drain_wallet,
            single_recipient: self.single_recipient,
            fee_policy: self.fee_policy,
            internal_policy_path: self.internal_policy_path,
            external_policy_path: self.external_policy_path,
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
            include_output_redeem_witness_script: self.include_output_redeem_witness_script,

            phantom: PhantomData,
        }
    }

    /// Fill-in the [`psbt::Output::redeem_script`](bitcoin::util::psbt::Output::redeem_script) and
    /// [`psbt::Output::witness_script`](bitcoin::util::psbt::Output::witness_script) fields.
    ///
    /// This is useful for signers which always require it, like ColdCard hardware wallets.
    pub fn include_output_redeem_witness_script(mut self) -> Self {
        self.include_output_redeem_witness_script = true;
        self
    }
}

// methods supported only by create_tx, and only for `DefaultCoinSelectionAlgorithm`
impl<D: Database> TxBuilder<D, DefaultCoinSelectionAlgorithm, CreateTx> {
    /// Create a builder starting from a list of recipients
    pub fn with_recipients(recipients: Vec<(Script, u64)>) -> Self {
        Self::default().set_recipients(recipients)
    }
}

// methods supported only by create_tx, for any `CoinSelectionAlgorithm`
impl<D: Database, Cs: CoinSelectionAlgorithm<D>> TxBuilder<D, Cs, CreateTx> {
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

    /// Set a single recipient that will get all the selected funds minus the fee. No change will
    /// be created
    ///
    /// This method overrides any recipient set with [`set_recipients`](Self::set_recipients) or
    /// [`add_recipient`](Self::add_recipient).
    ///
    /// It can only be used in conjunction with [`drain_wallet`](Self::drain_wallet) to send the
    /// entire content of the wallet (minus filters) to a single recipient or with a
    /// list of manually selected UTXOs by enabling [`manually_selected_only`](Self::manually_selected_only)
    /// and selecting them with [`utxos`](Self::utxos) or [`add_utxo`](Self::add_utxo).
    ///
    /// When bumping the fees of a transaction made with this option, the user should remeber to
    /// add [`maintain_single_recipient`](Self::maintain_single_recipient) to correctly update the
    /// single output instead of adding one more for the change.
    pub fn set_single_recipient(mut self, recipient: Script) -> Self {
        self.single_recipient = Some(recipient);
        self.recipients.clear();

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
}

// methods supported only by bump_fee
impl<D: Database> TxBuilder<D, DefaultCoinSelectionAlgorithm, BumpFee> {
    /// Bump the fees of a transaction made with [`set_single_recipient`](Self::set_single_recipient)
    ///
    /// Unless extra inputs are specified with [`add_utxo`] or [`utxos`], this flag will make
    /// `bump_fee` reduce the value of the existing output, or fail if it would be consumed
    /// entirely given the higher new fee rate.
    ///
    /// If extra inputs are added and they are not entirely consumed in fees, a change output will not
    /// be added; the existing output will simply grow in value.
    ///
    /// Fails if the transaction has more than one outputs.
    ///
    /// [`add_utxo`]: Self::add_utxo
    /// [`utxos`]: Self::utxos
    pub fn maintain_single_recipient(mut self) -> Self {
        self.single_recipient = Some(Script::default());
        self
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
