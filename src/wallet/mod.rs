use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ops::DerefMut;
use std::str::FromStr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bitcoin::blockdata::opcodes;
use bitcoin::blockdata::script::Builder;
use bitcoin::consensus::encode::serialize;
use bitcoin::util::psbt::PartiallySignedTransaction as PSBT;
use bitcoin::{
    Address, Network, OutPoint, PublicKey, Script, SigHashType, Transaction, TxIn, TxOut, Txid,
};

use miniscript::BitcoinSig;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

pub mod utils;

use self::utils::IsDust;
use crate::blockchain::{noop_progress, Blockchain, OfflineBlockchain, OnlineBlockchain};
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::descriptor::{get_checksum, DescriptorMeta, ExtendedDescriptor, ExtractPolicy, Policy};
use crate::error::Error;
use crate::psbt::{utils::PSBTUtils, PSBTSatisfier, PSBTSigner};
use crate::signer::Signer;
use crate::types::*;

pub type OfflineWallet<D> = Wallet<OfflineBlockchain, D>;

pub struct Wallet<B: Blockchain, D: BatchDatabase> {
    descriptor: ExtendedDescriptor,
    change_descriptor: Option<ExtendedDescriptor>,
    network: Network,

    current_height: Option<u32>,

    client: RefCell<B>,
    database: RefCell<D>,
}

// offline actions, always available
impl<B, D> Wallet<B, D>
where
    B: Blockchain,
    D: BatchDatabase,
{
    pub fn new_offline(
        descriptor: &str,
        change_descriptor: Option<&str>,
        network: Network,
        mut database: D,
    ) -> Result<Self, Error> {
        database.check_descriptor_checksum(
            ScriptType::External,
            get_checksum(descriptor)?.as_bytes(),
        )?;
        let descriptor = ExtendedDescriptor::from_str(descriptor)?;
        let change_descriptor = match change_descriptor {
            Some(desc) => {
                database.check_descriptor_checksum(
                    ScriptType::Internal,
                    get_checksum(desc)?.as_bytes(),
                )?;

                let parsed = ExtendedDescriptor::from_str(desc)?;
                if !parsed.same_structure(descriptor.as_ref()) {
                    return Err(Error::DifferentDescriptorStructure);
                }

                Some(parsed)
            }
            None => None,
        };

        Ok(Wallet {
            descriptor,
            change_descriptor,
            network,

            current_height: None,

            client: RefCell::new(B::offline()),
            database: RefCell::new(database),
        })
    }

    pub fn get_new_address(&self) -> Result<Address, Error> {
        let index = self
            .database
            .borrow_mut()
            .increment_last_index(ScriptType::External)?;
        // TODO: refill the address pool if index is close to the last cached addr

        self.descriptor
            .derive(index)?
            .address(self.network)
            .ok_or(Error::ScriptDoesntHaveAddressForm)
    }

    pub fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.database.borrow().is_mine(script)
    }

    pub fn list_unspent(&self) -> Result<Vec<UTXO>, Error> {
        self.database.borrow().iter_utxos()
    }

    pub fn list_transactions(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        self.database.borrow().iter_txs(include_raw)
    }

    pub fn get_balance(&self) -> Result<u64, Error> {
        Ok(self
            .list_unspent()?
            .iter()
            .fold(0, |sum, i| sum + i.txout.value))
    }

    // TODO: add a flag to ignore change in coin selection
    pub fn create_tx(
        &self,
        addressees: Vec<(Address, u64)>,
        send_all: bool,
        fee_perkb: f32,
        policy_path: Option<BTreeMap<String, Vec<usize>>>,
        utxos: Option<Vec<OutPoint>>,
        unspendable: Option<Vec<OutPoint>>,
    ) -> Result<(PSBT, TransactionDetails), Error> {
        let policy = self.descriptor.extract_policy()?.unwrap();
        if policy.requires_path() && policy_path.is_none() {
            return Err(Error::SpendingPolicyRequired);
        }
        let requirements = policy.get_requirements(&policy_path.unwrap_or(BTreeMap::new()))?;
        debug!("requirements: {:?}", requirements);

        let mut tx = Transaction {
            version: 2,
            lock_time: requirements.timelock.unwrap_or(0),
            input: vec![],
            output: vec![],
        };

        let fee_rate = fee_perkb * 100_000.0;
        if send_all && addressees.len() != 1 {
            return Err(Error::SendAllMultipleOutputs);
        }

        // we keep it as a float while we accumulate it, and only round it at the end
        let mut fee_val: f32 = 0.0;
        let mut outgoing: u64 = 0;
        let mut received: u64 = 0;

        let calc_fee_bytes = |wu| (wu as f32) * fee_rate / 4.0;
        fee_val += calc_fee_bytes(tx.get_weight());

        for (index, (address, satoshi)) in addressees.iter().enumerate() {
            let value = match send_all {
                true => 0,
                false if satoshi.is_dust() => return Err(Error::OutputBelowDustLimit(index)),
                false => *satoshi,
            };

            // TODO: check address network
            if self.is_mine(&address.script_pubkey())? {
                received += value;
            }

            let new_out = TxOut {
                script_pubkey: address.script_pubkey(),
                value,
            };
            fee_val += calc_fee_bytes(serialize(&new_out).len() * 4);

            tx.output.push(new_out);

            outgoing += value;
        }

        // TODO: assumes same weight to spend external and internal
        let input_witness_weight = self.descriptor.max_satisfaction_weight();

        let (available_utxos, use_all_utxos) =
            self.get_available_utxos(&utxos, &unspendable, send_all)?;
        let (mut inputs, paths, selected_amount, mut fee_val) = self.coin_select(
            available_utxos,
            use_all_utxos,
            fee_rate,
            outgoing,
            input_witness_weight,
            fee_val,
        )?;
        let n_sequence = if let Some(csv) = requirements.csv {
            csv
        } else if requirements.timelock.is_some() {
            0xFFFFFFFE
        } else {
            0xFFFFFFFF
        };
        inputs.iter_mut().for_each(|i| i.sequence = n_sequence);
        tx.input.append(&mut inputs);

        // prepare the change output
        let change_output = match send_all {
            true => None,
            false => {
                let change_script = self.get_change_address()?;
                let change_output = TxOut {
                    script_pubkey: change_script,
                    value: 0,
                };

                // take the change into account for fees
                fee_val += calc_fee_bytes(serialize(&change_output).len() * 4);
                Some(change_output)
            }
        };

        let change_val = selected_amount - outgoing - (fee_val.ceil() as u64);
        if !send_all && !change_val.is_dust() {
            let mut change_output = change_output.unwrap();
            change_output.value = change_val;
            received += change_val;

            tx.output.push(change_output);
        } else if send_all && !change_val.is_dust() {
            // set the outgoing value to whatever we've put in
            outgoing = selected_amount;
            // there's only one output, send everything to it
            tx.output[0].value = change_val;

            // send_all to our address
            if self.is_mine(&tx.output[0].script_pubkey)? {
                received = change_val;
            }
        } else if send_all {
            // send_all but the only output would be below dust limit
            return Err(Error::InsufficientFunds); // TODO: or OutputBelowDustLimit?
        }

        // TODO: shuffle the outputs

        let txid = tx.txid();
        let mut psbt = PSBT::from_unsigned_tx(tx)?;

        // add metadata for the inputs
        for ((psbt_input, (script_type, child)), input) in psbt
            .inputs
            .iter_mut()
            .zip(paths.into_iter())
            .zip(psbt.global.unsigned_tx.input.iter())
        {
            let desc = self.get_descriptor_for(script_type);
            psbt_input.hd_keypaths = desc.get_hd_keypaths(child).unwrap();
            let derived_descriptor = desc.derive(child).unwrap();

            // TODO: figure out what do redeem_script and witness_script mean
            psbt_input.redeem_script = derived_descriptor.psbt_redeem_script();
            psbt_input.witness_script = derived_descriptor.psbt_witness_script();

            let prev_output = input.previous_output;
            let prev_tx = self
                .database
                .borrow()
                .get_raw_tx(&prev_output.txid)?
                .unwrap(); // TODO: remove unwrap

            if derived_descriptor.is_witness() {
                psbt_input.witness_utxo = Some(prev_tx.output[prev_output.vout as usize].clone());
            } else {
                psbt_input.non_witness_utxo = Some(prev_tx);
            };

            // we always sign with SIGHASH_ALL
            psbt_input.sighash_type = Some(SigHashType::All);
        }

        for (psbt_output, tx_output) in psbt
            .outputs
            .iter_mut()
            .zip(psbt.global.unsigned_tx.output.iter())
        {
            if let Some((script_type, child)) = self
                .database
                .borrow()
                .get_path_from_script_pubkey(&tx_output.script_pubkey)?
            {
                let desc = self.get_descriptor_for(script_type);
                psbt_output.hd_keypaths = desc.get_hd_keypaths(child)?;
            }
        }

        let transaction_details = TransactionDetails {
            transaction: None,
            txid: txid,
            timestamp: Self::get_timestamp(),
            received,
            sent: outgoing,
            height: None,
        };

        Ok((psbt, transaction_details))
    }

    // TODO: define an enum for signing errors
    pub fn sign(&self, mut psbt: PSBT, assume_height: Option<u32>) -> Result<(PSBT, bool), Error> {
        // this helps us doing our job later
        self.add_hd_keypaths(&mut psbt)?;

        let tx = &psbt.global.unsigned_tx;

        let mut signer = PSBTSigner::from_descriptor(&psbt.global.unsigned_tx, &self.descriptor)?;
        if let Some(desc) = &self.change_descriptor {
            let change_signer = PSBTSigner::from_descriptor(&psbt.global.unsigned_tx, desc)?;
            signer.extend(change_signer)?;
        }

        // sign everything we can. TODO: ideally we should only sign with the keys in the policy
        // path selected, if present
        for (i, input) in psbt.inputs.iter_mut().enumerate() {
            let sighash = input.sighash_type.unwrap_or(SigHashType::All);
            let prevout = tx.input[i].previous_output;

            let mut partial_sigs = BTreeMap::new();
            {
                let mut push_sig = |pubkey: &PublicKey, opt_sig: Option<BitcoinSig>| {
                    if let Some((signature, sighash)) = opt_sig {
                        let mut concat_sig = Vec::new();
                        concat_sig.extend_from_slice(&signature.serialize_der());
                        concat_sig.extend_from_slice(&[sighash as u8]);
                        //input.partial_sigs.insert(*pubkey, concat_sig);
                        partial_sigs.insert(*pubkey, concat_sig);
                    }
                };

                if let Some(non_wit_utxo) = &input.non_witness_utxo {
                    if non_wit_utxo.txid() != prevout.txid {
                        return Err(Error::InputTxidMismatch((non_wit_utxo.txid(), prevout)));
                    }

                    let prev_script = &non_wit_utxo.output
                        [psbt.global.unsigned_tx.input[i].previous_output.vout as usize]
                        .script_pubkey;

                    // return (signature, sighash) from here
                    let sign_script = if let Some(redeem_script) = &input.redeem_script {
                        if &redeem_script.to_p2sh() != prev_script {
                            return Err(Error::InputRedeemScriptMismatch((
                                prev_script.clone(),
                                redeem_script.clone(),
                            )));
                        }

                        redeem_script
                    } else {
                        prev_script
                    };

                    for (pubkey, (fing, path)) in &input.hd_keypaths {
                        push_sig(
                            pubkey,
                            signer.sig_legacy_from_fingerprint(
                                i,
                                sighash,
                                fing,
                                path,
                                sign_script,
                            )?,
                        );
                    }
                    // TODO: this sucks, we sign with every key
                    for pubkey in signer.all_public_keys() {
                        push_sig(
                            pubkey,
                            signer.sig_legacy_from_pubkey(i, sighash, pubkey, sign_script)?,
                        );
                    }
                } else if let Some(witness_utxo) = &input.witness_utxo {
                    let value = witness_utxo.value;

                    let script = match &input.redeem_script {
                        Some(script) if script.to_p2sh() != witness_utxo.script_pubkey => {
                            return Err(Error::InputRedeemScriptMismatch((
                                witness_utxo.script_pubkey.clone(),
                                script.clone(),
                            )))
                        }
                        Some(script) => script,
                        None => &witness_utxo.script_pubkey,
                    };

                    let sign_script = if script.is_v0_p2wpkh() {
                        self.to_p2pkh(&script.as_bytes()[2..])
                    } else if script.is_v0_p2wsh() {
                        match &input.witness_script {
                            None => Err(Error::InputMissingWitnessScript(i)),
                            Some(witness_script) if script != &witness_script.to_v0_p2wsh() => {
                                Err(Error::InputRedeemScriptMismatch((
                                    script.clone(),
                                    witness_script.clone(),
                                )))
                            }
                            Some(witness_script) => Ok(witness_script),
                        }?
                        .clone()
                    } else {
                        return Err(Error::InputUnknownSegwitScript(script.clone()));
                    };

                    for (pubkey, (fing, path)) in &input.hd_keypaths {
                        push_sig(
                            pubkey,
                            signer.sig_segwit_from_fingerprint(
                                i,
                                sighash,
                                fing,
                                path,
                                &sign_script,
                                value,
                            )?,
                        );
                    }
                    // TODO: this sucks, we sign with every key
                    for pubkey in signer.all_public_keys() {
                        push_sig(
                            pubkey,
                            signer.sig_segwit_from_pubkey(
                                i,
                                sighash,
                                pubkey,
                                &sign_script,
                                value,
                            )?,
                        );
                    }
                } else {
                    return Err(Error::MissingUTXO);
                }
            }

            // push all the signatures into the psbt
            input.partial_sigs.append(&mut partial_sigs);
        }

        // attempt to finalize
        let finalized = self.finalize_psbt(&mut psbt, assume_height)?;

        Ok((psbt, finalized))
    }

    pub fn policies(&self, script_type: ScriptType) -> Result<Option<Policy>, Error> {
        match (script_type, self.change_descriptor.as_ref()) {
            (ScriptType::External, _) => Ok(self.descriptor.extract_policy()?),
            (ScriptType::Internal, None) => Ok(None),
            (ScriptType::Internal, Some(desc)) => Ok(desc.extract_policy()?),
        }
    }

    pub fn public_descriptor(
        &self,
        script_type: ScriptType,
    ) -> Result<Option<ExtendedDescriptor>, Error> {
        match (script_type, self.change_descriptor.as_ref()) {
            (ScriptType::External, _) => Ok(Some(self.descriptor.as_public_version()?)),
            (ScriptType::Internal, None) => Ok(None),
            (ScriptType::Internal, Some(desc)) => Ok(Some(desc.as_public_version()?)),
        }
    }

    pub fn finalize_psbt(
        &self,
        psbt: &mut PSBT,
        assume_height: Option<u32>,
    ) -> Result<bool, Error> {
        let mut tx = psbt.global.unsigned_tx.clone();

        for (n, input) in tx.input.iter_mut().enumerate() {
            // safe to run only on the descriptor because we assume the change descriptor also has
            // the same structure
            let desc = self.descriptor.derive_from_psbt_input(psbt, n);
            debug!("{:?}", psbt.inputs[n].hd_keypaths);
            debug!("reconstructed descriptor is {:?}", desc);

            let desc = match desc {
                Err(_) => return Ok(false),
                Ok(desc) => desc,
            };

            // if the height is None in the database it means it's still unconfirmed, so consider
            // that as a very high value
            let create_height = self
                .database
                .borrow()
                .get_tx(&input.previous_output.txid, false)?
                .and_then(|tx| Some(tx.height.unwrap_or(std::u32::MAX)));
            let current_height = assume_height.or(self.current_height);

            debug!(
                "Input #{} - {}, using `create_height` = {:?}, `current_height` = {:?}",
                n, input.previous_output, create_height, current_height
            );

            // TODO: use height once we sync headers
            let satisfier =
                PSBTSatisfier::new(&psbt.inputs[n], false, create_height, current_height);

            match desc.satisfy(input, satisfier) {
                Ok(_) => continue,
                Err(e) => {
                    debug!("satisfy error {:?} for input {}", e, n);
                    return Ok(false);
                }
            }
        }

        // consume tx to extract its input's script_sig and witnesses and move them into the psbt
        for (input, psbt_input) in tx.input.into_iter().zip(psbt.inputs.iter_mut()) {
            psbt_input.final_script_sig = Some(input.script_sig);
            psbt_input.final_script_witness = Some(input.witness);
        }

        Ok(true)
    }

    // Internals

    #[cfg(not(target_arch = "wasm32"))]
    fn get_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[cfg(target_arch = "wasm32")]
    fn get_timestamp() -> u64 {
        0
    }

    fn get_descriptor_for(&self, script_type: ScriptType) -> &ExtendedDescriptor {
        let desc = match script_type {
            ScriptType::External => &self.descriptor,
            ScriptType::Internal => &self.change_descriptor.as_ref().unwrap_or(&self.descriptor),
        };

        desc
    }

    fn to_p2pkh(&self, pubkey_hash: &[u8]) -> Script {
        Builder::new()
            .push_opcode(opcodes::all::OP_DUP)
            .push_opcode(opcodes::all::OP_HASH160)
            .push_slice(pubkey_hash)
            .push_opcode(opcodes::all::OP_EQUALVERIFY)
            .push_opcode(opcodes::all::OP_CHECKSIG)
            .into_script()
    }

    fn get_change_address(&self) -> Result<Script, Error> {
        let (desc, script_type) = if self.change_descriptor.is_none() {
            (&self.descriptor, ScriptType::External)
        } else {
            (
                self.change_descriptor.as_ref().unwrap(),
                ScriptType::Internal,
            )
        };

        // TODO: refill the address pool if index is close to the last cached addr
        let index = self
            .database
            .borrow_mut()
            .increment_last_index(script_type)?;

        Ok(desc.derive(index)?.script_pubkey())
    }

    fn get_available_utxos(
        &self,
        utxo: &Option<Vec<OutPoint>>,
        unspendable: &Option<Vec<OutPoint>>,
        send_all: bool,
    ) -> Result<(Vec<UTXO>, bool), Error> {
        // TODO: should we consider unconfirmed received rbf txs as "unspendable" too by default?
        let unspendable_set = match unspendable {
            None => HashSet::new(),
            Some(vec) => vec.into_iter().collect(),
        };

        match utxo {
            // with manual coin selection we always want to spend all the selected utxos, no matter
            // what (even if they are marked as unspendable)
            Some(raw_utxos) => {
                // TODO: unwrap to remove
                let full_utxos: Vec<_> = raw_utxos
                    .iter()
                    .map(|u| self.database.borrow().get_utxo(&u).unwrap())
                    .collect();
                if !full_utxos.iter().all(|u| u.is_some()) {
                    return Err(Error::UnknownUTXO);
                }

                Ok((full_utxos.into_iter().map(|x| x.unwrap()).collect(), true))
            }
            // otherwise limit ourselves to the spendable utxos and the `send_all` setting
            None => Ok((
                self.list_unspent()?
                    .into_iter()
                    .filter(|u| !unspendable_set.contains(&u.outpoint))
                    .collect(),
                send_all,
            )),
        }
    }

    fn coin_select(
        &self,
        mut utxos: Vec<UTXO>,
        use_all_utxos: bool,
        fee_rate: f32,
        outgoing: u64,
        input_witness_weight: usize,
        mut fee_val: f32,
    ) -> Result<(Vec<TxIn>, Vec<(ScriptType, u32)>, u64, f32), Error> {
        let mut answer = Vec::new();
        let mut deriv_indexes = Vec::new();
        let calc_fee_bytes = |wu| (wu as f32) * fee_rate / 4.0;

        debug!(
            "coin select: outgoing = `{}`, fee_val = `{}`, fee_rate = `{}`",
            outgoing, fee_val, fee_rate
        );

        // sort so that we pick them starting from the larger. TODO: proper coin selection
        utxos.sort_by(|a, b| a.txout.value.partial_cmp(&b.txout.value).unwrap());

        let mut selected_amount: u64 = 0;
        while use_all_utxos || selected_amount < outgoing + (fee_val.ceil() as u64) {
            let utxo = match utxos.pop() {
                Some(utxo) => utxo,
                None if selected_amount < outgoing + (fee_val.ceil() as u64) => {
                    return Err(Error::InsufficientFunds)
                }
                None if use_all_utxos => break,
                None => return Err(Error::InsufficientFunds),
            };

            let new_in = TxIn {
                previous_output: utxo.outpoint,
                script_sig: Script::default(),
                sequence: 0xFFFFFFFD, // TODO: change according to rbf/csv
                witness: vec![],
            };
            fee_val += calc_fee_bytes(serialize(&new_in).len() * 4 + input_witness_weight);
            debug!("coin select new fee_val = `{}`", fee_val);

            answer.push(new_in);
            selected_amount += utxo.txout.value;

            let child = self
                .database
                .borrow()
                .get_path_from_script_pubkey(&utxo.txout.script_pubkey)?
                .unwrap(); // TODO: remove unrwap
            deriv_indexes.push(child);
        }

        Ok((answer, deriv_indexes, selected_amount, fee_val))
    }

    fn add_hd_keypaths(&self, psbt: &mut PSBT) -> Result<(), Error> {
        let mut input_utxos = Vec::with_capacity(psbt.inputs.len());
        for n in 0..psbt.inputs.len() {
            input_utxos.push(psbt.get_utxo_for(n).clone());
        }

        // try to add hd_keypaths if we've already seen the output
        for (psbt_input, out) in psbt.inputs.iter_mut().zip(input_utxos.iter()) {
            debug!("searching hd_keypaths for out: {:?}", out);

            if let Some(out) = out {
                let option_path = self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&out.script_pubkey)?;

                debug!("found descriptor path {:?}", option_path);

                let (script_type, child) = match option_path {
                    None => continue,
                    Some((script_type, child)) => (script_type, child),
                };

                // merge hd_keypaths
                let desc = self.get_descriptor_for(script_type);
                let mut hd_keypaths = desc.get_hd_keypaths(child)?;
                psbt_input.hd_keypaths.append(&mut hd_keypaths);
            }
        }

        Ok(())
    }
}

impl<B, D> Wallet<B, D>
where
    B: OnlineBlockchain,
    D: BatchDatabase,
{
    pub fn new(
        descriptor: &str,
        change_descriptor: Option<&str>,
        network: Network,
        mut database: D,
        mut client: B,
    ) -> Result<Self, Error> {
        database.check_descriptor_checksum(
            ScriptType::External,
            get_checksum(descriptor)?.as_bytes(),
        )?;
        let descriptor = ExtendedDescriptor::from_str(descriptor)?;
        let change_descriptor = match change_descriptor {
            Some(desc) => {
                database.check_descriptor_checksum(
                    ScriptType::Internal,
                    get_checksum(desc)?.as_bytes(),
                )?;

                let parsed = ExtendedDescriptor::from_str(desc)?;
                if !parsed.same_structure(descriptor.as_ref()) {
                    return Err(Error::DifferentDescriptorStructure);
                }

                Some(parsed)
            }
            None => None,
        };

        let current_height = Some(client.get_height()? as u32);

        Ok(Wallet {
            descriptor,
            change_descriptor,
            network,

            current_height,

            client: RefCell::new(client),
            database: RefCell::new(database),
        })
    }

    pub fn sync(
        &self,
        max_address: Option<u32>,
        _batch_query_size: Option<usize>,
    ) -> Result<(), Error> {
        debug!("begin sync...");
        // TODO: consider taking an RwLock as writere here to prevent other "read-only" calls to
        // break because the db is in an inconsistent state

        let max_address = if self.descriptor.is_fixed() {
            0
        } else {
            max_address.unwrap_or(100)
        };

        // TODO:
        // let batch_query_size = batch_query_size.unwrap_or(20);

        let last_addr = self
            .database
            .borrow()
            .get_script_pubkey_from_path(ScriptType::External, max_address)?;

        // cache a few of our addresses
        if last_addr.is_none() {
            let mut address_batch = self.database.borrow().begin_batch();
            #[cfg(not(target_arch = "wasm32"))]
            let start = Instant::now();

            for i in 0..=max_address {
                let derived = self.descriptor.derive(i).unwrap();

                address_batch.set_script_pubkey(
                    &derived.script_pubkey(),
                    ScriptType::External,
                    i,
                )?;
            }
            if self.change_descriptor.is_some() {
                for i in 0..=max_address {
                    let derived = self.change_descriptor.as_ref().unwrap().derive(i).unwrap();

                    address_batch.set_script_pubkey(
                        &derived.script_pubkey(),
                        ScriptType::Internal,
                        i,
                    )?;
                }
            }

            #[cfg(not(target_arch = "wasm32"))]
            info!(
                "derivation of {} addresses, took {} ms",
                max_address,
                start.elapsed().as_millis()
            );
            self.database.borrow_mut().commit_batch(address_batch)?;
        }

        self.client.borrow_mut().sync(
            None,
            self.database.borrow_mut().deref_mut(),
            noop_progress(),
        )
    }

    pub fn broadcast(&self, tx: Transaction) -> Result<Txid, Error> {
        self.client.borrow_mut().broadcast(&tx)?;

        Ok(tx.txid())
    }
}
