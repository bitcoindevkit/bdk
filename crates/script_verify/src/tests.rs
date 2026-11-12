use super::*;
use crate::sighash::work::{self, Counts};
use bitcoin::hashes::Hash;
use bitcoin::key::{Keypair, TapTweak};
use bitcoin::script::Builder;
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{EcdsaSighashType, Prevouts, SighashCache, TapSighashType};
use bitcoin::{
    absolute, transaction, Amount, CompressedPublicKey, OutPoint, ScriptBuf, TxIn, Txid, Witness,
};

#[derive(Clone, Copy)]
enum Kind {
    Segwit,
    Taproot,
    TaprootAnyoneCanPay,
    Legacy,
}

fn signed(kinds: &[Kind]) -> (Transaction, Vec<TxOut>) {
    let secp = Secp256k1::new();
    let secret = SecretKey::from_slice(&[1; 32]).unwrap();
    let keypair = Keypair::from_secret_key(&secp, &secret);
    let pubkey = CompressedPublicKey(secret.public_key(&secp));
    let prevouts = kinds
        .iter()
        .map(|kind| TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: match kind {
                Kind::Segwit => ScriptBuf::new_p2wpkh(&pubkey.wpubkey_hash()),
                Kind::Taproot | Kind::TaprootAnyoneCanPay => {
                    ScriptBuf::new_p2tr(&secp, keypair.x_only_public_key().0, None)
                }
                Kind::Legacy => {
                    ScriptBuf::new_p2pkh(&bitcoin::PublicKey::from(pubkey).pubkey_hash())
                }
            },
        })
        .collect::<Vec<_>>();
    let mut tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: (0..kinds.len())
            .map(|i| TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([2; 32]), i as u32),
                ..TxIn::default()
            })
            .collect(),
        output: vec![TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    for (vin, kind) in kinds.iter().enumerate() {
        match kind {
            Kind::Segwit | Kind::Legacy => {
                let mut cache = SighashCache::new(&tx);
                let hash = match kind {
                    Kind::Segwit => cache
                        .p2wpkh_signature_hash(
                            vin,
                            &prevouts[vin].script_pubkey,
                            prevouts[vin].value,
                            EcdsaSighashType::All,
                        )
                        .unwrap()
                        .to_byte_array(),
                    _ => cache
                        .legacy_signature_hash(
                            vin,
                            &prevouts[vin].script_pubkey,
                            EcdsaSighashType::All as u32,
                        )
                        .unwrap()
                        .to_byte_array(),
                };
                let sig = bitcoin::ecdsa::Signature::sighash_all(
                    secp.sign_ecdsa(&Message::from_digest(hash), &secret),
                );
                match kind {
                    Kind::Segwit => tx.input[vin].witness = Witness::p2wpkh(&sig, &pubkey.0),
                    _ => {
                        tx.input[vin].script_sig = Builder::new()
                            .push_slice(sig.serialize())
                            .push_key(&pubkey.into())
                            .into_script()
                    }
                }
            }
            Kind::Taproot | Kind::TaprootAnyoneCanPay => {
                let ty = if matches!(kind, Kind::Taproot) {
                    TapSighashType::Default
                } else {
                    TapSighashType::AllPlusAnyoneCanPay
                };
                let hash = SighashCache::new(&tx)
                    .taproot_key_spend_signature_hash(vin, &Prevouts::All(&prevouts), ty)
                    .unwrap();
                let tweaked = keypair.tap_tweak(&secp, None).to_keypair();
                let sig = bitcoin::taproot::Signature {
                    signature: secp.sign_schnorr_no_aux_rand(
                        &Message::from_digest(hash.to_byte_array()),
                        &tweaked,
                    ),
                    sighash_type: ty,
                };
                tx.input[vin].witness = Witness::from_slice(&[sig.to_vec()]);
            }
        }
    }
    (tx, prevouts)
}

fn check_parity(tx: &Transaction, prevouts: &[Option<&TxOut>]) -> Vec<Verification> {
    let shared = verify_transaction(tx, prevouts, Flags::CONSENSUS);
    let independent = (0..tx.input.len())
        .map(|vin| {
            verify_input(
                &mut VerificationCache::new(tx, prevouts),
                vin,
                Flags::CONSENSUS,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(shared, independent);
    let mut context = VerificationCache::new(tx, prevouts);
    for vin in (0..tx.input.len())
        .rev()
        .chain(0..tx.input.len())
        .chain((0..tx.input.len()).rev())
    {
        assert_eq!(
            verify_input(&mut context, vin, Flags::CONSENSUS),
            independent[vin]
        );
    }
    shared
}

#[test]
fn transaction_api_reuses_shared_work() {
    for kind in [Kind::Segwit, Kind::Taproot] {
        for n in [1, 8, 64] {
            let (tx, spent) = signed(&vec![kind; n]);
            let prevouts = spent.iter().map(Some).collect::<Vec<_>>();
            work::take();
            let shared = verify_transaction(&tx, &prevouts, Flags::CONSENSUS);
            assert!(shared.iter().all(|v| v.result.is_ok()));
            assert_eq!(
                work::take(),
                Counts {
                    contexts: 1,
                    bip143: usize::from(matches!(kind, Kind::Segwit)),
                    taproot_prevouts: usize::from(matches!(kind, Kind::Taproot)),
                }
            );
            for (vin, expected) in shared.iter().enumerate() {
                assert_eq!(
                    &verify_input(
                        &mut VerificationCache::new(&tx, &prevouts),
                        vin,
                        Flags::CONSENSUS
                    ),
                    expected
                );
            }
            assert_eq!(
                work::take(),
                Counts {
                    contexts: n,
                    bip143: if matches!(kind, Kind::Segwit) { n } else { 0 },
                    taproot_prevouts: if matches!(kind, Kind::Taproot) { n } else { 0 },
                }
            );
        }
    }
}

#[test]
fn dependencies_are_per_input_even_after_errors_or_missing_prevouts() {
    let (tx, spent) = signed(&[
        Kind::Taproot,
        Kind::TaprootAnyoneCanPay,
        Kind::Segwit,
        Kind::Legacy,
    ]);
    let prevouts = spent.iter().map(Some).collect::<Vec<_>>();
    let results = check_parity(&tx, &prevouts);
    assert!(results.iter().all(|v| v.result.is_ok()));
    assert_eq!(
        results.iter().map(|v| v.depends_on).collect::<Vec<_>>(),
        vec![
            DependsOn::AllPrevouts,
            DependsOn::OwnPrevout,
            DependsOn::OwnPrevout,
            DependsOn::OwnPrevout
        ]
    );

    let mut missing = prevouts.clone();
    missing[3] = None;
    let results = check_parity(&tx, &missing);
    assert_eq!(results[0].result, Err(Error::MissingPrevouts(vec![3])));
    assert_eq!(results[0].depends_on, DependsOn::AllPrevouts);
    assert!(results[1].result.is_ok());
    assert_eq!(results[1].depends_on, DependsOn::OwnPrevout);
    assert_eq!(results[3].result, Err(Error::MissingPrevouts(vec![3])));
    assert_eq!(results[3].depends_on, DependsOn::OwnPrevout);

    let mut wrong = spent.clone();
    wrong[0].value += Amount::from_sat(1);
    let wrong_refs = wrong.iter().map(Some).collect::<Vec<_>>();
    let results = check_parity(&tx, &wrong_refs);
    assert!(matches!(results[0].result, Err(Error::Script(_))));
    assert_eq!(results[0].depends_on, DependsOn::AllPrevouts);
    assert!(results[1..]
        .iter()
        .all(|v| v.result.is_ok() && v.depends_on == DependsOn::OwnPrevout));

    let mut junk = tx.clone();
    junk.input[1].witness = Witness::from_slice(&[vec![0; 9]]);
    let results = check_parity(&junk, &prevouts);
    assert!(matches!(results[1].result, Err(Error::Script(_))));
    assert_eq!(results[1].depends_on, DependsOn::OwnPrevout);
    assert_eq!(tx.compute_txid(), junk.compute_txid());
    assert_ne!(tx.compute_wtxid(), junk.compute_wtxid());
    assert!(check_parity(&tx, &prevouts)
        .iter()
        .all(|v| v.result.is_ok()));
}

#[test]
fn anyonecanpay_does_not_prepare_other_prevouts() {
    let (tx, spent) = signed(&[Kind::TaprootAnyoneCanPay, Kind::TaprootAnyoneCanPay]);
    work::take();
    let results = verify_transaction(&tx, &[Some(&spent[0]), None], Flags::CONSENSUS);
    assert!(results[0].result.is_ok());
    assert_eq!(results[0].depends_on, DependsOn::OwnPrevout);
    assert_eq!(
        work::take(),
        Counts {
            contexts: 1,
            ..Counts::default()
        }
    );
}
