use std::cell::RefCell;
use std::cmp;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::convert::TryFrom;
use std::io::{Read, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bitcoin::blockdata::opcodes;
use bitcoin::blockdata::script::Builder;
use bitcoin::consensus::encode::serialize;
use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::util::bip32::{ChildNumber, DerivationPath};
use bitcoin::util::psbt::PartiallySignedTransaction as PSBT;
use bitcoin::{
    Address, Network, OutPoint, PublicKey, Script, SigHashType, Transaction, TxIn, TxOut, Txid,
};

use miniscript::BitcoinSig;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

pub mod offline_stream;
pub mod utils;

use self::utils::{ChunksIterator, IsDust};
use crate::database::{BatchDatabase, BatchOperations};
use crate::descriptor::{
    DerivedDescriptor, DescriptorMeta, ExtendedDescriptor, ExtractPolicy, Policy,
};
use crate::error::Error;
use crate::psbt::{PSBTSatisfier, PSBTSigner};
use crate::signer::Signer;
use crate::types::*;

#[cfg(any(feature = "electrum", feature = "default"))]
use electrum_client::types::*;
#[cfg(any(feature = "electrum", feature = "default"))]
use electrum_client::Client;
#[cfg(not(any(feature = "electrum", feature = "default")))]
use std::marker::PhantomData as Client;

// TODO: force descriptor and change_descriptor to have the same policies?
pub struct Wallet<S: Read + Write, D: BatchDatabase> {
    descriptor: ExtendedDescriptor,
    change_descriptor: Option<ExtendedDescriptor>,
    network: Network,

    client: Option<RefCell<Client<S>>>,
    database: RefCell<D>, // TODO: save descriptor checksum and check when loading
    _secp: Secp256k1<All>,
}

// offline actions, always available
impl<S, D> Wallet<S, D>
where
    S: Read + Write,
    D: BatchDatabase,
{
    pub fn new_offline(
        descriptor: ExtendedDescriptor,
        change_descriptor: Option<ExtendedDescriptor>,
        network: Network,
        database: D,
    ) -> Self {
        Wallet {
            descriptor,
            change_descriptor,
            network,

            client: None,
            database: RefCell::new(database),
            _secp: Secp256k1::gen_new(),
        }
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
        self.get_path(script).map(|x| x.is_some())
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
        policy_path: Option<Vec<Vec<usize>>>,
        utxos: Option<Vec<OutPoint>>,
        unspendable: Option<Vec<OutPoint>>,
    ) -> Result<(PSBT, TransactionDetails), Error> {
        // TODO: run before deriving the descriptor
        let policy = self.descriptor.extract_policy().unwrap();
        if policy.requires_path() && policy_path.is_none() {
            return Err(Error::SpendingPolicyRequired);
        }
        let requirements = policy_path.map_or(Ok(Default::default()), |path| {
            policy.get_requirements(&path)
        })?;
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
        inputs
            .iter_mut()
            .for_each(|i| i.sequence = requirements.csv.unwrap_or(0xFFFFFFFF));
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
        for ((psbt_input, (script_type, path)), input) in psbt
            .inputs
            .iter_mut()
            .zip(paths.into_iter())
            .zip(psbt.global.unsigned_tx.input.iter())
        {
            let path: Vec<ChildNumber> = path.into();
            let index = match path.last() {
                Some(ChildNumber::Normal { index }) => *index,
                Some(ChildNumber::Hardened { index }) => *index,
                None => 0,
            };

            let desc = self.get_descriptor_for(script_type);
            psbt_input.hd_keypaths = desc.get_hd_keypaths(index).unwrap();
            let derived_descriptor = desc.derive(index).unwrap();

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

        // TODO: add metadata for the outputs, like derivation paths for change addrs
        /*for psbt_output in psbt.outputs.iter_mut().zip(psbt.global.unsigned_tx.output.iter()) {
        }*/

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
    pub fn sign(&self, mut psbt: PSBT) -> Result<(PSBT, bool), Error> {
        let mut derived_descriptors = BTreeMap::new();

        let tx = &psbt.global.unsigned_tx;

        // try to add hd_keypaths if we've already seen the output
        for (n, psbt_input) in psbt.inputs.iter_mut().enumerate() {
            let out = match (&psbt_input.witness_utxo, &psbt_input.non_witness_utxo) {
                (Some(wit_out), _) => Some(wit_out),
                (_, Some(in_tx))
                    if (tx.input[n].previous_output.vout as usize) < in_tx.output.len() =>
                {
                    Some(&in_tx.output[tx.input[n].previous_output.vout as usize])
                }
                _ => None,
            };

            debug!("searching hd_keypaths for out: {:?}", out);

            if let Some(out) = out {
                let option_path = self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&out.script_pubkey)?;

                debug!("found descriptor path {:?}", option_path);

                let (script_type, path) = match option_path {
                    None => continue,
                    Some((script_type, path)) => (script_type, path),
                };

                // TODO: this is duplicated code
                let index = match path.into_iter().last() {
                    Some(ChildNumber::Normal { index }) => *index,
                    Some(ChildNumber::Hardened { index }) => *index,
                    None => 0,
                };

                let desc = self.get_descriptor_for(script_type);
                let derived_descriptor = desc.derive(index)?;
                derived_descriptors.insert(n, derived_descriptor);

                // merge hd_keypaths
                let mut hd_keypaths = desc.get_hd_keypaths(index)?;
                psbt_input.hd_keypaths.append(&mut hd_keypaths);
            }
        }

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
        let finalized = self.finalize_psbt(tx.clone(), &mut psbt, derived_descriptors);

        Ok((psbt, finalized))
    }

    pub fn policies(&self, script_type: ScriptType) -> Result<Option<Policy>, Error> {
        match (script_type, self.change_descriptor.as_ref()) {
            (ScriptType::External, _) => Ok(self.descriptor.extract_policy()),
            (ScriptType::Internal, None) => Ok(None),
            (ScriptType::Internal, Some(desc)) => Ok(desc.extract_policy()),
        }
    }

    // Internals

    fn get_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn get_path(&self, script: &Script) -> Result<Option<(ScriptType, DerivationPath)>, Error> {
        self.database.borrow().get_path_from_script_pubkey(script)
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
    ) -> Result<(Vec<TxIn>, Vec<(ScriptType, DerivationPath)>, u64, f32), Error> {
        let mut answer = Vec::new();
        let mut paths = Vec::new();
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

            let path = self
                .database
                .borrow()
                .get_path_from_script_pubkey(&utxo.txout.script_pubkey)?
                .unwrap(); // TODO: remove unrwap
            paths.push(path);
        }

        Ok((answer, paths, selected_amount, fee_val))
    }

    fn finalize_psbt(
        &self,
        mut tx: Transaction,
        psbt: &mut PSBT,
        derived_descriptors: BTreeMap<usize, DerivedDescriptor>,
    ) -> bool {
        for (n, input) in tx.input.iter_mut().enumerate() {
            debug!("getting descriptor for {}", n);

            let desc = match derived_descriptors.get(&n) {
                None => return false,
                Some(desc) => desc,
            };

            // TODO: use height once we sync headers
            let satisfier = PSBTSatisfier::new(&psbt.inputs[n], None, None);

            match desc.satisfy(input, satisfier) {
                Ok(_) => continue,
                Err(e) => {
                    debug!("satisfy error {:?} for input {}", e, n);
                    return false;
                }
            }
        }

        // consume tx to extract its input's script_sig and witnesses and move them into the psbt
        for (input, psbt_input) in tx.input.into_iter().zip(psbt.inputs.iter_mut()) {
            psbt_input.final_script_sig = Some(input.script_sig);
            psbt_input.final_script_witness = Some(input.witness);
        }

        true
    }
}

#[cfg(any(feature = "electrum", feature = "default"))]
impl<S, D> Wallet<S, D>
where
    S: Read + Write,
    D: BatchDatabase,
{
    pub fn new(
        descriptor: ExtendedDescriptor,
        change_descriptor: Option<ExtendedDescriptor>,
        network: Network,
        database: D,
        client: Client<S>,
    ) -> Self {
        Wallet {
            descriptor,
            change_descriptor,
            network,

            client: Some(RefCell::new(client)),
            database: RefCell::new(database),
            _secp: Secp256k1::gen_new(),
        }
    }

    fn get_previous_output(&self, outpoint: &OutPoint) -> Option<TxOut> {
        // the fact that we visit addresses in a BFS fashion starting from the external addresses
        // should ensure that this query is always consistent (i.e. when we get to call this all
        // the transactions at a lower depth have already been indexed, so if an outpoint is ours
        // we are guaranteed to have it in the db).
        self.database
            .borrow()
            .get_raw_tx(&outpoint.txid)
            .unwrap()
            .map(|previous_tx| previous_tx.output[outpoint.vout as usize].clone())
    }

    fn check_tx_and_descendant(
        &self,
        txid: &Txid,
        height: Option<u32>,
        cur_script: &Script,
        change_max_deriv: &mut u32,
    ) -> Result<Vec<Script>, Error> {
        debug!(
            "check_tx_and_descendant of {}, height: {:?}, script: {}",
            txid, height, cur_script
        );
        let mut updates = self.database.borrow().begin_batch();
        let tx = match self.database.borrow().get_tx(&txid, true)? {
            // TODO: do we need the raw?
            Some(mut saved_tx) => {
                // update the height if it's different (in case of reorg)
                if saved_tx.height != height {
                    info!(
                        "updating height from {:?} to {:?} for tx {}",
                        saved_tx.height, height, txid
                    );
                    saved_tx.height = height;
                    updates.set_tx(&saved_tx)?;
                }

                debug!("already have {} in db, returning the cached version", txid);

                // unwrap since we explicitly ask for the raw_tx, if it's not present something
                // went wrong
                saved_tx.transaction.unwrap()
            }
            None => self
                .client
                .as_ref()
                .unwrap()
                .borrow_mut()
                .transaction_get(&txid)?,
        };

        let mut incoming: u64 = 0;
        let mut outgoing: u64 = 0;

        // look for our own inputs
        for (i, input) in tx.input.iter().enumerate() {
            if let Some(previous_output) = self.get_previous_output(&input.previous_output) {
                if self.is_mine(&previous_output.script_pubkey)? {
                    outgoing += previous_output.value;

                    debug!("{} input #{} is mine, removing from utxo", txid, i);
                    updates.del_utxo(&input.previous_output)?;
                }
            }
        }

        let mut to_check_later = vec![];
        for (i, output) in tx.output.iter().enumerate() {
            // this output is ours, we have a path to derive it
            if let Some((script_type, path)) = self.get_path(&output.script_pubkey)? {
                debug!("{} output #{} is mine, adding utxo", txid, i);
                updates.set_utxo(&UTXO {
                    outpoint: OutPoint::new(tx.txid(), i as u32),
                    txout: output.clone(),
                })?;
                incoming += output.value;

                if output.script_pubkey != *cur_script {
                    debug!("{} output #{} script {} was not current script, adding script to be checked later", txid, i, output.script_pubkey);
                    to_check_later.push(output.script_pubkey.clone())
                }

                // derive as many change addrs as external addresses that we've seen
                if script_type == ScriptType::Internal
                    && u32::from(path.as_ref()[0]) > *change_max_deriv
                {
                    *change_max_deriv = u32::from(path.as_ref()[0]);
                }
            }
        }

        let tx = TransactionDetails {
            txid: tx.txid(),
            transaction: Some(tx),
            received: incoming,
            sent: outgoing,
            height,
            timestamp: 0,
        };
        info!("Saving tx {}", txid);

        updates.set_tx(&tx)?;
        self.database.borrow_mut().commit_batch(updates)?;

        Ok(to_check_later)
    }

    fn check_history(
        &self,
        script_pubkey: Script,
        txs: Vec<GetHistoryRes>,
        change_max_deriv: &mut u32,
    ) -> Result<Vec<Script>, Error> {
        let mut to_check_later = Vec::new();

        debug!(
            "history of {} script {} has {} tx",
            Address::from_script(&script_pubkey, self.network).unwrap(),
            script_pubkey,
            txs.len()
        );

        for tx in txs {
            let height: Option<u32> = match tx.height {
                0 | -1 => None,
                x => u32::try_from(x).ok(),
            };

            to_check_later.extend_from_slice(&self.check_tx_and_descendant(
                &tx.tx_hash,
                height,
                &script_pubkey,
                change_max_deriv,
            )?);
        }

        Ok(to_check_later)
    }

    pub fn sync(
        &self,
        max_address: Option<u32>,
        batch_query_size: Option<usize>,
    ) -> Result<(), Error> {
        debug!("begin sync...");
        // TODO: consider taking an RwLock as writere here to prevent other "read-only" calls to
        // break because the db is in an inconsistent state

        let max_address = if self.descriptor.is_fixed() {
            0
        } else {
            max_address.unwrap_or(100)
        };

        let batch_query_size = batch_query_size.unwrap_or(20);
        let stop_gap = batch_query_size;

        let path = DerivationPath::from(vec![ChildNumber::Normal { index: max_address }]);
        let last_addr = self
            .database
            .borrow()
            .get_script_pubkey_from_path(ScriptType::External, &path)?;

        // cache a few of our addresses
        if last_addr.is_none() {
            let mut address_batch = self.database.borrow().begin_batch();
            let start = Instant::now();

            for i in 0..=max_address {
                let derived = self.descriptor.derive(i).unwrap();
                let full_path = DerivationPath::from(vec![ChildNumber::Normal { index: i }]);

                address_batch.set_script_pubkey(
                    &derived.script_pubkey(),
                    ScriptType::External,
                    &full_path,
                )?;
            }
            if self.change_descriptor.is_some() {
                for i in 0..=max_address {
                    let derived = self.change_descriptor.as_ref().unwrap().derive(i).unwrap();
                    let full_path = DerivationPath::from(vec![ChildNumber::Normal { index: i }]);

                    address_batch.set_script_pubkey(
                        &derived.script_pubkey(),
                        ScriptType::Internal,
                        &full_path,
                    )?;
                }
            }

            info!(
                "derivation of {} addresses, took {} ms",
                max_address,
                start.elapsed().as_millis()
            );
            self.database.borrow_mut().commit_batch(address_batch)?;
        }

        // check unconfirmed tx, delete so they are retrieved later
        let mut del_batch = self.database.borrow().begin_batch();
        for tx in self.database.borrow().iter_txs(false)? {
            if tx.height.is_none() {
                del_batch.del_tx(&tx.txid, false)?;
            }
        }
        self.database.borrow_mut().commit_batch(del_batch)?;

        // maximum derivation index for a change address that we've seen during sync
        let mut change_max_deriv = 0;

        let mut already_checked: HashSet<Script> = HashSet::new();
        let mut to_check_later = VecDeque::with_capacity(batch_query_size);

        // insert the first chunk
        let mut iter_scriptpubkeys = self
            .database
            .borrow()
            .iter_script_pubkeys(Some(ScriptType::External))?
            .into_iter();
        let chunk: Vec<Script> = iter_scriptpubkeys.by_ref().take(batch_query_size).collect();
        for item in chunk.into_iter().rev() {
            to_check_later.push_front(item);
        }

        let mut iterating_external = true;
        let mut index = 0;
        let mut last_found = 0;
        while !to_check_later.is_empty() {
            trace!("to_check_later size {}", to_check_later.len());

            let until = cmp::min(to_check_later.len(), batch_query_size);
            let chunk: Vec<Script> = to_check_later.drain(..until).collect();
            let call_result = self
                .client
                .as_ref()
                .unwrap()
                .borrow_mut()
                .batch_script_get_history(chunk.iter().collect::<Vec<_>>())?; // TODO: fix electrum client

            for (script, history) in chunk.into_iter().zip(call_result.into_iter()) {
                trace!("received history for {:?}, size {}", script, history.len());

                if !history.is_empty() {
                    last_found = index;

                    let mut check_later_scripts = self
                        .check_history(script, history, &mut change_max_deriv)?
                        .into_iter()
                        .filter(|x| already_checked.insert(x.clone()))
                        .collect();
                    to_check_later.append(&mut check_later_scripts);
                }

                index += 1;
            }

            match iterating_external {
                true if index - last_found >= stop_gap => iterating_external = false,
                true => {
                    trace!("pushing one more batch from `iter_scriptpubkeys`. index = {}, last_found = {}, stop_gap = {}", index, last_found, stop_gap);

                    let chunk: Vec<Script> =
                        iter_scriptpubkeys.by_ref().take(batch_query_size).collect();
                    for item in chunk.into_iter().rev() {
                        to_check_later.push_front(item);
                    }
                }
                _ => {}
            }
        }

        // check utxo
        // TODO: try to minimize network requests and re-use scripts if possible
        let mut batch = self.database.borrow().begin_batch();
        for chunk in ChunksIterator::new(
            self.database.borrow().iter_utxos()?.into_iter(),
            batch_query_size,
        ) {
            let scripts: Vec<_> = chunk.iter().map(|u| &u.txout.script_pubkey).collect();
            let call_result = self
                .client
                .as_ref()
                .unwrap()
                .borrow_mut()
                .batch_script_list_unspent(scripts)?;

            // check which utxos are actually still unspent
            for (utxo, list_unspent) in chunk.into_iter().zip(call_result.iter()) {
                debug!(
                    "outpoint {:?} is unspent for me, list unspent is {:?}",
                    utxo.outpoint, list_unspent
                );

                let mut spent = true;
                for unspent in list_unspent {
                    let res_outpoint = OutPoint::new(unspent.tx_hash, unspent.tx_pos as u32);
                    if utxo.outpoint == res_outpoint {
                        spent = false;
                        break;
                    }
                }
                if spent {
                    info!("{} not anymore unspent, removing", utxo.outpoint);
                    batch.del_utxo(&utxo.outpoint)?;
                }
            }
        }

        let current_ext = self
            .database
            .borrow()
            .get_last_index(ScriptType::External)?
            .unwrap_or(0);
        let first_ext_new = last_found as u32 + 1;
        if first_ext_new > current_ext {
            info!("Setting external index to {}", first_ext_new);
            self.database
                .borrow_mut()
                .set_last_index(ScriptType::External, first_ext_new)?;
        }

        let current_int = self
            .database
            .borrow()
            .get_last_index(ScriptType::Internal)?
            .unwrap_or(0);
        let first_int_new = change_max_deriv + 1;
        if first_int_new > current_int {
            info!("Setting internal index to {}", first_int_new);
            self.database
                .borrow_mut()
                .set_last_index(ScriptType::Internal, first_int_new)?;
        }

        self.database.borrow_mut().commit_batch(batch)?;

        Ok(())
    }

    pub fn broadcast(&mut self, psbt: PSBT) -> Result<Transaction, Error> {
        let extracted = psbt.extract_tx();
        self.client
            .as_ref()
            .unwrap()
            .borrow_mut()
            .transaction_broadcast(&extracted)?;

        Ok(extracted)
    }
}
