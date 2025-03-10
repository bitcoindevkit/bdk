use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Context;
use assert_matches::assert_matches;
use bdk_chain::{BlockId, ChainPosition, ConfirmationBlockTime};
use bdk_wallet::coin_selection::{self, LargestFirstCoinSelection};
use bdk_wallet::descriptor::{calc_checksum, DescriptorError, IntoWalletDescriptor};
use bdk_wallet::error::CreateTxError;
use bdk_wallet::psbt::PsbtUtils;
use bdk_wallet::signer::{SignOptions, SignerError};
use bdk_wallet::test_utils::*;
use bdk_wallet::tx_builder::AddForeignUtxoError;
use bdk_wallet::{
    AddressInfo, Balance, ChangeSet, PersistedWallet, Update, Wallet, WalletPersister, WalletTx,
};
use bdk_wallet::{KeychainKind, LoadError, LoadMismatch, LoadWithPersistError};
use bitcoin::constants::{ChainHash, COINBASE_MATURITY};
use bitcoin::hashes::Hash;
use bitcoin::key::Secp256k1;
use bitcoin::psbt;
use bitcoin::script::PushBytesBuf;
use bitcoin::sighash::{EcdsaSighashType, TapSighashType};
use bitcoin::taproot::TapNodeHash;
use bitcoin::{
    absolute, transaction, Address, Amount, BlockHash, FeeRate, Network, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Weight,
};
use miniscript::{descriptor::KeyMap, Descriptor, DescriptorPublicKey};
use rand::rngs::StdRng;
use rand::SeedableRng;

fn parse_descriptor(s: &str) -> (Descriptor<DescriptorPublicKey>, KeyMap) {
    <Descriptor<DescriptorPublicKey>>::parse_descriptor(&Secp256k1::new(), s)
        .expect("failed to parse descriptor")
}

// The satisfaction size of a P2WPKH is 112 WU =
// 1 (elements in witness) + 1 (OP_PUSH) + 33 (pk) + 1 (OP_PUSH) + 72 (signature + sighash) + 1*4 (script len)
// On the witness itself, we have to push once for the pk (33WU) and once for signature + sighash (72WU), for
// a total of 105 WU.
// Here, we push just once for simplicity, so we have to add an extra byte for the missing
// OP_PUSH.
const P2WPKH_FAKE_WITNESS_SIZE: usize = 106;

const DB_MAGIC: &[u8] = &[0x21, 0x24, 0x48];

#[test]
fn wallet_is_persisted() -> anyhow::Result<()> {
    fn run<Db, CreateDb, OpenDb>(
        filename: &str,
        create_db: CreateDb,
        open_db: OpenDb,
    ) -> anyhow::Result<()>
    where
        CreateDb: Fn(&Path) -> anyhow::Result<Db>,
        OpenDb: Fn(&Path) -> anyhow::Result<Db>,
        Db: WalletPersister,
        Db::Error: std::error::Error + Send + Sync + 'static,
    {
        let temp_dir = tempfile::tempdir().expect("must create tempdir");
        let file_path = temp_dir.path().join(filename);
        let (external_desc, internal_desc) = get_test_tr_single_sig_xprv_and_change_desc();

        // create new wallet
        let wallet_spk_index = {
            let mut db = create_db(&file_path)?;
            let mut wallet = Wallet::create(external_desc, internal_desc)
                .network(Network::Testnet)
                .create_wallet(&mut db)?;
            wallet.reveal_next_address(KeychainKind::External);

            // persist new wallet changes
            assert!(wallet.persist(&mut db)?, "must write");
            wallet.spk_index().clone()
        };

        // recover wallet
        {
            let mut db = open_db(&file_path).context("failed to recover db")?;
            let _ = Wallet::load()
                .check_network(Network::Testnet)
                .load_wallet(&mut db)?
                .expect("wallet must exist");
        }
        {
            let mut db = open_db(&file_path).context("failed to recover db")?;
            let wallet = Wallet::load()
                .descriptor(KeychainKind::External, Some(external_desc))
                .descriptor(KeychainKind::Internal, Some(internal_desc))
                .check_network(Network::Testnet)
                .load_wallet(&mut db)?
                .expect("wallet must exist");

            assert_eq!(wallet.network(), Network::Testnet);
            assert_eq!(
                wallet.spk_index().keychains().collect::<Vec<_>>(),
                wallet_spk_index.keychains().collect::<Vec<_>>()
            );
            assert_eq!(
                wallet.spk_index().last_revealed_indices(),
                wallet_spk_index.last_revealed_indices()
            );
            let secp = Secp256k1::new();
            assert_eq!(
                *wallet.public_descriptor(KeychainKind::External),
                external_desc
                    .into_wallet_descriptor(&secp, wallet.network())
                    .unwrap()
                    .0
            );
        }

        Ok(())
    }

    run(
        "store.db",
        |path| Ok(bdk_file_store::Store::create_new(DB_MAGIC, path)?),
        |path| Ok(bdk_file_store::Store::open(DB_MAGIC, path)?),
    )?;
    run::<bdk_chain::rusqlite::Connection, _, _>(
        "store.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
    )?;

    Ok(())
}

#[test]
fn wallet_load_checks() -> anyhow::Result<()> {
    fn run<Db, CreateDb, OpenDb>(
        filename: &str,
        create_db: CreateDb,
        open_db: OpenDb,
    ) -> anyhow::Result<()>
    where
        CreateDb: Fn(&Path) -> anyhow::Result<Db>,
        OpenDb: Fn(&Path) -> anyhow::Result<Db>,
        Db: WalletPersister + std::fmt::Debug,
        Db::Error: std::error::Error + Send + Sync + 'static,
    {
        let temp_dir = tempfile::tempdir().expect("must create tempdir");
        let file_path = temp_dir.path().join(filename);
        let network = Network::Testnet;
        let (external_desc, internal_desc) = get_test_tr_single_sig_xprv_and_change_desc();

        // create new wallet
        let _ = Wallet::create(external_desc, internal_desc)
            .network(network)
            .create_wallet(&mut create_db(&file_path)?)?;

        assert_matches!(
            Wallet::load()
                .check_network(Network::Regtest)
                .load_wallet(&mut open_db(&file_path)?),
            Err(LoadWithPersistError::InvalidChangeSet(LoadError::Mismatch(
                LoadMismatch::Network {
                    loaded: Network::Testnet,
                    expected: Network::Regtest,
                }
            ))),
            "unexpected network check result: Regtest (check) is not Testnet (loaded)",
        );
        let mainnet_hash = BlockHash::from_byte_array(ChainHash::BITCOIN.to_bytes());
        assert_matches!(
            Wallet::load().check_genesis_hash(mainnet_hash).load_wallet(&mut open_db(&file_path)?),
            Err(LoadWithPersistError::InvalidChangeSet(LoadError::Mismatch(LoadMismatch::Genesis { .. }))),
            "unexpected genesis hash check result: mainnet hash (check) is not testnet hash (loaded)",
        );
        assert_matches!(
            Wallet::load()
                .descriptor(KeychainKind::External, Some(internal_desc))
                .load_wallet(&mut open_db(&file_path)?),
            Err(LoadWithPersistError::InvalidChangeSet(LoadError::Mismatch(
                LoadMismatch::Descriptor { .. }
            ))),
            "unexpected descriptors check result",
        );
        assert_matches!(
            Wallet::load()
                .descriptor(KeychainKind::External, Option::<&str>::None)
                .load_wallet(&mut open_db(&file_path)?),
            Err(LoadWithPersistError::InvalidChangeSet(LoadError::Mismatch(
                LoadMismatch::Descriptor { .. }
            ))),
            "unexpected descriptors check result",
        );
        // check setting keymaps
        let (_, external_keymap) = parse_descriptor(external_desc);
        let (_, internal_keymap) = parse_descriptor(internal_desc);
        let wallet = Wallet::load()
            .keymap(KeychainKind::External, external_keymap)
            .keymap(KeychainKind::Internal, internal_keymap)
            .load_wallet(&mut open_db(&file_path)?)
            .expect("db should not fail")
            .expect("wallet was persisted");
        for keychain in [KeychainKind::External, KeychainKind::Internal] {
            let keymap = wallet.get_signers(keychain).as_key_map(wallet.secp_ctx());
            assert!(
                !keymap.is_empty(),
                "load should populate keymap for keychain {keychain:?}"
            );
        }
        Ok(())
    }

    run(
        "store.db",
        |path| {
            Ok(bdk_file_store::Store::<ChangeSet>::create_new(
                DB_MAGIC, path,
            )?)
        },
        |path| Ok(bdk_file_store::Store::<ChangeSet>::open(DB_MAGIC, path)?),
    )?;
    run(
        "store.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
    )?;

    Ok(())
}

#[test]
fn wallet_should_persist_anchors_and_recover() {
    use bdk_chain::rusqlite;
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("wallet.db");
    let mut db = rusqlite::Connection::open(db_path).unwrap();

    let desc = get_test_tr_single_sig_xprv();
    let mut wallet = Wallet::create_single(desc)
        .network(Network::Testnet)
        .create_wallet(&mut db)
        .unwrap();
    let small_output_tx = Transaction {
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
        version: transaction::Version::non_standard(0),
        lock_time: absolute::LockTime::ZERO,
    };
    let txid = small_output_tx.compute_txid();
    insert_tx(&mut wallet, small_output_tx);
    let expected_anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().block_id(),
        confirmation_time: 200,
    };
    insert_anchor(&mut wallet, txid, expected_anchor);
    assert!(wallet.persist(&mut db).unwrap());

    // should recover persisted wallet
    let secp = wallet.secp_ctx();
    let (_, keymap) = <Descriptor<DescriptorPublicKey>>::parse_descriptor(secp, desc).unwrap();
    assert!(!keymap.is_empty());
    let wallet = Wallet::load()
        .descriptor(KeychainKind::External, Some(desc))
        .extract_keys()
        .load_wallet(&mut db)
        .unwrap()
        .expect("must have loaded changeset");
    // stored anchor should be retrieved in the same condition it was persisted
    if let ChainPosition::Confirmed {
        anchor: obtained_anchor,
        ..
    } = wallet
        .get_tx(txid)
        .expect("should retrieve stored tx")
        .chain_position
    {
        assert_eq!(obtained_anchor, expected_anchor)
    } else {
        panic!("Should have got ChainPosition::Confirmed)");
    }
}

#[test]
fn single_descriptor_wallet_persist_and_recover() {
    use bdk_chain::miniscript::Descriptor;
    use bdk_chain::miniscript::DescriptorPublicKey;
    use bdk_chain::rusqlite;
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("wallet.db");
    let mut db = rusqlite::Connection::open(db_path).unwrap();

    let desc = get_test_tr_single_sig_xprv();
    let mut wallet = Wallet::create_single(desc)
        .network(Network::Testnet)
        .create_wallet(&mut db)
        .unwrap();
    let _ = wallet.reveal_addresses_to(KeychainKind::External, 2);
    assert!(wallet.persist(&mut db).unwrap());

    // should recover persisted wallet
    let secp = wallet.secp_ctx();
    let (_, keymap) = <Descriptor<DescriptorPublicKey>>::parse_descriptor(secp, desc).unwrap();
    assert!(!keymap.is_empty());
    let wallet = Wallet::load()
        .descriptor(KeychainKind::External, Some(desc))
        .extract_keys()
        .load_wallet(&mut db)
        .unwrap()
        .expect("must have loaded changeset");
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(2));
    // should have private key
    assert_eq!(
        wallet.get_signers(KeychainKind::External).as_key_map(secp),
        keymap,
    );

    // should error on wrong internal params
    let desc = get_test_wpkh();
    let (exp_desc, _) = <Descriptor<DescriptorPublicKey>>::parse_descriptor(secp, desc).unwrap();
    let err = Wallet::load()
        .descriptor(KeychainKind::Internal, Some(desc))
        .extract_keys()
        .load_wallet(&mut db);
    assert_matches!(
        err,
        Err(LoadWithPersistError::InvalidChangeSet(LoadError::Mismatch(LoadMismatch::Descriptor { keychain, loaded, expected })))
        if keychain == KeychainKind::Internal && loaded.is_none() && expected == Some(exp_desc),
        "single descriptor wallet should refuse change descriptor param"
    );
}

#[test]
fn test_error_external_and_internal_are_the_same() {
    // identical descriptors should fail to create wallet
    let desc = get_test_wpkh();
    let err = Wallet::create(desc, desc)
        .network(Network::Testnet)
        .create_wallet_no_persist();
    assert!(
        matches!(&err, Err(DescriptorError::ExternalAndInternalAreTheSame)),
        "expected same descriptors error, got {:?}",
        err,
    );

    // public + private of same descriptor should fail to create wallet
    let desc = "wpkh(tprv8ZgxMBicQKsPdcAqYBpzAFwU5yxBUo88ggoBqu1qPcHUfSbKK1sKMLmC7EAk438btHQrSdu3jGGQa6PA71nvH5nkDexhLteJqkM4dQmWF9g/84'/1'/0'/0/*)";
    let change_desc = "wpkh([3c31d632/84'/1'/0']tpubDCYwFkks2cg78N7eoYbBatsFEGje8vW8arSKW4rLwD1AU1s9KJMDRHE32JkvYERuiFjArrsH7qpWSpJATed5ShZbG9KsskA5Rmi6NSYgYN2/0/*)";
    let err = Wallet::create(desc, change_desc)
        .network(Network::Testnet)
        .create_wallet_no_persist();
    assert!(
        matches!(err, Err(DescriptorError::ExternalAndInternalAreTheSame)),
        "expected same descriptors error, got {:?}",
        err,
    );
}

#[test]
fn test_descriptor_checksum() {
    let (wallet, _) = get_funded_wallet_wpkh();
    let checksum = wallet.descriptor_checksum(KeychainKind::External);
    assert_eq!(checksum.len(), 8);

    let raw_descriptor = wallet
        .keychains()
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
    let (wallet, _) = get_funded_wallet_wpkh();

    // The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
    // to a foreign address and one returning 50_000 back to the wallet as change. The remaining 1000
    // sats are the transaction fee.
    assert_eq!(wallet.balance().confirmed, Amount::from_sat(50_000));
}

#[test]
fn test_get_funded_wallet_sent_and_received() {
    let (wallet, txid) = get_funded_wallet_wpkh();

    let mut tx_amounts: Vec<(Txid, (Amount, Amount))> = wallet
        .transactions()
        .map(|ct| (ct.tx_node.txid, wallet.sent_and_received(&ct.tx_node)))
        .collect();
    tx_amounts.sort_by(|a1, a2| a1.0.cmp(&a2.0));

    let tx = wallet.get_tx(txid).expect("transaction").tx_node.tx;
    let (sent, received) = wallet.sent_and_received(&tx);

    // The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
    // to a foreign address and one returning 50_000 back to the wallet as change. The remaining 1000
    // sats are the transaction fee.
    assert_eq!(sent.to_sat(), 76_000);
    assert_eq!(received.to_sat(), 50_000);
}

#[test]
fn test_get_funded_wallet_tx_fees() {
    let (wallet, txid) = get_funded_wallet_wpkh();

    let tx = wallet.get_tx(txid).expect("transaction").tx_node.tx;
    let tx_fee = wallet.calculate_fee(&tx).expect("transaction fee");

    // The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
    // to a foreign address and one returning 50_000 back to the wallet as change. The remaining 1000
    // sats are the transaction fee.
    assert_eq!(tx_fee, Amount::from_sat(1000))
}

#[test]
fn test_get_funded_wallet_tx_fee_rate() {
    let (wallet, txid) = get_funded_wallet_wpkh();

    let tx = wallet.get_tx(txid).expect("transaction").tx_node.tx;
    let tx_fee_rate = wallet
        .calculate_fee_rate(&tx)
        .expect("transaction fee rate");

    // The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
    // to a foreign address and one returning 50_000 back to the wallet as change. The remaining 1000
    // sats are the transaction fee.

    // tx weight = 452 wu, as vbytes = (452 + 3) / 4 = 113
    // fee_rate (sats per kwu) = fee / weight = 1000sat / 0.452kwu = 2212
    // fee_rate (sats per vbyte ceil) = fee / vsize = 1000sat / 113vb = 9
    assert_eq!(tx_fee_rate.to_sat_per_kwu(), 2212);
    assert_eq!(tx_fee_rate.to_sat_per_vb_ceil(), 9);
}

#[test]
fn test_list_output() {
    let (wallet, txid) = get_funded_wallet_wpkh();
    let txos = wallet
        .list_output()
        .map(|op| (op.outpoint, op))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(txos.len(), 2);
    for (op, txo) in txos {
        if op.txid == txid {
            assert_eq!(txo.txout.value.to_sat(), 50_000);
            assert!(!txo.is_spent);
        } else {
            assert_eq!(txo.txout.value.to_sat(), 76_000);
            assert!(txo.is_spent);
        }
    }
}

macro_rules! assert_fee_rate {
    ($psbt:expr, $fees:expr, $fee_rate:expr $( ,@dust_change $( $dust_change:expr )* )* $( ,@add_signature $( $add_signature:expr )* )* ) => ({
        let psbt = $psbt.clone();
        #[allow(unused_mut)]
        let mut tx = $psbt.clone().extract_tx().expect("failed to extract tx");
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
            .fold(Amount::ZERO, |acc, i| acc + i.witness_utxo.as_ref().unwrap().value)
            - psbt
            .unsigned_tx
            .output
            .iter()
            .fold(Amount::ZERO, |acc, o| acc + o.value);

        assert_eq!(fee_amount, $fees);

        let tx_fee_rate = (fee_amount / tx.weight())
            .to_sat_per_kwu();
        let fee_rate = $fee_rate.to_sat_per_kwu();
        let half_default = FeeRate::BROADCAST_MIN.checked_div(2)
            .unwrap()
            .to_sat_per_kwu();

        if !dust_change {
            assert!(tx_fee_rate >= fee_rate && tx_fee_rate - fee_rate < half_default, "Expected fee rate of {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
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
    let (mut wallet, _) = get_funded_wallet_wpkh();
    wallet.build_tx().finish().unwrap();
}

#[test]
#[should_panic(expected = "NoUtxosSelected")]
fn test_create_tx_manually_selected_empty_utxos() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .manually_selected_only();
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_version_0() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .version(0);
    assert!(matches!(builder.finish(), Err(CreateTxError::Version0)));
}

#[test]
fn test_create_tx_version_1_csv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_csv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .version(1);
    assert!(matches!(builder.finish(), Err(CreateTxError::Version1Csv)));
}

#[test]
fn test_create_tx_custom_version() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .version(42);
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.version.0, 42);
}

#[test]
fn test_create_tx_default_locktime_is_last_sync_height() {
    let (mut wallet, _) = get_funded_wallet_wpkh();

    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    // Since we never synced the wallet we don't have a last_sync_height
    // we could use to try to prevent fee sniping. We default to 0.
    assert_eq!(psbt.unsigned_tx.lock_time.to_consensus_u32(), 2_000);
}

#[test]
fn test_create_tx_fee_sniping_locktime_last_sync() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));

    let psbt = builder.finish().unwrap();

    // If there's no current_height we're left with using the last sync height
    assert_eq!(
        psbt.unsigned_tx.lock_time.to_consensus_u32(),
        wallet.latest_checkpoint().height()
    );
}

#[test]
fn test_create_tx_default_locktime_cltv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_cltv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.lock_time.to_consensus_u32(), 100_000);
}

#[test]
fn test_create_tx_locktime_cltv_timestamp() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_cltv_timestamp());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.lock_time.to_consensus_u32(), 1_734_230_218);

    let finalized = wallet.sign(&mut psbt, SignOptions::default()).unwrap();

    assert!(finalized);
}

#[test]
fn test_create_tx_custom_locktime() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .current_height(630_001)
        .nlocktime(absolute::LockTime::from_height(630_000).unwrap());
    let psbt = builder.finish().unwrap();

    // When we explicitly specify a nlocktime
    // we don't try any fee sniping prevention trick
    // (we ignore the current_height)
    assert_eq!(psbt.unsigned_tx.lock_time.to_consensus_u32(), 630_000);
}

#[test]
fn test_create_tx_custom_locktime_compatible_with_cltv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_cltv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .nlocktime(absolute::LockTime::from_height(630_000).unwrap());
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.lock_time.to_consensus_u32(), 630_000);
}

#[test]
fn test_create_tx_custom_locktime_incompatible_with_cltv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_cltv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .nlocktime(absolute::LockTime::from_height(50000).unwrap());
    assert!(matches!(builder.finish(),
        Err(CreateTxError::LockTime { requested, required })
        if requested.to_consensus_u32() == 50_000 && required.to_consensus_u32() == 100_000));
}

#[test]
fn test_create_tx_custom_csv() {
    // desc: wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_csv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .set_exact_sequence(Sequence(42))
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();
    // we allow setting a sequence higher than required
    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(42));
}

#[test]
fn test_create_tx_no_rbf_csv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_csv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(6));
}

#[test]
fn test_create_tx_incompatible_csv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_csv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .set_exact_sequence(Sequence(3));
    assert!(matches!(builder.finish(),
        Err(CreateTxError::RbfSequenceCsv { sequence, csv })
        if sequence.to_consensus_u32() == 3 && csv.to_consensus_u32() == 6));
}

#[test]
fn test_create_tx_with_default_rbf_csv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_csv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();
    // When CSV is enabled it takes precedence over the rbf value (unless forced by the user).
    // It will be set to the OP_CSV value, in this case 6
    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(6));
}

#[test]
fn test_create_tx_no_rbf_cltv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_cltv());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    builder.set_exact_sequence(Sequence(0xFFFFFFFE));
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFE));
}

#[test]
fn test_create_tx_custom_rbf_sequence() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .set_exact_sequence(Sequence(0xDEADBEEF));
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xDEADBEEF));
}

#[test]
fn test_create_tx_change_policy() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .do_not_spend_change();
    assert!(builder.finish().is_ok());

    // wallet has no change, so setting `only_spend_change`
    // should cause tx building to fail
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .only_spend_change();
    assert!(matches!(
        builder.finish(),
        Err(CreateTxError::CoinSelection(
            coin_selection::InsufficientFunds { .. }
        )),
    ));
}

#[test]
fn test_create_tx_default_sequence() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFD));
}

macro_rules! check_fee {
    ($wallet:expr, $psbt: expr) => {{
        let tx = $psbt.clone().extract_tx().expect("failed to extract tx");
        let tx_fee = $wallet.calculate_fee(&tx).ok();
        assert_eq!(tx_fee, $psbt.fee_amount());
        tx_fee
    }};
}

#[test]
fn test_create_tx_drain_wallet_and_drain_to() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        Amount::from_sat(50_000) - fee.unwrap_or(Amount::ZERO)
    );
}

#[test]
fn test_create_tx_drain_wallet_and_drain_to_and_with_recipient() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt")
        .unwrap()
        .assume_checked();
    let drain_addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(20_000))
        .drain_to(drain_addr.script_pubkey())
        .drain_wallet();
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);
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
    assert_eq!(main_output.value, Amount::from_sat(20_000));
    assert_eq!(
        drain_output.value,
        Amount::from_sat(30_000) - fee.unwrap_or(Amount::ZERO)
    );
}

#[test]
fn test_create_tx_drain_to_and_utxos() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let utxos: Vec<_> = wallet.list_unspent().map(|u| u.outpoint).collect();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxos(&utxos)
        .unwrap();
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        Amount::from_sat(50_000) - fee.unwrap_or(Amount::ZERO)
    );
}

#[test]
#[should_panic(expected = "NoRecipients")]
fn test_create_tx_drain_to_no_drain_wallet_no_utxos() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let drain_addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(drain_addr.script_pubkey());
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_default_fee_rate() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::BROADCAST_MIN, @add_signature);
}

#[test]
fn test_create_tx_custom_fee_rate() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(5));
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(5), @add_signature);
}

#[test]
fn test_create_tx_absolute_fee() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_absolute(Amount::from_sat(100));
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(100));
    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        Amount::from_sat(50_000) - fee.unwrap_or(Amount::ZERO)
    );
}

#[test]
fn test_create_tx_absolute_zero_fee() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_absolute(Amount::ZERO);
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::ZERO);
    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        Amount::from_sat(50_000) - fee.unwrap_or(Amount::ZERO)
    );
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_create_tx_absolute_high_fee() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_absolute(Amount::from_sat(60_000));
    let _ = builder.finish().unwrap();
}

#[test]
fn test_create_tx_add_change() {
    use bdk_wallet::tx_builder::TxOrdering;
    let seed = [0; 32];
    let mut rng: StdRng = SeedableRng::from_seed(seed);
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .ordering(TxOrdering::Shuffle);
    let psbt = builder.finish_with_aux_rand(&mut rng).unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(psbt.unsigned_tx.output.len(), 2);
    assert_eq!(psbt.unsigned_tx.output[0].value, Amount::from_sat(25_000));
    assert_eq!(
        psbt.unsigned_tx.output[1].value,
        Amount::from_sat(25_000) - fee.unwrap_or(Amount::ZERO)
    );
}

#[test]
fn test_create_tx_skip_change_dust() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(49_800));
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(psbt.unsigned_tx.output.len(), 1);
    assert_eq!(psbt.unsigned_tx.output[0].value.to_sat(), 49_800);
    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(200));
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_create_tx_drain_to_dust_amount() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    // very high fee rate, so that the only output would be below dust
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(454));
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_ordering_respected() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);

    let bip69_txin_cmp = |tx_a: &TxIn, tx_b: &TxIn| {
        let project_outpoint = |t: &TxIn| (t.previous_output.txid, t.previous_output.vout);
        project_outpoint(tx_a).cmp(&project_outpoint(tx_b))
    };

    let bip69_txout_cmp = |tx_a: &TxOut, tx_b: &TxOut| {
        let project_utxo = |t: &TxOut| (t.value, t.script_pubkey.clone());
        project_utxo(tx_a).cmp(&project_utxo(tx_b))
    };

    let custom_bip69_ordering = bdk_wallet::tx_builder::TxOrdering::Custom {
        input_sort: Arc::new(bip69_txin_cmp),
        output_sort: Arc::new(bip69_txout_cmp),
    };

    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .add_recipient(addr.script_pubkey(), Amount::from_sat(10_000))
        .ordering(custom_bip69_ordering);

    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(psbt.unsigned_tx.output.len(), 3);
    assert_eq!(
        psbt.unsigned_tx.output[0].value,
        Amount::from_sat(10_000) - fee.unwrap_or(Amount::ZERO)
    );
    assert_eq!(psbt.unsigned_tx.output[1].value, Amount::from_sat(10_000));
    assert_eq!(psbt.unsigned_tx.output[2].value, Amount::from_sat(30_000));
}

#[test]
fn test_create_tx_default_sighash() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(30_000));
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.inputs[0].sighash_type, None);
}

#[test]
fn test_create_tx_custom_sighash() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .sighash(EcdsaSighashType::Single.into());
    let psbt = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0].sighash_type,
        Some(EcdsaSighashType::Single.into())
    );
}

#[test]
fn test_create_tx_input_hd_keypaths() {
    use bitcoin::bip32::{DerivationPath, Fingerprint};
    use core::str::FromStr;

    let (mut wallet, _) = get_funded_wallet_single("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

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
    use bitcoin::bip32::{DerivationPath, Fingerprint};
    use core::str::FromStr;

    let (mut wallet, _) = get_funded_wallet_single("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");

    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

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
    use bitcoin::hex::FromHex;

    let (mut wallet, _) =
        get_funded_wallet_single("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0].redeem_script,
        Some(ScriptBuf::from(
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
    use bitcoin::hex::FromHex;

    let (mut wallet, _) =
        get_funded_wallet_single("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.inputs[0].redeem_script, None);
    assert_eq!(
        psbt.inputs[0].witness_script,
        Some(ScriptBuf::from(
            Vec::<u8>::from_hex(
                "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
            )
            .unwrap()
        ))
    );
}

#[test]
fn test_create_tx_set_redeem_witness_script_p2wsh_p2sh() {
    let (mut wallet, _) = get_funded_wallet_single(
        "sh(wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)))",
    );
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    let script = ScriptBuf::from_hex(
        "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac",
    )
    .unwrap();

    assert_eq!(psbt.inputs[0].redeem_script, Some(script.to_p2wsh()));
    assert_eq!(psbt.inputs[0].witness_script, Some(script));
}

#[test]
fn test_create_tx_non_witness_utxo() {
    let (mut wallet, _) =
        get_funded_wallet_single("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert!(psbt.inputs[0].non_witness_utxo.is_some());
    assert!(psbt.inputs[0].witness_utxo.is_none());
}

#[test]
fn test_create_tx_only_witness_utxo() {
    let (mut wallet, _) =
        get_funded_wallet_single("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .only_witness_utxo()
        .drain_wallet();
    let psbt = builder.finish().unwrap();

    assert!(psbt.inputs[0].non_witness_utxo.is_none());
    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn test_create_tx_shwpkh_has_witness_utxo() {
    let (mut wallet, _) =
        get_funded_wallet_single("sh(wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn test_create_tx_both_non_witness_utxo_and_witness_utxo_default() {
    let (mut wallet, _) =
        get_funded_wallet_single("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert!(psbt.inputs[0].non_witness_utxo.is_some());
    assert!(psbt.inputs[0].witness_utxo.is_some());
}

#[test]
fn test_create_tx_add_utxo() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let small_output_tx = Transaction {
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
        version: transaction::Version::non_standard(0),
        lock_time: absolute::LockTime::ZERO,
    };
    let txid = small_output_tx.compute_txid();
    insert_tx(&mut wallet, small_output_tx);
    let anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().get(2000).unwrap().block_id(),
        confirmation_time: 200,
    };
    insert_anchor(&mut wallet, txid, anchor);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .add_utxo(OutPoint { txid, vout: 0 })
        .unwrap();
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));

    assert_eq!(
        psbt.unsigned_tx.input.len(),
        2,
        "should add an additional input since 25_000 < 30_000"
    );
    assert_eq!(
        sent_received.0,
        Amount::from_sat(75_000),
        "total should be sum of both inputs"
    );
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_create_tx_manually_selected_insufficient() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let small_output_tx = Transaction {
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
        version: transaction::Version::non_standard(0),
        lock_time: absolute::LockTime::ZERO,
    };
    let txid = small_output_tx.compute_txid();
    insert_tx(&mut wallet, small_output_tx.clone());
    let anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().get(2000).unwrap().block_id(),
        confirmation_time: 200,
    };
    insert_anchor(&mut wallet, txid, anchor);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .add_utxo(OutPoint { txid, vout: 0 })
        .unwrap()
        .manually_selected_only();
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "SpendingPolicyRequired(External)")]
fn test_create_tx_policy_path_required() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_a_or_b_plus_csv());

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(10_000));
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_policy_path_no_csv() {
    let (descriptor, change_descriptor) = get_test_wpkh_and_change_desc();
    let mut wallet = Wallet::create(descriptor, change_descriptor)
        .network(Network::Regtest)
        .create_wallet_no_persist()
        .expect("wallet");

    let tx = Transaction {
        version: transaction::Version::non_standard(0),
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(50_000),
        }],
    };
    insert_tx(&mut wallet, tx);

    let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
    let root_id = external_policy.id;
    // child #0 is just the key "A"
    let path = vec![(root_id, vec![0])].into_iter().collect();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .policy_path(path, KeychainKind::External);
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFD));
}

#[test]
fn test_create_tx_policy_path_use_csv() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_a_or_b_plus_csv());

    let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
    let root_id = external_policy.id;
    // child #1 is or(pk(B),older(144))
    let path = vec![(root_id, vec![1])].into_iter().collect();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .policy_path(path, KeychainKind::External);
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(144));
}

#[test]
fn test_create_tx_policy_path_ignored_subtree_with_csv() {
    let (mut wallet, _) = get_funded_wallet_single("wsh(or_d(pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu),or_i(and_v(v:pkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(30)),and_v(v:pkh(cMnkdebixpXMPfkcNEjjGin7s94hiehAH4mLbYkZoh9KSiNNmqC8),older(90)))))");

    let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
    let root_id = external_policy.id;
    // child #0 is pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu)
    let path = vec![(root_id, vec![0])].into_iter().collect();

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(30_000))
        .policy_path(path, KeychainKind::External);
    let psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFD));
}

#[test]
fn test_create_tx_global_xpubs_with_origin() {
    use bitcoin::bip32;
    let (mut wallet, _) = get_funded_wallet_single("wpkh([73756c7f/48'/0'/0'/2']tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .add_global_xpubs();
    let psbt = builder.finish().unwrap();

    let key = bip32::Xpub::from_str("tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3").unwrap();
    let fingerprint = bip32::Fingerprint::from_hex("73756c7f").unwrap();
    let path = bip32::DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();

    assert_eq!(psbt.xpub.len(), 1);
    assert_eq!(psbt.xpub.get(&key), Some(&(fingerprint, path)));
}

#[test]
fn test_create_tx_increment_change_index() {
    // Test derivation index and unused index of change keychain when creating a transaction
    // Cases include wildcard and non-wildcard descriptors with and without an internal keychain
    // note the test assumes that the first external address is revealed since we're using
    // `receive_output`
    struct TestCase {
        name: &'static str,
        descriptor: &'static str,
        change_descriptor: Option<&'static str>,
        // amount to send
        to_send: u64,
        // (derivation index, next unused index) of *change keychain*
        expect: (Option<u32>, u32),
    }
    // total wallet funds
    let amount = 10_000;
    let recipient = Address::from_str("bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5")
        .unwrap()
        .assume_checked()
        .script_pubkey();
    let (desc, change_desc) = get_test_tr_single_sig_xprv_and_change_desc();
    [
        TestCase {
            name: "two wildcard, builder error",
            descriptor: desc,
            change_descriptor: Some(change_desc),
            to_send: amount + 1,
            // should not use or derive change index
            expect: (None, 0),
        },
        TestCase {
            name: "two wildcard, create change",
            descriptor: desc,
            change_descriptor: Some(change_desc),
            to_send: 5_000,
            // should use change index
            expect: (Some(0), 1),
        },
        TestCase {
            name: "two wildcard, no change",
            descriptor: desc,
            change_descriptor: Some(change_desc),
            to_send: 9_850,
            // should not use change index
            expect: (None, 0),
        },
        TestCase {
            name: "one wildcard, create change",
            descriptor: desc,
            change_descriptor: None,
            to_send: 5_000,
            // should use change index of external keychain
            expect: (Some(1), 2),
        },
        TestCase {
            name: "one wildcard, no change",
            descriptor: desc,
            change_descriptor: None,
            to_send: 9_850,
            // should not use change index
            expect: (Some(0), 1),
        },
        TestCase {
            name: "single key, create change",
            descriptor: get_test_tr_single_sig(),
            change_descriptor: None,
            to_send: 5_000,
            // single key only has one derivation index (0)
            expect: (Some(0), 0),
        },
        TestCase {
            name: "single key, no change",
            descriptor: get_test_tr_single_sig(),
            change_descriptor: None,
            to_send: 9_850,
            expect: (Some(0), 0),
        },
    ]
    .into_iter()
    .for_each(|test| {
        // create wallet
        let (params, change_keychain) = match test.change_descriptor {
            Some(change_desc) => (
                Wallet::create(test.descriptor, change_desc),
                KeychainKind::Internal,
            ),
            None => (
                Wallet::create_single(test.descriptor),
                KeychainKind::External,
            ),
        };
        let mut wallet = params
            .network(Network::Regtest)
            .create_wallet_no_persist()
            .unwrap();
        // fund wallet
        receive_output(&mut wallet, amount, ReceiveTo::Mempool(0));
        // create tx
        let mut builder = wallet.build_tx();
        builder.add_recipient(recipient.clone(), Amount::from_sat(test.to_send));
        let res = builder.finish();
        if !test.name.contains("error") {
            assert!(res.is_ok());
        }
        let (exp_derivation_index, exp_next_unused) = test.expect;
        assert_eq!(
            wallet.derivation_index(change_keychain),
            exp_derivation_index,
            "derivation index test {}",
            test.name,
        );
        assert_eq!(
            wallet.next_unused_address(change_keychain).index,
            exp_next_unused,
            "next unused index test {}",
            test.name,
        );
    });
}

#[test]
fn test_add_foreign_utxo() {
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, _) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo = wallet2.list_unspent().next().expect("must take!");
    let foreign_utxo_satisfaction = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let psbt_input = psbt::Input {
        witness_utxo: Some(utxo.txout.clone()),
        ..Default::default()
    };

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(60_000))
        .only_witness_utxo()
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let mut psbt = builder.finish().unwrap();
    wallet1.insert_txout(utxo.outpoint, utxo.txout);
    let fee = check_fee!(wallet1, psbt);
    let sent_received =
        wallet1.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));

    assert_eq!(
        (sent_received.0 - sent_received.1),
        Amount::from_sat(10_000) + fee.unwrap_or(Amount::ZERO),
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
fn test_calculate_fee_with_missing_foreign_utxo() {
    use bdk_chain::tx_graph::CalculateFeeError;
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, _) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo = wallet2.list_unspent().next().expect("must take!");
    let foreign_utxo_satisfaction = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let psbt_input = psbt::Input {
        witness_utxo: Some(utxo.txout.clone()),
        ..Default::default()
    };

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(60_000))
        .only_witness_utxo()
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let res = wallet1.calculate_fee(&tx);
    assert!(
        matches!(res, Err(CalculateFeeError::MissingTxOut(outpoints)) if outpoints[0] == utxo.outpoint)
    );
}

#[test]
fn test_add_foreign_utxo_invalid_psbt_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let outpoint = wallet.list_unspent().next().expect("must exist").outpoint;
    let foreign_utxo_satisfaction = wallet
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let mut builder = wallet.build_tx();
    let result =
        builder.add_foreign_utxo(outpoint, psbt::Input::default(), foreign_utxo_satisfaction);
    assert!(matches!(result, Err(AddForeignUtxoError::MissingUtxo)));
}

#[test]
fn test_add_foreign_utxo_where_outpoint_doesnt_match_psbt_input() {
    let (mut wallet1, txid1) = get_funded_wallet_wpkh();
    let (wallet2, txid2) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let utxo2 = wallet2.list_unspent().next().unwrap();
    let tx1 = wallet1.get_tx(txid1).unwrap().tx_node.tx.clone();
    let tx2 = wallet2.get_tx(txid2).unwrap().tx_node.tx.clone();

    let satisfaction_weight = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let mut builder = wallet1.build_tx();
    assert!(
        builder
            .add_foreign_utxo(
                utxo2.outpoint,
                psbt::Input {
                    non_witness_utxo: Some(tx1.as_ref().clone()),
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
                    non_witness_utxo: Some(tx2.as_ref().clone()),
                    ..Default::default()
                },
                satisfaction_weight
            )
            .is_ok(),
        "should be ok when outpoint does match psbt_input"
    );
}

#[test]
fn test_add_foreign_utxo_only_witness_utxo() {
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, txid2) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo2 = wallet2.list_unspent().next().unwrap();

    let satisfaction_weight = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    {
        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), Amount::from_sat(60_000));

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
        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), Amount::from_sat(60_000));

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
        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), Amount::from_sat(60_000));

        let tx2 = wallet2.get_tx(txid2).unwrap().tx_node.tx;
        let psbt_input = psbt::Input {
            non_witness_utxo: Some(tx2.as_ref().clone()),
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
    let (wallet, _) = get_funded_wallet_wpkh();
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
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .add_global_xpubs();
    builder.finish().unwrap();
}

#[test]
fn test_create_tx_global_xpubs_master_without_origin() {
    use bitcoin::bip32;
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tpubD6NzVbkrYhZ4Y55A58Gv9RSNF5hy84b5AJqYy7sCcjFrkcLpPre8kmgfit6kY1Zs3BLgeypTDBZJM222guPpdz7Cup5yzaMu62u7mYGbwFL/0/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(25_000))
        .add_global_xpubs();
    let psbt = builder.finish().unwrap();

    let key = bip32::Xpub::from_str("tpubD6NzVbkrYhZ4Y55A58Gv9RSNF5hy84b5AJqYy7sCcjFrkcLpPre8kmgfit6kY1Zs3BLgeypTDBZJM222guPpdz7Cup5yzaMu62u7mYGbwFL").unwrap();
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
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    builder.set_exact_sequence(Sequence(0xFFFFFFFE));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    wallet.build_fee_bump(txid).unwrap().finish().unwrap();
}

#[test]
#[should_panic(expected = "TransactionConfirmed")]
fn test_bump_fee_confirmed_tx() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();

    insert_tx(&mut wallet, tx);

    let anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().get(42).unwrap().block_id(),
        confirmation_time: 42_000,
    };
    insert_anchor(&mut wallet, txid, anchor);

    wallet.build_fee_bump(txid).unwrap().finish().unwrap();
}

#[test]
fn test_bump_fee_low_fee_rate() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));

    let psbt = builder.finish().unwrap();
    let feerate = psbt.fee_rate().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::BROADCAST_MIN);
    let res = builder.finish();
    assert_matches!(
        res,
        Err(CreateTxError::FeeRateTooLow { .. }),
        "expected FeeRateTooLow error"
    );

    let required = feerate.to_sat_per_kwu() + 250; // +1 sat/vb
    let sat_vb = required as f64 / 250.0;
    let expect = format!("Fee rate too low: required {} sat/vb", sat_vb);
    assert_eq!(res.unwrap_err().to_string(), expect);
}

#[test]
#[should_panic(expected = "FeeTooLow")]
fn test_bump_fee_low_abs() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::from_sat(10));
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "FeeTooLow")]
fn test_bump_fee_zero_abs() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::ZERO);
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_reduce_change() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();
    let original_sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let original_fee = check_fee!(wallet, psbt);

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let feerate = FeeRate::from_sat_per_kwu(625); // 2.5 sat/vb
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(feerate);
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert_eq!(
        sent_received.1 + fee.unwrap_or(Amount::ZERO),
        original_sent_received.1 + original_fee.unwrap_or(Amount::ZERO)
    );
    assert!(fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(25_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), feerate, @add_signature);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::from_sat(200));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert_eq!(
        sent_received.1 + fee.unwrap_or(Amount::ZERO),
        original_sent_received.1 + original_fee.unwrap_or(Amount::ZERO)
    );
    assert!(
        fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO),
        "{} > {}",
        fee.unwrap_or(Amount::ZERO),
        original_fee.unwrap_or(Amount::ZERO)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(25_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(200));
}

#[test]
fn test_bump_fee_reduce_single_recipient() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();
    let tx = psbt.clone().extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let original_fee = check_fee!(wallet, psbt);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let feerate = FeeRate::from_sat_per_kwu(625); // 2.5 sat/vb
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_rate(feerate)
        // remove original tx drain_to address and amount
        .set_recipients(Vec::new())
        // set back original drain_to address
        .drain_to(addr.script_pubkey())
        // drain wallet output amount will be re-calculated with new fee rate
        .drain_wallet();
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert!(fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output[0].value + fee.unwrap_or(Amount::ZERO),
        sent_received.0
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), feerate, @add_signature);
}

#[test]
fn test_bump_fee_absolute_reduce_single_recipient() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();
    let original_fee = check_fee!(wallet, psbt);
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_absolute(Amount::from_sat(300))
        // remove original tx drain_to address and amount
        .set_recipients(Vec::new())
        // set back original drain_to address
        .drain_to(addr.script_pubkey())
        // drain wallet output amount will be re-calculated with new fee rate
        .drain_wallet();
    let psbt = builder.finish().unwrap();
    let tx = &psbt.unsigned_tx;
    let sent_received = wallet.sent_and_received(tx);
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert!(fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO));

    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output[0].value + fee.unwrap_or(Amount::ZERO),
        sent_received.0
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(300));
}

#[test]
fn test_bump_fee_drain_wallet() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    // receive an extra tx so that our wallet has two utxos.
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx.clone());
    let anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().block_id(),
        confirmation_time: 42_000,
    };
    insert_anchor(&mut wallet, txid, anchor);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();

    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(OutPoint {
            txid: tx.compute_txid(),
            vout: 0,
        })
        .unwrap()
        .manually_selected_only();
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);

    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    assert_eq!(original_sent_received.0, Amount::from_sat(25_000));

    // for the new feerate, it should be enough to reduce the output, but since we specify
    // `drain_wallet` we expect to spend everything
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .drain_wallet()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(5));
    let psbt = builder.finish().unwrap();
    let sent_received = wallet.sent_and_received(&psbt.extract_tx().expect("failed to extract tx"));

    assert_eq!(sent_received.0, Amount::from_sat(75_000));
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_bump_fee_remove_output_manually_selected_only() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    // receive an extra tx so that our wallet has two utxos. then we manually pick only one of
    // them, and make sure that `bump_fee` doesn't try to add more. This fails because we've
    // told the wallet it's not allowed to add more inputs AND it can't reduce the value of the
    // existing output. In other words, bump_fee + manually_selected_only is always an error
    // unless there is a change output.
    let init_tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };

    let position: ChainPosition<ConfirmationBlockTime> =
        wallet.transactions().last().unwrap().chain_position;
    insert_tx(&mut wallet, init_tx.clone());
    match position {
        ChainPosition::Confirmed { anchor, .. } => {
            insert_anchor(&mut wallet, init_tx.compute_txid(), anchor)
        }
        other => panic!("all wallet txs must be confirmed: {:?}", other),
    }

    let outpoint = OutPoint {
        txid: init_tx.compute_txid(),
        vout: 0,
    };
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(outpoint)
        .unwrap()
        .manually_selected_only();
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    assert_eq!(original_sent_received.0, Amount::from_sat(25_000));

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .manually_selected_only()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(255));
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let init_tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };
    let txid = init_tx.compute_txid();
    let pos: ChainPosition<ConfirmationBlockTime> =
        wallet.transactions().last().unwrap().chain_position;
    insert_tx(&mut wallet, init_tx);
    match pos {
        ChainPosition::Confirmed { anchor, .. } => insert_anchor(&mut wallet, txid, anchor),
        other => panic!("all wallet txs must be confirmed: {:?}", other),
    }

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_details = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb_unchecked(50));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);
    assert_eq!(
        sent_received.0,
        original_details.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
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
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(50), @add_signature);
}

#[test]
fn test_bump_fee_absolute_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    receive_output_in_latest_block(&mut wallet, 25_000);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::from_sat(6_000));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
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
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(6_000));
}

#[test]
fn test_bump_fee_no_change_add_input_and_change() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let op = receive_output_in_latest_block(&mut wallet, 25_000);

    // initially make a tx without change by using `drain_to`
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(op)
        .unwrap()
        .manually_selected_only();
    let psbt = builder.finish().unwrap();
    let original_sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let original_fee = check_fee!(wallet, psbt);

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    // Now bump the fees, the wallet should add an extra input and a change output, and leave
    // the original output untouched.
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb_unchecked(50));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    let original_send_all_amount = original_sent_received.0 - original_fee.unwrap_or(Amount::ZERO);
    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(50_000)
    );
    assert_eq!(
        sent_received.1,
        Amount::from_sat(75_000) - original_send_all_amount - fee.unwrap_or(Amount::ZERO)
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
        Amount::from_sat(75_000) - original_send_all_amount - fee.unwrap_or(Amount::ZERO)
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(50), @add_signature);
}

#[test]
fn test_bump_fee_add_input_change_dust() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    receive_output_in_latest_block(&mut wallet, 25_000);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let original_sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let original_fee = check_fee!(wallet, psbt);

    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // to get realistic weight
    }
    let original_tx_weight = tx.weight();
    assert_eq!(tx.input.len(), 1);
    assert_eq!(tx.output.len(), 2);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    // We set a fee high enough that during rbf we are forced to add
    // a new input and also that we have to remove the change
    // that we had previously

    // We calculate the new weight as:
    //   original weight
    // + extra input weight: 160 WU = (32 (prevout) + 4 (vout) + 4 (nsequence)) * 4
    // + input satisfaction weight: 112 WU = 106 (witness) + 2 (witness len) + (1 (script len)) * 4
    // - change output weight: 124 WU = (8 (value) + 1 (script len) + 22 (script)) * 4
    let new_tx_weight =
        original_tx_weight + Weight::from_wu(160) + Weight::from_wu(112) - Weight::from_wu(124);
    // two inputs (50k, 25k) and one output (45k) - epsilon
    // We use epsilon here to avoid asking for a slightly too high feerate
    let fee_abs = 50_000 + 25_000 - 45_000 - 10;
    builder.fee_rate(Amount::from_sat(fee_abs) / new_tx_weight);
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        original_sent_received.1,
        Amount::from_sat(5_000) - original_fee.unwrap_or(Amount::ZERO)
    );

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(30_000));
    assert_eq!(sent_received.1, Amount::ZERO);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(45_000)
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(140), @dust_change, @add_signature);
}

#[test]
fn test_bump_fee_force_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let incoming_op = receive_output_in_latest_block(&mut wallet, 25_000);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx.clone());
    // the new fee_rate is low enough that just reducing the change would be fine, but we force
    // the addition of an extra input with `add_utxo()`
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .add_utxo(incoming_op)
        .unwrap()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(5));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
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
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(5), @add_signature);
}

#[test]
fn test_bump_fee_absolute_force_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let incoming_op = receive_output_in_latest_block(&mut wallet, 25_000);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    // skip saving the new utxos, we know they can't be used anyways
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx.clone());

    // the new fee_rate is low enough that just reducing the change would be fine, but we force
    // the addition of an extra input with `add_utxo()`
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .add_utxo(incoming_op)
        .unwrap()
        .fee_absolute(Amount::from_sat(250));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
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
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(250));
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
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.drain_wallet().drain_to(addr.script_pubkey());
    let psbt = builder.finish().unwrap();
    // Now we receive one transaction with 0 confirmations. We won't be able to use that for
    // fee bumping, as it's still unconfirmed!
    receive_output(&mut wallet, 25_000, ReceiveTo::Mempool(0));
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx);
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb_unchecked(25));
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_unconfirmed_input() {
    // We create a tx draining the wallet and spending one confirmed
    // and one unconfirmed UTXO. We check that we can fee bump normally
    // (BIP125 rule 2 only apply to newly added unconfirmed input, you can
    // always fee bump with an unconfirmed input if it was included in the
    // original transaction)
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    // We receive a tx with 0 confirmations, which will be used as an input
    // in the drain tx.
    receive_output(&mut wallet, 25_000, ReceiveTo::Mempool(0));
    let mut builder = wallet.build_tx();
    builder.drain_wallet().drain_to(addr.script_pubkey());
    let psbt = builder.finish().unwrap();
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(15))
        // remove original tx drain_to address and amount
        .set_recipients(Vec::new())
        // set back original drain_to address
        .drain_to(addr.script_pubkey())
        // drain wallet output amount will be re-calculated with new fee rate
        .drain_wallet();
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
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let send_to = Address::from_str("tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt")
        .unwrap()
        .assume_checked();
    let fee_rate = FeeRate::from_sat_per_kwu(500);
    let incoming_op = receive_output_in_latest_block(&mut wallet, 8859);

    let mut builder = wallet.build_tx();
    builder
        .add_recipient(send_to.script_pubkey(), Amount::from_sat(8630))
        .add_utxo(incoming_op)
        .unwrap()
        .fee_rate(fee_rate);
    let psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    assert_eq!(psbt.inputs.len(), 1);
    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), fee_rate, @add_signature);
}

#[test]
fn test_sign_single_xprv() {
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_with_master_fingerprint_and_path() {
    let (mut wallet, _) = get_funded_wallet_single("wpkh([d34db33f/84h/1h/0h]tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_bip44_path() {
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/44'/0'/0'/0/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_sh_wpkh() {
    let (mut wallet, _) = get_funded_wallet_single("sh(wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*))");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_wif() {
    let (mut wallet, _) =
        get_funded_wallet_single("wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_sign_single_xprv_no_hd_keypaths() {
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    psbt.inputs[0].bip32_derivation.clear();
    assert_eq!(psbt.inputs[0].bip32_derivation.len(), 0);

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(extracted.input[0].witness.len(), 2);
}

#[test]
fn test_include_output_redeem_witness_script() {
    let desc = get_test_wpkh();
    let change_desc = "sh(wsh(multi(1,cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW,cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu)))";
    let (mut wallet, _) = get_funded_wallet(desc, change_desc);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(45_000))
        .include_output_redeem_witness_script();
    let psbt = builder.finish().unwrap();

    // p2sh-p2wsh transaction should contain both witness and redeem scripts
    assert!(psbt
        .outputs
        .iter()
        .any(|output| output.redeem_script.is_some() && output.witness_script.is_some()));
}

#[test]
fn test_signing_only_one_of_multiple_inputs() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(45_000))
        .include_output_redeem_witness_script();
    let mut psbt = builder.finish().unwrap();

    // add another input to the psbt that is at least passable.
    let dud_input = bitcoin::psbt::Input {
        witness_utxo: Some(TxOut {
            value: Amount::from_sat(100_000),
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
fn test_try_finalize_sign_option() {
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");

    for try_finalize in &[true, false] {
        let addr = wallet.next_unused_address(KeychainKind::External);
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let mut psbt = builder.finish().unwrap();

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
                assert!(input.final_script_sig.is_none());
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
fn test_taproot_try_finalize_sign_option() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree());

    for try_finalize in &[true, false] {
        let addr = wallet.next_unused_address(KeychainKind::External);
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let mut psbt = builder.finish().unwrap();

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
                assert!(input.final_script_sig.is_none());
                assert!(input.final_script_witness.is_some());
                assert!(input.tap_key_sig.is_none());
                assert!(input.tap_script_sigs.is_empty());
                assert!(input.tap_scripts.is_empty());
                assert!(input.tap_key_origins.is_empty());
                assert!(input.tap_internal_key.is_none());
                assert!(input.tap_merkle_root.is_none());
            } else {
                assert!(!finalized);
                assert!(input.final_script_sig.is_none());
                assert!(input.final_script_witness.is_none());
            }
        });
        psbt.outputs.iter().for_each(|output| {
            if *try_finalize {
                assert!(finalized);
                assert!(output.tap_key_origins.is_empty());
            } else {
                assert!(!finalized);
                assert!(!output.tap_key_origins.is_empty());
            }
        });
    }
}

#[test]
fn test_sign_nonstandard_sighash() {
    let sighash = EcdsaSighashType::NonePlusAnyoneCanPay;

    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .sighash(sighash.into())
        .drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(
        result.is_err(),
        "Signing should have failed because the TX uses non-standard sighashes"
    );
    assert_matches!(
        result,
        Err(SignerError::NonStandardSighash),
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

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(
        *extracted.input[0].witness.to_vec()[0].last().unwrap(),
        sighash.to_u32() as u8,
        "The signature should have been made with the right sighash"
    );
}

#[test]
fn test_unused_address() {
    let descriptor = "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)";
    let change_descriptor = get_test_wpkh();
    let mut wallet = Wallet::create(descriptor, change_descriptor)
        .network(Network::Testnet)
        .create_wallet_no_persist()
        .expect("wallet");

    // `list_unused_addresses` should be empty if we haven't revealed any
    assert!(wallet
        .list_unused_addresses(KeychainKind::External)
        .next()
        .is_none());

    assert_eq!(
        wallet
            .next_unused_address(KeychainKind::External)
            .to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
    assert_eq!(
        wallet
            .list_unused_addresses(KeychainKind::External)
            .next()
            .unwrap()
            .to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
}

#[test]
fn test_next_unused_address() {
    let descriptor = "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)";
    let change_descriptor = get_test_wpkh();
    let mut wallet = Wallet::create(descriptor, change_descriptor)
        .network(Network::Testnet)
        .create_wallet_no_persist()
        .expect("wallet");
    assert_eq!(wallet.derivation_index(KeychainKind::External), None);

    assert_eq!(
        wallet
            .next_unused_address(KeychainKind::External)
            .to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(0));
    // calling next_unused again gives same address
    assert_eq!(
        wallet
            .next_unused_address(KeychainKind::External)
            .to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(0));

    // test mark used / unused
    assert!(wallet.mark_used(KeychainKind::External, 0));
    let next_unused_addr = wallet.next_unused_address(KeychainKind::External);
    assert_eq!(next_unused_addr.index, 1);

    assert!(wallet.unmark_used(KeychainKind::External, 0));
    let next_unused_addr = wallet.next_unused_address(KeychainKind::External);
    assert_eq!(next_unused_addr.index, 0);

    // use the above address
    receive_output(&mut wallet, 25_000, ReceiveTo::Mempool(0));

    assert_eq!(
        wallet
            .next_unused_address(KeychainKind::External)
            .to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(1));

    // trying to mark index 0 unused should return false
    assert!(!wallet.unmark_used(KeychainKind::External, 0));
}

#[test]
fn test_peek_address_at_index() {
    let descriptor = "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)";
    let change_descriptor = get_test_wpkh();
    let mut wallet = Wallet::create(descriptor, change_descriptor)
        .network(Network::Testnet)
        .create_wallet_no_persist()
        .expect("wallet");

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 1).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 0).to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 2).to_string(),
        "tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2"
    );

    // current new address is not affected
    assert_eq!(
        wallet
            .reveal_next_address(KeychainKind::External)
            .to_string(),
        "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
    );

    assert_eq!(
        wallet
            .reveal_next_address(KeychainKind::External)
            .to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );
}

#[test]
fn test_peek_address_at_index_not_derivable() {
    let descriptor = "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/1)";
    let wallet = Wallet::create(descriptor, get_test_wpkh())
        .network(Network::Testnet)
        .create_wallet_no_persist()
        .unwrap();

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 1).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 0).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 2).to_string(),
        "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
    );
}

#[test]
fn test_returns_index_and_address() {
    let descriptor =
        "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)";
    let mut wallet = Wallet::create(descriptor, get_test_wpkh())
        .network(Network::Testnet)
        .create_wallet_no_persist()
        .unwrap();

    // new index 0
    assert_eq!(
        wallet.reveal_next_address(KeychainKind::External),
        AddressInfo {
            index: 0,
            address: Address::from_str("tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a")
                .unwrap()
                .assume_checked(),
            keychain: KeychainKind::External,
        }
    );

    // new index 1
    assert_eq!(
        wallet.reveal_next_address(KeychainKind::External),
        AddressInfo {
            index: 1,
            address: Address::from_str("tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7")
                .unwrap()
                .assume_checked(),
            keychain: KeychainKind::External,
        }
    );

    // peek index 25
    assert_eq!(
        wallet.peek_address(KeychainKind::External, 25),
        AddressInfo {
            index: 25,
            address: Address::from_str("tb1qsp7qu0knx3sl6536dzs0703u2w2ag6ppl9d0c2")
                .unwrap()
                .assume_checked(),
            keychain: KeychainKind::External,
        }
    );

    // new index 2
    assert_eq!(
        wallet.reveal_next_address(KeychainKind::External),
        AddressInfo {
            index: 2,
            address: Address::from_str("tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2")
                .unwrap()
                .assume_checked(),
            keychain: KeychainKind::External,
        }
    );
}

#[test]
fn test_sending_to_bip350_bech32m_address() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("tb1pqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesf3hn0c")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    builder.finish().unwrap();
}

#[test]
fn test_get_address() {
    use bdk_wallet::descriptor::template::Bip84;
    let key = bitcoin::bip32::Xpriv::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
    let wallet = Wallet::create(
        Bip84(key, KeychainKind::External),
        Bip84(key, KeychainKind::Internal),
    )
    .network(Network::Regtest)
    .create_wallet_no_persist()
    .unwrap();

    assert_eq!(
        wallet.peek_address(KeychainKind::External, 0),
        AddressInfo {
            index: 0,
            address: Address::from_str("bcrt1qrhgaqu0zvf5q2d0gwwz04w0dh0cuehhqvzpp4w")
                .unwrap()
                .assume_checked(),
            keychain: KeychainKind::External,
        }
    );

    assert_eq!(
        wallet.peek_address(KeychainKind::Internal, 0),
        AddressInfo {
            index: 0,
            address: Address::from_str("bcrt1q0ue3s5y935tw7v3gmnh36c5zzsaw4n9c9smq79")
                .unwrap()
                .assume_checked(),
            keychain: KeychainKind::Internal,
        }
    );
}

#[test]
fn test_reveal_addresses() {
    let (desc, change_desc) = get_test_tr_single_sig_xprv_and_change_desc();
    let mut wallet = Wallet::create(desc, change_desc)
        .network(Network::Signet)
        .create_wallet_no_persist()
        .unwrap();
    let keychain = KeychainKind::External;

    let last_revealed_addr = wallet.reveal_addresses_to(keychain, 9).last().unwrap();
    assert_eq!(wallet.derivation_index(keychain), Some(9));

    let unused_addrs = wallet.list_unused_addresses(keychain).collect::<Vec<_>>();
    assert_eq!(unused_addrs.len(), 10);
    assert_eq!(unused_addrs.last().unwrap(), &last_revealed_addr);

    // revealing to an already revealed index returns nothing
    let mut already_revealed = wallet.reveal_addresses_to(keychain, 9);
    assert!(already_revealed.next().is_none());
}

#[test]
fn test_get_address_no_reuse() {
    use bdk_wallet::descriptor::template::Bip84;
    use std::collections::HashSet;

    let key = bitcoin::bip32::Xpriv::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
    let mut wallet = Wallet::create(
        Bip84(key, KeychainKind::External),
        Bip84(key, KeychainKind::Internal),
    )
    .network(Network::Regtest)
    .create_wallet_no_persist()
    .unwrap();

    let mut used_set = HashSet::new();

    (0..3).for_each(|_| {
        let external_addr = wallet.reveal_next_address(KeychainKind::External).address;
        assert!(used_set.insert(external_addr));

        let internal_addr = wallet.reveal_next_address(KeychainKind::Internal).address;
        assert!(used_set.insert(internal_addr));
    });
}

#[test]
fn test_taproot_psbt_populate_tap_key_origins() {
    let (desc, change_desc) = get_test_tr_single_sig_xprv_and_change_desc();
    let (mut wallet, _) = get_funded_wallet(desc, change_desc);
    let addr = wallet.reveal_next_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0]
            .tap_key_origins
            .clone()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![(
            from_str!("0841db1dbaf949dbbda893e01a18f2cca9179cf8ea2d8e667857690502b06483"),
            (vec![], (from_str!("f6a5cb8b"), from_str!("m/0/0")))
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
            from_str!("9187c1e80002d19ddde9c5c7f5394e9a063cee8695867b58815af0562695ca21"),
            (vec![], (from_str!("f6a5cb8b"), from_str!("m/0/1")))
        )],
        "Wrong output tap_key_origins"
    );
}

#[test]
fn test_taproot_psbt_populate_tap_key_origins_repeated_key() {
    let (mut wallet, _) = get_funded_wallet(get_test_tr_repeated_key(), get_test_tr_single_sig());
    let addr = wallet.reveal_next_address(KeychainKind::External);

    let path = vec![("rn4nre9c".to_string(), vec![0])]
        .into_iter()
        .collect();

    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .policy_path(path, KeychainKind::External);
    let psbt = builder.finish().unwrap();

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
            ),
            (
                from_str!("b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55"),
                (
                    vec![],
                    (FromStr::from_str("871fd295").unwrap(), vec![].into())
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
    use bitcoin::hex::FromHex;
    use bitcoin::taproot;

    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree());
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();

    assert_eq!(
        psbt.inputs[0].tap_merkle_root,
        Some(
            TapNodeHash::from_str(
                "61f81509635053e52d9d1217545916167394490da2287aca4693606e43851986"
            )
            .unwrap()
        ),
    );
    assert_eq!(
        psbt.inputs[0].tap_scripts.clone().into_iter().collect::<Vec<_>>(),
        vec![
            (taproot::ControlBlock::decode(&Vec::<u8>::from_hex("c0b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55b7ef769a745e625ed4b9a4982a4dc08274c59187e73e6f07171108f455081cb2").unwrap()).unwrap(), (ScriptBuf::from_hex("208aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642ac").unwrap(), taproot::LeafVersion::TapScript)),
            (taproot::ControlBlock::decode(&Vec::<u8>::from_hex("c0b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55b9a515f7be31a70186e3c5937ee4a70cc4b4e1efe876c1d38e408222ffc64834").unwrap()).unwrap(), (ScriptBuf::from_hex("2051494dc22e24a32fe9dcfbd7e85faf345fa1df296fb49d156e859ef345201295ac").unwrap(), taproot::LeafVersion::TapScript)),
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

    let tap_tree: bitcoin::taproot::TapTree = serde_json::from_str(r#"[1,{"Script":["2051494dc22e24a32fe9dcfbd7e85faf345fa1df296fb49d156e859ef345201295ac",192]},1,{"Script":["208aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642ac",192]}]"#).unwrap();
    assert_eq!(psbt.outputs[0].tap_tree, Some(tap_tree));
}

#[test]
fn test_taproot_sign_missing_witness_utxo() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_single_sig());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();
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
        Err(SignerError::MissingWitnessUtxo),
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
    let (mut wallet, prev_txid) = get_funded_wallet_single(get_test_tr_single_sig());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();

    psbt.inputs[0].witness_utxo = None;
    psbt.inputs[0].non_witness_utxo =
        Some(wallet.get_tx(prev_txid).unwrap().tx_node.as_ref().clone());
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
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, _) = get_funded_wallet_single(get_test_tr_single_sig());

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo = wallet2.list_unspent().next().unwrap();
    let psbt_input = wallet2.get_psbt_input(utxo.clone(), None, false).unwrap();
    let foreign_utxo_satisfaction = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    assert!(
        psbt_input.non_witness_utxo.is_none(),
        "`non_witness_utxo` should never be populated for taproot"
    );

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(60_000))
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet1.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    wallet1.insert_txout(utxo.outpoint, utxo.txout);
    let fee = check_fee!(wallet1, psbt);

    assert_eq!(
        sent_received.0 - sent_received.1,
        Amount::from_sat(10_000) + fee.unwrap_or(Amount::ZERO),
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
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();

    assert_eq!(psbt.unsigned_tx.version.0, 2);
    assert!(
        wallet.sign(&mut psbt, Default::default()).unwrap(),
        "Unable to finalize tx"
    );
}

//     #[test]
//     fn test_taproot_key_spend() {
//         let (mut wallet, _) = get_funded_wallet_single(get_test_tr_single_sig());
//         test_spend_from_wallet(wallet);

//         let (mut wallet, _) = get_funded_wallet_single(get_test_tr_single_sig_xprv());
//         test_spend_from_wallet(wallet);
//     }

#[test]
fn test_taproot_no_key_spend() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree_both_priv());
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();

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
    let (wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree());
    test_spend_from_wallet(wallet);

    let (wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree_xprv());
    test_spend_from_wallet(wallet);
}

#[test]
fn test_taproot_script_spend_sign_all_leaves() {
    use bdk_wallet::signer::TapLeavesOptions;
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree_both_priv());
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();

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
    use bdk_wallet::signer::TapLeavesOptions;
    use bitcoin::taproot::TapLeafHash;

    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree_both_priv());
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();
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
    use bdk_wallet::signer::TapLeavesOptions;
    use bitcoin::taproot::TapLeafHash;

    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree_both_priv());
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();
    let mut script_leaves: Vec<_> = psbt.inputs[0]
        .tap_scripts
        .clone()
        .values()
        .map(|(script, version)| TapLeafHash::from_script(script, *version))
        .collect();
    let included_script_leaves = [script_leaves.pop().unwrap()];
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
    use bdk_wallet::signer::TapLeavesOptions;
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_with_taptree_both_priv());
    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();

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
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_single_sig_xprv());

    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let mut psbt = builder.finish().unwrap();

    // re-create the wallet with an empty db
    let wallet_empty = Wallet::create(get_test_tr_single_sig_xprv(), get_test_tr_single_sig())
        .network(Network::Regtest)
        .create_wallet_no_persist()
        .unwrap();

    // signing with an empty db means that we will only look at the psbt to infer the
    // derivation index
    assert!(
        wallet_empty.sign(&mut psbt, Default::default()).unwrap(),
        "Unable to finalize tx"
    );
}

#[test]
fn test_taproot_sign_explicit_sighash_all() {
    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_single_sig());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .sighash(TapSighashType::All.into())
        .drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(
        result.is_ok(),
        "Signing should work because SIGHASH_ALL is safe"
    )
}

#[test]
fn test_taproot_sign_non_default_sighash() {
    let sighash = TapSighashType::NonePlusAnyoneCanPay;

    let (mut wallet, _) = get_funded_wallet_single(get_test_tr_single_sig());
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .sighash(sighash.into())
        .drain_wallet();
    let mut psbt = builder.finish().unwrap();

    let witness_utxo = psbt.inputs[0].witness_utxo.take();

    let result = wallet.sign(&mut psbt, Default::default());
    assert!(
        result.is_err(),
        "Signing should have failed because the TX uses non-standard sighashes"
    );
    assert_matches!(
        result,
        Err(SignerError::NonStandardSighash),
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
        Err(SignerError::MissingWitnessUtxo),
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

    let extracted = psbt.extract_tx().expect("failed to extract tx");
    assert_eq!(
        *extracted.input[0].witness.to_vec()[0].last().unwrap(),
        sighash as u8,
        "The signature should have been made with the right sighash"
    );
}

#[test]
fn test_spend_coinbase() {
    let (desc, change_desc) = get_test_wpkh_and_change_desc();
    let mut wallet = Wallet::create(desc, change_desc)
        .network(Network::Regtest)
        .create_wallet_no_persist()
        .unwrap();

    let confirmation_height = 5;
    let confirmation_block_id = BlockId {
        height: confirmation_height,
        hash: BlockHash::all_zeros(),
    };
    insert_checkpoint(&mut wallet, confirmation_block_id);
    let coinbase_tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };
    let txid = coinbase_tx.compute_txid();
    insert_tx(&mut wallet, coinbase_tx);
    let anchor = ConfirmationBlockTime {
        block_id: confirmation_block_id,
        confirmation_time: 30_000,
    };
    insert_anchor(&mut wallet, txid, anchor);

    // NOTE: A transaction spending an output coming from the coinbase tx at height h, is eligible
    // to be included in block h + [100 = COINBASE_MATURITY] or higher.
    // Tx elibible to be included in the next block will be accepted in the mempool, used in block
    // templates and relayed on the network.
    // Miners may include such tx in a block when their chaintip is at h + [99 = COINBASE_MATURITY - 1].
    // This means these coins are available for selection at height h + 99.
    //
    // By https://bitcoin.stackexchange.com/a/119017
    let not_yet_mature_time = confirmation_height + COINBASE_MATURITY - 2;
    let maturity_time = confirmation_height + COINBASE_MATURITY - 1;

    let balance = wallet.balance();
    assert_eq!(
        balance,
        Balance {
            immature: Amount::from_sat(25_000),
            trusted_pending: Amount::ZERO,
            untrusted_pending: Amount::ZERO,
            confirmed: Amount::ZERO
        }
    );

    // We try to create a transaction, only to notice that all
    // our funds are unspendable
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), balance.immature / 2)
        .current_height(confirmation_height);
    assert!(matches!(
        builder.finish(),
        Err(CreateTxError::CoinSelection(
            coin_selection::InsufficientFunds {
                needed: _,
                available: Amount::ZERO
            }
        ))
    ));

    // Still unspendable...
    let mut builder = wallet.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), balance.immature / 2)
        .current_height(not_yet_mature_time);
    assert_matches!(
        builder.finish(),
        Err(CreateTxError::CoinSelection(
            coin_selection::InsufficientFunds {
                needed: _,
                available: Amount::ZERO
            }
        ))
    );

    insert_checkpoint(
        &mut wallet,
        BlockId {
            height: maturity_time,
            hash: BlockHash::all_zeros(),
        },
    );
    let balance = wallet.balance();
    assert_eq!(
        balance,
        Balance {
            immature: Amount::ZERO,
            trusted_pending: Amount::ZERO,
            untrusted_pending: Amount::ZERO,
            confirmed: Amount::from_sat(25_000)
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
    let (mut wallet, _) = get_funded_wallet_single(get_test_single_sig_cltv());

    let addr = wallet.next_unused_address(KeychainKind::External);

    let mut builder = wallet.build_tx();

    builder.add_recipient(addr.script_pubkey(), Amount::ZERO);

    assert_matches!(
        builder.finish(),
        Err(CreateTxError::OutputBelowDustLimit(0))
    );

    let mut builder = wallet.build_tx();

    builder
        .allow_dust(true)
        .add_recipient(addr.script_pubkey(), Amount::ZERO);

    assert!(builder.finish().is_ok());
}

#[test]
fn test_fee_rate_sign_no_grinding_high_r() {
    // Our goal is to obtain a transaction with a signature with high-R (71 bytes
    // instead of 70). We then check that our fee rate and fee calculation is
    // alright.
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let fee_rate = FeeRate::from_sat_per_vb_unchecked(1);
    let mut builder = wallet.build_tx();
    let mut data = PushBytesBuf::try_from(vec![0]).unwrap();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_rate(fee_rate)
        .add_data(&data);
    let mut psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);
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
        data.as_mut_bytes()[0] += 1;
        psbt.unsigned_tx.output[op_return_vout].script_pubkey = ScriptBuf::new_op_return(&data);
        // Clearing the previous signature
        psbt.inputs[0].partial_sigs.clear();
        // Signing
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    try_finalize: false,
                    allow_grinding: false,
                    ..Default::default()
                },
            )
            .unwrap();
        // We only have one key in the partial_sigs map, this is a trick to retrieve it
        let key = psbt.inputs[0].partial_sigs.keys().next().unwrap();
        sig_len = psbt.inputs[0].partial_sigs[key]
            .signature
            .serialize_der()
            .len();
    }
    // Actually finalizing the transaction...
    wallet
        .sign(
            &mut psbt,
            SignOptions {
                allow_grinding: false,
                ..Default::default()
            },
        )
        .unwrap();
    // ...and checking that everything is fine
    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), fee_rate);
}

#[test]
fn test_fee_rate_sign_grinding_low_r() {
    // Our goal is to obtain a transaction with a signature with low-R (70 bytes)
    // by setting the `allow_grinding` signing option as true.
    // We then check that our fee rate and fee calculation is alright and that our
    // signature is 70 bytes.
    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.next_unused_address(KeychainKind::External);
    let fee_rate = FeeRate::from_sat_per_vb_unchecked(1);
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .drain_wallet()
        .fee_rate(fee_rate);
    let mut psbt = builder.finish().unwrap();
    let fee = check_fee!(wallet, psbt);

    wallet
        .sign(
            &mut psbt,
            SignOptions {
                try_finalize: false,
                allow_grinding: true,
                ..Default::default()
            },
        )
        .unwrap();

    let key = psbt.inputs[0].partial_sigs.keys().next().unwrap();
    let sig_len = psbt.inputs[0].partial_sigs[key]
        .signature
        .serialize_der()
        .len();
    assert_eq!(sig_len, 70);
    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), fee_rate);
}

#[test]
fn test_taproot_load_descriptor_duplicated_keys() {
    // Added after issue https://github.com/bitcoindevkit/bdk/issues/760
    //
    // Having the same key in multiple taproot leaves is safe and should be accepted by BDK

    let (wallet, _) = get_funded_wallet_single(get_test_tr_dup_keys());
    let addr = wallet.peek_address(KeychainKind::External, 0);

    assert_eq!(
        addr.to_string(),
        "bcrt1pvysh4nmh85ysrkpwtrr8q8gdadhgdejpy6f9v424a8v9htjxjhyqw9c5s5"
    );
}

/// In dev mode this test panics, but in release mode, or if the `debug_panic` in `TxOutIndex::replenish_inner_index`
/// is commented out, there is no panic and the balance is calculated correctly. See issue [#1483]
/// and PR [#1486] for discussion on mixing non-wildcard and wildcard descriptors.
///
/// [#1483]: https://github.com/bitcoindevkit/bdk/issues/1483
/// [#1486]: https://github.com/bitcoindevkit/bdk/pull/1486
#[test]
#[cfg(debug_assertions)]
#[should_panic(
    expected = "replenish lookahead: must not have existing spk: keychain=Internal, lookahead=25, next_store_index=0, next_reveal_index=0"
)]
fn test_keychains_with_overlapping_spks() {
    // this can happen if a non-wildcard descriptor keychain derives an spk that a
    // wildcard descriptor keychain in the same wallet also derives.

    // index 1 spk overlaps with non-wildcard change descriptor
    let wildcard_keychain = "wpkh(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*)";
    let non_wildcard_keychain = "wpkh(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/1)";

    let (mut wallet, _) = get_funded_wallet(wildcard_keychain, non_wildcard_keychain);
    assert_eq!(wallet.balance().confirmed, Amount::from_sat(50000));

    let addr = wallet
        .reveal_addresses_to(KeychainKind::External, 1)
        .last()
        .unwrap()
        .address;
    let anchor = ConfirmationBlockTime {
        block_id: BlockId {
            height: 2000,
            hash: BlockHash::all_zeros(),
        },
        confirmation_time: 0,
    };
    let _outpoint = receive_output_to_address(&mut wallet, addr, 8000, anchor);
    assert_eq!(wallet.balance().confirmed, Amount::from_sat(58000));
}

#[test]
/// The wallet should re-use previously allocated change addresses when the tx using them is cancelled
fn test_tx_cancellation() {
    macro_rules! new_tx {
        ($wallet:expr) => {{
            let addr = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt")
                .unwrap()
                .assume_checked();
            let mut builder = $wallet.build_tx();
            builder.add_recipient(addr.script_pubkey(), Amount::from_sat(10_000));

            let psbt = builder.finish().unwrap();

            psbt
        }};
    }

    let (mut wallet, _) = get_funded_wallet(get_test_wpkh(), get_test_tr_single_sig_xprv());

    let psbt1 = new_tx!(wallet);
    let change_derivation_1 = psbt1
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(txout.script_pubkey.clone()))
        .unwrap();
    assert_eq!(change_derivation_1, (KeychainKind::Internal, 0));

    let psbt2 = new_tx!(wallet);

    let change_derivation_2 = psbt2
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(txout.script_pubkey.clone()))
        .unwrap();
    assert_eq!(change_derivation_2, (KeychainKind::Internal, 1));

    wallet.cancel_tx(&psbt1.extract_tx().expect("failed to extract tx"));

    let psbt3 = new_tx!(wallet);
    let change_derivation_3 = psbt3
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(txout.script_pubkey.clone()))
        .unwrap();
    assert_eq!(change_derivation_3, (KeychainKind::Internal, 0));

    let psbt3 = new_tx!(wallet);
    let change_derivation_3 = psbt3
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(txout.script_pubkey.clone()))
        .unwrap();
    assert_eq!(change_derivation_3, (KeychainKind::Internal, 2));

    wallet.cancel_tx(&psbt3.extract_tx().expect("failed to extract tx"));

    let psbt3 = new_tx!(wallet);
    let change_derivation_4 = psbt3
        .unsigned_tx
        .output
        .iter()
        .find_map(|txout| wallet.derivation_of_spk(txout.script_pubkey.clone()))
        .unwrap();
    assert_eq!(change_derivation_4, (KeychainKind::Internal, 2));
}

#[test]
fn test_thread_safety() {
    fn thread_safe<T: Send + Sync>() {}
    thread_safe::<Wallet>(); // compiles only if true
    thread_safe::<PersistedWallet<bdk_chain::rusqlite::Connection>>();
}

#[test]
fn single_descriptor_wallet_can_create_tx_and_receive_change() {
    // create single descriptor wallet and fund it
    let mut wallet = Wallet::create_single(get_test_tr_single_sig_xprv())
        .network(Network::Testnet)
        .create_wallet_no_persist()
        .unwrap();
    assert_eq!(wallet.keychains().count(), 1);
    let amt = Amount::from_sat(5_000);
    receive_output(&mut wallet, 2 * amt.to_sat(), ReceiveTo::Mempool(2));
    // create spend tx that produces a change output
    let addr = Address::from_str("bcrt1qc6fweuf4xjvz4x3gx3t9e0fh4hvqyu2qw4wvxm")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), amt);
    let mut psbt = builder.finish().unwrap();
    assert!(wallet.sign(&mut psbt, SignOptions::default()).unwrap());
    let tx = psbt.extract_tx().unwrap();
    let _txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    let unspent: Vec<_> = wallet.list_unspent().collect();
    assert_eq!(unspent.len(), 1);
    let utxo = unspent.first().unwrap();
    assert!(utxo.txout.value < amt);
    assert_eq!(
        utxo.keychain,
        KeychainKind::External,
        "tx change should go to external keychain"
    );
}

#[test]
fn test_transactions_sort_by() {
    let (mut wallet, _txid) = get_funded_wallet_wpkh();
    receive_output(&mut wallet, 25_000, ReceiveTo::Mempool(0));

    // sort by chain position, unconfirmed then confirmed by descending block height
    let sorted_txs: Vec<WalletTx> =
        wallet.transactions_sort_by(|t1, t2| t2.chain_position.cmp(&t1.chain_position));
    let conf_heights: Vec<Option<u32>> = sorted_txs
        .iter()
        .map(|tx| tx.chain_position.confirmation_height_upper_bound())
        .collect();
    assert_eq!([None, Some(2000), Some(1000)], conf_heights.as_slice());
}

#[test]
fn test_tx_builder_is_send_safe() {
    let (mut wallet, _txid) = get_funded_wallet_wpkh();
    let _box: Box<dyn Send + Sync> = Box::new(wallet.build_tx());
}

#[test]
fn test_wallet_transactions_relevant() {
    let (mut test_wallet, _txid) = get_funded_wallet_wpkh();
    let relevant_tx_count_before = test_wallet.transactions().count();
    let full_tx_count_before = test_wallet.tx_graph().full_txs().count();
    let chain_tip = test_wallet.local_chain().tip().block_id();
    let canonical_tx_count_before = test_wallet
        .tx_graph()
        .list_canonical_txs(test_wallet.local_chain(), chain_tip)
        .count();

    // add not relevant transaction to test wallet
    let (other_external_desc, other_internal_desc) = get_test_tr_single_sig_xprv_and_change_desc();
    let (other_wallet, other_txid) = get_funded_wallet(other_internal_desc, other_external_desc);
    let test_wallet_update = Update {
        tx_update: other_wallet.tx_graph().clone().into(),
        ..Default::default()
    };
    test_wallet.apply_update(test_wallet_update).unwrap();

    // verify transaction from other wallet was added but is not in relevant transactions list.
    let relevant_tx_count_after = test_wallet.transactions().count();
    let full_tx_count_after = test_wallet.tx_graph().full_txs().count();
    let canonical_tx_count_after = test_wallet
        .tx_graph()
        .list_canonical_txs(test_wallet.local_chain(), chain_tip)
        .count();

    assert_eq!(relevant_tx_count_before, relevant_tx_count_after);
    assert!(!test_wallet
        .transactions()
        .any(|wallet_tx| wallet_tx.tx_node.txid == other_txid));
    assert!(test_wallet
        .tx_graph()
        .list_canonical_txs(test_wallet.local_chain(), chain_tip)
        .any(|wallet_tx| wallet_tx.tx_node.txid == other_txid));
    assert!(full_tx_count_before < full_tx_count_after);
    assert!(canonical_tx_count_before < canonical_tx_count_after);
}
