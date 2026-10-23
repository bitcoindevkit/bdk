use bdk_chain::{local_chain::LocalChain, BlockId, TxGraph};
use bitcoin::{
    absolute, hashes::Hash, transaction, Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn,
    TxOut, Txid,
};

#[test]
fn null_prevout_conflicts_remain_canonical() {
    let mut graph = TxGraph::<BlockId>::default();
    for tag in [1u8, 2] {
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: [
                OutPoint::null(),
                OutPoint::new(Txid::from_byte_array([tag; 32]), 0),
            ]
            .map(|previous_output| TxIn {
                previous_output,
                ..Default::default()
            })
            .to_vec(),
            output: vec![TxOut {
                value: Amount::from_sat(1),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        assert!(!tx.is_coinbase());
        let _ = graph.insert_seen_at(tx.compute_txid(), u64::from(tag));
        let _ = graph.insert_tx(tx);
    }
    let chain =
        LocalChain::from_blocks([(0, BlockHash::all_zeros())].into_iter().collect()).unwrap();
    let view = chain.canonical_view(&graph, chain.tip().block_id(), Default::default());
    let count = view.txs().len();
    assert!(count <= 1, "canonical spenders of null outpoint: {count}");
}

#[test]
fn test_null_prevout_direct_conflicts() {
    let mut graph = TxGraph::<BlockId>::default();

    let tx1 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint::null(),
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([1; 32]), 0),
                ..Default::default()
            },
        ],
        output: vec![TxOut {
            value: Amount::from_sat(1),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    let tx2 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint::null(),
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([2; 32]), 0),
                ..Default::default()
            },
        ],
        output: vec![TxOut {
            value: Amount::from_sat(1),
            script_pubkey: ScriptBuf::new(),
        }],
    };

    assert!(!tx1.is_coinbase());
    assert!(!tx2.is_coinbase());

    let txid1 = tx1.compute_txid();
    let txid2 = tx2.compute_txid();

    let _ = graph.insert_tx(tx1.clone());
    let _ = graph.insert_tx(tx2.clone());

    // Both non-coinbase transactions spend OutPoint::null(), so they must conflict with each other
    let conflicts1: Vec<_> = graph.direct_conflicts(&tx1).collect();
    let conflicts2: Vec<_> = graph.direct_conflicts(&tx2).collect();

    assert_eq!(conflicts1, vec![(0, txid2)]);
    assert_eq!(conflicts2, vec![(0, txid1)]);
    assert_eq!(graph.outspends(OutPoint::null()).len(), 2);
}

#[test]
fn test_coinbase_isolated_from_null_prevout_conflicts() {
    let mut graph = TxGraph::<BlockId>::default();

    // Valid coinbase transaction
    let coinbase = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    assert!(coinbase.is_coinbase());
    let cb_txid = coinbase.compute_txid();
    let _ = graph.insert_tx(coinbase.clone());

    // Coinbase does not count as spending OutPoint::null()
    assert!(graph.outspends(OutPoint::null()).is_empty());
    assert_eq!(graph.direct_conflicts(&coinbase).count(), 0);

    // Malformed non-coinbase transaction spending OutPoint::null()
    let malformed = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint::null(),
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([3; 32]), 0),
                ..Default::default()
            },
        ],
        output: vec![TxOut {
            value: Amount::from_sat(1),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    assert!(!malformed.is_coinbase());
    let malformed_txid = malformed.compute_txid();
    let _ = graph.insert_tx(malformed.clone());

    // The coinbase still has no conflicts and is not considered in conflict with the malformed tx
    assert_eq!(graph.direct_conflicts(&coinbase).count(), 0);
    // The malformed tx does not conflict with the coinbase either
    assert_eq!(graph.direct_conflicts(&malformed).count(), 0);
    // Only the malformed tx is listed under outspends of null outpoint
    let null_outspends = graph.outspends(OutPoint::null());
    assert_eq!(null_outspends.len(), 1);
    assert!(null_outspends.contains(&malformed_txid));
    assert!(!null_outspends.contains(&cb_txid));
}
