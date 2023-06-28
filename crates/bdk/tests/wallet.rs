use assert_matches::assert_matches;
use bdk::descriptor::calc_checksum;
use bdk::signer::{SignOptions, SignerError};
use bdk::wallet::coin_selection::LargestFirstCoinSelection;
use bdk::wallet::AddressIndex::*;
use bdk::wallet::{AddressIndex, AddressInfo, Balance, Wallet};
use bdk::Error;
use bdk::FeeRate;
use bdk::KeychainKind;
use bdk_chain::BlockId;
use bdk_chain::ConfirmationTime;
use bdk_chain::COINBASE_MATURITY;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use bitcoin::Script;
use bitcoin::{util::psbt, Network};
use bitcoin::{
    Address, EcdsaSighashType, LockTime, OutPoint, PackedLockTime, SchnorrSighashType, Sequence,
    Transaction, TxIn, TxOut,
};
use core::str::FromStr;

mod common;
use common::*;

fn receive_output(wallet: &mut Wallet, value: u64, height: ConfirmationTime) -> OutPoint {
    let tx = Transaction {
        version: 1,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet.get_address(LastUnused).script_pubkey(),
            value,
        }],
    };

    wallet.insert_tx(tx.clone(), height).unwrap();

    OutPoint {
        txid: tx.txid(),
        vout: 0,
    }
}

fn receive_output_in_latest_block(wallet: &mut Wallet, value: u64) -> OutPoint {
    let height = match wallet.latest_checkpoint() {
        Some(cp) => ConfirmationTime::Confirmed {
            height: cp.height(),
            time: 0,
        },
        None => ConfirmationTime::Unconfirmed { last_seen: 0 },
    };
    receive_output(wallet, value, height)
}

// The satisfaction size of a P2WPKH is 112 WU =
// 1 (elements in witness) + 1 (OP_PUSH) + 33 (pk) + 1 (OP_PUSH) + 72 (signature + sighash) + 1*4 (script len)
// On the witness itself, we have to push once for the pk (33WU) and once for signature + sighash (72WU), for
// a total of 105 WU.
// Here, we push just once for simplicity, so we have to add an extra byte for the missing
// OP_PUSH.
const P2WPKH_FAKE_WITNESS_SIZE: usize = 106;

#[test]
fn test_descriptor_checksum() {
    let (wallet, _) = get_funded_wallet(get_test_wpkh());
    let checksum = wallet.descriptor_checksum(KeychainKind::External);
    assert_eq!(checksum.len(), 8);

    let raw_descriptor = wallet
        .keychains()
        .iter()
        .next()
        .unwrap()
        .1
        .to_string()
        .split_once('#')
        .unwrap()
        .0
        .to_string();
    assert_eq!(calc_checksum(&raw_descriptor).unwrap(), checksum);
}

#[test]
fn test_get_funded_wallet_balance() {
    let (wallet, _) = get_funded_wallet(get_test_wpkh());
    assert_eq!(wallet.get_balance().confirmed, 50000);
}

macro_rules! assert_fee_rate {
    ($psbt:expr, $fees:expr, $fee_rate:expr $( ,@dust_change $( $dust_change:expr )* )* $( ,@add_signature $( $add_signature:expr )* )* ) => ({
        let psbt = $psbt.clone();
        #[allow(unused_mut)]
        let mut tx = $psbt.clone().extract_tx();
        $(
            $( $add_signature )*
                for txin in &mut tx.input {
                    txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
                }
        )*

            #[allow(unused_mut)]
        #[allow(unused_assignments)]
        let mut dust_change = false;
        $(
            $( $dust_change )*
                dust_change = true;
        )*

            let fee_amount = psbt
            .inputs
            .iter()
            .fold(0, |acc, i| acc + i.witness_utxo.as_ref().unwrap().value)
            - psbt
            .unsigned_tx
            .output
            .iter()
            .fold(0, |acc, o| acc + o.value);

        assert_eq!(fee_amount, $fees);

        let tx_fee_rate = FeeRate::from_wu($fees, tx.weight());
        let fee_rate = $fee_rate;

        if !dust_change {
            assert!(tx_fee_rate >= fee_rate && (tx_fee_rate - fee_rate).as_sat_per_vb().abs() < 0.5, "Expected fee rate of {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
        } else {
            assert!(tx_fee_rate >= fee_rate, "Expected fee rate of at least {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
        }
    });
}

macro_rules! from_str {
    ($e:expr, $t:ty) => {{
        use core::str::FromStr;
        <$t>::from_str($e).unwrap()
    }};

    ($e:expr) => {
        from_str!($e, _)
    };
}

#[test]
#[should_panic(expected = "NoRecipients")]
fn test_create_tx_empty_recipients() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    wallet.build_tx().finish().unwrap();
}

#[test]
#[should_panic(expected = "NoUtxosSelected")]
fn test_create_tx_manually_selected_empty_utxos() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .manually_selected_only();
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "Invalid version `0`")]
fn test_create_tx_version_0() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .version(0);
    builder.finish().unwrap();
}

#[test]
#[should_panic(
    expected = "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
)]
fn test_create_tx_version_1_csv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_csv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .version(1);
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_custom_version() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .version(42);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.version, 42);
}

#[test]
fn test_create_tx_default_locktime_is_last_sync_height() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());

    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    // Since we never synced the wallet we don't have a last_sync_height
    // we could use to try to prevent fee sniping. We default to 0.
    assert_eq!(psbt.unsigned_tx.lock_time.0, 1_000);
}

#[test]
fn test_create_tx_fee_sniping_locktime_last_sync() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);

    let (psbt, _) = builder.finish().unwrap();

    // If there's no current_height we're left with using the last sync height
    assert_eq!(
        psbt.unsigned_tx.lock_time.0,
        wallet.latest_checkpoint().unwrap().height()
    );
}

#[test]
fn test_create_tx_default_locktime_cltv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_cltv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.lock_time.0, 100_000);
}

#[test]
fn test_create_tx_custom_locktime() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .current_height(630_001)
        .nlocktime(LockTime::from_height(630_000).unwrap());
    let (psbt, _) = builder.finish().unwrap();

    // When we explicitly specify a nlocktime
    // we don't try any fee sniping prevention trick
    // (we ignore the current_height)
    assert_eq!(psbt.unsigned_tx.lock_time.0, 630_000);
}

#[test]
fn test_create_tx_custom_locktime_compatible_with_cltv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_cltv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .nlocktime(LockTime::from_height(630_000).unwrap());
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.lock_time.0, 630_000);
}

#[test]
#[should_panic(
    expected = "TxBuilder requested timelock of `Blocks(Height(50000))`, but at least `Blocks(Height(100000))` is required to spend from this script"
)]
fn test_create_tx_custom_locktime_incompatible_with_cltv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_cltv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .nlocktime(LockTime::from_height(50000).unwrap());
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_no_rbf_csv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_csv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(6));
}

#[test]
fn test_create_tx_with_default_rbf_csv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_csv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf();
    let (psbt, _) = builder.finish().unwrap();
    // When CSV is enabled it takes precedence over the rbf value (unless forced by the user).
    // It will be set to the OP_CSV value, in this case 6
    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(6));
}

#[test]
#[should_panic(
    expected = "Cannot enable RBF with nSequence `Sequence(3)` given a required OP_CSV of `Sequence(6)`"
)]
fn test_create_tx_with_custom_rbf_csv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_csv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf_with_sequence(Sequence(3));
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_no_rbf_cltv() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_cltv());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFE));
}

#[test]
#[should_panic(expected = "Cannot enable RBF with a nSequence >= 0xFFFFFFFE")]
fn test_create_tx_invalid_rbf_sequence() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf_with_sequence(Sequence(0xFFFFFFFE));
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_custom_rbf_sequence() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf_with_sequence(Sequence(0xDEADBEEF));
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xDEADBEEF));
}

#[test]
fn test_create_tx_default_sequence() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFE));
}

#[test]
#[should_panic(
    expected = "The `change_policy` can be set only if the wallet has a change_descriptor"
)]
fn test_create_tx_change_policy_no_internal() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .do_not_spend_change();
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_drain_wallet_and_drain_to() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        50_000 - details.fee.unwrap_or(0)
    );
}

#[test]
fn test_create_tx_drain_wallet_and_drain_to_and_with_recipient() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    let drain_addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 20_000)
        .drain_to(drain_addr.script_pubkey())
        .drain_wallet();
    let (psbt, details) = builder.finish().unwrap();
    let outputs = psbt.unsigned_tx.output;

    assert_eq!(outputs.len(), 2);
    let main_output = outputs
        .iter()
        .find(|x| x.script_pubkey == addr.script_pubkey())
        .unwrap();
    let drain_output = outputs
        .iter()
        .find(|x| x.script_pubkey == drain_addr.script_pubkey())
        .unwrap();
    assert_eq!(main_output.value, 20_000,);
    assert_eq!(drain_output.value, 30_000 - details.fee.unwrap_or(0));
}

#[test]
fn test_create_tx_drain_to_and_utxos() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let utxos: Vec<_> = wallet.list_unspent().map(|u| u.outpoint).collect();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxos(&utxos)
        .unwrap();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        50_000 - details.fee.unwrap_or(0)
    );
}

#[test]
#[should_panic(expected = "NoRecipients")]
fn test_create_tx_drain_to_no_drain_wallet_no_utxos() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let drain_addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(drain_addr.script_pubkey());
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_default_fee_rate() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, details) = builder.finish().unwrap();

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::default(), @add_signature);
}

#[test]
fn test_create_tx_custom_fee_rate() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .fee_rate(FeeRate::from_sat_per_vb(5.0));
    let (psbt, details) = builder.finish().unwrap();

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(5.0), @add_signature);
}

#[test]
fn test_create_tx_absolute_fee() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_absolute(100);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.fee.unwrap_or(0), 100);
    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        50_000 - details.fee.unwrap_or(0)
    );
}

#[test]
fn test_create_tx_absolute_zero_fee() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_absolute(0);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.fee.unwrap_or(0), 0);
    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        50_000 - details.fee.unwrap_or(0)
    );
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_create_tx_absolute_high_fee() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_absolute(60_000);
    let (_psbt, _details) = builder.finish().unwrap();
}

#[test]
fn test_create_tx_add_change() {
    use bdk::wallet::tx_builder::TxOrdering;

    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .ordering(TxOrdering::Untouched);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.output.len(), 2);
    assert_eq!(psbt.unsigned_tx.output[0].value, 25_000);
    assert_eq!(
        psbt.unsigned_tx.output[1].value,
        25_000 - details.fee.unwrap_or(0)
    );
}

#[test]
fn test_create_tx_skip_change_dust() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 49_800);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(psbt.unsigned_tx.output[0].value, 49_800);
    assert_eq!(details.fee.unwrap_or(0), 200);
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_create_tx_drain_to_dust_amount() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    // very high fee rate, so that the only output would be below dust
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_rate(FeeRate::from_sat_per_vb(453.0));
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_ordering_respected() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .add_recipient(addr.script_pubkey(), 10_000)
        .ordering(bdk::wallet::tx_builder::TxOrdering::Bip69Lexicographic);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.output.len(), 3);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        10_000 - details.fee.unwrap_or(0)
    );
    assert_eq!(psbt.unsigned_tx.output[1].value, 10_000);
    assert_eq!(psbt.unsigned_tx.output[2].value, 30_000);
}

#[test]
fn test_create_tx_default_sighash() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 30_000);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.inputs[0].sighash_type, None);
}

#[test]
fn test_create_tx_custom_sighash() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .sighash(bitcoin::EcdsaSighashType::Single.into());
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0].sighash_type,
        Some(bitcoin::EcdsaSighashType::Single.into())
    );
}

#[test]
fn test_create_tx_input_hd_keypaths() {
    use bitcoin::util::bip32::{DerivationPath, Fingerprint};
    use core::str::FromStr;

    let (mut wallet, _) = get_funded_wallet("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.inputs[0].bip32_derivation.len(), 1);
    assert_eq!(
        psbt.inputs[0].bip32_derivation.values().next().unwrap(),
        &(
            Fingerprint::from_str("d34db33f").unwrap(),
            DerivationPath::from_str("m/44'/0'/0'/0/0").unwrap()
        )
    );
}

#[test]
fn test_create_tx_output_hd_keypaths() {
    use bitcoin::util::bip32::{DerivationPath, Fingerprint};
    use core::str::FromStr;

    let (mut wallet, _) = get_funded_wallet("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");

    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.outputs[0].bip32_derivation.len(), 1);
    let expected_derivation_path = format!("m/44'/0'/0'/0/{}", addr.index);
    assert_eq!(
        psbt.outputs[0].bip32_derivation.values().next().unwrap(),
        &(
            Fingerprint::from_str("d34db33f").unwrap(),
            DerivationPath::from_str(&expected_derivation_path).unwrap()
        )
    );
}

#[test]
fn test_create_tx_set_redeem_script_p2sh() {
    use bitcoin::hashes::hex::FromHex;

    let (mut wallet, _) =
        get_funded_wallet("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0].redeem_script,
        Some(Script::from(
            Vec::<u8>::from_hex(
                "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
            )
            .unwrap()
        ))
    );
    assert_eq!(psbt.inputs[0].witness_script, None);
}

#[test]
fn test_create_tx_set_witness_script_p2wsh() {
    use bitcoin::hashes::hex::FromHex;

    let (mut wallet, _) =
        get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.inputs[0].redeem_script, None);
    assert_eq!(
        psbt.inputs[0].witness_script,
        Some(Script::from(
            Vec::<u8>::from_hex(
                "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
            )
            .unwrap()
        ))
    );
}

#[test]
fn test_create_tx_set_redeem_witness_script_p2wsh_p2sh() {
    use bitcoin::hashes::hex::FromHex;

    let (mut wallet, _) =
        get_funded_wallet("sh(wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    let script = Script::from(
        Vec::<u8>::from_hex(
            "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac",
        )
        .unwrap(),
    );

    assert_eq!(psbt.inputs[0].redeem_script, Some(script.to_v0_p2wsh()));
    assert_eq!(psbt.inputs[0].witness_script, Some(script));
}

#[test]
fn test_create_tx_non_witness_utxo() {
    let (mut wallet, _) =
        get_funded_wallet("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert!(psbt.inputs[0].non_witness_utxo.is_some());
    assert!(psbt.inputs[0].witness_utxo.is_none());
}

#[test]
fn test_create_tx_only_witness_utxo() {
    let (mut wallet, _) =
        get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .only_witness_utxo()
        .drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert!(psbt.inputs[0].non_witness_utxo.is_none());
    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn test_create_tx_shwpkh_has_witness_utxo() {
    let (mut wallet, _) =
        get_funded_wallet("sh(wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn test_create_tx_both_non_witness_utxo_and_witness_utxo_default() {
    let (mut wallet, _) =
        get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert!(psbt.inputs[0].non_witness_utxo.is_some());
    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn test_create_tx_add_utxo() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let small_output_tx = Transaction {
        input: vec![],
        output: vec![TxOut {
            value: 25_000,
            script_pubkey: wallet.get_address(New).address.script_pubkey(),
        }],
        version: 0,
        lock_time: PackedLockTime(0),
    };
    wallet
        .insert_tx(
            small_output_tx.clone(),
            ConfirmationTime::Unconfirmed { last_seen: 0 },
        )
        .unwrap();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .add_utxo(OutPoint {
            txid: small_output_tx.txid(),
            vout: 0,
        })
        .unwrap();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(
        psbt.unsigned_tx.input.len(),
        2,
        "should add an additional input since 25_000 < 30_000"
    );
    assert_eq!(details.sent, 75_000, "total should be sum of both inputs");
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_create_tx_manually_selected_insufficient() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let small_output_tx = Transaction {
        input: vec![],
        output: vec![TxOut {
            value: 25_000,
            script_pubkey: wallet.get_address(New).address.script_pubkey(),
        }],
        version: 0,
        lock_time: PackedLockTime(0),
    };

    wallet
        .insert_tx(
            small_output_tx.clone(),
            ConfirmationTime::Unconfirmed { last_seen: 0 },
        )
        .unwrap();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .add_utxo(OutPoint {
            txid: small_output_tx.txid(),
            vout: 0,
        })
        .unwrap()
        .manually_selected_only();
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "SpendingPolicyRequired(External)")]
fn test_create_tx_policy_path_required() {
    let (mut wallet, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 30_000);
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_policy_path_no_csv() {
    let descriptors = get_test_wpkh();
    let mut wallet = Wallet::new_no_persist(descriptors, None, Network::Regtest).unwrap();

    let tx = Transaction {
        version: 0,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 50_000,
            script_pubkey: wallet.get_address(New).script_pubkey(),
        }],
    };
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
    let root_id = external_policy.id;
    // child #0 is just the key "A"
    let path = vec![(root_id, vec![0])].into_iter().collect();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .policy_path(path, KeychainKind::External);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFF));
}

#[test]
fn test_create_tx_policy_path_use_csv() {
    let (mut wallet, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

    let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
    let root_id = external_policy.id;
    // child #1 is or(pk(B),older(144))
    let path = vec![(root_id, vec![1])].into_iter().collect();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .policy_path(path, KeychainKind::External);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(144));
}

#[test]
fn test_create_tx_policy_path_ignored_subtree_with_csv() {
    let (mut wallet, _) = get_funded_wallet("wsh(or_d(pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu),or_i(and_v(v:pkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(30)),and_v(v:pkh(cMnkdebixpXMPfkcNEjjGin7s94hiehAH4mLbYkZoh9KSiNNmqC8),older(90)))))");

    let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
    let root_id = external_policy.id;
    // child #0 is pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu)
    let path = vec![(root_id, vec![0])].into_iter().collect();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 30_000)
        .policy_path(path, KeychainKind::External);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFE));
}

#[test]
fn test_create_tx_global_xpubs_with_origin() {
    use bitcoin::hashes::hex::FromHex;
    use bitcoin::util::bip32;

    let (mut wallet, _) = get_funded_wallet("wpkh([73756c7f/48'/0'/0'/2']tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .add_global_xpubs();
    let (psbt, _) = builder.finish().unwrap();

    let key = bip32::ExtendedPubKey::from_str("tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3").unwrap();
    let fingerprint = bip32::Fingerprint::from_hex("73756c7f").unwrap();
    let path = bip32::DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();

    assert_eq!(psbt.xpub.len(), 1);
    assert_eq!(psbt.xpub.get(&key), Some(&(fingerprint, path)));
}

#[test]
fn test_add_foreign_utxo() {
    let (mut wallet1, _) = get_funded_wallet(get_test_wpkh());
    let (wallet2, _) =
        get_funded_wallet("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let utxo = wallet2.list_unspent().next().expect("must take!");
    let foreign_utxo_satisfaction = wallet2
        .get_descriptor_for_keychain(KeychainKind::External)
        .max_satisfaction_weight()
        .unwrap();

    let psbt_input = psbt::Input {
        witness_utxo: Some(utxo.txout.clone()),
        ..Default::default()
    };

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 60_000)
        .only_witness_utxo()
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let (mut psbt, details) = builder.finish().unwrap();

    assert_eq!(
        details.sent - details.received,
        10_000 + details.fee.unwrap_or(0),
        "we should have only net spent ~10_000"
    );

    assert!(
        psbt.unsigned_tx
            .input
            .iter()
            .any(|input| input.previous_output == utxo.outpoint),
        "foreign_utxo should be in there"
    );

    let finished = wallet1
        .sign(
            &mut psbt,
            SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        !finished,
        "only one of the inputs should have been signed so far"
    );

    let finished = wallet2
        .sign(
            &mut psbt,
            SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(finished, "all the inputs should have been signed now");
}

#[test]
#[should_panic(expected = "Generic(\"Foreign utxo missing witness_utxo or non_witness_utxo\")")]
fn test_add_foreign_utxo_invalid_psbt_input() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let outpoint = wallet.list_unspent().next().expect("must exist").outpoint;
    let foreign_utxo_satisfaction = wallet
        .get_descriptor_for_keychain(KeychainKind::External)
        .max_satisfaction_weight()
        .unwrap();

    let mut builder = wallet.build_tx();
    builder
        .add_foreign_utxo(outpoint, psbt::Input::default(), foreign_utxo_satisfaction)
        .unwrap();
}

#[test]
fn test_add_foreign_utxo_where_outpoint_doesnt_match_psbt_input() {
    let (mut wallet1, txid1) = get_funded_wallet(get_test_wpkh());
    let (wallet2, txid2) =
        get_funded_wallet("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let utxo2 = wallet2.list_unspent().next().unwrap();
    let tx1 = wallet1.get_tx(txid1, true).unwrap().transaction.unwrap();
    let tx2 = wallet2.get_tx(txid2, true).unwrap().transaction.unwrap();

    let satisfaction_weight = wallet2
        .get_descriptor_for_keychain(KeychainKind::External)
        .max_satisfaction_weight()
        .unwrap();

    let mut builder = wallet1.build_tx();
    assert!(
        builder
            .add_foreign_utxo(
                utxo2.outpoint,
                psbt::Input {
                    non_witness_utxo: Some(tx1),
                    ..Default::default()
                },
                satisfaction_weight
            )
            .is_err(),
        "should fail when outpoint doesn't match psbt_input"
    );
    assert!(
        builder
            .add_foreign_utxo(
                utxo2.outpoint,
                psbt::Input {
                    non_witness_utxo: Some(tx2),
                    ..Default::default()
                },
                satisfaction_weight
            )
            .is_ok(),
        "shoulld be ok when outpoint does match psbt_input"
    );
}

#[test]
fn test_add_foreign_utxo_only_witness_utxo() {
    let (mut wallet1, _) = get_funded_wallet(get_test_wpkh());
    let (wallet2, txid2) =
        get_funded_wallet("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let utxo2 = wallet2.list_unspent().next().unwrap();

    let satisfaction_weight = wallet2
        .get_descriptor_for_keychain(KeychainKind::External)
        .max_satisfaction_weight()
        .unwrap();

    let mut builder = wallet1.build_tx();
    builder.add_recipient(addr.script_pubkey(), 60_000);

    {
        let mut builder = builder.clone();
        let psbt_input = psbt::Input {
            witness_utxo: Some(utxo2.txout.clone()),
            ..Default::default()
        };
        builder
            .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
            .unwrap();
        assert!(
            builder.finish().is_err(),
            "psbt_input with witness_utxo should fail with only witness_utxo"
        );
    }

    {
        let mut builder = builder.clone();
        let psbt_input = psbt::Input {
            witness_utxo: Some(utxo2.txout.clone()),
            ..Default::default()
        };
        builder
            .only_witness_utxo()
            .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
            .unwrap();
        assert!(
            builder.finish().is_ok(),
            "psbt_input with just witness_utxo should succeed when `only_witness_utxo` is enabled"
        );
    }

    {
        let mut builder = builder.clone();
        let tx2 = wallet2.get_tx(txid2, true).unwrap().transaction.unwrap();
        let psbt_input = psbt::Input {
            non_witness_utxo: Some(tx2),
            ..Default::default()
        };
        builder
            .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
            .unwrap();
        assert!(
            builder.finish().is_ok(),
            "psbt_input with non_witness_utxo should succeed by default"
        );
    }
}

#[test]
fn test_get_psbt_input() {
    // this should grab a known good utxo and set the input
    let (wallet, _) = get_funded_wallet(get_test_wpkh());
    for utxo in wallet.list_unspent() {
        let psbt_input = wallet.get_psbt_input(utxo, None, false).unwrap();
        assert!(psbt_input.witness_utxo.is_some() || psbt_input.non_witness_utxo.is_some());
    }
}

#[test]
#[should_panic(
    expected = "MissingKeyOrigin(\"tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3\")"
)]
fn test_create_tx_global_xpubs_origin_missing() {
    let (mut wallet, _) = get_funded_wallet("wpkh(tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .add_global_xpubs();
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_global_xpubs_master_without_origin() {
    use bitcoin::hashes::hex::FromHex;
    use bitcoin::util::bip32;

    let (mut wallet, _) = get_funded_wallet("wpkh(tpubD6NzVbkrYhZ4Y55A58Gv9RSNF5hy84b5AJqYy7sCcjFrkcLpPre8kmgfit6kY1Zs3BLgeypTDBZJM222guPpdz7Cup5yzaMu62u7mYGbwFL/0/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .add_global_xpubs();
    let (psbt, _) = builder.finish().unwrap();

    let key = bip32::ExtendedPubKey::from_str("tpubD6NzVbkrYhZ4Y55A58Gv9RSNF5hy84b5AJqYy7sCcjFrkcLpPre8kmgfit6kY1Zs3BLgeypTDBZJM222guPpdz7Cup5yzaMu62u7mYGbwFL").unwrap();
    let fingerprint = bip32::Fingerprint::from_hex("997a323b").unwrap();

    assert_eq!(psbt.xpub.len(), 1);
    assert_eq!(
        psbt.xpub.get(&key),
        Some(&(fingerprint, bip32::DerivationPath::default()))
    );
}

#[test]
#[should_panic(expected = "IrreplaceableTransaction")]
fn test_bump_fee_irreplaceable_tx() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();
    wallet.build_fee_bump(txid).unwrap().finish().unwrap();
}

#[test]
#[should_panic(expected = "TransactionConfirmed")]
fn test_bump_fee_confirmed_tx() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    let tx = psbt.extract_tx();
    let txid = tx.txid();

    wallet
        .insert_tx(
            tx,
            ConfirmationTime::Confirmed {
                height: 42,
                time: 42_000,
            },
        )
        .unwrap();

    wallet.build_fee_bump(txid).unwrap().finish().unwrap();
}

#[test]
#[should_panic(expected = "FeeRateTooLow")]
fn test_bump_fee_low_fee_rate() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf();
    let (psbt, _) = builder.finish().unwrap();

    let tx = psbt.extract_tx();
    let txid = tx.txid();

    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb(1.0));
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "FeeTooLow")]
fn test_bump_fee_low_abs() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf();
    let (psbt, _) = builder.finish().unwrap();

    let tx = psbt.extract_tx();
    let txid = tx.txid();

    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(10);
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "FeeTooLow")]
fn test_bump_fee_zero_abs() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf();
    let (psbt, _) = builder.finish().unwrap();

    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(0);
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_reduce_change() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb(2.5)).enable_rbf();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent);
    assert_eq!(
        details.received + details.fee.unwrap_or(0),
        original_details.received + original_details.fee.unwrap_or(0)
    );
    assert!(details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        25_000
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        details.received
    );

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(2.5), @add_signature);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(200);
    builder.enable_rbf();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent);
    assert_eq!(
        details.received + details.fee.unwrap_or(0),
        original_details.received + original_details.fee.unwrap_or(0)
    );
    assert!(
        details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0),
        "{} > {}",
        details.fee.unwrap_or(0),
        original_details.fee.unwrap_or(0)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        25_000
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        details.received
    );

    assert_eq!(details.fee.unwrap_or(0), 200);
}

#[test]
fn test_bump_fee_reduce_single_recipient() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_rate(FeeRate::from_sat_per_vb(2.5))
        .allow_shrinking(addr.script_pubkey())
        .unwrap();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent);
    assert!(details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 1);
    assert_eq!(tx.output[0].value + details.fee.unwrap_or(0), details.sent);

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(2.5), @add_signature);
}

#[test]
fn test_bump_fee_absolute_reduce_single_recipient() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .allow_shrinking(addr.script_pubkey())
        .unwrap()
        .fee_absolute(300);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent);
    assert!(details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 1);
    assert_eq!(tx.output[0].value + details.fee.unwrap_or(0), details.sent);

    assert_eq!(details.fee.unwrap_or(0), 300);
}

#[test]
fn test_bump_fee_drain_wallet() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    // receive an extra tx so that our wallet has two utxos.
    let tx = Transaction {
        version: 1,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 25_000,
            script_pubkey: wallet.get_address(New).script_pubkey(),
        }],
    };
    wallet
        .insert_tx(
            tx.clone(),
            ConfirmationTime::Confirmed {
                height: wallet.latest_checkpoint().unwrap().height(),
                time: 42_000,
            },
        )
        .unwrap();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();

    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(OutPoint {
            txid: tx.txid(),
            vout: 0,
        })
        .unwrap()
        .manually_selected_only()
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();
    assert_eq!(original_details.sent, 25_000);

    // for the new feerate, it should be enough to reduce the output, but since we specify
    // `drain_wallet` we expect to spend everything
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .drain_wallet()
        .allow_shrinking(addr.script_pubkey())
        .unwrap()
        .fee_rate(FeeRate::from_sat_per_vb(5.0));
    let (_, details) = builder.finish().unwrap();

    assert_eq!(details.sent, 75_000);
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_bump_fee_remove_output_manually_selected_only() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    // receive an extra tx so that our wallet has two utxos. then we manually pick only one of
    // them, and make sure that `bump_fee` doesn't try to add more. This fails because we've
    // told the wallet it's not allowed to add more inputs AND it can't reduce the value of the
    // existing output. In other words, bump_fee + manually_selected_only is always an error
    // unless you've also set "allow_shrinking" OR there is a change output.
    let init_tx = Transaction {
        version: 1,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet.get_address(New).script_pubkey(),
            value: 25_000,
        }],
    };
    wallet
        .insert_tx(
            init_tx.clone(),
            wallet
                .transactions()
                .last()
                .unwrap()
                .observed_as
                .cloned()
                .into(),
        )
        .unwrap();
    let outpoint = OutPoint {
        txid: init_tx.txid(),
        vout: 0,
    };
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(outpoint)
        .unwrap()
        .manually_selected_only()
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();
    assert_eq!(original_details.sent, 25_000);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .manually_selected_only()
        .fee_rate(FeeRate::from_sat_per_vb(255.0));
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_add_input() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let init_tx = Transaction {
        version: 1,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet.get_address(New).script_pubkey(),
            value: 25_000,
        }],
    };
    let pos = wallet
        .transactions()
        .last()
        .unwrap()
        .observed_as
        .cloned()
        .into();
    wallet.insert_tx(init_tx, pos).unwrap();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb(50.0));
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent + 25_000);
    assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        45_000
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        details.received
    );

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(50.0), @add_signature);
}

#[test]
fn test_bump_fee_absolute_add_input() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    receive_output_in_latest_block(&mut wallet, 25_000);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(6_000);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent + 25_000);
    assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        45_000
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        details.received
    );

    assert_eq!(details.fee.unwrap_or(0), 6_000);
}

#[test]
fn test_bump_fee_no_change_add_input_and_change() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let op = receive_output_in_latest_block(&mut wallet, 25_000);

    // initially make a tx without change by using `drain_to`
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(op)
        .unwrap()
        .manually_selected_only()
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();

    let tx = psbt.extract_tx();
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    // now bump the fees without using `allow_shrinking`. the wallet should add an
    // extra input and a change output, and leave the original output untouched
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb(50.0));
    let (psbt, details) = builder.finish().unwrap();

    let original_send_all_amount = original_details.sent - original_details.fee.unwrap_or(0);
    assert_eq!(details.sent, original_details.sent + 50_000);
    assert_eq!(
        details.received,
        75_000 - original_send_all_amount - details.fee.unwrap_or(0)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        original_send_all_amount
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        75_000 - original_send_all_amount - details.fee.unwrap_or(0)
    );

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(50.0), @add_signature);
}

#[test]
fn test_bump_fee_add_input_change_dust() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    receive_output_in_latest_block(&mut wallet, 25_000);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let mut tx = psbt.extract_tx();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // to get realisitc weight
    }
    let original_tx_weight = tx.weight();
    assert_eq!(tx.input.len(), 1);
    assert_eq!(tx.output.len(), 2);
    let txid = tx.txid();
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    // We set a fee high enough that during rbf we are forced to add
    // a new input and also that we have to remove the change
    // that we had previously

    // We calculate the new weight as:
    //   original weight
    // + extra input weight: 160 WU = (32 (prevout) + 4 (vout) + 4 (nsequence)) * 4
    // + input satisfaction weight: 112 WU = 106 (witness) + 2 (witness len) + (1 (script len)) * 4
    // - change output weight: 124 WU = (8 (value) + 1 (script len) + 22 (script)) * 4
    let new_tx_weight = original_tx_weight + 160 + 112 - 124;
    // two inputs (50k, 25k) and one output (45k) - epsilon
    // We use epsilon here to avoid asking for a slightly too high feerate
    let fee_abs = 50_000 + 25_000 - 45_000 - 10;
    builder.fee_rate(FeeRate::from_wu(fee_abs, new_tx_weight));
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(
        original_details.received,
        5_000 - original_details.fee.unwrap_or(0)
    );

    assert_eq!(details.sent, original_details.sent + 25_000);
    assert_eq!(details.fee.unwrap_or(0), 30_000);
    assert_eq!(details.received, 0);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        45_000
    );

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(140.0), @dust_change, @add_signature);
}

#[test]
fn test_bump_fee_force_add_input() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let incoming_op = receive_output_in_latest_block(&mut wallet, 25_000);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let mut tx = psbt.extract_tx();
    let txid = tx.txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    wallet
        .insert_tx(tx.clone(), ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();
    // the new fee_rate is low enough that just reducing the change would be fine, but we force
    // the addition of an extra input with `add_utxo()`
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .add_utxo(incoming_op)
        .unwrap()
        .fee_rate(FeeRate::from_sat_per_vb(5.0));
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent + 25_000);
    assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        45_000
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        details.received
    );

    assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(5.0), @add_signature);
}

#[test]
fn test_bump_fee_absolute_force_add_input() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let incoming_op = receive_output_in_latest_block(&mut wallet, 25_000);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .enable_rbf();
    let (psbt, original_details) = builder.finish().unwrap();
    let mut tx = psbt.extract_tx();
    let txid = tx.txid();
    // skip saving the new utxos, we know they can't be used anyways
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    wallet
        .insert_tx(tx.clone(), ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    // the new fee_rate is low enough that just reducing the change would be fine, but we force
    // the addition of an extra input with `add_utxo()`
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.add_utxo(incoming_op).unwrap().fee_absolute(250);
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(details.sent, original_details.sent + 25_000);
    assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        45_000
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        details.received
    );

    assert_eq!(details.fee.unwrap_or(0), 250);
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_bump_fee_unconfirmed_inputs_only() {
    // We try to bump the fee, but:
    // - We can't reduce the change, as we have no change
    // - All our UTXOs are unconfirmed
    // So, we fail with "InsufficientFunds", as per RBF rule 2:
    // The replacement transaction may only include an unconfirmed input
    // if that input was included in one of the original transactions.
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .drain_wallet()
        .drain_to(addr.script_pubkey())
        .enable_rbf();
    let (psbt, __details) = builder.finish().unwrap();
    // Now we receive one transaction with 0 confirmations. We won't be able to use that for
    // fee bumping, as it's still unconfirmed!
    receive_output(
        &mut wallet,
        25_000,
        ConfirmationTime::Unconfirmed { last_seen: 0 },
    );
    let mut tx = psbt.extract_tx();
    let txid = tx.txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb(25.0));
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_unconfirmed_input() {
    // We create a tx draining the wallet and spending one confirmed
    // and one unconfirmed UTXO. We check that we can fee bump normally
    // (BIP125 rule 2 only apply to newly added unconfirmed input, you can
    // always fee bump with an unconfirmed input if it was included in the
    // original transaction)
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    // We receive a tx with 0 confirmations, which will be used as an input
    // in the drain tx.
    receive_output(&mut wallet, 25_000, ConfirmationTime::unconfirmed(0));
    let mut builder = wallet.build_tx();
    builder
        .drain_wallet()
        .drain_to(addr.script_pubkey())
        .enable_rbf();
    let (psbt, _) = builder.finish().unwrap();
    let mut tx = psbt.extract_tx();
    let txid = tx.txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    wallet
        .insert_tx(tx, ConfirmationTime::Unconfirmed { last_seen: 0 })
        .unwrap();

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_rate(FeeRate::from_sat_per_vb(15.0))
        .allow_shrinking(addr.script_pubkey())
        .unwrap();
    builder.finish().unwrap();
}

#[test]
fn test_fee_amount_negative_drain_val() {
    // While building the transaction, bdk would calculate the drain_value
    // as
    // current_delta - fee_amount - drain_fee
    // using saturating_sub, meaning that if the result would end up negative,
    // it'll remain to zero instead.
    // This caused a bug in master where we would calculate the wrong fee
    // for a transaction.
    // See https://github.com/bitcoindevkit/bdk/issues/660
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let send_to = Address::from_str("tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt").unwrap();
    let fee_rate = FeeRate::from_sat_per_vb(2.01);
    let incoming_op = receive_output_in_latest_block(&mut wallet, 8859);

    let mut builder = wallet.build_tx();
    builder
        .add_recipient(send_to.script_pubkey(), 8630)
        .add_utxo(incoming_op)
        .unwrap()
        .enable_rbf()
        .fee_rate(fee_rate);
    let (psbt, details) = builder.finish().unwrap();

    assert!(psbt.inputs.len() == 1);
    assert_fee_rate!(psbt, details.fee.unwrap_or(0), fee_rate, @add_signature);
}

#[test]
fn test_sign_single_xprv() {
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx();
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_with_master_fingerprint_and_path() {
    let (mut wallet, _) = get_funded_wallet("wpkh([d34db33f/84h/1h/0h]tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx();
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_bip44_path() {
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/44'/0'/0'/0/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx();
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_sh_wpkh() {
    let (mut wallet, _) = get_funded_wallet("sh(wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*))");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx();
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_wif() {
    let (mut wallet, _) =
        get_funded_wallet("wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx();
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_no_hd_keypaths() {
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    psbt.inputs[0].bip32_derivation.clear();
    assert_eq!(psbt.inputs[0].bip32_derivation.len(), 0);

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx();
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_include_output_redeem_witness_script() {
    let (mut wallet, _) = get_funded_wallet("sh(wsh(multi(1,cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW,cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu)))");
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .include_output_redeem_witness_script();
    let (psbt, _) = builder.finish().unwrap();

    // p2sh-p2wsh transaction should contain both witness and redeem scripts
    assert!(psbt
        .outputs
        .iter()
        .any(|output| output.redeem_script.is_some() && output.witness_script.is_some()));
}

#[test]
fn test_signing_only_one_of_multiple_inputs() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 45_000)
        .include_output_redeem_witness_script();
    let (mut psbt, _) = builder.finish().unwrap();

    // add another input to the psbt that is at least passable.
    let dud_input = bitcoin::util::psbt::Input {
        witness_utxo: Some(TxOut {
            value: 100_000,
            script_pubkey: miniscript::Descriptor::<bitcoin::PublicKey>::from_str(
                "wpkh(025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)",
            )
            .unwrap()
            .script_pubkey(),
        }),
        ..Default::default()
    };

    psbt.inputs.push(dud_input);
    psbt.unsigned_tx.input.push(bitcoin::TxIn::default());
    let is_final = wallet
        .sign(
            &mut psbt,
            SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        !is_final,
        "shouldn't be final since we can't sign one of the inputs"
    );
    assert!(
        psbt.inputs[0].final_script_witness.is_some(),
        "should finalized input it signed"
    )
}

#[test]
fn test_remove_partial_sigs_after_finalize_sign_option() {
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");

    for remove_partial_sigs in &[true, false] {
        let addr = wallet.get_address(New);
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let mut psbt = builder.finish().unwrap().0;

        assert!(wallet
            .sign(
                &mut psbt,
                SignOptions {
                    remove_partial_sigs: *remove_partial_sigs,
                    ..Default::default()
                },
            )
            .unwrap());

        psbt.inputs.iter().for_each(|input| {
            if *remove_partial_sigs {
                assert!(input.partial_sigs.is_empty())
            } else {
                assert!(!input.partial_sigs.is_empty())
            }
        });
    }
}

#[test]
fn test_try_finalize_sign_option() {
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");

    for try_finalize in &[true, false] {
        let addr = wallet.get_address(New);
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let mut psbt = builder.finish().unwrap().0;

        let finalized = wallet
            .sign(
                &mut psbt,
                SignOptions {
                    try_finalize: *try_finalize,
                    ..Default::default()
                },
            )
            .unwrap();

        psbt.inputs.iter().for_each(|input| {
            if *try_finalize {
                assert!(finalized);
                assert!(input.final_script_sig.is_some());
                assert!(input.final_script_witness.is_some());
            } else {
                assert!(!finalized);
                assert!(input.final_script_sig.is_none());
                assert!(input.final_script_witness.is_none());
            }
        });
    }
}

#[test]
fn test_sign_nonstandard_sighash() {
    let sighash = EcdsaSighashType::NonePlusAnyoneCanPay;

    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .sighash(sighash.into())
        .drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(
        result.is_err(),
        "Signing should have failed because the TX uses non-standard sighashes"
    );
    assert_matches!(
        result,
        Err(bdk::Error::Signer(SignerError::NonStandardSighash)),
        "Signing failed with the wrong error type"
    );

    // try again after opting-in
    let result = wallet.sign(
        &mut psbt,
        SignOptions {
            allow_all_sighashes: true,
            ..Default::default()
        },
    );
    assert!(result.is_ok(), "Signing should have worked");
    assert!(
        result.unwrap(),
        "Should finalize the input since we can produce signatures"
    );

    let extracted = psbt.extract_tx();
    assert_eq!(
        *extracted.input[0].witness.to_vec()[0].last().unwrap(),
        sighash.to_u32() as u8,
        "The signature should have been made with the right sighash"
    );
}

#[test]
fn test_unused_address() {
    let mut wallet = Wallet::new_no_persist("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                 None, Network::Testnet).unwrap();

    assert_eq!(
        wallet.get_address(LastUnused).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
    assert_eq!(
        wallet.get_address(LastUnused).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
}

#[test]
fn test_next_unused_address() {
    let descriptor = "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)";
    let mut wallet = Wallet::new_no_persist(descriptor, None, Network::Testnet).unwrap();
    assert_eq!(wallet.derivation_index(KeychainKind::External), None);

    assert_eq!(
        wallet.get_address(LastUnused).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(0));
    assert_eq!(
        wallet.get_address(LastUnused).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(0));

    // use the above address
    receive_output_in_latest_block(&mut wallet, 25_000);

    assert_eq!(
        wallet.get_address(LastUnused).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(1));
}

#[test]
fn test_peek_address_at_index() {
    let mut wallet = Wallet::new_no_persist("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                 None, Network::Testnet).unwrap();

    assert_eq!(
        wallet.get_address(Peek(1)).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );

    assert_eq!(
        wallet.get_address(Peek(0)).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );

    assert_eq!(
        wallet.get_address(Peek(2)).to_string(),
        "tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2"
    );

    // current new address is not affected
    assert_eq!(
        wallet.get_address(New).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );

    assert_eq!(
        wallet.get_address(New).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );
}

#[test]
fn test_peek_address_at_index_not_derivable() {
    let mut wallet = Wallet::new_no_persist("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/1)",
                                 None, Network::Testnet).unwrap();

    assert_eq!(
        wallet.get_address(Peek(1)).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );

    assert_eq!(
        wallet.get_address(Peek(0)).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );

    assert_eq!(
        wallet.get_address(Peek(2)).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );
}

#[test]
fn test_returns_index_and_address() {
    let mut wallet = Wallet::new_no_persist("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                 None, Network::Testnet).unwrap();

    // new index 0
    assert_eq!(
        wallet.get_address(New),
        AddressInfo {
            index: 0,
            address: Address::from_str("tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a").unwrap(),
            keychain: KeychainKind::External,
        }
    );

    // new index 1
    assert_eq!(
        wallet.get_address(New),
        AddressInfo {
            index: 1,
            address: Address::from_str("tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7").unwrap(),
            keychain: KeychainKind::External,
        }
    );

    // peek index 25
    assert_eq!(
        wallet.get_address(Peek(25)),
        AddressInfo {
            index: 25,
            address: Address::from_str("tb1qsp7qu0knx3sl6536dzs0703u2w2ag6ppl9d0c2").unwrap(),
            keychain: KeychainKind::External,
        }
    );

    // new index 2
    assert_eq!(
        wallet.get_address(New),
        AddressInfo {
            index: 2,
            address: Address::from_str("tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2").unwrap(),
            keychain: KeychainKind::External,
        }
    );
}

#[test]
fn test_sending_to_bip350_bech32m_address() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let addr = Address::from_str("tb1pqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesf3hn0c")
        .unwrap();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 45_000);
    builder.finish().unwrap();
}

#[test]
fn test_get_address() {
    use bdk::descriptor::template::Bip84;
    let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
    let mut wallet = Wallet::new_no_persist(
        Bip84(key, KeychainKind::External),
        Some(Bip84(key, KeychainKind::Internal)),
        Network::Regtest,
    )
    .unwrap();

    assert_eq!(
        wallet.get_address(AddressIndex::New),
        AddressInfo {
            index: 0,
            address: Address::from_str("bcrt1qrhgaqu0zvf5q2d0gwwz04w0dh0cuehhqvzpp4w").unwrap(),
            keychain: KeychainKind::External,
        }
    );

    assert_eq!(
        wallet.get_internal_address(AddressIndex::New),
        AddressInfo {
            index: 0,
            address: Address::from_str("bcrt1q0ue3s5y935tw7v3gmnh36c5zzsaw4n9c9smq79").unwrap(),
            keychain: KeychainKind::Internal,
        }
    );

    let mut wallet =
        Wallet::new_no_persist(Bip84(key, KeychainKind::External), None, Network::Regtest).unwrap();

    assert_eq!(
        wallet.get_internal_address(AddressIndex::New),
        AddressInfo {
            index: 0,
            address: Address::from_str("bcrt1qrhgaqu0zvf5q2d0gwwz04w0dh0cuehhqvzpp4w").unwrap(),
            keychain: KeychainKind::External,
        },
        "when there's no internal descriptor it should just use external"
    );
}

#[test]
fn test_get_address_no_reuse_single_descriptor() {
    use bdk::descriptor::template::Bip84;
    use std::collections::HashSet;

    let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
    let mut wallet =
        Wallet::new_no_persist(Bip84(key, KeychainKind::External), None, Network::Regtest).unwrap();

    let mut used_set = HashSet::new();

    (0..3).for_each(|_| {
        let external_addr = wallet.get_address(AddressIndex::New).address;
        assert!(used_set.insert(external_addr));

        let internal_addr = wallet.get_internal_address(AddressIndex::New).address;
        assert!(used_set.insert(internal_addr));
    });
}

#[test]
fn test_taproot_psbt_populate_tap_key_origins() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig_xprv());
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0]
            .tap_key_origins
            .clone()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![(
            from_str!("b96d3a3dc76a4fc74e976511b23aecb78e0754c23c0ed7a6513e18cbbc7178e9"),
            (vec![], (from_str!("f6a5cb8b"), from_str!("m/0")))
        )],
        "Wrong input tap_key_origins"
    );
    assert_eq!(
        psbt.outputs[0]
            .tap_key_origins
            .clone()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![(
            from_str!("e9b03068cf4a2621d4f81e68f6c4216e6bd260fe6edf6acc55c8d8ae5aeff0a8"),
            (vec![], (from_str!("f6a5cb8b"), from_str!("m/1")))
        )],
        "Wrong output tap_key_origins"
    );
}

#[test]
fn test_taproot_psbt_populate_tap_key_origins_repeated_key() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_repeated_key());
    let addr = wallet.get_address(AddressIndex::New);

    let path = vec![("e5mmg3xh".to_string(), vec![0])]
        .into_iter()
        .collect();

    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 25_000)
        .policy_path(path, KeychainKind::External);
    let (psbt, _) = builder.finish().unwrap();

    let mut input_key_origins = psbt.inputs[0]
        .tap_key_origins
        .clone()
        .into_iter()
        .collect::<Vec<_>>();
    input_key_origins.sort();

    assert_eq!(
        input_key_origins,
        vec![
            (
                from_str!("b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55"),
                (
                    vec![],
                    (FromStr::from_str("871fd295").unwrap(), vec![].into())
                )
            ),
            (
                from_str!("2b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3"),
                (
                    vec![
                        from_str!(
                            "858ad7a7d7f270e2c490c4d6ba00c499e46b18fdd59ea3c2c47d20347110271e"
                        ),
                        from_str!(
                            "f6e927ad4492c051fe325894a4f5f14538333b55a35f099876be42009ec8f903"
                        ),
                    ],
                    (FromStr::from_str("ece52657").unwrap(), vec![].into())
                )
            )
        ],
        "Wrong input tap_key_origins"
    );

    let mut output_key_origins = psbt.outputs[0]
        .tap_key_origins
        .clone()
        .into_iter()
        .collect::<Vec<_>>();
    output_key_origins.sort();

    assert_eq!(
        input_key_origins, output_key_origins,
        "Wrong output tap_key_origins"
    );
}

#[test]
fn test_taproot_psbt_input_tap_tree() {
    use bdk::bitcoin::psbt::serialize::Deserialize;
    use bdk::bitcoin::psbt::TapTree;
    use bitcoin::hashes::hex::FromHex;
    use bitcoin::util::taproot;

    let (mut wallet, _) = get_funded_wallet(get_test_tr_with_taptree());
    let addr = wallet.get_address(AddressIndex::Peek(0));

    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (psbt, _) = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0].tap_merkle_root,
        Some(
            FromHex::from_hex("61f81509635053e52d9d1217545916167394490da2287aca4693606e43851986")
                .unwrap()
        ),
    );
    assert_eq!(
        psbt.inputs[0].tap_scripts.clone().into_iter().collect::<Vec<_>>(),
        vec![
            (taproot::ControlBlock::from_slice(&Vec::<u8>::from_hex("c0b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55b7ef769a745e625ed4b9a4982a4dc08274c59187e73e6f07171108f455081cb2").unwrap()).unwrap(), (from_str!("208aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642ac"), taproot::LeafVersion::TapScript)),
            (taproot::ControlBlock::from_slice(&Vec::<u8>::from_hex("c0b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55b9a515f7be31a70186e3c5937ee4a70cc4b4e1efe876c1d38e408222ffc64834").unwrap()).unwrap(), (from_str!("2051494dc22e24a32fe9dcfbd7e85faf345fa1df296fb49d156e859ef345201295ac"), taproot::LeafVersion::TapScript)),
        ],
    );
    assert_eq!(
        psbt.inputs[0].tap_internal_key,
        Some(from_str!(
            "b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55"
        ))
    );

    // Since we are creating an output to the same address as the input, assert that the
    // internal_key is the same
    assert_eq!(
        psbt.inputs[0].tap_internal_key,
        psbt.outputs[0].tap_internal_key
    );

    assert_eq!(
        psbt.outputs[0].tap_tree,
        Some(TapTree::deserialize(&Vec::<u8>::from_hex("01c022208aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642ac01c0222051494dc22e24a32fe9dcfbd7e85faf345fa1df296fb49d156e859ef345201295ac",).unwrap()).unwrap())
    );
}

#[test]
fn test_taproot_sign_missing_witness_utxo() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();
    let witness_utxo = psbt.inputs[0].witness_utxo.take();

    let result = wallet.sign(
        &mut psbt,
        SignOptions {
            allow_all_sighashes: true,
            ..Default::default()
        },
    );
    assert_matches!(
        result,
        Err(Error::Signer(SignerError::MissingWitnessUtxo)),
        "Signing should have failed with the correct error because the witness_utxo is missing"
    );

    // restore the witness_utxo
    psbt.inputs[0].witness_utxo = witness_utxo;

    let result = wallet.sign(
        &mut psbt,
        SignOptions {
            allow_all_sighashes: true,
            ..Default::default()
        },
    );

    assert_matches!(
        result,
        Ok(true),
        "Should finalize the input since we can produce signatures"
    );
}

#[test]
fn test_taproot_sign_using_non_witness_utxo() {
    let (mut wallet, prev_txid) = get_funded_wallet(get_test_tr_single_sig());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    psbt.inputs[0].witness_utxo = None;
    psbt.inputs[0].non_witness_utxo = wallet.get_tx(prev_txid, true).unwrap().transaction;
    assert!(
        psbt.inputs[0].non_witness_utxo.is_some(),
        "Previous tx should be present in the database"
    );

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(result.is_ok(), "Signing should have worked");
    assert!(
        result.unwrap(),
        "Should finalize the input since we can produce signatures"
    );
}

#[test]
fn test_taproot_foreign_utxo() {
    let (mut wallet1, _) = get_funded_wallet(get_test_wpkh());
    let (wallet2, _) = get_funded_wallet(get_test_tr_single_sig());

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let utxo = wallet2.list_unspent().next().unwrap();
    let psbt_input = wallet2.get_psbt_input(utxo.clone(), None, false).unwrap();
    let foreign_utxo_satisfaction = wallet2
        .get_descriptor_for_keychain(KeychainKind::External)
        .max_satisfaction_weight()
        .unwrap();

    assert!(
        psbt_input.non_witness_utxo.is_none(),
        "`non_witness_utxo` should never be populated for taproot"
    );

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), 60_000)
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let (psbt, details) = builder.finish().unwrap();

    assert_eq!(
        details.sent - details.received,
        10_000 + details.fee.unwrap_or(0),
        "we should have only net spent ~10_000"
    );

    assert!(
        psbt.unsigned_tx
            .input
            .iter()
            .any(|input| input.previous_output == utxo.outpoint),
        "foreign_utxo should be in there"
    );
}

fn test_spend_from_wallet(mut wallet: Wallet) {
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();

    assert!(
        wallet.sign(&mut psbt, Default::default()).unwrap(),
        "Unable to finalize tx"
    );
}

//     #[test]
//     fn test_taproot_key_spend() {
//         let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig());
//         test_spend_from_wallet(wallet);

//         let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig_xprv());
//         test_spend_from_wallet(wallet);
//     }

#[test]
fn test_taproot_no_key_spend() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();

    assert!(
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    sign_with_tap_internal_key: false,
                    ..Default::default()
                },
            )
            .unwrap(),
        "Unable to finalize tx"
    );

    assert!(psbt.inputs.iter().all(|i| i.tap_key_sig.is_none()));
}

#[test]
fn test_taproot_script_spend() {
    let (wallet, _) = get_funded_wallet(get_test_tr_with_taptree());
    test_spend_from_wallet(wallet);

    let (wallet, _) = get_funded_wallet(get_test_tr_with_taptree_xprv());
    test_spend_from_wallet(wallet);
}

#[test]
fn test_taproot_script_spend_sign_all_leaves() {
    use bdk::signer::TapLeavesOptions;
    let (mut wallet, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();

    assert!(
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    tap_leaves_options: TapLeavesOptions::All,
                    ..Default::default()
                },
            )
            .unwrap(),
        "Unable to finalize tx"
    );

    assert!(psbt
        .inputs
        .iter()
        .all(|i| i.tap_script_sigs.len() == i.tap_scripts.len()));
}

#[test]
fn test_taproot_script_spend_sign_include_some_leaves() {
    use bdk::signer::TapLeavesOptions;
    use bitcoin::util::taproot::TapLeafHash;

    let (mut wallet, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();
    let mut script_leaves: Vec<_> = psbt.inputs[0]
        .tap_scripts
        .clone()
        .values()
        .map(|(script, version)| TapLeafHash::from_script(script, *version))
        .collect();
    let included_script_leaves = vec![script_leaves.pop().unwrap()];
    let excluded_script_leaves = script_leaves;

    assert!(
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    tap_leaves_options: TapLeavesOptions::Include(included_script_leaves.clone()),
                    ..Default::default()
                },
            )
            .unwrap(),
        "Unable to finalize tx"
    );

    assert!(psbt.inputs[0]
        .tap_script_sigs
        .iter()
        .all(|s| included_script_leaves.contains(&s.0 .1)
            && !excluded_script_leaves.contains(&s.0 .1)));
}

#[test]
fn test_taproot_script_spend_sign_exclude_some_leaves() {
    use bdk::signer::TapLeavesOptions;
    use bitcoin::util::taproot::TapLeafHash;

    let (mut wallet, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();
    let mut script_leaves: Vec<_> = psbt.inputs[0]
        .tap_scripts
        .clone()
        .values()
        .map(|(script, version)| TapLeafHash::from_script(script, *version))
        .collect();
    let included_script_leaves = vec![script_leaves.pop().unwrap()];
    let excluded_script_leaves = script_leaves;

    assert!(
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    tap_leaves_options: TapLeavesOptions::Exclude(excluded_script_leaves.clone()),
                    ..Default::default()
                },
            )
            .unwrap(),
        "Unable to finalize tx"
    );

    assert!(psbt.inputs[0]
        .tap_script_sigs
        .iter()
        .all(|s| included_script_leaves.contains(&s.0 .1)
            && !excluded_script_leaves.contains(&s.0 .1)));
}

#[test]
fn test_taproot_script_spend_sign_no_leaves() {
    use bdk::signer::TapLeavesOptions;
    let (mut wallet, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();

    wallet
        .sign(
            &mut psbt,
            SignOptions {
                tap_leaves_options: TapLeavesOptions::None,
                ..Default::default()
            },
        )
        .unwrap();

    assert!(psbt.inputs.iter().all(|i| i.tap_script_sigs.is_empty()));
}

#[test]
fn test_taproot_sign_derive_index_from_psbt() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig_xprv());

    let addr = wallet.get_address(AddressIndex::New);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), 25_000);
    let (mut psbt, _) = builder.finish().unwrap();

    // re-create the wallet with an empty db
    let wallet_empty =
        Wallet::new_no_persist(get_test_tr_single_sig_xprv(), None, Network::Regtest).unwrap();

    // signing with an empty db means that we will only look at the psbt to infer the
    // derivation index
    assert!(
        wallet_empty.sign(&mut psbt, Default::default()).unwrap(),
        "Unable to finalize tx"
    );
}

#[test]
fn test_taproot_sign_explicit_sighash_all() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .sighash(SchnorrSighashType::All.into())
        .drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(
        result.is_ok(),
        "Signing should work because SIGHASH_ALL is safe"
    )
}

#[test]
fn test_taproot_sign_non_default_sighash() {
    let sighash = SchnorrSighashType::NonePlusAnyoneCanPay;

    let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig());
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .sighash(sighash.into())
        .drain_wallet();
    let (mut psbt, _) = builder.finish().unwrap();

    let witness_utxo = psbt.inputs[0].witness_utxo.take();

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(
        result.is_err(),
        "Signing should have failed because the TX uses non-standard sighashes"
    );
    assert_matches!(
        result,
        Err(Error::Signer(SignerError::NonStandardSighash)),
        "Signing failed with the wrong error type"
    );

    // try again after opting-in
    let result = wallet.sign(
        &mut psbt,
        SignOptions {
            allow_all_sighashes: true,
            ..Default::default()
        },
    );
    assert!(
        result.is_err(),
        "Signing should have failed because the witness_utxo is missing"
    );
    assert_matches!(
        result,
        Err(Error::Signer(SignerError::MissingWitnessUtxo)),
        "Signing failed with the wrong error type"
    );

    // restore the witness_utxo
    psbt.inputs[0].witness_utxo = witness_utxo;

    let result = wallet.sign(
        &mut psbt,
        SignOptions {
            allow_all_sighashes: true,
            ..Default::default()
        },
    );

    assert!(result.is_ok(), "Signing should have worked");
    assert!(
        result.unwrap(),
        "Should finalize the input since we can produce signatures"
    );

    let extracted = psbt.extract_tx();
    assert_eq!(
        *extracted.input[0].witness.to_vec()[0].last().unwrap(),
        sighash as u8,
        "The signature should have been made with the right sighash"
    );
}

#[test]
fn test_spend_coinbase() {
    let descriptor = get_test_wpkh();
    let mut wallet = Wallet::new_no_persist(descriptor, None, Network::Regtest).unwrap();

    let confirmation_height = 5;
    wallet
        .insert_checkpoint(BlockId {
            height: confirmation_height,
            hash: BlockHash::all_zeros(),
        })
        .unwrap();
    let coinbase_tx = Transaction {
        version: 1,
        lock_time: bitcoin::PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 25_000,
            script_pubkey: wallet.get_address(New).address.script_pubkey(),
        }],
    };
    wallet
        .insert_tx(
            coinbase_tx,
            ConfirmationTime::Confirmed {
                height: confirmation_height,
                time: 30_000,
            },
        )
        .unwrap();

    let not_yet_mature_time = confirmation_height + COINBASE_MATURITY - 1;
    let maturity_time = confirmation_height + COINBASE_MATURITY;

    let balance = wallet.get_balance();
    assert_eq!(
        balance,
        Balance {
            immature: 25_000,
            trusted_pending: 0,
            untrusted_pending: 0,
            confirmed: 0
        }
    );

    // We try to create a transaction, only to notice that all
    // our funds are unspendable
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), balance.immature / 2)
        .current_height(confirmation_height);
    assert!(matches!(
        builder.finish(),
        Err(Error::InsufficientFunds {
            needed: _,
            available: 0
        })
    ));

    // Still unspendable...
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), balance.immature / 2)
        .current_height(not_yet_mature_time);
    assert_matches!(
        builder.finish(),
        Err(Error::InsufficientFunds {
            needed: _,
            available: 0
        })
    );

    wallet
        .insert_checkpoint(BlockId {
            height: maturity_time,
            hash: BlockHash::all_zeros(),
        })
        .unwrap();
    let balance = wallet.get_balance();
    assert_eq!(
        balance,
        Balance {
            immature: 0,
            trusted_pending: 0,
            untrusted_pending: 0,
            confirmed: 25_000
        }
    );
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), balance.confirmed / 2)
        .current_height(maturity_time);
    builder.finish().unwrap();
}

#[test]
fn test_allow_dust_limit() {
    let (mut wallet, _) = get_funded_wallet(get_test_single_sig_cltv());

    let addr = wallet.get_address(New);

    let mut builder = wallet.build_tx();

    builder.add_recipient(addr.script_pubkey(), 0);

    assert_matches!(builder.finish(), Err(Error::OutputBelowDustLimit(0)));

    let mut builder = wallet.build_tx();

    builder
        .allow_dust(true)
        .add_recipient(addr.script_pubkey(), 0);

    assert!(builder.finish().is_ok());
}

#[test]
fn test_fee_rate_sign_no_grinding_high_r() {
    // Our goal is to obtain a transaction with a signature with high-R (71 bytes
    // instead of 70). We then check that our fee rate and fee calculation is
    // alright.
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let fee_rate = FeeRate::from_sat_per_vb(1.0);
    let mut builder = wallet.build_tx();
    let mut data = vec![0];
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_rate(fee_rate)
        .add_data(&data);
    let (mut psbt, details) = builder.finish().unwrap();
    let (op_return_vout, _) = psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .find(|(_n, i)| i.script_pubkey.is_op_return())
        .unwrap();

    let mut sig_len: usize = 0;
    // We try to sign many different times until we find a longer signature (71 bytes)
    while sig_len < 71 {
        // Changing the OP_RETURN data will make the signature change (but not the fee, until
        // data[0] is small enough)
        data[0] += 1;
        psbt.unsigned_tx.output[op_return_vout].script_pubkey = Script::new_op_return(&data);
        // Clearing the previous signature
        psbt.inputs[0].partial_sigs.clear();
        // Signing
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    remove_partial_sigs: false,
                    try_finalize: false,
                    allow_grinding: false,
                    ..Default::default()
                },
            )
            .unwrap();
        // We only have one key in the partial_sigs map, this is a trick to retrieve it
        let key = psbt.inputs[0].partial_sigs.keys().next().unwrap();
        sig_len = psbt.inputs[0].partial_sigs[key].sig.serialize_der().len();
    }
    // Actually finalizing the transaction...
    wallet
        .sign(
            &mut psbt,
            SignOptions {
                remove_partial_sigs: false,
                allow_grinding: false,
                ..Default::default()
            },
        )
        .unwrap();
    // ...and checking that everything is fine
    assert_fee_rate!(psbt, details.fee.unwrap_or(0), fee_rate);
}

#[test]
fn test_fee_rate_sign_grinding_low_r() {
    // Our goal is to obtain a transaction with a signature with low-R (70 bytes)
    // by setting the `allow_grinding` signing option as true.
    // We then check that our fee rate and fee calculation is alright and that our
    // signature is 70 bytes.
    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let fee_rate = FeeRate::from_sat_per_vb(1.0);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_rate(fee_rate);
    let (mut psbt, details) = builder.finish().unwrap();

    wallet
        .sign(
            &mut psbt,
            SignOptions {
                remove_partial_sigs: false,
                allow_grinding: true,
                ..Default::default()
            },
        )
        .unwrap();

    let key = psbt.inputs[0].partial_sigs.keys().next().unwrap();
    let sig_len = psbt.inputs[0].partial_sigs[key].sig.serialize_der().len();
    assert_eq!(sig_len, 70);
    assert_fee_rate!(psbt, details.fee.unwrap_or(0), fee_rate);
}

// #[cfg(feature = "test-hardware-signer")]
// #[test]
// fn test_hardware_signer() {
//     use std::sync::Arc;
//
//     use bdk::signer::SignerOrdering;
//     use bdk::wallet::hardwaresigner::HWISigner;
//     use hwi::types::HWIChain;
//     use hwi::HWIClient;
//
//     let mut devices = HWIClient::enumerate().unwrap();
//     if devices.is_empty() {
//         panic!("No devices found!");
//     }
//     let device = devices.remove(0).unwrap();
//     let client = HWIClient::get_client(&device, true, HWIChain::Regtest).unwrap();
//     let descriptors = client.get_descriptors::<String>(None).unwrap();
//     let custom_signer = HWISigner::from_device(&device, HWIChain::Regtest).unwrap();
//
//     let (mut wallet, _) = get_funded_wallet(&descriptors.internal[0]);
//     wallet.add_signer(
//         KeychainKind::External,
//         SignerOrdering(200),
//         Arc::new(custom_signer),
//     );
//
//     let addr = wallet.get_address(LastUnused);
//     let mut builder = wallet.build_tx();
//     builder.drain_to(addr.script_pubkey()).drain_wallet();
//     let (mut psbt, _) = builder.finish().unwrap();
//
//     let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
//     assert!(finalized);
// }

#[test]
fn test_taproot_load_descriptor_duplicated_keys() {
    // Added after issue https://github.com/bitcoindevkit/bdk/issues/760
    //
    // Having the same key in multiple taproot leaves is safe and should be accepted by BDK

    let (mut wallet, _) = get_funded_wallet(get_test_tr_dup_keys());
    let addr = wallet.get_address(New);

    assert_eq!(
        addr.to_string(),
        "bcrt1pvysh4nmh85ysrkpwtrr8q8gdadhgdejpy6f9v424a8v9htjxjhyqw9c5s5"
    );
}

#[test]
/// The wallet should re-use previously allocated change addresses when the tx using them is cancelled
fn test_tx_cancellation() {
    macro_rules! new_tx {
        ($wallet:expr) => {{
            let addr = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
            let mut builder = $wallet.build_tx();
            builder.add_recipient(addr.script_pubkey(), 10_000);

            let (psbt, _) = builder.finish().unwrap();

            psbt
        }};
    }

    let (mut wallet, _) =
        get_funded_wallet_with_change(get_test_wpkh(), Some(get_test_tr_single_sig_xprv()));

    let psbt1 = new_tx!(wallet);
    let change_derivation_1 = psbt1
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(&txout.script_pubkey))
        .unwrap();
    assert_eq!(change_derivation_1, (KeychainKind::Internal, 0));

    let psbt2 = new_tx!(wallet);

    let change_derivation_2 = psbt2
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(&txout.script_pubkey))
        .unwrap();
    assert_eq!(change_derivation_2, (KeychainKind::Internal, 1));

    wallet.cancel_tx(&psbt1.extract_tx());

    let psbt3 = new_tx!(wallet);
    let change_derivation_3 = psbt3
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(&txout.script_pubkey))
        .unwrap();
    assert_eq!(change_derivation_3, (KeychainKind::Internal, 0));

    let psbt3 = new_tx!(wallet);
    let change_derivation_3 = psbt3
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(&txout.script_pubkey))
        .unwrap();
    assert_eq!(change_derivation_3, (KeychainKind::Internal, 2));

    wallet.cancel_tx(&psbt3.extract_tx());

    let psbt3 = new_tx!(wallet);
    let change_derivation_4 = psbt3
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(&txout.script_pubkey))
        .unwrap();
    assert_eq!(change_derivation_4, (KeychainKind::Internal, 2));
}
