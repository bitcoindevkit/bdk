// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::collections::BTreeMap;
use std::str::FromStr;

use clap::{App, Arg, ArgMatches, SubCommand};

#[allow(unused_imports)]
use log::{debug, error, info, trace, LevelFilter};

use bitcoin::consensus::encode::{deserialize, serialize, serialize_hex};
use bitcoin::hashes::hex::FromHex;
use bitcoin::util::psbt::PartiallySignedTransaction;
use bitcoin::{Address, OutPoint, Script, Txid};

use crate::blockchain::log_progress;
use crate::error::Error;
use crate::types::ScriptType;
use crate::{FeeRate, TxBuilder, Wallet};

fn parse_recipient(s: &str) -> Result<(Script, u64), String> {
    let parts: Vec<_> = s.split(':').collect();
    if parts.len() != 2 {
        return Err("Invalid format".to_string());
    }

    let addr = Address::from_str(&parts[0]);
    if let Err(e) = addr {
        return Err(format!("{:?}", e));
    }
    let val = u64::from_str(&parts[1]);
    if let Err(e) = val {
        return Err(format!("{:?}", e));
    }

    Ok((addr.unwrap().script_pubkey(), val.unwrap()))
}

fn parse_outpoint(s: &str) -> Result<OutPoint, String> {
    OutPoint::from_str(s).map_err(|e| format!("{:?}", e))
}

fn recipient_validator(s: String) -> Result<(), String> {
    parse_recipient(&s).map(|_| ())
}

fn outpoint_validator(s: String) -> Result<(), String> {
    parse_outpoint(&s).map(|_| ())
}

pub fn make_cli_subcommands<'a, 'b>() -> App<'a, 'b> {
    App::new("Magical Bitcoin Wallet")
        .version(option_env!("CARGO_PKG_VERSION").unwrap_or("unknown"))
        .author(option_env!("CARGO_PKG_AUTHORS").unwrap_or(""))
        .about("A modern, lightweight, descriptor-based wallet")
        .subcommand(
            SubCommand::with_name("get_new_address").about("Generates a new external address"),
        )
        .subcommand(SubCommand::with_name("sync").about("Syncs with the chosen Electrum server").arg(
            Arg::with_name("max_addresses")
                .required(false)
                .takes_value(true)
                .long("max_addresses")
                .help("max addresses to consider"),
        ))
        .subcommand(
            SubCommand::with_name("list_unspent").about("Lists the available spendable UTXOs"),
        )
        .subcommand(
            SubCommand::with_name("list_transactions").about("Lists all the incoming and outgoing transactions of the wallet"),
        )
        .subcommand(
            SubCommand::with_name("get_balance").about("Returns the current wallet balance"),
        )
        .subcommand(
            SubCommand::with_name("create_tx")
                .about("Creates a new unsigned tranasaction")
                .arg(
                    Arg::with_name("to")
                        .long("to")
                        .value_name("ADDRESS:SAT")
                        .help("Adds a recipient to the transaction")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(true)
                        .multiple(true)
                        .validator(recipient_validator),
                )
                .arg(
                    Arg::with_name("send_all")
                        .short("all")
                        .long("send_all")
                        .help("Sends all the funds (or all the selected utxos). Requires only one recipients of value 0"),
                )
                .arg(
                    Arg::with_name("enable_rbf")
                        .short("rbf")
                        .long("enable_rbf")
                        .help("Enables Replace-By-Fee (BIP125)"),
                )
                .arg(
                    Arg::with_name("utxos")
                        .long("utxos")
                        .value_name("TXID:VOUT")
                        .help("Selects which utxos *must* be spent")
                        .takes_value(true)
                        .number_of_values(1)
                        .multiple(true)
                        .validator(outpoint_validator),
                )
                .arg(
                    Arg::with_name("unspendable")
                        .long("unspendable")
                        .value_name("TXID:VOUT")
                        .help("Marks an utxo as unspendable")
                        .takes_value(true)
                        .number_of_values(1)
                        .multiple(true)
                        .validator(outpoint_validator),
                )
                .arg(
                    Arg::with_name("fee_rate")
                        .short("fee")
                        .long("fee_rate")
                        .value_name("SATS_VBYTE")
                        .help("Fee rate to use in sat/vbyte")
                        .takes_value(true),
                )
                .arg(
                    Arg::with_name("external_policy")
                        .long("external_policy")
                        .value_name("POLICY")
                        .help("Selects which policy should be used to satisfy the external descriptor")
                        .takes_value(true)
                        .number_of_values(1),
                )
                .arg(
                    Arg::with_name("internal_policy")
                        .long("internal_policy")
                        .value_name("POLICY")
                        .help("Selects which policy should be used to satisfy the internal descriptor")
                        .takes_value(true)
                        .number_of_values(1),
                ),
        )
        .subcommand(
            SubCommand::with_name("bump_fee")
                .about("Bumps the fees of an RBF transaction")
                .arg(
                    Arg::with_name("txid")
                        .required(true)
                        .takes_value(true)
                        .short("txid")
                        .long("txid")
                        .help("TXID of the transaction to update"),
                )
                .arg(
                    Arg::with_name("send_all")
                        .short("all")
                        .long("send_all")
                        .help("Allows the wallet to reduce the amount of the only output in order to increase fees. This is generally the expected behavior for transactions originally created with `send_all`"),
                )
                .arg(
                    Arg::with_name("utxos")
                        .long("utxos")
                        .value_name("TXID:VOUT")
                        .help("Selects which utxos *must* be added to the tx. Unconfirmed utxos cannot be used")
                        .takes_value(true)
                        .number_of_values(1)
                        .multiple(true)
                        .validator(outpoint_validator),
                )
                .arg(
                    Arg::with_name("unspendable")
                        .long("unspendable")
                        .value_name("TXID:VOUT")
                        .help("Marks an utxo as unspendable, in case more inputs are needed to cover the extra fees")
                        .takes_value(true)
                        .number_of_values(1)
                        .multiple(true)
                        .validator(outpoint_validator),
                )
                .arg(
                    Arg::with_name("fee_rate")
                        .required(true)
                        .short("fee")
                        .long("fee_rate")
                        .value_name("SATS_VBYTE")
                        .help("The new targeted fee rate in sat/vbyte")
                        .takes_value(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("policies")
                .about("Returns the available spending policies for the descriptor")
            )
        .subcommand(
            SubCommand::with_name("public_descriptor")
                .about("Returns the public version of the wallet's descriptor(s)")
            )
        .subcommand(
            SubCommand::with_name("sign")
                .about("Signs and tries to finalize a PSBT")
                .arg(
                    Arg::with_name("psbt")
                        .long("psbt")
                        .value_name("BASE64_PSBT")
                        .help("Sets the PSBT to sign")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(true),
                )
                .arg(
                    Arg::with_name("assume_height")
                        .long("assume_height")
                        .value_name("HEIGHT")
                        .help("Assume the blockchain has reached a specific height. This affects the transaction finalization, if there are timelocks in the descriptor")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(false),
                ))
        .subcommand(
            SubCommand::with_name("broadcast")
                .about("Broadcasts a transaction to the network. Takes either a raw transaction or a PSBT to extract")
                .arg(
                    Arg::with_name("psbt")
                        .long("psbt")
                        .value_name("BASE64_PSBT")
                        .help("Sets the PSBT to extract and broadcast")
                        .takes_value(true)
                        .required_unless("tx")
                        .number_of_values(1))
                .arg(
                    Arg::with_name("tx")
                        .long("tx")
                        .value_name("RAWTX")
                        .help("Sets the raw transaction to broadcast")
                        .takes_value(true)
                        .required_unless("psbt")
                        .number_of_values(1))
                )
        .subcommand(
            SubCommand::with_name("extract_psbt")
                .about("Extracts a raw transaction from a PSBT")
                .arg(
                    Arg::with_name("psbt")
                        .long("psbt")
                        .value_name("BASE64_PSBT")
                        .help("Sets the PSBT to extract")
                        .takes_value(true)
                        .required(true)
                        .number_of_values(1))
                )
        .subcommand(
            SubCommand::with_name("finalize_psbt")
                .about("Finalizes a psbt")
                .arg(
                    Arg::with_name("psbt")
                        .long("psbt")
                        .value_name("BASE64_PSBT")
                        .help("Sets the PSBT to finalize")
                        .takes_value(true)
                        .required(true)
                        .number_of_values(1))
                .arg(
                    Arg::with_name("assume_height")
                        .long("assume_height")
                        .value_name("HEIGHT")
                        .help("Assume the blockchain has reached a specific height")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(false))
                )
        .subcommand(
            SubCommand::with_name("combine_psbt")
                .about("Combines multiple PSBTs into one")
                .arg(
                    Arg::with_name("psbt")
                        .long("psbt")
                        .value_name("BASE64_PSBT")
                        .help("Add one PSBT to comine. This option can be repeated multiple times, one for each PSBT")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(true)
                        .multiple(true))
                )
}

pub fn add_global_flags<'a, 'b>(app: App<'a, 'b>) -> App<'a, 'b> {
    let mut app = app
        .arg(
            Arg::with_name("network")
                .short("n")
                .long("network")
                .value_name("NETWORK")
                .help("Sets the network")
                .takes_value(true)
                .default_value("testnet")
                .possible_values(&["testnet", "regtest"]),
        )
        .arg(
            Arg::with_name("wallet")
                .short("w")
                .long("wallet")
                .value_name("WALLET_NAME")
                .help("Selects the wallet to use")
                .takes_value(true)
                .default_value("main"),
        )
        .arg(
            Arg::with_name("proxy")
                .short("p")
                .long("proxy")
                .value_name("SERVER:PORT")
                .help("Sets the SOCKS5 proxy for the Electrum client")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("descriptor")
                .short("d")
                .long("descriptor")
                .value_name("DESCRIPTOR")
                .help("Sets the descriptor to use for the external addresses")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("change_descriptor")
                .short("c")
                .long("change_descriptor")
                .value_name("DESCRIPTOR")
                .help("Sets the descriptor to use for internal addresses")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        );

    if cfg!(feature = "esplora") {
        app = app
            .arg(
                Arg::with_name("esplora")
                    .short("e")
                    .long("esplora")
                    .value_name("ESPLORA")
                    .help("Use the esplora server if given as parameter")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("esplora_concurrency")
                    .long("esplora_concurrency")
                    .value_name("ESPLORA_CONCURRENCY")
                    .help("Concurrency of requests made to the esplora server")
                    .default_value("4")
                    .takes_value(true),
            )
    }

    if cfg!(feature = "electrum") {
        app = app.arg(
            Arg::with_name("server")
                .short("s")
                .long("server")
                .value_name("SERVER:PORT")
                .help("Sets the Electrum server to use")
                .takes_value(true)
                .default_value("ssl://electrum.blockstream.info:60002"),
        );
    }

    app.subcommand(SubCommand::with_name("repl").about("Opens an interactive shell"))
}

#[maybe_async]
pub fn handle_matches<C, D>(
    wallet: &Wallet<C, D>,
    matches: ArgMatches<'_>,
) -> Result<serde_json::Value, Error>
where
    C: crate::blockchain::Blockchain,
    D: crate::database::BatchDatabase,
{
    if let Some(_sub_matches) = matches.subcommand_matches("get_new_address") {
        Ok(json!({
            "address": wallet.get_new_address()?
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("sync") {
        let max_addresses: Option<u32> = sub_matches
            .value_of("max_addresses")
            .and_then(|m| m.parse().ok());
        maybe_await!(wallet.sync(log_progress(), max_addresses))?;
        Ok(json!({}))
    } else if let Some(_sub_matches) = matches.subcommand_matches("list_unspent") {
        Ok(serde_json::to_value(&wallet.list_unspent()?)?)
    } else if let Some(_sub_matches) = matches.subcommand_matches("list_transactions") {
        Ok(serde_json::to_value(&wallet.list_transactions(false)?)?)
    } else if let Some(_sub_matches) = matches.subcommand_matches("get_balance") {
        Ok(json!({
            "satoshi": wallet.get_balance()?
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("create_tx") {
        let recipients = sub_matches
            .values_of("to")
            .unwrap()
            .map(|s| parse_recipient(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::Generic)?;
        let mut tx_builder = TxBuilder::new();

        if sub_matches.is_present("send_all") {
            tx_builder = tx_builder
                .drain_wallet()
                .set_single_recipient(recipients[0].0.clone());
        } else {
            tx_builder = tx_builder.set_recipients(recipients);
        }

        if sub_matches.is_present("enable_rbf") {
            tx_builder = tx_builder.enable_rbf();
        }

        if let Some(fee_rate) = sub_matches.value_of("fee_rate") {
            let fee_rate = f32::from_str(fee_rate).map_err(|s| Error::Generic(s.to_string()))?;
            tx_builder = tx_builder.fee_rate(FeeRate::from_sat_per_vb(fee_rate));
        }
        if let Some(utxos) = sub_matches.values_of("utxos") {
            let utxos = utxos
                .map(|i| parse_outpoint(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::Generic)?;
            tx_builder = tx_builder.utxos(utxos).manually_selected_only();
        }

        if let Some(unspendable) = sub_matches.values_of("unspendable") {
            let unspendable = unspendable
                .map(|i| parse_outpoint(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::Generic)?;
            tx_builder = tx_builder.unspendable(unspendable);
        }

        let policies = vec![
            sub_matches
                .value_of("external_policy")
                .map(|p| (p, ScriptType::External)),
            sub_matches
                .value_of("internal_policy")
                .map(|p| (p, ScriptType::Internal)),
        ];
        for (policy, script_type) in policies.into_iter().filter_map(|x| x) {
            let policy = serde_json::from_str::<BTreeMap<String, Vec<usize>>>(&policy)
                .map_err(|s| Error::Generic(s.to_string()))?;
            tx_builder = tx_builder.policy_path(policy, script_type);
        }

        let (psbt, details) = wallet.create_tx(tx_builder)?;
        Ok(json!({
            "psbt": base64::encode(&serialize(&psbt)),
            "details": details,
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("bump_fee") {
        let txid = Txid::from_str(sub_matches.value_of("txid").unwrap())
            .map_err(|s| Error::Generic(s.to_string()))?;

        let fee_rate = f32::from_str(sub_matches.value_of("fee_rate").unwrap())
            .map_err(|s| Error::Generic(s.to_string()))?;
        let mut tx_builder = TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(fee_rate));

        if sub_matches.is_present("send_all") {
            tx_builder = tx_builder.maintain_single_recipient();
        }

        if let Some(utxos) = sub_matches.values_of("utxos") {
            let utxos = utxos
                .map(|i| parse_outpoint(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::Generic)?;
            tx_builder = tx_builder.utxos(utxos);
        }

        if let Some(unspendable) = sub_matches.values_of("unspendable") {
            let unspendable = unspendable
                .map(|i| parse_outpoint(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::Generic)?;
            tx_builder = tx_builder.unspendable(unspendable);
        }

        let (psbt, details) = wallet.bump_fee(&txid, tx_builder)?;
        Ok(json!({
            "psbt": base64::encode(&serialize(&psbt)),
            "details": details,
        }))
    } else if let Some(_sub_matches) = matches.subcommand_matches("policies") {
        Ok(json!({
            "external": wallet.policies(ScriptType::External)?,
            "internal": wallet.policies(ScriptType::Internal)?,
        }))
    } else if let Some(_sub_matches) = matches.subcommand_matches("public_descriptor") {
        Ok(json!({
            "external": wallet.public_descriptor(ScriptType::External)?.map(|d| d.to_string()),
            "internal": wallet.public_descriptor(ScriptType::Internal)?.map(|d| d.to_string()),
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("sign") {
        let psbt = base64::decode(sub_matches.value_of("psbt").unwrap()).unwrap();
        let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();
        let assume_height = sub_matches
            .value_of("assume_height")
            .map(|s| s.parse().unwrap());
        let (psbt, finalized) = wallet.sign(psbt, assume_height)?;
        Ok(json!({
            "psbt": base64::encode(&serialize(&psbt)),
            "is_finalized": finalized,
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("broadcast") {
        let tx = if sub_matches.value_of("psbt").is_some() {
            let psbt = base64::decode(&sub_matches.value_of("psbt").unwrap()).unwrap();
            let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();
            psbt.extract_tx()
        } else if sub_matches.value_of("tx").is_some() {
            deserialize(&Vec::<u8>::from_hex(&sub_matches.value_of("tx").unwrap()).unwrap())
                .unwrap()
        } else {
            panic!("Missing `psbt` and `tx` option");
        };

        let txid = maybe_await!(wallet.broadcast(tx))?;
        Ok(json!({ "txid": txid }))
    } else if let Some(sub_matches) = matches.subcommand_matches("extract_psbt") {
        let psbt = base64::decode(&sub_matches.value_of("psbt").unwrap()).unwrap();
        let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();
        Ok(json!({
            "raw_tx": serialize_hex(&psbt.extract_tx()),
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("finalize_psbt") {
        let psbt = base64::decode(&sub_matches.value_of("psbt").unwrap()).unwrap();
        let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();

        let assume_height = sub_matches
            .value_of("assume_height")
            .map(|s| s.parse().unwrap());

        let (psbt, finalized) = wallet.finalize_psbt(psbt, assume_height)?;
        Ok(json!({
            "psbt": base64::encode(&serialize(&psbt)),
            "is_finalized": finalized,
        }))
    } else if let Some(sub_matches) = matches.subcommand_matches("combine_psbt") {
        let mut psbts = sub_matches
            .values_of("psbt")
            .unwrap()
            .map(|s| {
                let psbt = base64::decode(&s).unwrap();
                let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();

                psbt
            })
            .collect::<Vec<_>>();

        let init_psbt = psbts.pop().unwrap();
        let final_psbt = psbts
            .into_iter()
            .try_fold::<_, _, Result<PartiallySignedTransaction, Error>>(
                init_psbt,
                |mut acc, x| {
                    acc.merge(x)?;
                    Ok(acc)
                },
            )?;

        Ok(json!({ "psbt": base64::encode(&serialize(&final_psbt)) }))
    } else {
        Ok(serde_json::Value::Null)
    }
}
