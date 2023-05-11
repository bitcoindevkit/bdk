#![cfg(feature = "miniscript")]
#[macro_use]
mod common;

use bdk_chain::{
    keychain::{Balance, KeychainTracker},
    miniscript::{
        bitcoin::{secp256k1::Secp256k1, OutPoint, PackedLockTime, Transaction, TxOut},
        Descriptor,
    },
    BlockId, ConfirmationTime, TxHeight,
};
use bitcoin::TxIn;

#[test]
fn test_insert_tx() {
    let mut tracker = KeychainTracker::default();
    let secp = Secp256k1::new();
    let (descriptor, _) = Descriptor::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
    tracker.add_keychain((), descriptor.clone());
    let txout = TxOut {
        value: 100_000,
        script_pubkey: descriptor.at_derivation_index(5).script_pubkey(),
    };

    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![txout],
    };

    let _ = tracker.txout_index.reveal_to_target(&(), 5);

    let changeset = tracker
        .insert_tx_preview(tx.clone(), ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();
    tracker.apply_changeset(changeset);
    assert_eq!(
        tracker
            .chain_graph()
            .transactions_in_chain()
            .collect::<Vec<_>>(),
        vec![(&ConfirmationTime::Unconfirmed { last_seen: 0 }, &tx,)]
    );

    assert_eq!(
        tracker
            .txout_index
            .txouts_of_keychain(&())
            .collect::<Vec<_>>(),
        vec![(
            5,
            OutPoint {
                txid: tx.txid(),
                vout: 0
            }
        )]
    );
}

#[test]
fn test_balance() {
    use core::str::FromStr;
    #[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
    enum Keychain {
        One,
        Two,
    }
    let mut tracker = KeychainTracker::default();
    let one = Descriptor::from_str("tr([73c5da0a/86'/0'/0']xpub6BgBgsespWvERF3LHQu6CnqdvfEvtMcQjYrcRzx53QJjSxarj2afYWcLteoGVky7D3UKDP9QyrLprQ3VCECoY49yfdDEHGCtMMj92pReUsQ/0/*)#rg247h69").unwrap();
    let two = Descriptor::from_str("tr([73c5da0a/86'/0'/0']xpub6BgBgsespWvERF3LHQu6CnqdvfEvtMcQjYrcRzx53QJjSxarj2afYWcLteoGVky7D3UKDP9QyrLprQ3VCECoY49yfdDEHGCtMMj92pReUsQ/1/*)#ju05rz2a").unwrap();
    tracker.add_keychain(Keychain::One, one);
    tracker.add_keychain(Keychain::Two, two);

    let tx1 = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 13_000,
            script_pubkey: tracker
                .txout_index
                .reveal_next_spk(&Keychain::One)
                .0
                 .1
                .clone(),
        }],
    };

    let tx2 = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 7_000,
            script_pubkey: tracker
                .txout_index
                .reveal_next_spk(&Keychain::Two)
                .0
                 .1
                .clone(),
        }],
    };

    let tx_coinbase = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: 11_000,
            script_pubkey: tracker
                .txout_index
                .reveal_next_spk(&Keychain::Two)
                .0
                 .1
                .clone(),
        }],
    };

    assert!(tx_coinbase.is_coin_base());

    let _ = tracker
        .insert_checkpoint(BlockId {
            height: 5,
            hash: h!("1"),
        })
        .unwrap();

    let should_trust = |keychain: &Keychain| match *keychain {
        Keychain::One => false,
        Keychain::Two => true,
    };

    assert_eq!(tracker.balance(should_trust), Balance::default());

    let _ = tracker
        .insert_tx(tx1.clone(), TxHeight::Unconfirmed)
        .unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            untrusted_pending: 13_000,
            ..Default::default()
        }
    );

    let _ = tracker
        .insert_tx(tx2.clone(), TxHeight::Unconfirmed)
        .unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            trusted_pending: 7_000,
            untrusted_pending: 13_000,
            ..Default::default()
        }
    );

    let _ = tracker
        .insert_tx(tx_coinbase, TxHeight::Confirmed(0))
        .unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            trusted_pending: 7_000,
            untrusted_pending: 13_000,
            immature: 11_000,
            ..Default::default()
        }
    );

    let _ = tracker.insert_tx(tx1, TxHeight::Confirmed(1)).unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            trusted_pending: 7_000,
            untrusted_pending: 0,
            immature: 11_000,
            confirmed: 13_000,
        }
    );

    let _ = tracker.insert_tx(tx2, TxHeight::Confirmed(2)).unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            trusted_pending: 0,
            untrusted_pending: 0,
            immature: 11_000,
            confirmed: 20_000,
        }
    );

    let _ = tracker
        .insert_checkpoint(BlockId {
            height: 98,
            hash: h!("98"),
        })
        .unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            trusted_pending: 0,
            untrusted_pending: 0,
            immature: 11_000,
            confirmed: 20_000,
        }
    );

    let _ = tracker
        .insert_checkpoint(BlockId {
            height: 99,
            hash: h!("99"),
        })
        .unwrap();

    assert_eq!(
        tracker.balance(should_trust),
        Balance {
            trusted_pending: 0,
            untrusted_pending: 0,
            immature: 0,
            confirmed: 31_000,
        }
    );

    assert_eq!(tracker.balance_at(0), 0);
    assert_eq!(tracker.balance_at(1), 13_000);
    assert_eq!(tracker.balance_at(2), 20_000);
    assert_eq!(tracker.balance_at(98), 20_000);
    assert_eq!(tracker.balance_at(99), 31_000);
    assert_eq!(tracker.balance_at(100), 31_000);
}
