use bdk_chain::{spk_txout::SpkTxOutIndex, Indexer};
use bitcoin::{
    absolute, transaction, Amount, OutPoint, ScriptBuf, SignedAmount, Transaction, TxIn, TxOut,
};
use core::ops::Bound;

#[test]
fn spk_txout_sent_and_received() {
    let spk1 = ScriptBuf::from_hex("001404f1e52ce2bab3423c6a8c63b7cd730d8f12542c").unwrap();
    let spk2 = ScriptBuf::from_hex("00142b57404ae14f08c3a0c903feb2af7830605eb00f").unwrap();

    let mut index = SpkTxOutIndex::default();
    index.insert_spk(0, spk1.clone());
    index.insert_spk(1, spk2.clone());

    let tx1 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(42_000),
            script_pubkey: spk1.clone(),
        }],
    };

    assert_eq!(
        index.sent_and_received(&tx1, ..),
        (Amount::from_sat(0), Amount::from_sat(42_000))
    );
    assert_eq!(
        index.sent_and_received(&tx1, ..1),
        (Amount::from_sat(0), Amount::from_sat(42_000))
    );
    assert_eq!(
        index.sent_and_received(&tx1, 1..),
        (Amount::from_sat(0), Amount::from_sat(0))
    );
    assert_eq!(index.net_value(&tx1, ..), SignedAmount::from_sat(42_000));
    index.index_tx(&tx1);
    assert_eq!(
        index.sent_and_received(&tx1, ..),
        (Amount::from_sat(0), Amount::from_sat(42_000)),
        "shouldn't change after scanning"
    );

    let tx2 = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: tx1.compute_txid(),
                vout: 0,
            },
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: spk2,
            },
            TxOut {
                script_pubkey: spk1,
                value: Amount::from_sat(30_000),
            },
        ],
    };

    assert_eq!(
        index.sent_and_received(&tx2, ..),
        (Amount::from_sat(42_000), Amount::from_sat(50_000))
    );
    assert_eq!(
        index.sent_and_received(&tx2, ..1),
        (Amount::from_sat(42_000), Amount::from_sat(30_000))
    );
    assert_eq!(
        index.sent_and_received(&tx2, 1..),
        (Amount::from_sat(0), Amount::from_sat(20_000))
    );
    assert_eq!(index.net_value(&tx2, ..), SignedAmount::from_sat(8_000));
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
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(42_000),
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
    assert!(spk_index.unused_spks(..).collect::<Vec<_>>().is_empty());
}

#[test]
fn outputs_in_range_excluded_bounds() {
    let spk1 = ScriptBuf::from_hex("001404f1e52ce2bab3423c6a8c63b7cd730d8f12542c").unwrap();
    let spk2 = ScriptBuf::from_hex("00142b57404ae14f08c3a0c903feb2af7830605eb00f").unwrap();
    let spk3 = ScriptBuf::from_hex("0014afa973f4364b2772d35f7a13ed83eb0c3330cf9c").unwrap();
    let spk4 = ScriptBuf::from_hex("00140707d2493460cad9bb20f5f447a5a89d16d9e21c").unwrap();
    let spk5 = ScriptBuf::from_hex("0014a10d9257489e685dda030662390dc177852faf13").unwrap();

    let mut spk_index = SpkTxOutIndex::default();
    spk_index.insert_spk(1, spk1.clone());
    spk_index.insert_spk(2, spk2.clone());
    spk_index.insert_spk(3, spk3.clone());
    spk_index.insert_spk(4, spk4.clone());
    spk_index.insert_spk(5, spk5.clone());

    let tx1 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: spk1,
        }],
    };

    let tx2 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(20_000),
            script_pubkey: spk2,
        }],
    };

    let tx3 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(30_000),
            script_pubkey: spk3,
        }],
    };

    let tx4 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(40_000),
            script_pubkey: spk4,
        }],
    };

    let tx5 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: spk5,
        }],
    };

    spk_index.index_tx(&tx1);
    spk_index.index_tx(&tx2);
    spk_index.index_tx(&tx3);
    spk_index.index_tx(&tx4);
    spk_index.index_tx(&tx5);

    let tx1_op = OutPoint {
        txid: tx1.compute_txid(),
        vout: 0,
    };
    let tx2_op = OutPoint {
        txid: tx2.compute_txid(),
        vout: 0,
    };
    let tx3_op = OutPoint {
        txid: tx3.compute_txid(),
        vout: 0,
    };
    let tx4_op = OutPoint {
        txid: tx4.compute_txid(),
        vout: 0,
    };

    // Full range (unbounded)
    let all_outputs: Vec<_> = spk_index
        .outputs_in_range((Bound::Unbounded::<u32>, Bound::Unbounded::<u32>))
        .collect();
    assert_eq!(all_outputs.len(), 5);

    // Included start, included end
    let outputs_one_to_four: Vec<_> = spk_index
        .outputs_in_range((Bound::Included(1u32), Bound::Included(4u32)))
        .collect();
    assert_eq!(outputs_one_to_four.len(), 4);
    assert!(outputs_one_to_four
        .iter()
        .any(|(i, op)| **i == 1 && *op == tx1_op));
    assert!(outputs_one_to_four
        .iter()
        .any(|(i, op)| **i == 4 && *op == tx4_op));
    assert!(!outputs_one_to_four.iter().any(|(i, _)| **i == 5));

    // Included start, Excluded end
    let outputs_one_to_four_excl: Vec<_> = spk_index
        .outputs_in_range((Bound::Included(1u32), Bound::Excluded(4u32)))
        .collect();
    assert_eq!(outputs_one_to_four_excl.len(), 3);
    assert!(outputs_one_to_four_excl.iter().any(|(i, _)| **i == 1));
    assert!(outputs_one_to_four_excl
        .iter()
        .any(|(i, op)| **i == 3 && *op == tx3_op));
    assert!(!outputs_one_to_four_excl.iter().any(|(i, _)| **i == 4));

    // Excluded start, Included end
    let outputs_one_excl_to_four: Vec<_> = spk_index
        .outputs_in_range((Bound::Excluded(1u32), Bound::Included(4u32)))
        .collect();
    assert_eq!(outputs_one_excl_to_four.len(), 3,);
    assert!(!outputs_one_excl_to_four.iter().any(|(i, _)| **i == 1));
    assert!(outputs_one_excl_to_four
        .iter()
        .any(|(i, op)| **i == 2 && *op == tx2_op));
    assert!(outputs_one_excl_to_four.iter().any(|(i, _)| **i == 4));

    // Excluded start, Excluded end
    let outputs_one_excl_to_four_excl: Vec<_> = spk_index
        .outputs_in_range((Bound::Excluded(1u32), Bound::Excluded(4u32)))
        .collect();
    assert_eq!(outputs_one_excl_to_four_excl.len(), 2);
    assert!(!outputs_one_excl_to_four_excl.iter().any(|(i, _)| **i == 1));
    assert!(outputs_one_excl_to_four_excl.iter().any(|(i, _)| **i == 2));
    assert!(outputs_one_excl_to_four_excl.iter().any(|(i, _)| **i == 3));
    assert!(!outputs_one_excl_to_four_excl.iter().any(|(i, _)| **i == 4));

    // Unbounded start, Included end
    let outputs_to_three: Vec<_> = spk_index
        .outputs_in_range((Bound::Unbounded::<u32>, Bound::Included(3u32)))
        .collect();
    assert_eq!(outputs_to_three.len(), 3,);
    assert!(outputs_to_three.iter().any(|(i, _)| **i == 1));
    assert!(outputs_to_three.iter().any(|(i, _)| **i == 3));
    assert!(!outputs_to_three.iter().any(|(i, _)| **i == 4));

    // Unbounded start, excluded end
    let outputs_to_three_excl: Vec<_> = spk_index
        .outputs_in_range((Bound::Unbounded::<u32>, Bound::Excluded(3u32)))
        .collect();
    assert_eq!(outputs_to_three_excl.len(), 2,);
    assert!(outputs_to_three_excl.iter().any(|(i, _)| **i == 1));
    assert!(outputs_to_three_excl.iter().any(|(i, _)| **i == 2));
    assert!(!outputs_to_three_excl.iter().any(|(i, _)| **i == 3),);

    // Included start, Unbounded end
    let outputs_to_three: Vec<_> = spk_index
        .outputs_in_range((Bound::Included(3u32), Bound::Unbounded::<u32>))
        .collect();
    assert_eq!(outputs_to_three.len(), 3,);
    assert!(outputs_to_three.iter().any(|(i, _)| **i == 3));
    assert!(outputs_to_three.iter().any(|(i, _)| **i == 5),);
    assert!(!outputs_to_three.iter().any(|(i, _)| **i == 2),);

    // Excluded start, Unbounded end
    let outputs_to_three_excl: Vec<_> = spk_index
        .outputs_in_range((Bound::Excluded(3u32), Bound::Unbounded::<u32>))
        .collect();
    assert_eq!(outputs_to_three_excl.len(), 2,);
    assert!(!outputs_to_three_excl.iter().any(|(i, _)| **i == 3));
    assert!(outputs_to_three_excl.iter().any(|(i, _)| **i == 5),);
    assert!(outputs_to_three_excl.iter().any(|(i, _)| **i == 4));

    // Single element range
    let output_at_three: Vec<_> = spk_index
        .outputs_in_range((Bound::Included(3u32), Bound::Included(3u32)))
        .collect();
    assert_eq!(output_at_three.len(), 1,);
    assert!(output_at_three.iter().any(|(i, _)| **i == 3));
    assert!(!output_at_three.iter().any(|(i, _)| **i == 4));
    assert!(!output_at_three.iter().any(|(i, _)| **i == 2));

    // Empty range with excluded bound
    let outputs_empty: Vec<_> = spk_index
        .outputs_in_range((Bound::Included(3u32), Bound::Excluded(3u32)))
        .collect();
    assert_eq!(outputs_empty.len(), 0);
}
