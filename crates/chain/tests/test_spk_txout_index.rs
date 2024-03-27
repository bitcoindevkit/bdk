use bdk_chain::{indexed_tx_graph::Indexer, SpkTxOutIndex};
use bitcoin::{absolute, OutPoint, ScriptBuf, Transaction, TxIn, TxOut};

#[test]
fn spk_txout_sent_and_received() {
    let spk1 = ScriptBuf::from_hex("001404f1e52ce2bab3423c6a8c63b7cd730d8f12542c").unwrap();
    let spk2 = ScriptBuf::from_hex("00142b57404ae14f08c3a0c903feb2af7830605eb00f").unwrap();

    let mut index = SpkTxOutIndex::default();
    index.insert_spk(0, spk1.clone());
    index.insert_spk(1, spk2.clone());

    let tx1 = Transaction {
        version: 0x02,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: 42_000,
            script_pubkey: spk1.clone(),
        }],
    };

    assert_eq!(index.sent_and_received(&tx1), (0, 42_000));
    assert_eq!(index.net_value(&tx1), 42_000);
    index.index_tx(&tx1);
    assert_eq!(
        index.sent_and_received(&tx1),
        (0, 42_000),
        "shouldn't change after scanning"
    );

    let tx2 = Transaction {
        version: 0x1,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: tx1.txid(),
                vout: 0,
            },
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: 20_000,
                script_pubkey: spk2,
            },
            TxOut {
                script_pubkey: spk1,
                value: 30_000,
            },
        ],
    };

    assert_eq!(index.sent_and_received(&tx2), (42_000, 50_000));
    assert_eq!(index.net_value(&tx2), 8_000);
}

#[test]
fn mark_used() {
    let spk1 = ScriptBuf::from_hex("001404f1e52ce2bab3423c6a8c63b7cd730d8f12542c").unwrap();
    let spk2 = ScriptBuf::from_hex("00142b57404ae14f08c3a0c903feb2af7830605eb00f").unwrap();

    let mut spk_index = SpkTxOutIndex::default();
    spk_index.insert_spk(1, spk1.clone());
    spk_index.insert_spk(2, spk2);

    assert!(!spk_index.is_used(&1));
    spk_index.mark_used(&1);
    assert!(spk_index.is_used(&1));
    spk_index.unmark_used(&1);
    assert!(!spk_index.is_used(&1));
    spk_index.mark_used(&1);
    assert!(spk_index.is_used(&1));

    let tx1 = Transaction {
        version: 0x02,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: 42_000,
            script_pubkey: spk1,
        }],
    };

    spk_index.index_tx(&tx1);
    spk_index.unmark_used(&1);
    assert!(
        spk_index.is_used(&1),
        "even though we unmark_used it doesn't matter because there was a tx scanned that used it"
    );
}

#[test]
fn unmark_used_does_not_result_in_invalid_representation() {
    let mut spk_index = SpkTxOutIndex::default();
    assert!(!spk_index.unmark_used(&0));
    assert!(!spk_index.unmark_used(&1));
    assert!(!spk_index.unmark_used(&2));
    assert!(spk_index.unused_spks(..).next().is_none());
}
