use bdk_chain::{
    bitcoin,
    miniscript::{Descriptor, DescriptorPublicKey},
    BlockId,
};
use bitcoin::{
    absolute::LockTime, constants, hashes::Hash, key::Secp256k1, transaction::Version, BlockHash,
    Network, ScriptBuf, Transaction, TxOut,
};

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

/// Generate a dummy script pubkey.
#[allow(unused_macros)]
#[macro_export]
macro_rules! spk {
    () => {{
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let (x_only_pk, _) = bitcoin::secp256k1::SecretKey::new(&mut rand::thread_rng())
            .public_key(&secp)
            .x_only_public_key();
        bitcoin::ScriptBuf::new_p2tr(&secp, x_only_pk, None)
    }};
}

#[allow(unused)]
pub fn new_tx(lt: u32) -> Transaction {
    Transaction {
        version: bitcoin::transaction::Version::non_standard(0x00),
        lock_time: bitcoin::absolute::LockTime::from_consensus(lt),
        input: vec![],
        output: vec![],
    }
}

/// Initialize a standard transaction with a guaranteed output.
pub fn new_standard_tx(lt: u32) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::from_consensus(lt),
        input: vec![],
        output: vec![TxOut::NULL],
    }
}

pub fn genesis_block_id() -> BlockId {
    BlockId {
        height: 0,
        hash: constants::genesis_block(Network::Regtest).block_hash(),
    }
}

pub fn tip_block_id(height: u32) -> BlockId {
    BlockId {
        height,
        hash: BlockHash::all_zeros(),
    }
}

/// Derives a [`ScriptBuf`] (scriptPubkey) from the provided descriptor at a specific index.
pub fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
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
