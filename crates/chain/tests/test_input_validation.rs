use bdk_chain::local_chain::LocalChain;
use bdk_chain::validation::{InputValidity, TxValidity};
use bdk_chain::{CanonicalParams, CanonicalView, ConfirmationBlockTime, TxGraph};
use bdk_testenv::hash;
use bitcoin::hashes::Hash;
use bitcoin::key::{Keypair, TapTweak};
use bitcoin::script::{Builder, PushBytesBuf};
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{EcdsaSighashType, Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot::{LeafVersion, TaprootBuilder};
use bitcoin::{
    absolute, opcodes, transaction, Amount, CompressedPublicKey, OutPoint, ScriptBuf, Sequence,
    TapLeafHash, Transaction, TxIn, TxOut, Txid, Witness,
};

#[derive(Clone, Copy)]
enum Kind {
    P2wpkh,
    P2pkh,
    P2shMultisig,
    P2trKey,
    P2trScript,
}

#[derive(Clone)]
struct Coin {
    kind: Kind,
    key: u8,
    outpoint: OutPoint,
    txout: TxOut,
    /// The whole transaction creating the coin, whose txid authenticates `txout`.
    parent: Transaction,
}

fn secret(key: u8) -> SecretKey {
    SecretKey::from_slice(&[key; 32]).unwrap()
}

fn pubkey(key: u8) -> CompressedPublicKey {
    CompressedPublicKey(secret(key).public_key(&Secp256k1::new()))
}

fn keypair(key: u8) -> Keypair {
    Keypair::from_secret_key(&Secp256k1::new(), &secret(key))
}

fn multisig_script(key: u8) -> ScriptBuf {
    Builder::new()
        .push_int(2)
        .push_key(&pubkey(key).into())
        .push_key(&pubkey(key + 1).into())
        .push_int(2)
        .push_opcode(opcodes::all::OP_CHECKMULTISIG)
        .into_script()
}

fn leaf_script(key: u8) -> ScriptBuf {
    Builder::new()
        .push_x_only_key(&keypair(key).x_only_public_key().0)
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .into_script()
}

/// The script tree's internal key is `key + 1` so that only the script path can spend.
fn tap_tree(key: u8) -> bitcoin::taproot::TaprootSpendInfo {
    TaprootBuilder::new()
        .add_leaf(0, leaf_script(key))
        .unwrap()
        .finalize(&Secp256k1::new(), keypair(key + 1).x_only_public_key().0)
        .unwrap()
}

fn script_pubkey(kind: Kind, key: u8) -> ScriptBuf {
    let secp = Secp256k1::new();
    match kind {
        Kind::P2wpkh => ScriptBuf::new_p2wpkh(&pubkey(key).wpubkey_hash()),
        Kind::P2pkh => ScriptBuf::new_p2pkh(&bitcoin::PublicKey::from(pubkey(key)).pubkey_hash()),
        Kind::P2shMultisig => ScriptBuf::new_p2sh(&multisig_script(key).script_hash()),
        Kind::P2trKey => ScriptBuf::new_p2tr(&secp, keypair(key).x_only_public_key().0, None),
        Kind::P2trScript => ScriptBuf::new_p2tr_tweaked(tap_tree(key).output_key()),
    }
}

/// A coin created by a parent transaction that spends an output no graph has.
fn coin(kind: Kind, key: u8, sats: u64) -> Coin {
    let parent = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::hash(&[kind as u8, key]), 0),
            ..Default::default()
        }],
        output: vec![pay(kind, key, sats)],
    };
    output_coin(&parent, 0, kind, key)
}

fn output_coin(tx: &Transaction, vout: u32, kind: Kind, key: u8) -> Coin {
    Coin {
        kind,
        key,
        outpoint: OutPoint::new(tx.compute_txid(), vout),
        txout: tx.output[vout as usize].clone(),
        parent: tx.clone(),
    }
}

/// Builds and signs a transaction spending `coins` to `outputs`.
fn spend(coins: &[Coin], outputs: Vec<TxOut>) -> Transaction {
    let secp = Secp256k1::new();
    let mut tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: coins
            .iter()
            .map(|c| TxIn {
                previous_output: c.outpoint,
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..Default::default()
            })
            .collect(),
        output: outputs,
    };
    let prevouts = coins.iter().map(|c| c.txout.clone()).collect::<Vec<_>>();
    let mut cache = SighashCache::new(tx.clone());
    let ecdsa = |digest: [u8; 32], key: u8| {
        let sig = secp.sign_ecdsa(&Message::from_digest(digest), &secret(key));
        bitcoin::ecdsa::Signature::sighash_all(sig)
    };
    for (vin, c) in coins.iter().enumerate() {
        let txin = &mut tx.input[vin];
        match c.kind {
            Kind::P2wpkh => {
                let hash = cache
                    .p2wpkh_signature_hash(
                        vin,
                        &c.txout.script_pubkey,
                        c.txout.value,
                        EcdsaSighashType::All,
                    )
                    .unwrap();
                let sig = ecdsa(hash.to_byte_array(), c.key);
                txin.witness = Witness::p2wpkh(&sig, &pubkey(c.key).0);
            }
            Kind::P2pkh => {
                let hash = cache
                    .legacy_signature_hash(vin, &c.txout.script_pubkey, 1)
                    .unwrap();
                let sig = ecdsa(hash.to_byte_array(), c.key);
                txin.script_sig = Builder::new()
                    .push_slice(PushBytesBuf::try_from(sig.to_vec()).unwrap())
                    .push_key(&pubkey(c.key).into())
                    .into_script();
            }
            Kind::P2shMultisig => {
                let redeem = multisig_script(c.key);
                let hash = cache.legacy_signature_hash(vin, &redeem, 1).unwrap();
                let sig1 = ecdsa(hash.to_byte_array(), c.key);
                let sig2 = ecdsa(hash.to_byte_array(), c.key + 1);
                txin.script_sig = Builder::new()
                    .push_int(0)
                    .push_slice(PushBytesBuf::try_from(sig1.to_vec()).unwrap())
                    .push_slice(PushBytesBuf::try_from(sig2.to_vec()).unwrap())
                    .push_slice(PushBytesBuf::try_from(redeem.to_bytes()).unwrap())
                    .into_script();
            }
            Kind::P2trKey => {
                let hash = cache
                    .taproot_key_spend_signature_hash(
                        vin,
                        &Prevouts::All(&prevouts),
                        TapSighashType::Default,
                    )
                    .unwrap();
                let tweaked = keypair(c.key).tap_tweak(&secp, None).to_keypair();
                let sig = secp.sign_schnorr_no_aux_rand(
                    &Message::from_digest(hash.to_byte_array()),
                    &tweaked,
                );
                txin.witness = Witness::from_slice(&[sig.as_ref()]);
            }
            Kind::P2trScript => {
                let script = leaf_script(c.key);
                let leaf_hash = TapLeafHash::from_script(&script, LeafVersion::TapScript);
                let hash = cache
                    .taproot_script_spend_signature_hash(
                        vin,
                        &Prevouts::All(&prevouts),
                        leaf_hash,
                        TapSighashType::Default,
                    )
                    .unwrap();
                let sig = secp.sign_schnorr_no_aux_rand(
                    &Message::from_digest(hash.to_byte_array()),
                    &keypair(c.key),
                );
                let control = tap_tree(c.key)
                    .control_block(&(script.clone(), LeafVersion::TapScript))
                    .unwrap();
                txin.witness = Witness::from_slice(&[
                    sig.as_ref().to_vec(),
                    script.to_bytes(),
                    control.serialize(),
                ]);
            }
        }
    }
    tx
}

fn pay(kind: Kind, key: u8, sats: u64) -> TxOut {
    TxOut {
        value: Amount::from_sat(sats),
        script_pubkey: script_pubkey(kind, key),
    }
}

/// Spends `coin` with a signature over different outputs, so the scripts fail.
fn bad_spend(coin: &Coin, outputs: Vec<TxOut>) -> Transaction {
    let mut other = outputs.clone();
    other[0].value += Amount::from_sat(1);
    let mut tx = spend(core::slice::from_ref(coin), other);
    tx.output = outputs;
    tx
}

type Graph = TxGraph<ConfirmationBlockTime>;

fn insert_coin(graph: &mut Graph, coin: &Coin) {
    let _ = graph.insert_tx(coin.parent.clone());
}

fn chain() -> LocalChain {
    LocalChain::from_blocks(
        [(0, hash!("0")), (1, hash!("1")), (2, hash!("2"))]
            .into_iter()
            .collect(),
    )
    .unwrap()
}

fn anchor(chain: &LocalChain, height: u32) -> ConfirmationBlockTime {
    ConfirmationBlockTime {
        block_id: chain.get(height).unwrap().block_id(),
        confirmation_time: height as u64,
    }
}

fn view(
    chain: &LocalChain,
    graph: &Graph,
    params: CanonicalParams,
) -> CanonicalView<ConfirmationBlockTime> {
    chain.canonical_view(graph, chain.tip().block_id(), params)
}

fn validity(graph: &Graph, tx: &Transaction) -> TxValidity {
    graph.validate_tx(tx).validity()
}

const VALID: InputValidity = InputValidity::Valid {
    relies_on_floating: false,
};

const VALID_ON_FLOATING: InputValidity = InputValidity::Valid {
    relies_on_floating: true,
};

fn inputs(graph: &Graph, tx: &Transaction) -> Vec<InputValidity> {
    graph.validate_tx(tx).inputs
}

const KINDS: [Kind; 5] = [
    Kind::P2wpkh,
    Kind::P2pkh,
    Kind::P2shMultisig,
    Kind::P2trKey,
    Kind::P2trScript,
];

#[test]
fn every_spend_type_validates() {
    let coins = KINDS
        .iter()
        .enumerate()
        .map(|(i, &kind)| coin(kind, 10 * (i as u8 + 1), 10_000))
        .collect::<Vec<_>>();
    let tx = spend(&coins, vec![pay(Kind::P2wpkh, 1, 40_000)]);
    let mut graph = Graph::default();
    coins.iter().for_each(|c| insert_coin(&mut graph, c));
    assert_eq!(validity(&graph, &tx), TxValidity::Valid);
    assert_eq!(inputs(&graph, &tx), vec![VALID; KINDS.len()]);

    let mut tampered = tx.clone();
    tampered.lock_time = absolute::LockTime::from_consensus(1);
    assert_eq!(validity(&graph, &tampered), TxValidity::Invalid);
    for (vin, validity) in inputs(&graph, &tampered).iter().enumerate() {
        assert!(
            matches!(validity, InputValidity::Invalid { .. }),
            "input {vin}: {validity:?}"
        );
    }
}

#[test]
fn validate_tx_checks_a_tx_the_graph_does_not_have() {
    let present = coin(Kind::P2trKey, 1, 10_000);
    let also_present = coin(Kind::P2wpkh, 2, 10_000);
    let missing = coin(Kind::P2wpkh, 3, 10_000);
    let tx = spend(
        &[present.clone(), also_present.clone(), missing.clone()],
        vec![pay(Kind::P2wpkh, 4, 29_000)],
    );
    let mut graph = Graph::default();
    insert_coin(&mut graph, &present);
    insert_coin(&mut graph, &also_present);
    let before = graph.clone();

    // The taproot input signs every prevout, so it waits on the missing one too.
    assert_eq!(
        inputs(&graph, &tx),
        vec![
            InputValidity::Unknown {
                missing: vec![missing.outpoint]
            },
            VALID,
            InputValidity::Unknown {
                missing: vec![missing.outpoint]
            },
        ]
    );
    assert_eq!(validity(&graph, &tx), TxValidity::Unknown);
    assert_eq!(graph.get_tx(tx.compute_txid()), None);
    assert_eq!(graph, before);

    // A failing input is conclusive whatever else is missing.
    let mut bad = tx.clone();
    bad.input[1].witness = Witness::from_slice(&[vec![1; 9], pubkey(2).to_bytes().to_vec()]);
    let per_input = inputs(&graph, &bad);
    assert!(matches!(per_input[1], InputValidity::Invalid { .. }));
    assert_eq!(
        per_input[2],
        InputValidity::Unknown {
            missing: vec![missing.outpoint]
        }
    );
    assert_eq!(validity(&graph, &bad), TxValidity::Invalid);

    insert_coin(&mut graph, &missing);
    assert_eq!(inputs(&graph, &tx), vec![VALID; 3]);
}

#[test]
fn unconfirmed_invalid_candidate_never_wins_on_last_seen() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let good = spend(core::slice::from_ref(&c), vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let bad = bad_spend(&c, vec![pay(Kind::P2wpkh, 3, 9_000)]);
    let (good_txid, bad_txid) = (good.compute_txid(), bad.compute_txid());

    let mut graph = Graph::default();
    insert_coin(&mut graph, &c);
    let _ = graph.insert_tx(good);
    let _ = graph.insert_tx(bad);
    let _ = graph.insert_seen_at(good_txid, 1);
    let _ = graph.insert_seen_at(bad_txid, 100);

    let view = view(&chain, &graph, Default::default());
    assert!(view.tx(bad_txid).is_none());
    assert!(view.tx(good_txid).is_some());
    assert!(view.txout(OutPoint::new(bad_txid, 0)).is_none());
}

/// Confirmed transactions are out of validation's scope: an anchor in the best chain is trusted
/// as it is without validation, and so is what it pulls in.
#[test]
fn anchored_invalid_tx_and_its_ancestry_stay_canonical() {
    let chain = chain();
    let c = coin(Kind::P2trKey, 1, 10_000);
    let bad = bad_spend(&c, vec![pay(Kind::P2trKey, 2, 9_000)]);
    let child = spend(
        &[output_coin(&bad, 0, Kind::P2trKey, 2)],
        vec![pay(Kind::P2wpkh, 3, 8_000)],
    );
    let (bad_txid, child_txid) = (bad.compute_txid(), child.compute_txid());
    let mut graph = Graph::default();
    insert_coin(&mut graph, &c);
    let _ = graph.insert_tx(bad.clone());
    let _ = graph.insert_tx(child);
    assert_eq!(validity(&graph, &bad), TxValidity::Invalid);

    let mut anchored_bad = graph.clone();
    let _ = anchored_bad.insert_anchor(bad_txid, anchor(&chain, 1));
    let canonical = view(&chain, &anchored_bad, Default::default());
    assert!(canonical.tx(bad_txid).is_some());

    let mut anchored_child = graph;
    let _ = anchored_child.insert_anchor(child_txid, anchor(&chain, 2));
    let canonical = view(&chain, &anchored_child, Default::default());
    assert!(canonical.tx(child_txid).is_some());
    assert!(canonical.tx(bad_txid).is_some());
}

#[test]
fn assumed_invalid_tx_and_its_ancestry_stay_canonical() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let good = spend(core::slice::from_ref(&c), vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let bad = bad_spend(&c, vec![pay(Kind::P2wpkh, 3, 9_000)]);
    let child = spend(
        &[output_coin(&bad, 0, Kind::P2wpkh, 3)],
        vec![pay(Kind::P2wpkh, 4, 8_000)],
    );
    let [good_txid, bad_txid, child_txid] = [&good, &bad, &child].map(|tx| tx.compute_txid());
    let mut graph = Graph::default();
    insert_coin(&mut graph, &c);
    for tx in [good, bad, child] {
        let _ = graph.insert_tx(tx);
    }
    let _ = graph.insert_seen_at(good_txid, 100);
    let _ = graph.insert_seen_at(bad_txid, 1);

    for assumed in [bad_txid, child_txid] {
        let params = CanonicalParams {
            assume_canonical: vec![assumed],
        };
        let view = view(&chain, &graph, params);
        assert!(view.tx(bad_txid).is_some(), "assuming {assumed}");
        assert!(view.tx(good_txid).is_none(), "assuming {assumed}");
    }
}

/// A wallet's pending spend of a confirmed or assumed transaction must stay in view even if the
/// parent is one the verifier calls Invalid: only the child is unconfirmed, and it is valid.
#[test]
fn unconfirmed_spend_of_a_trusted_invalid_tx_stays_canonical() {
    let chain = chain();
    let c = coin(Kind::P2trKey, 1, 10_000);
    let bad = bad_spend(&c, vec![pay(Kind::P2trKey, 2, 9_000)]);
    let child = spend(
        &[output_coin(&bad, 0, Kind::P2trKey, 2)],
        vec![pay(Kind::P2wpkh, 3, 8_000)],
    );
    let (bad_txid, child_txid) = (bad.compute_txid(), child.compute_txid());
    let mut graph = Graph::default();
    insert_coin(&mut graph, &c);
    let _ = graph.insert_tx(bad.clone());
    let _ = graph.insert_tx(child.clone());
    let _ = graph.insert_seen_at(child_txid, 5);
    assert_eq!(validity(&graph, &bad), TxValidity::Invalid);
    assert_eq!(validity(&graph, &child), TxValidity::Valid);

    let mut anchored = graph.clone();
    let _ = anchored.insert_anchor(bad_txid, anchor(&chain, 1));
    let assumed = CanonicalParams {
        assume_canonical: vec![bad_txid],
    };
    for (graph, params) in [(&anchored, CanonicalParams::default()), (&graph, assumed)] {
        let canonical = view(&chain, graph, params);
        assert!(canonical.tx(bad_txid).is_some());
        assert!(canonical.tx(child_txid).is_some());
    }
}

#[test]
fn descendants_of_an_unconfirmed_invalid_tx_are_not_canonical() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let parent = bad_spend(&c, vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let child = spend(
        &[output_coin(&parent, 0, Kind::P2wpkh, 2)],
        vec![pay(Kind::P2wpkh, 3, 8_000)],
    );
    let grandchild = spend(
        &[output_coin(&child, 0, Kind::P2wpkh, 3)],
        vec![pay(Kind::P2wpkh, 4, 7_000)],
    );
    let honest = spend(core::slice::from_ref(&c), vec![pay(Kind::P2wpkh, 5, 9_000)]);
    let txs = [&parent, &child, &grandchild, &honest];
    let txids = txs.map(|tx| tx.compute_txid());

    let mut graph = Graph::default();
    insert_coin(&mut graph, &c);
    for tx in txs {
        let _ = graph.insert_tx(tx.clone());
    }
    // The descendants are seen last, so only validation keeps them out.
    let _ = graph.insert_seen_at(txids[2], 60);
    let _ = graph.insert_seen_at(txids[1], 50);
    let _ = graph.insert_seen_at(txids[3], 1);

    // Script validity is per transaction; ancestry is canonicalization's concern.
    assert_eq!(validity(&graph, &parent), TxValidity::Invalid);
    assert_eq!(validity(&graph, &child), TxValidity::Valid);
    assert_eq!(validity(&graph, &grandchild), TxValidity::Valid);

    let view = view(&chain, &graph, Default::default());
    for excluded in &txids[..3] {
        assert!(view.tx(*excluded).is_none());
    }
    assert!(view.tx(txids[3]).is_some());
    let outpoints = txids.iter().map(|&txid| ((), OutPoint::new(txid, 0)));
    let utxos = view
        .filter_unspent_outpoints(outpoints)
        .map(|(_, txo)| txo.outpoint.txid)
        .collect::<Vec<_>>();
    assert_eq!(utxos, vec![txids[3]]);
}

/// An anchor in a block the best chain no longer has confirms nothing, so the transaction is
/// unconfirmed again and checked as one, whether or not it was ever seen in the mempool.
#[test]
fn reorged_out_tx_is_checked_again() {
    let chain = chain();
    let reorged = LocalChain::from_blocks(
        [(0, hash!("0")), (1, hash!("1'")), (2, hash!("2'"))]
            .into_iter()
            .collect(),
    )
    .unwrap();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let other = coin(Kind::P2wpkh, 2, 10_000);
    let bad = bad_spend(&c, vec![pay(Kind::P2wpkh, 3, 9_000)]);
    let good = spend(
        core::slice::from_ref(&other),
        vec![pay(Kind::P2wpkh, 4, 9_000)],
    );
    let (bad_txid, good_txid) = (bad.compute_txid(), good.compute_txid());

    for seen in [false, true] {
        let mut graph = Graph::default();
        insert_coin(&mut graph, &c);
        insert_coin(&mut graph, &other);
        for tx in [&bad, &good] {
            let _ = graph.insert_tx(tx.clone());
            let _ = graph.insert_anchor(tx.compute_txid(), anchor(&chain, 1));
            if seen {
                let _ = graph.insert_seen_at(tx.compute_txid(), 1);
            }
        }
        let before = view(&chain, &graph, Default::default());
        assert!(before.tx(bad_txid).is_some(), "seen: {seen}");

        let after = view(&reorged, &graph, Default::default());
        assert!(after.tx(bad_txid).is_none(), "seen: {seen}");
        assert!(after.tx(good_txid).is_some(), "seen: {seen}");
    }
}

#[test]
fn unknown_tx_stays_eligible() {
    let chain = chain();
    let c = coin(Kind::P2trKey, 1, 10_000);
    let tx = spend(core::slice::from_ref(&c), vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let txid = tx.compute_txid();
    let mut graph = Graph::default();
    let _ = graph.insert_tx(tx.clone());
    let _ = graph.insert_seen_at(txid, 1);
    assert_eq!(
        inputs(&graph, &tx),
        vec![InputValidity::Unknown {
            missing: vec![c.outpoint]
        }]
    );
    let view = view(&chain, &graph, Default::default());
    assert!(view.tx(txid).is_some());
    assert!(view.txout(OutPoint::new(txid, 0)).is_some());
}

/// Nothing is remembered between canonicalizations, so a verdict always reflects the graph as it
/// is when canonicalizing.
#[test]
fn a_bad_spend_is_excluded_once_its_parent_arrives() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let bad = bad_spend(&c, vec![pay(Kind::P2wpkh, 3, 9_000)]);
    let txid = bad.compute_txid();

    let mut graph = Graph::default();
    let _ = graph.insert_tx(bad.clone());
    let _ = graph.insert_seen_at(txid, 1);
    assert_eq!(validity(&graph, &bad), TxValidity::Unknown);
    assert!(view(&chain, &graph, Default::default()).tx(txid).is_some());

    insert_coin(&mut graph, &c);
    assert_eq!(validity(&graph, &bad), TxValidity::Invalid);
    assert!(view(&chain, &graph, Default::default()).tx(txid).is_none());
}

/// A wallet output whose receiving transaction the graph lacks is known only as the chain source's
/// floating txout, and a forged spend of it cannot carry a valid signature for the wallet's script.
#[test]
fn an_invalid_spend_of_an_output_known_only_as_a_floating_txout_is_excluded() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let good = spend(core::slice::from_ref(&c), vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let bad = bad_spend(&c, vec![pay(Kind::P2wpkh, 3, 9_000)]);
    let (good_txid, bad_txid) = (good.compute_txid(), bad.compute_txid());

    let mut graph = Graph::default();
    let _ = graph.insert_txout(c.outpoint, c.txout.clone());
    let _ = graph.insert_tx(good.clone());
    let _ = graph.insert_tx(bad.clone());
    let _ = graph.insert_seen_at(good_txid, 1);
    let _ = graph.insert_seen_at(bad_txid, 100);

    assert_eq!(inputs(&graph, &good), vec![VALID_ON_FLOATING]);
    assert!(matches!(
        inputs(&graph, &bad)[..],
        [InputValidity::Invalid {
            relies_on_floating: true,
            ..
        }]
    ));
    let view = view(&chain, &graph, Default::default());
    assert!(view.tx(bad_txid).is_none());
    assert!(view.tx(good_txid).is_some());
}

/// A taproot signature's hash commits to every spent output unless it is ANYONECANPAY, so only then
/// does the input's verdict rest on the other inputs' prevouts.
#[test]
fn taproot_provenance_follows_the_prevouts_its_sighash_read() {
    let tap = coin(Kind::P2trKey, 1, 10_000);
    let seg = coin(Kind::P2wpkh, 2, 10_000);
    let tx = spend(
        &[tap.clone(), seg.clone()],
        vec![pay(Kind::P2wpkh, 3, 19_000)],
    );
    let mut graph = Graph::default();
    insert_coin(&mut graph, &tap);
    let _ = graph.insert_txout(seg.outpoint, seg.txout.clone());
    assert_eq!(inputs(&graph, &tx), vec![VALID_ON_FLOATING; 2]);

    // A signature that fails before any hash is computed read only its own prevout.
    let mut early = tx.clone();
    early.input[0].witness = Witness::from_slice(&[vec![1; 9]]);
    assert!(matches!(
        inputs(&graph, &early)[..],
        [
            InputValidity::Invalid {
                relies_on_floating: false,
                ..
            },
            VALID_ON_FLOATING
        ]
    ));

    let secp = Secp256k1::new();
    let mut acp = tx.clone();
    let hash = SighashCache::new(&acp)
        .taproot_key_spend_signature_hash(
            0,
            &Prevouts::One(0, &tap.txout),
            TapSighashType::AllPlusAnyoneCanPay,
        )
        .unwrap();
    let tweaked = keypair(tap.key).tap_tweak(&secp, None).to_keypair();
    let sig = secp.sign_schnorr_no_aux_rand(&Message::from_digest(hash.to_byte_array()), &tweaked);
    let mut sig = sig.as_ref().to_vec();
    sig.push(TapSighashType::AllPlusAnyoneCanPay as u8);
    acp.input[0].witness = Witness::from_slice(&[sig]);
    assert_eq!(inputs(&graph, &acp), vec![VALID, VALID_ON_FLOATING]);
}

#[test]
fn shared_hashes_do_not_leak_floating_provenance_between_inputs() {
    let coins = [
        coin(Kind::P2trKey, 1, 10_000),
        coin(Kind::P2trKey, 2, 10_000),
        coin(Kind::P2wpkh, 3, 10_000),
        coin(Kind::P2pkh, 4, 10_000),
    ];
    let mut tx = spend(&coins, vec![pay(Kind::P2wpkh, 5, 39_000)]);
    let secp = Secp256k1::new();
    let ty = TapSighashType::AllPlusAnyoneCanPay;
    let hash = SighashCache::new(&tx)
        .taproot_key_spend_signature_hash(1, &Prevouts::One(1, &coins[1].txout), ty)
        .unwrap();
    let tweaked = keypair(coins[1].key).tap_tweak(&secp, None).to_keypair();
    let sig = bitcoin::taproot::Signature {
        signature: secp
            .sign_schnorr_no_aux_rand(&Message::from_digest(hash.to_byte_array()), &tweaked),
        sighash_type: ty,
    };
    tx.input[1].witness = Witness::from_slice(&[sig.to_vec()]);

    for wrong in [false, true] {
        let mut graph = Graph::default();
        let mut floating = coins[0].txout.clone();
        if wrong {
            floating.value += Amount::from_sat(1);
        }
        let _ = graph.insert_txout(coins[0].outpoint, floating);
        for coin in &coins[1..3] {
            insert_coin(&mut graph, coin);
        }
        let incomplete = inputs(&graph, &tx);
        assert_eq!(
            incomplete[0],
            InputValidity::Unknown {
                missing: vec![coins[3].outpoint]
            }
        );
        assert_eq!(incomplete[1..3], [VALID, VALID]);
        assert_eq!(
            incomplete[3],
            InputValidity::Unknown {
                missing: vec![coins[3].outpoint]
            }
        );

        insert_coin(&mut graph, &coins[3]);
        let complete = inputs(&graph, &tx);
        if wrong {
            assert!(matches!(
                complete[0],
                InputValidity::Invalid {
                    relies_on_floating: true,
                    ..
                }
            ));
        } else {
            assert_eq!(complete[0], VALID_ON_FLOATING);
        }
        assert_eq!(complete[1..], [VALID, VALID, VALID]);
    }
}

/// Compare the old per-input cache lifetime with the graph's shared verification path.
/// Signing and graph construction are excluded; timings are evidence, not pass/fail thresholds.
#[test]
#[ignore = "manual release-mode signature-hash scaling measurement"]
#[allow(clippy::print_stdout)] // This ignored benchmark reports timings for manual comparison.
fn signature_hash_scaling() {
    use std::{hint::black_box, time::Instant};
    for (label, kind) in [("segwit-v0", Kind::P2wpkh), ("taproot", Kind::P2trKey)] {
        for n in [1, 16, 128, 1024] {
            let coins = (0..n)
                .map(|i| coin(kind, 1, 10_000 + i as u64))
                .collect::<Vec<_>>();
            let tx = spend(&coins, vec![pay(Kind::P2wpkh, 2, 1_000)]);
            let prevouts = coins
                .iter()
                .map(|coin| Some(&coin.txout))
                .collect::<Vec<_>>();
            let mut graph = Graph::default();
            for coin in &coins {
                insert_coin(&mut graph, coin);
            }
            assert_eq!(validity(&graph, &tx), TxValidity::Valid);
            for vin in 0..n {
                assert!(bdk_script_verify::verify_input(
                    &mut bdk_script_verify::VerificationCache::new(&tx, &prevouts),
                    vin,
                    bdk_script_verify::Flags::CONSENSUS
                )
                .result
                .is_ok());
            }
            let rounds = if n < 128 { 100 } else { 10 };
            let start = Instant::now();
            for _ in 0..rounds {
                for vin in 0..n {
                    black_box(bdk_script_verify::verify_input(
                        &mut bdk_script_verify::VerificationCache::new(
                            black_box(&tx),
                            black_box(&prevouts),
                        ),
                        vin,
                        bdk_script_verify::Flags::CONSENSUS,
                    ));
                }
            }
            let independent = start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds);
            let start = Instant::now();
            for _ in 0..rounds {
                black_box(graph.validate_tx(black_box(&tx)));
            }
            let shared = start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds);
            std::println!("{label}: inputs={n}, independent_us={independent:.1}, graph_shared_us={shared:.1}, speedup={:.2}", independent / shared);
        }
    }
}

/// Floating data that a check read before stopping on a missing prevout gave no verdict to rely on.
#[test]
fn an_unknown_input_relies_on_nothing() {
    let tap = coin(Kind::P2trKey, 1, 10_000);
    let seg = coin(Kind::P2wpkh, 2, 10_000);
    let tx = spend(
        &[tap.clone(), seg.clone()],
        vec![pay(Kind::P2wpkh, 3, 19_000)],
    );
    let mut graph = Graph::default();
    let _ = graph.insert_txout(tap.outpoint, tap.txout.clone());
    let unknown = InputValidity::Unknown {
        missing: vec![seg.outpoint],
    };
    assert_eq!(inputs(&graph, &tx), vec![unknown.clone(), unknown]);
    assert_eq!(validity(&graph, &tx), TxValidity::Unknown);
    assert!(!graph.validate_tx(&tx).relies_on_floating());
}

#[test]
fn a_full_parent_takes_precedence_over_a_floating_txout() {
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let tx = spend(core::slice::from_ref(&c), vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let mut wrong = c.txout.clone();
    wrong.value += Amount::from_sat(1);

    // A lying amount fails a genuine segwit signature, and the verdict says whose word it took.
    let mut graph = Graph::default();
    let _ = graph.insert_txout(c.outpoint, wrong);
    assert!(matches!(
        inputs(&graph, &tx)[..],
        [InputValidity::Invalid {
            relies_on_floating: true,
            ..
        }]
    ));
    assert!(graph.validate_tx(&tx).relies_on_floating());

    insert_coin(&mut graph, &c);
    assert_eq!(inputs(&graph, &tx), vec![VALID]);
    assert!(!graph.validate_tx(&tx).relies_on_floating());
}

#[test]
fn aggregate_and_per_input_validity() {
    let signed = coin(Kind::P2wpkh, 1, 10_000);
    let missing = coin(Kind::P2wpkh, 2, 10_000);
    let mut tx = spend(
        &[signed.clone(), missing.clone()],
        vec![pay(Kind::P2wpkh, 3, 19_000)],
    );
    let mut graph = Graph::default();
    insert_coin(&mut graph, &signed);
    assert_eq!(validity(&graph, &tx), TxValidity::Unknown);
    assert_eq!(
        inputs(&graph, &tx),
        vec![
            VALID,
            InputValidity::Unknown {
                missing: vec![missing.outpoint]
            },
        ]
    );

    // One failing input makes the transaction invalid even while another is unknown.
    tx.input[0].witness = Witness::from_slice(&[vec![1; 9], pubkey(1).to_bytes().to_vec()]);
    assert_eq!(validity(&graph, &tx), TxValidity::Invalid);
    let per_input = inputs(&graph, &tx);
    assert!(matches!(per_input[0], InputValidity::Invalid { .. }));
    assert_eq!(
        per_input[1],
        InputValidity::Unknown {
            missing: vec![missing.outpoint]
        }
    );
}

fn graph_with(txs: &[&Transaction]) -> Graph {
    let mut graph = Graph::default();
    for tx in txs {
        let _ = graph.insert_tx((*tx).clone());
    }
    graph
}

/// A valid spend of `c`, plus bogus copies whose witnesses are smaller than the valid one's and
/// sort below and above each other.
fn valid_and_bogus_copies(c: &Coin) -> (Transaction, [Transaction; 2]) {
    let valid = spend(core::slice::from_ref(c), vec![pay(Kind::P2wpkh, 2, 9_000)]);
    let bogus = |fill: u8| {
        let mut tx = valid.clone();
        tx.input[0].witness = Witness::from_slice(&[vec![fill; 9], pubkey(1).to_bytes().to_vec()]);
        tx
    };
    let bogus = [bogus(0x00), bogus(0xff)];
    (valid, bogus)
}

/// A source must not hide a real payment by supplying a smaller junk witness for the same txid.
#[test]
fn junk_witness_cannot_exclude_a_valid_transaction_or_its_descendants() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let (valid, [bogus, _]) = valid_and_bogus_copies(&c);
    let txid = valid.compute_txid();
    let child = spend(
        &[output_coin(&valid, 0, Kind::P2wpkh, 2)],
        vec![pay(Kind::P2wpkh, 3, 8_000)],
    );
    for parent_seen in [false, true] {
        let mut graph = graph_with(&[&c.parent, &valid, &child]);
        let _ = graph.insert_seen_at(child.compute_txid(), 1);
        if parent_seen {
            let _ = graph.insert_seen_at(txid, 2);
        }
        let before = view(&chain, &graph, Default::default());
        assert_eq!(*before.tx(txid).unwrap().tx, valid);
        assert!(before.tx(child.compute_txid()).is_some());

        let changeset = graph.insert_tx(bogus.clone());
        assert!(changeset.txs.contains(&bogus));
        assert_eq!(*graph.get_tx(txid).unwrap(), bogus);
        let after = view(&chain, &graph, Default::default());
        assert_eq!(*after.tx(txid).unwrap().tx, valid);
        assert_eq!(*after.tx(child.compute_txid()).unwrap().tx, child);
    }
}

#[test]
fn all_invalid_copies_are_excluded_unless_anchored_or_assumed() {
    let chain = chain();
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let (valid, bogus) = valid_and_bogus_copies(&c);
    let txid = valid.compute_txid();
    let child = spend(
        &[output_coin(&valid, 0, Kind::P2wpkh, 2)],
        vec![pay(Kind::P2wpkh, 3, 8_000)],
    );
    let child_txid = child.compute_txid();
    let mut graph = graph_with(&[&c.parent, &bogus[0], &bogus[1], &child]);
    let _ = graph.insert_seen_at(txid, 1);
    let _ = graph.insert_seen_at(child_txid, 2);
    let canonical = view(&chain, &graph, Default::default());
    assert!(canonical.tx(txid).is_none());
    assert!(canonical.tx(child_txid).is_none());

    // Bypassed ancestry must keep the graph's exact preferred copy, even with a valid alternative.
    let _ = graph.insert_tx(valid.clone());
    let preferred = graph.get_tx(txid).unwrap();
    assert_ne!(*preferred, valid);
    let assumed = view(
        &chain,
        &graph,
        CanonicalParams {
            assume_canonical: vec![child_txid],
        },
    );
    assert_eq!(assumed.tx(txid).unwrap().tx, preferred);
    let _ = graph.insert_anchor(child_txid, anchor(&chain, 1));
    let anchored = view(&chain, &graph, Default::default());
    assert_eq!(anchored.tx(txid).unwrap().tx, preferred);
}

#[test]
fn unknown_copy_survives_an_invalid_alternative_and_is_rechecked_with_parents() {
    let coins = [coin(Kind::P2wpkh, 1, 10_000), coin(Kind::P2wpkh, 3, 10_000)];
    let signed = spend(&coins, vec![pay(Kind::P2wpkh, 5, 19_000)]);
    let txid = signed.compute_txid();
    let mut bogus = signed.clone();
    bogus.input[0].witness = Witness::from_slice(&[vec![0; 9], pubkey(1).to_bytes().to_vec()]);
    let mut graph = graph_with(&[&coins[0].parent, &signed, &bogus]);
    let _ = graph.insert_seen_at(txid, 1);
    assert_eq!(validity(&graph, &signed), TxValidity::Unknown);
    assert_eq!(validity(&graph, &bogus), TxValidity::Invalid);
    assert_eq!(
        *view(&chain(), &graph, Default::default())
            .tx(txid)
            .unwrap()
            .tx,
        signed
    );

    insert_coin(&mut graph, &coins[1]);
    assert_eq!(validity(&graph, &signed), TxValidity::Valid);
    assert_eq!(
        *view(&chain(), &graph, Default::default())
            .tx(txid)
            .unwrap()
            .tx,
        signed
    );
}

#[test]
fn eligible_copies_use_deterministic_witness_order() {
    let script = Builder::new()
        .push_opcode(opcodes::all::OP_DROP)
        .push_int(1)
        .into_script();
    let mut c = coin(Kind::P2wpkh, 1, 10_000);
    c.parent.output[0].script_pubkey = script.to_p2wsh();
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(c.parent.compute_txid(), 0),
            ..Default::default()
        }],
        output: vec![pay(Kind::P2wpkh, 2, 9_000)],
    };
    let copies = [vec![0; 2], vec![1], vec![2]].map(|data| {
        let mut copy = tx.clone();
        copy.input[0].witness = Witness::from_slice(&[data, script.as_bytes().to_vec()]);
        copy
    });
    let expected = copies[1..]
        .iter()
        .min_by_key(|tx| tx.compute_wtxid())
        .unwrap();
    for with_parent in [false, true] {
        for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
            let mut graph = Graph::default();
            if with_parent {
                let _ = graph.insert_tx(c.parent.clone());
            }
            for i in order {
                let _ = graph.insert_tx(copies[i].clone());
                assert_eq!(
                    validity(&graph, &copies[i]),
                    if with_parent {
                        TxValidity::Valid
                    } else {
                        TxValidity::Unknown
                    }
                );
            }
            let _ = graph.insert_seen_at(tx.compute_txid(), 1);
            let canonical = view(&chain(), &graph, Default::default());
            assert_eq!(*canonical.tx(tx.compute_txid()).unwrap().tx, *expected);
        }
    }
}

#[test]
fn every_arrival_order_resolves_to_the_same_copy() {
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let (valid, bogus) = valid_and_bogus_copies(&c);
    let txid = valid.compute_txid();
    let items = [&c.parent, &valid, &bogus[0], &bogus[1]];
    let mut expected = None;
    for a in 0..4 {
        for b in (0..4).filter(|&b| b != a) {
            for c_ in (0..4).filter(|&c_| c_ != a && c_ != b) {
                let d = 6 - a - b - c_;
                let mut graph = graph_with(&[items[a], items[b], items[c_], items[d]]);
                let _ = graph.insert_seen_at(txid, 1);
                assert_eq!(graph.tx_copies(txid).count(), 3);
                assert_eq!(
                    *view(&chain(), &graph, Default::default())
                        .tx(txid)
                        .unwrap()
                        .tx,
                    valid
                );
                assert_eq!(*expected.get_or_insert_with(|| graph.clone()), graph);
                let mut reloaded = Graph::default();
                reloaded.apply_changeset(graph.initial_changeset());
                assert_eq!(
                    *view(&chain(), &reloaded, Default::default())
                        .tx(txid)
                        .unwrap()
                        .tx,
                    valid
                );
            }
        }
    }
}

#[test]
fn copies_are_never_spliced() {
    let coins = [coin(Kind::P2wpkh, 1, 10_000), coin(Kind::P2wpkh, 3, 10_000)];
    let signed = spend(&coins, vec![pay(Kind::P2wpkh, 5, 19_000)]);
    let txid = signed.compute_txid();
    let partial = |vin: usize| {
        let mut tx = signed.clone();
        tx.input[vin].witness = Witness::new();
        tx
    };
    let (first_signed, second_signed) = (partial(1), partial(0));
    let received = [first_signed.clone(), second_signed.clone()];

    let mut graph = Graph::default();
    coins.iter().for_each(|c| insert_coin(&mut graph, c));
    let _ = graph.insert_tx(first_signed.clone());
    let _ = graph.insert_tx(second_signed.clone());
    let chosen = graph.get_tx(txid).unwrap();
    assert!(received.contains(&chosen));
    assert_eq!(validity(&graph, &chosen), TxValidity::Invalid);
    let copies = graph.tx_copies(txid).collect::<Vec<_>>();
    assert_eq!(copies.len(), 2);
    assert!(copies.iter().all(|copy| received.contains(copy)));

    let _ = graph.insert_tx(signed.clone());
    assert_eq!(*graph.get_tx(txid).unwrap(), signed);
    assert_eq!(graph.tx_copies(txid).count(), 3);
}

#[test]
fn tx_update_has_one_copy_per_txid() {
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let (valid, bogus) = valid_and_bogus_copies(&c);
    let txid = valid.compute_txid();
    let graph = graph_with(&[&bogus[0], &valid, &bogus[1], &c.parent]);
    assert_eq!(graph.tx_copies(txid).count(), 3);
    let chosen = graph.get_tx(txid).unwrap();
    let update = bdk_chain::TxUpdate::from(graph);
    let txs = update
        .txs
        .iter()
        .filter(|tx| tx.compute_txid() == txid)
        .collect::<Vec<_>>();
    assert_eq!(txs, vec![&chosen]);
}

#[cfg(feature = "rusqlite")]
#[test]
fn every_copy_survives_sqlite() {
    type ChangeSet = bdk_chain::tx_graph::ChangeSet<ConfirmationBlockTime>;
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let (valid, [bogus, _]) = valid_and_bogus_copies(&c);

    let mut graph = Graph::default();
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    let db = conn.transaction().unwrap();
    ChangeSet::init_sqlite_tables(&db).unwrap();
    for tx in [c.parent.clone(), valid, bogus] {
        graph.insert_tx(tx).persist_to_sqlite(&db).unwrap();
    }

    let mut reloaded = Graph::default();
    reloaded.apply_changeset(ChangeSet::from_sqlite(&db).unwrap());
    assert_eq!(reloaded, graph);

    db.commit().unwrap();

    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    let fresh = conn.transaction().unwrap();
    ChangeSet::init_sqlite_tables(&fresh).unwrap();
    fresh
        .execute_batch("DELETE FROM bdk_tx_copies; DELETE FROM bdk_txs;")
        .unwrap();
    graph.initial_changeset().persist_to_sqlite(&fresh).unwrap();
    let mut reloaded = Graph::default();
    reloaded.apply_changeset(ChangeSet::from_sqlite(&fresh).unwrap());
    assert_eq!(reloaded, graph);
}

#[cfg(feature = "rusqlite")]
#[test]
fn copies_staged_together_survive_a_reload() {
    use bdk_chain::Merge;
    type ChangeSet = bdk_chain::tx_graph::ChangeSet<ConfirmationBlockTime>;
    let c = coin(Kind::P2wpkh, 1, 10_000);
    let (valid, bogus_copies) = valid_and_bogus_copies(&c);
    for bogus in bogus_copies {
        let mut graph = Graph::default();
        let mut staged = graph.insert_tx(bogus.clone());
        staged.merge(graph.insert_tx(c.parent.clone()));
        staged.merge(graph.insert_tx(valid.clone()));
        staged.merge(graph.insert_seen_at(valid.compute_txid(), 1));

        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let db = conn.transaction().unwrap();
        ChangeSet::init_sqlite_tables(&db).unwrap();
        staged.persist_to_sqlite(&db).unwrap();

        let mut reloaded = Graph::default();
        reloaded.apply_changeset(ChangeSet::from_sqlite(&db).unwrap());
        assert_eq!(reloaded, graph);
        assert_eq!(
            *view(&chain(), &reloaded, Default::default())
                .tx(valid.compute_txid())
                .unwrap()
                .tx,
            valid
        );

        let mut parent_last = Graph::default();
        let mut changeset = reloaded.initial_changeset();
        changeset
            .txs
            .retain(|tx| tx.compute_txid() != c.parent.compute_txid());
        parent_last.apply_changeset(changeset);
        let _ = parent_last.insert_tx(c.parent.clone());
        assert_eq!(
            *view(&chain(), &parent_last, Default::default())
                .tx(valid.compute_txid())
                .unwrap()
                .tx,
            valid
        );
    }
}
