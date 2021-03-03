// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

#[macro_use]
extern crate serde_json;

pub use serial_test::serial;

use std::collections::HashMap;
use std::env;
use std::ops::Deref;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::sha256d;
use bitcoin::secp256k1::{Secp256k1, Verification};
use bitcoin::{Address, Amount, PublicKey, Script, Transaction, Txid};

use miniscript::descriptor::DescriptorPublicKey;
use miniscript::{Descriptor, MiniscriptKey, TranslatePk};

pub use bitcoincore_rpc::bitcoincore_rpc_json::AddressType;
pub use bitcoincore_rpc::{Auth, Client as RpcClient, RpcApi};

pub use electrum_client::{Client as ElectrumClient, ElectrumApi};

// TODO: we currently only support env vars, we could also parse a toml file
fn get_auth() -> Auth {
    match env::var("BDK_RPC_AUTH").as_ref().map(String::as_ref) {
        Ok("USER_PASS") => Auth::UserPass(
            env::var("BDK_RPC_USER").unwrap(),
            env::var("BDK_RPC_PASS").unwrap(),
        ),
        _ => Auth::CookieFile(PathBuf::from(
            env::var("BDK_RPC_COOKIEFILE")
                .unwrap_or_else(|_| "/home/user/.bitcoin/regtest/.cookie".to_string()),
        )),
    }
}

pub fn get_electrum_url() -> String {
    env::var("BDK_ELECTRUM_URL").unwrap_or_else(|_| "tcp://127.0.0.1:50001".to_string())
}

pub struct TestClient {
    client: RpcClient,
    electrum: ElectrumClient,
}

#[derive(Clone, Debug)]
pub struct TestIncomingOutput {
    pub value: u64,
    pub to_address: String,
}

impl TestIncomingOutput {
    pub fn new(value: u64, to_address: Address) -> Self {
        Self {
            value,
            to_address: to_address.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestIncomingTx {
    pub output: Vec<TestIncomingOutput>,
    pub min_confirmations: Option<u64>,
    pub locktime: Option<i64>,
    pub replaceable: Option<bool>,
}

impl TestIncomingTx {
    pub fn new(
        output: Vec<TestIncomingOutput>,
        min_confirmations: Option<u64>,
        locktime: Option<i64>,
        replaceable: Option<bool>,
    ) -> Self {
        Self {
            output,
            min_confirmations,
            locktime,
            replaceable,
        }
    }

    pub fn add_output(&mut self, output: TestIncomingOutput) {
        self.output.push(output);
    }
}

#[doc(hidden)]
pub trait TranslateDescriptor {
    // derive and translate a `Descriptor<DescriptorPublicKey>` into a `Descriptor<PublicKey>`
    fn derive_translated<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        index: u32,
    ) -> Descriptor<PublicKey>;
}

impl TranslateDescriptor for Descriptor<DescriptorPublicKey> {
    fn derive_translated<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        index: u32,
    ) -> Descriptor<PublicKey> {
        let translate = |key: &DescriptorPublicKey| -> PublicKey {
            match key {
                DescriptorPublicKey::XPub(xpub) => {
                    xpub.xkey
                        .derive_pub(secp, &xpub.derivation_path)
                        .expect("hardened derivation steps")
                        .public_key
                }
                DescriptorPublicKey::SinglePub(key) => key.key,
            }
        };

        self.derive(index)
            .translate_pk_infallible(|pk| translate(pk), |pkh| translate(pkh).to_pubkeyhash())
    }
}

#[macro_export]
macro_rules! testutils {
    ( @external $descriptors:expr, $child:expr ) => ({
        use bitcoin::secp256k1::Secp256k1;
        use miniscript::descriptor::{Descriptor, DescriptorPublicKey, DescriptorTrait};

        use $crate::TranslateDescriptor;

        let secp = Secp256k1::new();

        let parsed = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &$descriptors.0).expect("Failed to parse descriptor in `testutils!(@external)`").0;
        parsed.derive_translated(&secp, $child).address(bitcoin::Network::Regtest).expect("No address form")
    });
    ( @internal $descriptors:expr, $child:expr ) => ({
        use bitcoin::secp256k1::Secp256k1;
        use miniscript::descriptor::{Descriptor, DescriptorPublicKey, DescriptorTrait};

        use $crate::TranslateDescriptor;

        let secp = Secp256k1::new();

        let parsed = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &$descriptors.1.expect("Missing internal descriptor")).expect("Failed to parse descriptor in `testutils!(@internal)`").0;
        parsed.derive_translated(&secp, $child).address(bitcoin::Network::Regtest).expect("No address form")
    });
    ( @e $descriptors:expr, $child:expr ) => ({ testutils!(@external $descriptors, $child) });
    ( @i $descriptors:expr, $child:expr ) => ({ testutils!(@internal $descriptors, $child) });

    ( @tx ( $( ( $( $addr:tt )* ) => $amount:expr ),+ ) $( ( @locktime $locktime:expr ) )* $( ( @confirmations $confirmations:expr ) )* $( ( @replaceable $replaceable:expr ) )* ) => ({
        let mut outs = Vec::new();
        $( outs.push(testutils::TestIncomingOutput::new($amount, testutils!( $($addr)* ))); )+

        let mut locktime = None::<i64>;
        $( locktime = Some($locktime); )*

        let mut min_confirmations = None::<u64>;
        $( min_confirmations = Some($confirmations); )*

        let mut replaceable = None::<bool>;
        $( replaceable = Some($replaceable); )*

        testutils::TestIncomingTx::new(outs, min_confirmations, locktime, replaceable)
    });

    ( @literal $key:expr ) => ({
        let key = $key.to_string();
        (key, None::<String>, None::<String>)
    });
    ( @generate_xprv $( $external_path:expr )* $( ,$internal_path:expr )* ) => ({
        use rand::Rng;

        let mut seed = [0u8; 32];
        rand::thread_rng().fill(&mut seed[..]);

        let key = bitcoin::util::bip32::ExtendedPrivKey::new_master(
            bitcoin::Network::Testnet,
            &seed,
        );

        let mut external_path = None::<String>;
        $( external_path = Some($external_path.to_string()); )*

        let mut internal_path = None::<String>;
        $( internal_path = Some($internal_path.to_string()); )*

        (key.unwrap().to_string(), external_path, internal_path)
    });
    ( @generate_wif ) => ({
        use rand::Rng;

        let mut key = [0u8; bitcoin::secp256k1::constants::SECRET_KEY_SIZE];
        rand::thread_rng().fill(&mut key[..]);

        (bitcoin::PrivateKey {
            compressed: true,
            network: bitcoin::Network::Testnet,
            key: bitcoin::secp256k1::SecretKey::from_slice(&key).unwrap(),
        }.to_string(), None::<String>, None::<String>)
    });

    ( @keys ( $( $alias:expr => ( $( $key_type:tt )* ) ),+ ) ) => ({
        let mut map = std::collections::HashMap::new();
        $(
            let alias: &str = $alias;
            map.insert(alias, testutils!( $($key_type)* ));
        )+

        map
    });

    ( @descriptors ( $external_descriptor:expr ) $( ( $internal_descriptor:expr ) )* $( ( @keys $( $keys:tt )* ) )* ) => ({
        use std::str::FromStr;
        use std::collections::HashMap;
        use std::convert::TryInto;

        use miniscript::descriptor::{Descriptor, DescriptorPublicKey};
        use miniscript::TranslatePk;

        let mut keys: HashMap<&'static str, (String, Option<String>, Option<String>)> = HashMap::new();
        $(
            keys = testutils!{ @keys $( $keys )* };
        )*

        let external: Descriptor<String> = FromStr::from_str($external_descriptor).unwrap();
        let external: Descriptor<String> = external.translate_pk_infallible::<_, _>(|k| {
            if let Some((key, ext_path, _)) = keys.get(&k.as_str()) {
                format!("{}{}", key, ext_path.as_ref().unwrap_or(&"".into()))
            } else {
                k.clone()
            }
        }, |kh| {
            if let Some((key, ext_path, _)) = keys.get(&kh.as_str()) {
                format!("{}{}", key, ext_path.as_ref().unwrap_or(&"".into()))
            } else {
                kh.clone()
            }

        });
        let external = external.to_string();

        let mut internal = None::<String>;
        $(
            let string_internal: Descriptor<String> = FromStr::from_str($internal_descriptor).unwrap();

            let string_internal: Descriptor<String> = string_internal.translate_pk_infallible::<_, _>(|k| {
                if let Some((key, _, int_path)) = keys.get(&k.as_str()) {
                    format!("{}{}", key, int_path.as_ref().unwrap_or(&"".into()))
                } else {
                    k.clone()
                }
            }, |kh| {
                if let Some((key, _, int_path)) = keys.get(&kh.as_str()) {
                    format!("{}{}", key, int_path.as_ref().unwrap_or(&"".into()))
                } else {
                    kh.clone()
                }
            });
            internal = Some(string_internal.to_string());
        )*

        (external, internal)
    })
}

fn exponential_backoff_poll<T, F>(mut poll: F) -> T
where
    F: FnMut() -> Option<T>,
{
    let mut delay = Duration::from_millis(64);
    loop {
        match poll() {
            Some(data) => break data,
            None if delay.as_millis() < 512 => delay = delay.mul_f32(2.0),
            None => {}
        }

        std::thread::sleep(delay);
    }
}

impl TestClient {
    pub fn new() -> Self {
        let url = env::var("BDK_RPC_URL").unwrap_or_else(|_| "127.0.0.1:18443".to_string());
        let wallet = env::var("BDK_RPC_WALLET").unwrap_or_else(|_| "bdk-test".to_string());
        let client =
            RpcClient::new(format!("http://{}/wallet/{}", url, wallet), get_auth()).unwrap();
        let electrum = ElectrumClient::new(&get_electrum_url()).unwrap();

        TestClient { client, electrum }
    }

    fn wait_for_tx(&mut self, txid: Txid, monitor_script: &Script) {
        // wait for electrs to index the tx
        exponential_backoff_poll(|| {
            trace!("wait_for_tx {}", txid);

            self.electrum
                .script_get_history(monitor_script)
                .unwrap()
                .iter()
                .position(|entry| entry.tx_hash == txid)
        });
    }

    fn wait_for_block(&mut self, min_height: usize) {
        self.electrum.block_headers_subscribe().unwrap();

        loop {
            let header = exponential_backoff_poll(|| {
                self.electrum.ping().unwrap();
                self.electrum.block_headers_pop().unwrap()
            });
            if header.height >= min_height {
                break;
            }
        }
    }

    pub fn receive(&mut self, meta_tx: TestIncomingTx) -> Txid {
        assert!(
            !meta_tx.output.is_empty(),
            "can't create a transaction with no outputs"
        );

        let mut map = HashMap::new();

        let mut required_balance = 0;
        for out in &meta_tx.output {
            required_balance += out.value;
            map.insert(out.to_address.clone(), Amount::from_sat(out.value));
        }

        if self.get_balance(None, None).unwrap() < Amount::from_sat(required_balance) {
            panic!("Insufficient funds in bitcoind. Please generate a few blocks with: `bitcoin-cli generatetoaddress 10 {}`", self.get_new_address(None, None).unwrap());
        }

        // FIXME: core can't create a tx with two outputs to the same address
        let tx = self
            .create_raw_transaction_hex(&[], &map, meta_tx.locktime, meta_tx.replaceable)
            .unwrap();
        let tx = self.fund_raw_transaction(tx, None, None).unwrap();
        let mut tx: Transaction = deserialize(&tx.hex).unwrap();

        if let Some(true) = meta_tx.replaceable {
            // for some reason core doesn't set this field right
            for input in &mut tx.input {
                input.sequence = 0xFFFFFFFD;
            }
        }

        let tx = self
            .sign_raw_transaction_with_wallet(&serialize(&tx), None, None)
            .unwrap();

        // broadcast through electrum so that it caches the tx immediately
        let txid = self
            .electrum
            .transaction_broadcast(&deserialize(&tx.hex).unwrap())
            .unwrap();

        if let Some(num) = meta_tx.min_confirmations {
            self.generate(num, None);
        }

        let monitor_script = Address::from_str(&meta_tx.output[0].to_address)
            .unwrap()
            .script_pubkey();
        self.wait_for_tx(txid, &monitor_script);

        debug!("Sent tx: {}", txid);

        txid
    }

    pub fn bump_fee(&mut self, txid: &Txid) -> Txid {
        let tx = self.get_raw_transaction_info(txid, None).unwrap();
        assert!(
            tx.confirmations.is_none(),
            "Can't bump tx {} because it's already confirmed",
            txid
        );

        let bumped: serde_json::Value = self.call("bumpfee", &[txid.to_string().into()]).unwrap();
        let new_txid = Txid::from_str(&bumped["txid"].as_str().unwrap().to_string()).unwrap();

        let monitor_script =
            tx.vout[0].script_pub_key.addresses.as_ref().unwrap()[0].script_pubkey();
        self.wait_for_tx(new_txid, &monitor_script);

        debug!("Bumped {}, new txid {}", txid, new_txid);

        new_txid
    }

    pub fn generate_manually(&mut self, txs: Vec<Transaction>) -> String {
        use bitcoin::blockdata::block::{Block, BlockHeader};
        use bitcoin::blockdata::script::Builder;
        use bitcoin::blockdata::transaction::{OutPoint, TxIn, TxOut};
        use bitcoin::hash_types::{BlockHash, TxMerkleNode};

        let block_template: serde_json::Value = self
            .call("getblocktemplate", &[json!({"rules": ["segwit"]})])
            .unwrap();
        trace!("getblocktemplate: {:#?}", block_template);

        let header = BlockHeader {
            version: block_template["version"].as_i64().unwrap() as i32,
            prev_blockhash: BlockHash::from_hex(
                block_template["previousblockhash"].as_str().unwrap(),
            )
            .unwrap(),
            merkle_root: TxMerkleNode::default(),
            time: block_template["curtime"].as_u64().unwrap() as u32,
            bits: u32::from_str_radix(block_template["bits"].as_str().unwrap(), 16).unwrap(),
            nonce: 0,
        };
        debug!("header: {:#?}", header);

        let height = block_template["height"].as_u64().unwrap() as i64;
        let witness_reserved_value: Vec<u8> = sha256d::Hash::default().as_ref().into();
        // burn block subsidy and fees, not a big deal
        let mut coinbase_tx = Transaction {
            version: 1,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: Builder::new().push_int(height).into_script(),
                sequence: 0xFFFFFFFF,
                witness: vec![witness_reserved_value],
            }],
            output: vec![],
        };

        let mut txdata = vec![coinbase_tx.clone()];
        txdata.extend_from_slice(&txs);

        let mut block = Block { header, txdata };

        let witness_root = block.witness_root();
        let witness_commitment =
            Block::compute_witness_commitment(&witness_root, &coinbase_tx.input[0].witness[0]);

        // now update and replace the coinbase tx
        let mut coinbase_witness_commitment_script = vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed];
        coinbase_witness_commitment_script.extend_from_slice(&witness_commitment);

        coinbase_tx.output.push(TxOut {
            value: 0,
            script_pubkey: coinbase_witness_commitment_script.into(),
        });
        block.txdata[0] = coinbase_tx;

        // set merkle root
        let merkle_root = block.merkle_root();
        block.header.merkle_root = merkle_root;

        assert!(block.check_merkle_root());
        assert!(block.check_witness_commitment());

        // now do PoW :)
        let target = block.header.target();
        while block.header.validate_pow(&target).is_err() {
            block.header.nonce = block.header.nonce.checked_add(1).unwrap(); // panic if we run out of nonces
        }

        let block_hex: String = serialize(&block).to_hex();
        debug!("generated block hex: {}", block_hex);

        self.electrum.block_headers_subscribe().unwrap();

        let submit_result: serde_json::Value =
            self.call("submitblock", &[block_hex.into()]).unwrap();
        debug!("submitblock: {:?}", submit_result);
        assert!(
            submit_result.is_null(),
            "submitblock error: {:?}",
            submit_result.as_str()
        );

        self.wait_for_block(height as usize);

        block.header.block_hash().to_hex()
    }

    pub fn generate(&mut self, num_blocks: u64, address: Option<Address>) {
        let address = address.unwrap_or_else(|| self.get_new_address(None, None).unwrap());
        let hashes = self.generate_to_address(num_blocks, &address).unwrap();
        let best_hash = hashes.last().unwrap();
        let height = self.get_block_info(best_hash).unwrap().height;

        self.wait_for_block(height);

        debug!("Generated blocks to new height {}", height);
    }

    pub fn invalidate(&mut self, num_blocks: u64) {
        self.electrum.block_headers_subscribe().unwrap();

        let best_hash = self.get_best_block_hash().unwrap();
        let initial_height = self.get_block_info(&best_hash).unwrap().height;

        let mut to_invalidate = best_hash;
        for i in 1..=num_blocks {
            trace!(
                "Invalidating block {}/{} ({})",
                i,
                num_blocks,
                to_invalidate
            );

            self.invalidate_block(&to_invalidate).unwrap();
            to_invalidate = self.get_best_block_hash().unwrap();
        }

        self.wait_for_block(initial_height - num_blocks as usize);

        debug!(
            "Invalidated {} blocks to new height of {}",
            num_blocks,
            initial_height - num_blocks as usize
        );
    }

    pub fn reorg(&mut self, num_blocks: u64) {
        self.invalidate(num_blocks);
        self.generate(num_blocks, None);
    }

    pub fn get_node_address(&self, address_type: Option<AddressType>) -> Address {
        Address::from_str(
            &self
                .get_new_address(None, address_type)
                .unwrap()
                .to_string(),
        )
        .unwrap()
    }
}

impl Deref for TestClient {
    type Target = RpcClient;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
