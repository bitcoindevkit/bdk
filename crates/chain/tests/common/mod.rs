use std::collections::BTreeMap;

use bdk_chain::{
    indexed_tx_graph::IndexedTxGraph, keychain::KeychainTxOutIndex, local_chain::LocalChain,
    ConfirmationHeightAnchor, SpkIterator,
};
use bitcoin::{secp256k1::Secp256k1, BlockHash, OutPoint, Transaction, TxIn, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

#[allow(unused_macros)]
macro_rules! h {
    ($index:expr) => {{
        bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

#[allow(unused_macros)]
macro_rules! local_chain {
    [ $(($height:expr, $block_hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $block_hash).into()),*])
    }};
}

#[allow(unused_macros)]
macro_rules! chain {
    ($([$($tt:tt)*]),*) => { chain!( checkpoints: [$([$($tt)*]),*] ) };
    (checkpoints: $($tail:tt)*) => { chain!( index: TxHeight, checkpoints: $($tail)*) };
    (index: $ind:ty, checkpoints: [ $([$height:expr, $block_hash:expr]),* ] $(,txids: [$(($txid:expr, $tx_height:expr)),*])?) => {{
        #[allow(unused_mut)]
        let mut chain = bdk_chain::sparse_chain::SparseChain::<$ind>::from_checkpoints([$(($height, $block_hash).into()),*]);

        $(
            $(
                let _ = chain.insert_tx($txid, $tx_height).expect("should succeed");
            )*
        )?

        chain
    }};
}

#[allow(unused_macros)]
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
        version: 0x00,
        lock_time: bitcoin::PackedLockTime(lt),
        input: vec![],
        output: vec![],
    }
}

#[allow(unused)]
pub fn single_descriptor_setup() -> (
    LocalChain,
    IndexedTxGraph<ConfirmationHeightAnchor, KeychainTxOutIndex<()>>,
    Descriptor<DescriptorPublicKey>,
) {
    let local_chain = (0..10)
        .map(|i| (i as u32, h!(format!("Block {}", i))))
        .collect::<BTreeMap<u32, BlockHash>>();
    let local_chain = LocalChain::from(local_chain);

    let (desc_1, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)").unwrap();

    let mut graph = IndexedTxGraph::<ConfirmationHeightAnchor, KeychainTxOutIndex<()>>::default();

    graph.index.add_keychain((), desc_1.clone());
    graph.index.set_lookahead_for_all(100);

    (local_chain, graph, desc_1)
}

#[allow(unused)]
pub fn setup_conflicts(
    spk_iter: &mut SpkIterator<&Descriptor<DescriptorPublicKey>>,
) -> (Transaction, Transaction, Transaction) {
    let tx1 = Transaction {
        output: vec![TxOut {
            script_pubkey: spk_iter.next().unwrap().1,
            value: 10000,
        }],
        ..new_tx(0)
    };

    let tx_conflict_1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx1.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            script_pubkey: spk_iter.next().unwrap().1,
            value: 20000,
        }],
        ..new_tx(0)
    };

    let tx_conflict_2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx1.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            script_pubkey: spk_iter.next().unwrap().1,
            value: 30000,
        }],
        ..new_tx(0)
    };

    (tx1, tx_conflict_1, tx_conflict_2)
}
