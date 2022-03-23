// Bitcoin Dev Kit
// Written in 2022 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2022 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use bitcoin::blockdata::script::Instruction;
use bitcoin::consensus::encode::serialize;
use bitcoin::hashes::{sha256, sha512, Hmac, HmacEngine};
use bitcoin::secp256k1::ecdh::SharedSecret;
use bitcoin::secp256k1::key::{PublicKey, SecretKey};
use bitcoin::util::base58;
use bitcoin::util::bip32;
use bitcoin::util::psbt;
use bitcoin::{Address, Network, OutPoint, Script, Transaction, TxIn, Txid};

use crate::blockchain::BlockchainFactory;
use crate::database::{BatchDatabase, MemoryDatabase};
use crate::descriptor::template::{DescriptorTemplate, DescriptorTemplateOut, P2Pkh};
use crate::descriptor::{DescriptorError, Legacy};
use crate::keys::{DerivableKey, DescriptorSecretKey, DescriptorSinglePriv, ExtendedKey};
use crate::wallet::coin_selection::DefaultCoinSelectionAlgorithm;
use crate::wallet::tx_builder::{CreateTx, TxBuilder, TxOrdering};
use crate::wallet::utils::SecpCtx;
use crate::wallet::{AddressIndex, SyncOptions, Wallet};
use crate::{Error as WalletError, KeychainKind, LocalUtxo, TransactionDetails};

#[derive(Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
pub struct PaymentCode {
    pub version: u8,
    pub features: u8,
    pub public_key: PublicKey,
    pub chain_code: bip32::ChainCode,
}

impl PaymentCode {
    pub fn decode(data: &[u8]) -> Result<PaymentCode, Error> {
        if data.len() != 80 {
            return Err(Error::WrongDataLength(data.len()));
        }

        let version = data[0];
        if version != 0x01 {
            return Err(Error::UnknownVersion(version));
        }
        let features = data[1];
        let sign = data[2];
        if sign != 0x02 && sign != 0x03 {
            return Err(Error::InvalidPublicKeySign(sign));
        }

        Ok(PaymentCode {
            version,
            features,
            public_key: PublicKey::from_slice(&data[2..35])?,
            chain_code: bip32::ChainCode::from(&data[35..67]),
        })
    }

    pub fn decode_blinded(
        data: &[u8],
        blinding_factor: BlindingFactor,
    ) -> Result<PaymentCode, Error> {
        let mut data = data.to_vec();

        for (a, b) in data[3..68].iter_mut().zip(&blinding_factor[..]) {
            *a ^= b;
        }

        Self::decode(&data)
    }

    pub fn encode(&self) -> [u8; 80] {
        let mut ret = [0; 80];
        ret[0] = self.version;
        ret[1] = self.features;
        ret[2..35].copy_from_slice(&self.public_key.serialize()[..]);
        ret[35..67].copy_from_slice(&self.chain_code[..]);
        ret[67..80].copy_from_slice(&[0; 13]);
        ret
    }

    pub fn encode_blinded(&self, blinding_factor: BlindingFactor) -> [u8; 80] {
        let mut encoded = self.encode();

        for (a, b) in encoded[3..68].iter_mut().zip(&blinding_factor[..]) {
            *a ^= b;
        }

        encoded
    }

    pub fn notification_address(&self, secp: &SecpCtx, network: Network) -> Address {
        Address::p2pkh(
            &bitcoin::PublicKey {
                compressed: true,
                key: self.derive(secp, 0),
            },
            network,
        )
    }

    pub fn derive(&self, secp: &SecpCtx, index: u32) -> PublicKey {
        self.to_xpub()
            .derive_pub(secp, &vec![bip32::ChildNumber::Normal { index }])
            .expect("Normal derivation should work")
            .public_key
            .key
    }

    fn to_xpub(&self) -> bip32::ExtendedPubKey {
        bip32::ExtendedPubKey {
            network: Network::Bitcoin,
            depth: 0,
            parent_fingerprint: bip32::Fingerprint::default(),
            child_number: bip32::ChildNumber::Normal { index: 0 },
            public_key: bitcoin::PublicKey {
                compressed: true,
                key: self.public_key,
            },
            chain_code: self.chain_code,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlindingFactor([u8; 64]);

impl BlindingFactor {
    pub fn new(shared_secret: SharedSecret, outpoint: &OutPoint) -> Self {
        use bitcoin::hashes::{Hash, HashEngine};

        let mut hmac = HmacEngine::<sha512::Hash>::new(&serialize(outpoint));
        hmac.input(&shared_secret);

        BlindingFactor(Hmac::<sha512::Hash>::from_engine(hmac).into_inner())
    }
}

impl Deref for BlindingFactor {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for PaymentCode {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut prefixed = [0; 81];
        prefixed[0] = 0x47;
        prefixed[1..].copy_from_slice(&self.encode()[..]);
        base58::check_encode_slice_to_fmt(fmt, &prefixed[..])
    }
}

impl FromStr for PaymentCode {
    type Err = Error;

    fn from_str(inp: &str) -> Result<PaymentCode, Error> {
        let data = base58::from_check(inp)?;

        if data.len() != 81 {
            return Err(base58::Error::InvalidLength(data.len()).into());
        }
        if data[0] != 0x47 {
            return Err(Error::InvalidPrefix(data[0]));
        }

        Ok(PaymentCode::decode(&data[1..])?)
    }
}

pub struct Bip47Notification<K: DerivableKey<Legacy>>(pub K);

impl<K: DerivableKey<Legacy>> DescriptorTemplate for Bip47Notification<K> {
    fn build(self) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Pkh((
            self.0,
            bip32::DerivationPath::from_str("m/47'/0'/0'").unwrap(),
        ))
        .build()
    }
}

pub struct Bip47Wallet<'w, D> {
    seed: bip32::ExtendedPrivKey,
    main_wallet: &'w Wallet<D>,
    notification_wallet: Wallet<MemoryDatabase>,
    inbound_wallets: HashMap<PaymentCode, BTreeMap<u32, Option<Wallet<MemoryDatabase>>>>,
    outbound_wallets: HashMap<PaymentCode, BTreeMap<u32, Option<Wallet<MemoryDatabase>>>>,
    outbound_txs: HashMap<Script, Txid>,
}

impl<'w, D: BatchDatabase> Bip47Wallet<'w, D> {
    pub fn new<K: Clone + DerivableKey<Legacy>>(
        seed: K,
        wallet: &'w Wallet<D>,
    ) -> Result<Self, Error> {
        let bip47_seed = match seed.clone().into_extended_key()? {
            ExtendedKey::Private((xprv, _)) => xprv
                .derive_priv(
                    wallet.secp_ctx(),
                    &bip32::DerivationPath::from_str("m/47'/0'/0'").unwrap(),
                )
                .map_err(WalletError::from)?,
            _ => panic!("The key must be derivable"),
        };

        Ok(Bip47Wallet {
            seed: bip47_seed,
            notification_wallet: Wallet::new(
                Bip47Notification(seed),
                None,
                wallet.network(),
                MemoryDatabase::new(),
            )?,
            main_wallet: wallet,
            inbound_wallets: HashMap::new(),
            outbound_wallets: HashMap::new(),
            outbound_txs: HashMap::new(),
        })
    }

    pub fn sync<B: BlockchainFactory>(&mut self, blockchain: &B) -> Result<(), Error> {
        fn sync_wallet<SD: BatchDatabase, BF: BlockchainFactory>(
            wallet: &Wallet<SD>,
            blockchain: &BF,
        ) -> Result<(), Error> {
            wallet.sync(
                &blockchain.build(&wallet.descriptor_checksum(KeychainKind::External), None)?,
                SyncOptions::default(),
            )?;

            Ok(())
        }

        sync_wallet(&self.notification_wallet, blockchain)?;
        for tx in self.notification_wallet.list_transactions(true)? {
            // let conf_height = match tx.confirmation_time {
            //     None => continue,
            //     Some(bt) => bt.height,
            // };
            let tx = tx.transaction.as_ref().expect("Missing rawtx");

            if let Some(payment_code) = self.handshake_inbound(&tx) {
                println!("received notification from {}", payment_code.to_string());

                // remove the wallets to avoid a mutable borrow from `self`, which would
                // conflict with `self.derive_inbound_wallet()`.
                let mut wallets = self
                    .inbound_wallets
                    .remove(&payment_code)
                    .unwrap_or_else(|| BTreeMap::new());
                for i in 0u32.. {
                    println!("\tcheck {}", i);

                    match wallets
                        .entry(i)
                        .or_insert(self.derive_inbound_wallet(&payment_code, i)?)
                    {
                        Some(w) => {
                            sync_wallet(&w, blockchain)?;
                            println!(
                                "\tbalance ({}): {}",
                                w.get_address(AddressIndex::New)?,
                                w.get_balance()?
                            );
                            if w.get_balance()? == 0 {
                                break;
                            }
                        }
                        None => continue,
                    };
                }
                self.inbound_wallets.insert(payment_code, wallets);
            }
        }

        sync_wallet(&self.main_wallet, blockchain)?;
        for tx in self.main_wallet.list_transactions(true)? {
            let tx = tx.transaction.as_ref().expect("Missing rawtx");
            if let Some((scripts, txid)) = self.handshake_outbound(&tx)? {
                println!(
                    "handshake outbound found potential notification tx: {}",
                    txid
                );

                for s in scripts {
                    self.outbound_txs.insert(s, txid);
                }
            }
        }

        // remove the wallets to avoid a mutable borrow from `self`, which would
        // conflict with `self.derive_outbound_wallet()`.
        let mut outbound_wallets = self.outbound_wallets.drain().collect::<Vec<_>>();
        for (payment_code, wallets) in outbound_wallets.iter_mut() {
            for i in 0u32.. {
                println!("\tcheck {} (out)", i);

                match wallets
                    .entry(i)
                    .or_insert(self.derive_outbound_wallet(&payment_code, i)?)
                {
                    Some(w) => {
                        sync_wallet(&w, blockchain)?;
                        println!("\tbalance: {}", w.get_balance()?);
                        if w.get_balance()? == 0 {
                            break;
                        }
                    }
                    None => continue,
                };
            }
        }
        self.outbound_wallets.extend(outbound_wallets.into_iter());

        Ok(())
    }

    fn handshake_inbound(&self, tx: &Transaction) -> Option<PaymentCode> {
        let pk = match get_designated_pubkey(&tx.input[0]) {
            Some(pk) => pk,
            None => return None,
        };

        let secret = self.secret(&bip32::DerivationPath::default());
        let shared_secret = SharedSecret::new(&pk, &secret);
        let blinding_factor = BlindingFactor::new(shared_secret, &tx.input[0].previous_output);

        get_op_return_data(tx)
            .and_then(|data| PaymentCode::decode_blinded(&data, blinding_factor).ok())
    }

    fn handshake_outbound(&self, tx: &Transaction) -> Result<Option<(Vec<Script>, Txid)>, Error> {
        if self
            .main_wallet
            .get_utxo(tx.input[0].previous_output)?
            .is_some()
        {
            if let Some(data) = get_op_return_data(tx) {
                if data.len() != 80 {
                    return Ok(None);
                }

                // Potential notification addresses
                let scripts = tx
                    .output
                    .iter()
                    .map(|out| &out.script_pubkey)
                    .filter(|script| script.is_p2pkh())
                    .cloned()
                    .collect::<Vec<_>>();
                return Ok(Some((scripts, tx.txid())));
            }
        }

        Ok(None)
    }

    fn derive_inbound_wallet(
        &self,
        payment_code: &PaymentCode,
        index: u32,
    ) -> Result<Option<Wallet<MemoryDatabase>>, Error> {
        use bitcoin::hashes::Hash;

        let secp = self.main_wallet.secp_ctx();
        let network = self.main_wallet.network();

        let mut pk = payment_code.derive(secp, 0);
        let mut sk = self.secret(&vec![bip32::ChildNumber::Normal { index }]);

        pk.mul_assign(secp, sk.as_ref())?;
        let shared_secret = sha256::Hash::hash(&pk.serialize()[1..]);
        if let Err(_) = SecretKey::from_slice(&shared_secret) {
            return Ok(None);
        }
        sk.add_assign(&shared_secret)?;

        let wallet = Wallet::new(
            P2Pkh(bitcoin::PrivateKey {
                key: sk,
                compressed: true,
                network,
            }),
            None,
            network,
            MemoryDatabase::new(),
        )?;

        Ok(Some(wallet))
    }

    fn derive_outbound_wallet(
        &self,
        payment_code: &PaymentCode,
        index: u32,
    ) -> Result<Option<Wallet<MemoryDatabase>>, Error> {
        use bitcoin::hashes::Hash;

        let secp = self.main_wallet.secp_ctx();
        let network = self.main_wallet.network();

        let pk = payment_code.derive(secp, index);
        let sk = self.secret(&vec![bip32::ChildNumber::Normal { index: 0 }]);

        let mut s = pk.clone();
        s.mul_assign(secp, sk.as_ref())?;
        let shared_secret = sha256::Hash::hash(&s.serialize()[1..]);
        let pk = match SecretKey::from_slice(&shared_secret) {
            Ok(sk) => pk.combine(&PublicKey::from_secret_key(secp, &sk))?,
            Err(_) => return Ok(None),
        };

        let wallet = Wallet::new(
            P2Pkh(bitcoin::PublicKey {
                key: pk,
                compressed: true,
            }),
            None,
            network,
            MemoryDatabase::new(),
        )?;

        Ok(Some(wallet))
    }

    fn secret<P: AsRef<[bip32::ChildNumber]>>(&self, derivation: &P) -> SecretKey {
        let derived = self
            .seed
            .derive_priv(self.main_wallet.secp_ctx(), derivation)
            .map_err(WalletError::from)
            .expect("Derivation should work");

        derived.private_key.key
    }

    pub fn payment_code(&self) -> PaymentCode {
        let xpub =
            bip32::ExtendedPubKey::from_private(self.notification_wallet.secp_ctx(), &self.seed);

        PaymentCode {
            version: 0x01,
            features: 0x00,
            chain_code: xpub.chain_code,
            public_key: xpub.public_key.key,
        }
    }

    pub fn notification_address(&self) -> Address {
        self.payment_code()
            .notification_address(self.main_wallet.secp_ctx(), self.main_wallet.network())
    }

    pub fn build_notification_tx(
        &mut self,
        payment_code: &PaymentCode,
        initial_builder: Option<TxBuilder<'w, D, DefaultCoinSelectionAlgorithm, CreateTx>>,
        amount: Option<u64>,
    ) -> Result<Option<(psbt::PartiallySignedTransaction, TransactionDetails)>, Error> {
        let secp = self.main_wallet.secp_ctx();
        let network = self.main_wallet.network();

        // We already know about this payment code
        if self.outbound_wallets.contains_key(payment_code) {
            return Ok(None);
        }

        // We might have sent a notification transaction to this code in the past, confirm it here
        if let Some(txid) = self.outbound_txs.get(
            &payment_code
                .notification_address(secp, network)
                .script_pubkey(),
        ) {
            if self.reconstruct_outbound_notification(txid, payment_code)? {
                self.record_notification_tx(payment_code);
                return Ok(None);
            }
        }

        let build_tx = |data| {
            let mut builder = initial_builder
                .clone()
                .unwrap_or(self.main_wallet.build_tx());
            builder
                .ordering(TxOrdering::Untouched)
                .add_data(data)
                .add_recipient(
                    payment_code
                        .notification_address(secp, network)
                        .script_pubkey(),
                    amount.unwrap_or(546),
                );

            builder
        };

        let utxos = {
            // Build a tx with a dummy payment code, to perform coin selection and fee estimation
            let (psbt, _) = build_tx(&[0u8; 80]).finish()?;
            // Then reuse the inputs
            psbt.global.unsigned_tx.input
        };

        let local_utxo = self
            .main_wallet
            .get_utxo(utxos[0].previous_output)?
            .ok_or_else(|| Error::InvalidUTXO(utxos[0].previous_output))?;

        let blinding_factor = self.generate_blinding_factor(local_utxo, &payment_code)?;
        let mut builder = build_tx(&self.payment_code().encode_blinded(blinding_factor));
        builder
            .add_utxos(&utxos.iter().map(|x| x.previous_output).collect::<Vec<_>>())?
            .manually_selected_only();

        Ok(Some(builder.finish()?))
    }

    pub fn record_notification_tx(&mut self, payment_code: &PaymentCode) {
        self.outbound_wallets.insert(*payment_code, BTreeMap::new());
    }

    pub fn get_payment_address(&self, payment_code: &PaymentCode) -> Result<Address, Error> {
        match self.outbound_wallets.get(payment_code) {
            Some(wallets) => match wallets.values().last() {
                Some(w) => Ok(w
                    .as_ref()
                    .expect("Last wallet is valid")
                    .get_address(AddressIndex::New)?
                    .address),
                _ => Err(Error::UnsyncedWallet),
            },
            _ => Err(Error::UnknownReceipient),
        }
    }

    fn reconstruct_outbound_notification(
        &self,
        txid: &Txid,
        payment_code: &PaymentCode,
    ) -> Result<bool, Error> {
        let tx = match self.main_wallet.get_tx(txid, true)? {
            Some(details) => details.transaction.expect("Raw tx requested"),
            None => return Ok(false),
        };

        if let Some(utxo) = self.main_wallet.get_utxo(tx.input[0].previous_output)? {
            if let Some(data) = get_op_return_data(&tx) {
                let blinding_factor = self.generate_blinding_factor(utxo, payment_code)?;

                return match PaymentCode::decode_blinded(data, blinding_factor) {
                    Ok(pc) if &pc == payment_code => Ok(true),
                    _ => Ok(false),
                };
            }
        }

        Ok(false)
    }

    fn generate_blinding_factor(
        &self,
        local_utxo: LocalUtxo,
        payment_code: &PaymentCode,
    ) -> Result<BlindingFactor, Error> {
        let secp = self.main_wallet.secp_ctx();

        let outpoint = local_utxo.outpoint.clone();

        let keychain = local_utxo.keychain;
        let psbt_input = self.main_wallet.get_psbt_input(local_utxo, None, true)?;
        let keys_map = self
            .main_wallet
            .get_signers(keychain)
            .signers()
            .iter()
            .filter_map(|signer| match signer.descriptor_secret_key() {
                Some(DescriptorSecretKey::SinglePriv(DescriptorSinglePriv { key, .. })) => {
                    Some((key.public_key(secp), key))
                }
                Some(DescriptorSecretKey::XPrv(xkey)) => {
                    for (_, keysource) in &psbt_input.bip32_derivation {
                        if xkey.matches(keysource, secp).is_some() {
                            let deriv_path = &keysource
                                .1
                                .into_iter()
                                .cloned()
                                .collect::<Vec<bip32::ChildNumber>>()
                                [xkey.origin.map(|o| o.1.len()).unwrap_or(0)..];
                            let key = xkey
                                .xkey
                                .derive_priv(secp, &deriv_path)
                                .expect("Derivation shouldn't fail")
                                .private_key;

                            return Some((key.public_key(secp), key));
                        }
                    }

                    None
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        if keys_map.len() != 1 {
            return Err(Error::UnsupportedWallet);
        }

        let shared_secret = SharedSecret::new(
            &payment_code.public_key,
            &keys_map.values().next().expect("Key is present").key,
        );
        Ok(BlindingFactor::new(shared_secret, &outpoint))
    }
}

fn get_designated_pubkey(txin: &TxIn) -> Option<PublicKey> {
    // From the BIP:
    //
    // > Alice SHOULD use an input script in one of the following standard forms to expose a public key, and compliant applications SHOULD recognize all of these forms.
    // > - P2PK (pay to pubkey)
    // > - P2PKH (pay to pubkey hash)
    // > - Multisig (bare multisig, without P2SH)
    // > - a script which spends any of the above script forms via P2SH (pay to script hash)
    //
    // TODO: Unfortunately to check the script type we need to know the previous transaction. For now,
    // assume it's a P2PKH and fail otherwise.

    match txin.script_sig.instructions().nth(1) {
        Some(Ok(Instruction::PushBytes(pk))) => PublicKey::from_slice(pk).ok(),
        _ => None,
    }
}

fn get_op_return_data(tx: &Transaction) -> Option<&[u8]> {
    if let Some(txout) = tx.output.iter().find(|o| o.script_pubkey.is_op_return()) {
        return match txout.script_pubkey.instructions().nth(1) {
            Some(Ok(Instruction::PushBytes(data))) => Some(data),
            _ => None,
        };
    }

    None
}

#[derive(Debug)]
pub enum Error {
    WrongDataLength(usize),
    UnknownVersion(u8),
    InvalidPrefix(u8),
    InvalidPublicKeySign(u8),
    InvalidUTXO(OutPoint),
    UnsupportedWallet,
    UnknownReceipient,
    UnsyncedWallet,
    Base58(base58::Error),
    SecpKey(bitcoin::secp256k1::Error),
    Key(crate::keys::KeyError),
    Wallet(WalletError),
}

// TODO: impl display, std::err

impl From<base58::Error> for Error {
    fn from(e: base58::Error) -> Error {
        Error::Base58(e)
    }
}
impl From<bitcoin::secp256k1::Error> for Error {
    fn from(e: bitcoin::secp256k1::Error) -> Error {
        Error::SecpKey(e)
    }
}
impl From<crate::keys::KeyError> for Error {
    fn from(e: crate::keys::KeyError) -> Error {
        Error::Key(e)
    }
}
impl From<WalletError> for Error {
    fn from(e: WalletError) -> Error {
        Error::Wallet(e)
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use bitcoin::*;
    use electrum_client::*;

    use super::*;
    use crate::blockchain::*;
    use crate::database::*;
    use crate::descriptor::template::*;
    use crate::wallet::*;

    #[test]
    fn t() {
        use crate::testutils::blockchain_tests::TestClient;

        let mut tc = TestClient::default();
        let blockchain = Arc::new(ElectrumBlockchain::from(
            electrum_client::Client::new(&tc.electrsd.electrum_url).unwrap(),
        ));

        // let client = electrum_client::Client::new("ssl://electrum.blockstream.info:60002").unwrap();
        // let blockchain = Arc::new(ElectrumBlockchain::from(client));

        // let key = PrivateKey::from_wif("cU1zSPAAHNGE8quJZkBsFJELTxfJRsS82Z4M4WPb95VcdpBM9gBv").unwrap();
        // let key =
        //     PrivateKey::from_wif("L3esrLZfpd5B2GGSjVSUiHGZvDtDpAAFRLGKP9UiMC46pbfYAJJk").unwrap();
        let m = crate::keys::bip39::Mnemonic::parse(
            "response seminar brave tip suit recall often sound stick owner lottery motion",
        )
        .unwrap();
        let alice_main_wallet = Wallet::new(
            Bip44(m.clone(), KeychainKind::External),
            None,
            Network::Regtest,
            MemoryDatabase::new(),
        )
        .unwrap();
        let tx = crate::testutils! {
            @tx ( (@addr alice_main_wallet.get_address(AddressIndex::Peek(5)).unwrap().address) => 50_000 )
        };
        tc.receive(tx);

        alice_main_wallet
            .sync(&blockchain, SyncOptions::default())
            .unwrap();
        println!("balance: {}", alice_main_wallet.get_balance().unwrap());
        println!(
            "{}",
            alice_main_wallet.get_address(AddressIndex::New).unwrap()
        );

        let mut alice = Bip47Wallet::new(m, &alice_main_wallet).unwrap();
        println!("{} {}", alice.payment_code(), alice.notification_address());
        assert_eq!(alice.payment_code().to_string(), "PM8TJTLJbPRGxSbc8EJi42Wrr6QbNSaSSVJ5Y3E4pbCYiTHUskHg13935Ubb7q8tx9GVbh2UuRnBc3WSyJHhUrw8KhprKnn9eDznYGieTzFcwQRya4GA");
        // assert_eq!(alice.notification_address().to_string(), "1JDdmqFLhpzcUwPeinhJbUPw4Co3aWLyzW");

        // let sharedsecret = Vec::<u8>::from_hex("736a25d9250238ad64ed5da03450c6a3f4f8f4dcdf0b58d1ed69029d76ead48d").unwrap();
        // let sharedsecret: [u8; 32] = sharedsecret.try_into().unwrap();
        // let sharedsecret = bitcoin::secp256k1::ecdh::SharedSecret::from(sharedsecret);
        // dbg!(&sharedsecret);

        // let outpoint = OutPoint::from_str("9c6000d597c5008f7bfc2618aed5e4a6ae57677aab95078aae708e1cab11f486:1").unwrap();
        // dbg!(&outpoint);
        // let bf = BlindingFactor::new(sharedsecret, &outpoint);

        // use bitcoin::hashes::hex::ToHex;
        // println!("blinded code: {}", alice.payment_code().encode_blinded(bf).to_hex());

        let m = crate::keys::bip39::Mnemonic::parse(
            "reward upper indicate eight swift arch injury crystal super wrestle already dentist",
        )
        .unwrap();
        let bob_main_wallet = Wallet::new(
            Bip44(m.clone(), KeychainKind::External),
            None,
            Network::Regtest,
            MemoryDatabase::new(),
        )
        .unwrap();
        let mut bob = Bip47Wallet::new(m, &bob_main_wallet).unwrap();
        println!("{} {}", bob.payment_code(), bob.notification_address());
        assert_eq!(bob.payment_code().to_string(), "PM8TJS2JxQ5ztXUpBBRnpTbcUXbUHy2T1abfrb3KkAAtMEGNbey4oumH7Hc578WgQJhPjBxteQ5GHHToTYHE3A1w6p7tU6KSoFmWBVbFGjKPisZDbP97");
        // assert_eq!(bob.notification_address().unwrap().to_string(), "1ChvUUvht2hUQufHBXF8NgLhW8SwE2ecGV");
        // bob.sync(&blockchain).unwrap();
        // bob.sync(&blockchain).unwrap();
        // bob.sync(&blockchain).unwrap();
        // bob.sync(&blockchain).unwrap();

        let (mut psbt, _) = alice
            .build_notification_tx(&bob.payment_code(), None, None)
            .unwrap()
            .unwrap();
        alice_main_wallet
            .sign(&mut psbt, Default::default())
            .unwrap();
        blockchain.broadcast(&psbt.extract_tx()).unwrap();
        alice.record_notification_tx(&bob.payment_code());

        alice.sync(&blockchain).unwrap();

        let bob_addr = alice.get_payment_address(&bob.payment_code()).unwrap();
        let (mut psbt, _) = {
            let mut builder = alice_main_wallet.build_tx();
            builder.add_recipient(bob_addr.script_pubkey(), 10_000);
            builder.finish().unwrap()
        };
        alice_main_wallet
            .sign(&mut psbt, Default::default())
            .unwrap();
        blockchain.broadcast(&psbt.extract_tx()).unwrap();

        alice.sync(&blockchain).unwrap();

        println!("bob sync");
        bob.sync(&blockchain).unwrap();

        // dbg!(&tx);
    }

    // TODO: test main wallet with single key and xprv
}
