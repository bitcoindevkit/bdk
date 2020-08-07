use std::collections::BTreeMap;
use std::default::Default;

use bitcoin::{Address, OutPoint, SigHashType, Transaction};

use super::coin_selection::{CoinSelectionAlgorithm, DefaultCoinSelectionAlgorithm};
use super::utils::FeeRate;

// TODO: add a flag to ignore change outputs (make them unspendable)
#[derive(Debug, Default)]
pub struct TxBuilder<Cs: CoinSelectionAlgorithm> {
    pub(crate) addressees: Vec<(Address, u64)>,
    pub(crate) send_all: bool,
    pub(crate) fee_rate: Option<FeeRate>,
    pub(crate) policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) utxos: Option<Vec<OutPoint>>,
    pub(crate) unspendable: Option<Vec<OutPoint>>,
    pub(crate) sighash: Option<SigHashType>,
    pub(crate) ordering: TxOrdering,
    pub(crate) locktime: Option<u32>,
    pub(crate) coin_selection: Cs,
}

impl TxBuilder<DefaultCoinSelectionAlgorithm> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_addressees(addressees: Vec<(Address, u64)>) -> Self {
        Self::default().set_addressees(addressees)
    }
}

impl<Cs: CoinSelectionAlgorithm> TxBuilder<Cs> {
    pub fn set_addressees(mut self, addressees: Vec<(Address, u64)>) -> Self {
        self.addressees = addressees;
        self
    }

    pub fn add_addressee(mut self, address: Address, amount: u64) -> Self {
        self.addressees.push((address, amount));
        self
    }

    pub fn send_all(mut self, send_all: bool) -> Self {
        self.send_all = send_all;
        self
    }

    pub fn fee_rate(mut self, fee_rate: FeeRate) -> Self {
        self.fee_rate = Some(fee_rate);
        self
    }

    pub fn policy_path(mut self, policy_path: BTreeMap<String, Vec<usize>>) -> Self {
        self.policy_path = Some(policy_path);
        self
    }

    pub fn utxos(mut self, utxos: Vec<OutPoint>) -> Self {
        self.utxos = Some(utxos);
        self
    }

    pub fn add_utxo(mut self, utxo: OutPoint) -> Self {
        self.utxos.get_or_insert(vec![]).push(utxo);
        self
    }

    pub fn unspendable(mut self, unspendable: Vec<OutPoint>) -> Self {
        self.unspendable = Some(unspendable);
        self
    }

    pub fn add_unspendable(mut self, unspendable: OutPoint) -> Self {
        self.unspendable.get_or_insert(vec![]).push(unspendable);
        self
    }

    pub fn sighash(mut self, sighash: SigHashType) -> Self {
        self.sighash = Some(sighash);
        self
    }

    pub fn ordering(mut self, ordering: TxOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    pub fn nlocktime(mut self, locktime: u32) -> Self {
        self.locktime = Some(locktime);
        self
    }

    pub fn coin_selection<P: CoinSelectionAlgorithm>(self, coin_selection: P) -> TxBuilder<P> {
        TxBuilder {
            addressees: self.addressees,
            send_all: self.send_all,
            fee_rate: self.fee_rate,
            policy_path: self.policy_path,
            utxos: self.utxos,
            unspendable: self.unspendable,
            sighash: self.sighash,
            ordering: self.ordering,
            locktime: self.locktime,
            coin_selection,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TxOrdering {
    Shuffle,
    Untouched,
    BIP69Lexicographic,
}

impl Default for TxOrdering {
    fn default() -> Self {
        TxOrdering::Shuffle
    }
}

impl TxOrdering {
    pub fn modify_tx(&self, tx: &mut Transaction) {
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
    fn test_output_ordering_untouched() {
        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::Untouched.modify_tx(&mut tx);

        assert_eq!(original_tx, tx);
    }

    #[test]
    fn test_output_ordering_shuffle() {
        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::Shuffle.modify_tx(&mut tx);

        assert_eq!(original_tx.input, tx.input);
        assert_ne!(original_tx.output, tx.output);
    }

    #[test]
    fn test_output_ordering_bip69() {
        use std::str::FromStr;

        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::BIP69Lexicographic.modify_tx(&mut tx);

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
}
