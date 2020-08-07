use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ops::DerefMut;
use std::str::FromStr;

use bitcoin::blockdata::opcodes;
use bitcoin::blockdata::script::Builder;
use bitcoin::consensus::encode::serialize;
use bitcoin::util::psbt::PartiallySignedTransaction as PSBT;
use bitcoin::{
    Address, Network, OutPoint, PublicKey, Script, SigHashType, Transaction, TxOut, Txid,
};

use miniscript::BitcoinSig;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

pub mod coin_selection;
pub mod export;
pub mod time;
pub mod tx_builder;
pub mod utils;

use tx_builder::TxBuilder;
use utils::{FeeRate, IsDust};

use crate::blockchain::{noop_progress, Blockchain, OfflineBlockchain, OnlineBlockchain};
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::descriptor::{get_checksum, DescriptorMeta, ExtendedDescriptor, ExtractPolicy, Policy};
use crate::error::Error;
use crate::psbt::{utils::PSBTUtils, PSBTSatisfier, PSBTSigner};
use crate::signer::Signer;
use crate::types::*;

const CACHE_ADDR_BATCH_SIZE: u32 = 100;

pub type OfflineWallet<D> = Wallet<OfflineBlockchain, D>;

pub struct Wallet<B: Blockchain, D: BatchDatabase> {
    descriptor: ExtendedDescriptor,
    change_descriptor: Option<ExtendedDescriptor>,
    network: Network,

    current_height: Option<u32>,

    client: B,
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

            client: B::offline(),
            database: RefCell::new(database),
        })
    }

    pub fn get_new_address(&self) -> Result<Address, Error> {
        let index = self.fetch_and_increment_index(ScriptType::External)?;

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

    pub fn create_tx<Cs: coin_selection::CoinSelectionAlgorithm>(
        &self,
        builder: TxBuilder<Cs>,
    ) -> Result<(PSBT, TransactionDetails), Error> {
        // TODO: fetch both internal and external policies
        let policy = self.descriptor.extract_policy()?.unwrap();
        if policy.requires_path() && builder.policy_path.is_none() {
            return Err(Error::SpendingPolicyRequired);
        }
        let requirements =
            policy.get_requirements(builder.policy_path.as_ref().unwrap_or(&BTreeMap::new()))?;
        debug!("requirements: {:?}", requirements);

        let version = match builder.version {
            tx_builder::Version(0) => return Err(Error::Generic("Invalid version `0`".into())),
            tx_builder::Version(1) if requirements.csv.is_some() => {
                return Err(Error::Generic(
                    "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
                        .into(),
                ))
            }
            tx_builder::Version(x) => x,
        };

        let lock_time = match builder.locktime {
            None => requirements.timelock.unwrap_or(0),
            Some(x) if requirements.timelock.is_none() => x,
            Some(x) if requirements.timelock.unwrap() <= x => x,
            Some(x) => return Err(Error::Generic(format!("TxBuilder requested timelock of `{}`, but at least `{}` is required to spend from this script", x, requirements.timelock.unwrap())))
        };

        let n_sequence = match (builder.rbf, requirements.csv) {
            (None, Some(csv)) => csv,
            (Some(rbf), Some(csv)) if rbf < csv => return Err(Error::Generic(format!("Cannot enable RBF with nSequence `{}`, since at least `{}` is required to spend with OP_CSV", rbf, csv))),
            (None, _) if requirements.timelock.is_some() => 0xFFFFFFFE,
            (Some(rbf), _) if rbf >= 0xFFFFFFFE => return Err(Error::Generic("Cannot enable RBF with anumber >= 0xFFFFFFFE".into())),
            (Some(rbf), _) => rbf,
            (None, _) => 0xFFFFFFFF,
        };

        let mut tx = Transaction {
            version,
            lock_time,
            input: vec![],
            output: vec![],
        };

        let fee_rate = builder.fee_rate.unwrap_or_default().as_sat_vb();
        if builder.send_all && builder.addressees.len() != 1 {
            return Err(Error::SendAllMultipleOutputs);
        }

        // we keep it as a float while we accumulate it, and only round it at the end
        let mut fee_amount: f32 = 0.0;
        let mut outgoing: u64 = 0;
        let mut received: u64 = 0;

        let calc_fee_bytes = |wu| (wu as f32) * fee_rate / 4.0;
        fee_amount += calc_fee_bytes(tx.get_weight());

        for (index, (address, satoshi)) in builder.addressees.iter().enumerate() {
            let value = match builder.send_all {
                true => 0,
                false if satoshi.is_dust() => return Err(Error::OutputBelowDustLimit(index)),
                false => *satoshi,
            };

            if address.network != self.network {
                return Err(Error::InvalidAddressNetork(address.clone()));
            } else if self.is_mine(&address.script_pubkey())? {
                received += value;
            }

            let new_out = TxOut {
                script_pubkey: address.script_pubkey(),
                value,
            };
            fee_amount += calc_fee_bytes(serialize(&new_out).len() * 4);

            tx.output.push(new_out);

            outgoing += value;
        }

        // TODO: use the right weight instead of the maximum, and only fall-back to it if the
        // script is unknown in the database
        let input_witness_weight = std::cmp::max(
            self.get_descriptor_for(ScriptType::Internal)
                .0
                .max_satisfaction_weight(),
            self.get_descriptor_for(ScriptType::External)
                .0
                .max_satisfaction_weight(),
        );

        let (available_utxos, use_all_utxos) = self.get_available_utxos(
            builder.change_policy,
            &builder.utxos,
            &builder.unspendable,
            builder.send_all,
        )?;
        let coin_selection::CoinSelectionResult {
            txin,
            total_amount,
            mut fee_amount,
        } = builder.coin_selection.coin_select(
            available_utxos,
            use_all_utxos,
            fee_rate,
            outgoing,
            input_witness_weight,
            fee_amount,
        )?;
        let (mut txin, prev_script_pubkeys): (Vec<_>, Vec<_>) = txin.into_iter().unzip();

        txin.iter_mut().for_each(|i| i.sequence = n_sequence);
        tx.input = txin;

        // prepare the change output
        let change_output = match builder.send_all {
            true => None,
            false => {
                let change_script = self.get_change_address()?;
                let change_output = TxOut {
                    script_pubkey: change_script,
                    value: 0,
                };

                // take the change into account for fees
                fee_amount += calc_fee_bytes(serialize(&change_output).len() * 4);
                Some(change_output)
            }
        };

        let change_val = total_amount - outgoing - (fee_amount.ceil() as u64);
        if !builder.send_all && !change_val.is_dust() {
            let mut change_output = change_output.unwrap();
            change_output.value = change_val;
            received += change_val;

            tx.output.push(change_output);
        } else if builder.send_all && !change_val.is_dust() {
            // set the outgoing value to whatever we've put in
            outgoing = total_amount;
            // there's only one output, send everything to it
            tx.output[0].value = change_val;

            // send_all to our address
            if self.is_mine(&tx.output[0].script_pubkey)? {
                received = change_val;
            }
        } else if builder.send_all {
            // send_all but the only output would be below dust limit
            return Err(Error::InsufficientFunds); // TODO: or OutputBelowDustLimit?
        }

        // sort input/outputs according to the chosen algorithm
        builder.ordering.modify_tx(&mut tx);

        let txid = tx.txid();
        let mut psbt = PSBT::from_unsigned_tx(tx)?;

        // add metadata for the inputs
        for ((psbt_input, prev_script), input) in psbt
            .inputs
            .iter_mut()
            .zip(prev_script_pubkeys.into_iter())
            .zip(psbt.global.unsigned_tx.input.iter())
        {
            // Add sighash, default is obviously "ALL"
            psbt_input.sighash_type = builder.sighash.or(Some(SigHashType::All));

            // Try to find the prev_script in our db to figure out if this is internal or external,
            // and the derivation index
            let (script_type, child) = match self
                .database
                .borrow()
                .get_path_from_script_pubkey(&prev_script)?
            {
                Some(x) => x,
                None => continue,
            };

            let (desc, _) = self.get_descriptor_for(script_type);
            psbt_input.hd_keypaths = desc.get_hd_keypaths(child)?;
            let derived_descriptor = desc.derive(child)?;

            psbt_input.redeem_script = derived_descriptor.psbt_redeem_script();
            psbt_input.witness_script = derived_descriptor.psbt_witness_script();

            let prev_output = input.previous_output;
            if let Some(prev_tx) = self.database.borrow().get_raw_tx(&prev_output.txid)? {
                if derived_descriptor.is_witness() {
                    psbt_input.witness_utxo =
                        Some(prev_tx.output[prev_output.vout as usize].clone());
                } else {
                    psbt_input.non_witness_utxo = Some(prev_tx);
                }
            }
        }

        self.add_hd_keypaths(&mut psbt)?;

        let transaction_details = TransactionDetails {
            transaction: None,
            txid,
            timestamp: time::get_timestamp(),
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

    fn get_descriptor_for(&self, script_type: ScriptType) -> (&ExtendedDescriptor, ScriptType) {
        let desc = match script_type {
            ScriptType::Internal if self.change_descriptor.is_some() => (
                self.change_descriptor.as_ref().unwrap(),
                ScriptType::Internal,
            ),
            _ => (&self.descriptor, ScriptType::External),
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
        let (desc, script_type) = self.get_descriptor_for(ScriptType::Internal);
        let index = self.fetch_and_increment_index(script_type)?;

        Ok(desc.derive(index)?.script_pubkey())
    }

    fn fetch_and_increment_index(&self, script_type: ScriptType) -> Result<u32, Error> {
        let (descriptor, script_type) = self.get_descriptor_for(script_type);
        let index = match descriptor.is_fixed() {
            true => 0,
            false => self
                .database
                .borrow_mut()
                .increment_last_index(script_type)?,
        };

        if self
            .database
            .borrow()
            .get_script_pubkey_from_path(script_type, index)?
            .is_none()
        {
            self.cache_addresses(script_type, index, CACHE_ADDR_BATCH_SIZE)?;
        }

        Ok(index)
    }

    fn cache_addresses(
        &self,
        script_type: ScriptType,
        from: u32,
        mut count: u32,
    ) -> Result<(), Error> {
        let (descriptor, script_type) = self.get_descriptor_for(script_type);
        if descriptor.is_fixed() {
            if from > 0 {
                return Ok(());
            }

            count = 1;
        }

        let mut address_batch = self.database.borrow().begin_batch();

        let start_time = time::Instant::new();
        for i in from..(from + count) {
            address_batch.set_script_pubkey(
                &descriptor.derive(i)?.script_pubkey(),
                script_type,
                i,
            )?;
        }

        info!(
            "Derivation of {} addresses from {} took {} ms",
            count,
            from,
            start_time.elapsed().as_millis()
        );

        self.database.borrow_mut().commit_batch(address_batch)?;

        Ok(())
    }

    fn get_available_utxos(
        &self,
        change_policy: tx_builder::ChangeSpendPolicy,
        utxo: &Option<Vec<OutPoint>>,
        unspendable: &Option<Vec<OutPoint>>,
        send_all: bool,
    ) -> Result<(Vec<UTXO>, bool), Error> {
        let unspendable_set = match unspendable {
            None => HashSet::new(),
            Some(vec) => vec.into_iter().collect(),
        };

        match utxo {
            // with manual coin selection we always want to spend all the selected utxos, no matter
            // what (even if they are marked as unspendable)
            Some(raw_utxos) => {
                let full_utxos = raw_utxos
                    .iter()
                    .map(|u| self.database.borrow().get_utxo(&u))
                    .collect::<Result<Vec<_>, _>>()?;
                if !full_utxos.iter().all(|u| u.is_some()) {
                    return Err(Error::UnknownUTXO);
                }

                Ok((full_utxos.into_iter().map(|x| x.unwrap()).collect(), true))
            }
            // otherwise limit ourselves to the spendable utxos for the selected policy, and the `send_all` setting
            None => {
                let utxos = self.list_unspent()?.into_iter();
                let utxos = change_policy.filter_utxos(utxos).into_iter();

                Ok((
                    utxos
                        .filter(|u| !unspendable_set.contains(&u.outpoint))
                        .collect(),
                    send_all,
                ))
            }
        }
    }

    fn add_hd_keypaths(&self, psbt: &mut PSBT) -> Result<(), Error> {
        let mut input_utxos = Vec::with_capacity(psbt.inputs.len());
        for n in 0..psbt.inputs.len() {
            input_utxos.push(psbt.get_utxo_for(n).clone());
        }

        // try to add hd_keypaths if we've already seen the output
        for (psbt_input, out) in psbt.inputs.iter_mut().zip(input_utxos.iter()) {
            if let Some(out) = out {
                if let Some((script_type, child)) = self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&out.script_pubkey)?
                {
                    debug!("Found descriptor {:?}/{}", script_type, child);

                    // merge hd_keypaths
                    let (desc, _) = self.get_descriptor_for(script_type);
                    let mut hd_keypaths = desc.get_hd_keypaths(child)?;
                    psbt_input.hd_keypaths.append(&mut hd_keypaths);
                }
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
    #[maybe_async]
    pub fn new(
        descriptor: &str,
        change_descriptor: Option<&str>,
        network: Network,
        database: D,
        client: B,
    ) -> Result<Self, Error> {
        let mut wallet = Self::new_offline(descriptor, change_descriptor, network, database)?;

        wallet.current_height = Some(maybe_await!(client.get_height())? as u32);
        wallet.client = client;

        Ok(wallet)
    }

    #[maybe_async]
    pub fn sync(&self, max_address_param: Option<u32>) -> Result<(), Error> {
        debug!("Begin sync...");

        let max_address = match self.descriptor.is_fixed() {
            true => 0,
            false => max_address_param.unwrap_or(CACHE_ADDR_BATCH_SIZE),
        };
        if self
            .database
            .borrow()
            .get_script_pubkey_from_path(ScriptType::External, max_address)?
            .is_none()
        {
            self.cache_addresses(ScriptType::External, 0, max_address)?;
        }

        if let Some(change_descriptor) = &self.change_descriptor {
            let max_address = match change_descriptor.is_fixed() {
                true => 0,
                false => max_address_param.unwrap_or(CACHE_ADDR_BATCH_SIZE),
            };

            if self
                .database
                .borrow()
                .get_script_pubkey_from_path(ScriptType::Internal, max_address)?
                .is_none()
            {
                self.cache_addresses(ScriptType::Internal, 0, max_address)?;
            }
        }

        maybe_await!(self.client.sync(
            None,
            self.database.borrow_mut().deref_mut(),
            noop_progress(),
        ))
    }

    #[maybe_async]
    pub fn broadcast(&self, tx: Transaction) -> Result<Txid, Error> {
        maybe_await!(self.client.broadcast(&tx))?;

        Ok(tx.txid())
    }

    #[maybe_async]
    pub fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        Ok(maybe_await!(self.client.estimate_fee(target))?)
    }
}

#[cfg(test)]
mod test {
    use bitcoin::Network;

    use crate::database::memory::MemoryDatabase;
    use crate::database::Database;
    use crate::types::ScriptType;

    use super::*;

    #[test]
    fn test_cache_addresses_fixed() {
        let db = MemoryDatabase::new();
        let wallet: OfflineWallet<_> = Wallet::new_offline(
            "wpkh(L5EZftvrYaSudiozVRzTqLcHLNDoVn7H5HSfM9BAN6tMJX8oTWz6)",
            None,
            Network::Testnet,
            db,
        )
        .unwrap();

        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1qj08ys4ct2hzzc2hcz6h2hgrvlmsjynaw43s835"
        );
        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1qj08ys4ct2hzzc2hcz6h2hgrvlmsjynaw43s835"
        );

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, 0)
            .unwrap()
            .is_some());
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::Internal, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cache_addresses() {
        let db = MemoryDatabase::new();
        let wallet: OfflineWallet<_> = Wallet::new_offline("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)", None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE - 1)
            .unwrap()
            .is_some());
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cache_addresses_refill() {
        let db = MemoryDatabase::new();
        let wallet: OfflineWallet<_> = Wallet::new_offline("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)", None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE - 1)
            .unwrap()
            .is_some());

        for _ in 0..CACHE_ADDR_BATCH_SIZE {
            wallet.get_new_address().unwrap();
        }

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE * 2 - 1)
            .unwrap()
            .is_some());
    }
}
