use bdk_chain::bitcoin::{
    self, absolute, transaction, Address, Amount, OutPoint, Transaction, TxIn, TxOut, Txid,
};
use core::str::FromStr;

#[cfg(feature = "miniscript")]
use bdk_chain::miniscript::{descriptor::KeyMap, Descriptor, DescriptorPublicKey};

#[allow(unused_macros)]
#[macro_export]
macro_rules! block_id {
    ($height:expr, $hash:literal) => {{
        bdk_chain::BlockId {
            height: $height,
            hash: bitcoin::hashes::Hash::hash($hash.as_bytes()),
        }
    }};
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! hash {
    ($index:literal) => {{
        bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! local_chain {
    [ $(($height:expr, $hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
    }};
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! chain_update {
    [ $(($height:expr, $hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
            .tip()
    }};
}

#[allow(unused_macros)]
#[macro_export]
macro_rules! changeset {
    (checkpoints: $($tail:tt)*) => { changeset!(index: TxHeight, checkpoints: $($tail)*) };
    (
        index: $ind:ty,
        checkpoints: [ $(( $height:expr, $cp_to:expr )),* ]
        $(,txids: [ $(( $txid:expr, $tx_to:expr )),* ])?
    ) => {{
        use bdk_chain::collections::BTreeMap;

        #[allow(unused_mut)]
        bdk_chain::sparse_chain::ChangeSet::<$ind> {
            checkpoints: {
                let mut changes = BTreeMap::default();
                $(changes.insert($height, $cp_to);)*
                changes
            },
            txids: {
                let mut changes = BTreeMap::default();
                $($(changes.insert($txid, $tx_to.map(|h: TxHeight| h.into()));)*)?
                changes
            }
        }
    }};
}

#[allow(unused)]
pub fn new_tx(lt: u32) -> bitcoin::Transaction {
    bitcoin::Transaction {
        version: bitcoin::transaction::Version::non_standard(0x00),
        lock_time: bitcoin::absolute::LockTime::from_consensus(lt),
        input: vec![],
        output: vec![],
    }
}

/// Utility function to create a [`TxOut`] given amount (in satoshis) and address.
pub fn create_txout(sats: u64, addr: &str) -> TxOut {
    TxOut {
        value: Amount::from_sat(sats),
        script_pubkey: Address::from_str(addr)
            .unwrap()
            .assume_checked()
            .script_pubkey(),
    }
}

/// Utility function to create a transaction given txids, vouts of inputs and amounts (in satoshis),
/// addresses of outputs.
///
/// The locktime should be in the form given to `OP_CHEKCLOCKTIMEVERIFY`.
pub fn create_test_tx(
    txids: impl IntoIterator<Item = Txid>,
    vouts: impl IntoIterator<Item = u32>,
    amounts: impl IntoIterator<Item = u64>,
    addrs: impl IntoIterator<Item = &'static str>,
    version: u32,
    locktime: u32,
) -> Transaction {
    let input_vec = core::iter::zip(txids, vouts)
        .map(|(txid, vout)| TxIn {
            previous_output: OutPoint::new(txid, vout),
            ..TxIn::default()
        })
        .collect();
    let output_vec = core::iter::zip(amounts, addrs)
        .map(|(amount, addr)| create_txout(amount, addr))
        .collect();
    let version = transaction::Version::non_standard(version as i32);
    assert!(version.is_standard());
    let lock_time = absolute::LockTime::from_consensus(locktime);
    assert_eq!(lock_time.to_consensus_u32(), locktime);
    Transaction {
        version,
        lock_time,
        input: input_vec,
        output: output_vec,
    }
}

/// Generates `script_pubkey` corresponding to `index` on keychain of `descriptor`.
#[cfg(feature = "miniscript")]
pub fn spk_at_index(
    descriptor: &Descriptor<DescriptorPublicKey>,
    index: u32,
) -> bdk_chain::bitcoin::ScriptBuf {
    use bdk_chain::bitcoin::key::Secp256k1;
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

/// Parses a descriptor string.
#[cfg(feature = "miniscript")]
pub fn parse_descriptor(descriptor: &str) -> (Descriptor<DescriptorPublicKey>, KeyMap) {
    use bdk_chain::bitcoin::key::Secp256k1;
    let secp = Secp256k1::signing_only();
    Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, descriptor).unwrap()
}

#[allow(unused)]
pub const DESCRIPTORS: [&str; 7] = [
    "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)",
    "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)",
    "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0/*)",
    "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)",
    "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/1/*)",
    "wpkh(xprv9s21ZrQH143K4EXURwMHuLS469fFzZyXk7UUpdKfQwhoHcAiYTakpe8pMU2RiEdvrU9McyuE7YDoKcXkoAwEGoK53WBDnKKv2zZbb9BzttX/1/0/*)",
    // non-wildcard
    "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0)",
];
