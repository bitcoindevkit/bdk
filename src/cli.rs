use std::collections::BTreeMap;
use std::str::FromStr;

use clap::{App, Arg, ArgMatches, SubCommand};

#[allow(unused_imports)]
use log::{debug, error, info, trace, LevelFilter};

use bitcoin::consensus::encode::{deserialize, serialize, serialize_hex};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::util::psbt::PartiallySignedTransaction;
use bitcoin::{Address, OutPoint};

use crate::error::Error;
use crate::types::ScriptType;
use crate::{FeeRate, TxBuilder, Wallet};

fn parse_addressee(s: &str) -> Result<(Address, u64), String> {
    let parts: Vec<_> = s.split(":").collect();
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

    Ok((addr.unwrap(), val.unwrap()))
}

fn parse_outpoint(s: &str) -> Result<OutPoint, String> {
    OutPoint::from_str(s).map_err(|e| format!("{:?}", e))
}

fn addressee_validator(s: String) -> Result<(), String> {
    parse_addressee(&s).map(|_| ())
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
        .subcommand(SubCommand::with_name("sync").about("Syncs with the chosen Electrum server"))
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
                        .help("Adds an addressee to the transaction")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(true)
                        .multiple(true)
                        .validator(addressee_validator),
                )
                .arg(
                    Arg::with_name("send_all")
                        .short("all")
                        .long("send_all")
                        .help("Sends all the funds (or all the selected utxos). Requires only one addressees of value 0"),
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
                    Arg::with_name("policy")
                        .long("policy")
                        .value_name("POLICY")
                        .help("Selects which policy should be used to satisfy the descriptor")
                        .takes_value(true)
                        .number_of_values(1),
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
    app.arg(
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
        Arg::with_name("server")
            .short("s")
            .long("server")
            .value_name("SERVER:PORT")
            .help("Sets the Electrum server to use")
            .takes_value(true)
            .default_value("ssl://electrum.blockstream.info:60002"),
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
    )
    .subcommand(SubCommand::with_name("repl").about("Opens an interactive shell"))
}

#[maybe_async]
pub fn handle_matches<C, D>(
    wallet: &Wallet<C, D>,
    matches: ArgMatches<'_>,
) -> Result<Option<String>, Error>
where
    C: crate::blockchain::OnlineBlockchain,
    D: crate::database::BatchDatabase,
{
    if let Some(_sub_matches) = matches.subcommand_matches("get_new_address") {
        Ok(Some(format!("{}", wallet.get_new_address()?)))
    } else if let Some(_sub_matches) = matches.subcommand_matches("sync") {
        maybe_await!(wallet.sync(None))?;
        Ok(None)
    } else if let Some(_sub_matches) = matches.subcommand_matches("list_unspent") {
        let mut res = String::new();
        for utxo in wallet.list_unspent()? {
            res += &format!("{} value {} SAT\n", utxo.outpoint, utxo.txout.value);
        }

        Ok(Some(res))
    } else if let Some(_sub_matches) = matches.subcommand_matches("list_transactions") {
        let mut res = String::new();
        for crate::types::TransactionDetails {
            txid,
            sent,
            received,
            height,
            ..
        } in wallet.list_transactions(false)?
        {
            res += &format!(
                "{} - sent {}, received {} - height: {:?}\n",
                txid, sent, received, height
            );
        }

        Ok(Some(res))
    } else if let Some(_sub_matches) = matches.subcommand_matches("get_balance") {
        Ok(Some(format!("{} SAT", wallet.get_balance()?)))
    } else if let Some(sub_matches) = matches.subcommand_matches("create_tx") {
        let addressees = sub_matches
            .values_of("to")
            .unwrap()
            .map(|s| parse_addressee(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|s| Error::Generic(s))?;
        let mut tx_builder = TxBuilder::from_addressees(addressees);

        if sub_matches.is_present("send_all") {
            tx_builder = tx_builder.send_all();
        }

        if let Some(fee_rate) = sub_matches.value_of("fee_rate") {
            let fee_rate = f32::from_str(fee_rate).map_err(|s| Error::Generic(s.to_string()))?;
            tx_builder = tx_builder.fee_rate(FeeRate::from_sat_per_vb(fee_rate));
        }
        if let Some(utxos) = sub_matches.values_of("utxos") {
            let utxos = utxos
                .map(|i| parse_outpoint(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|s| Error::Generic(s.to_string()))?;
            tx_builder = tx_builder.utxos(utxos);
        }

        if let Some(unspendable) = sub_matches.values_of("unspendable") {
            let unspendable = unspendable
                .map(|i| parse_outpoint(i))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|s| Error::Generic(s.to_string()))?;
            tx_builder = tx_builder.unspendable(unspendable);
        }
        if let Some(policy) = sub_matches.value_of("policy") {
            let policy = serde_json::from_str::<BTreeMap<String, Vec<usize>>>(&policy)
                .map_err(|s| Error::Generic(s.to_string()))?;
            tx_builder = tx_builder.policy_path(policy);
        }

        let result = wallet.create_tx(tx_builder)?;
        Ok(Some(format!(
            "{:#?}\nPSBT: {}",
            result.1,
            base64::encode(&serialize(&result.0))
        )))
    } else if let Some(_sub_matches) = matches.subcommand_matches("policies") {
        Ok(Some(format!(
            "External: {}\nInternal:{}",
            serde_json::to_string(&wallet.policies(ScriptType::External)?).unwrap(),
            serde_json::to_string(&wallet.policies(ScriptType::Internal)?).unwrap(),
        )))
    } else if let Some(_sub_matches) = matches.subcommand_matches("public_descriptor") {
        let external = match wallet.public_descriptor(ScriptType::External)? {
            Some(desc) => format!("{}", desc),
            None => "<NONE>".into(),
        };
        let internal = match wallet.public_descriptor(ScriptType::Internal)? {
            Some(desc) => format!("{}", desc),
            None => "<NONE>".into(),
        };

        Ok(Some(format!(
            "External: {}\nInternal:{}",
            external, internal
        )))
    } else if let Some(sub_matches) = matches.subcommand_matches("sign") {
        let psbt = base64::decode(sub_matches.value_of("psbt").unwrap()).unwrap();
        let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();
        let assume_height = sub_matches
            .value_of("assume_height")
            .and_then(|s| Some(s.parse().unwrap()));
        let (psbt, finalized) = wallet.sign(psbt, assume_height)?;

        let mut res = String::new();

        res += &format!("PSBT: {}\n", base64::encode(&serialize(&psbt)));
        res += &format!("Finalized: {}", finalized);
        if finalized {
            res += &format!("\nExtracted: {}", serialize_hex(&psbt.extract_tx()));
        }

        Ok(Some(res))
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

        Ok(Some(format!("TXID: {}", txid)))
    } else if let Some(sub_matches) = matches.subcommand_matches("extract_psbt") {
        let psbt = base64::decode(&sub_matches.value_of("psbt").unwrap()).unwrap();
        let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();

        Ok(Some(format!(
            "TX: {}",
            serialize(&psbt.extract_tx()).to_hex()
        )))
    } else if let Some(sub_matches) = matches.subcommand_matches("finalize_psbt") {
        let psbt = base64::decode(&sub_matches.value_of("psbt").unwrap()).unwrap();
        let mut psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();

        let assume_height = sub_matches
            .value_of("assume_height")
            .and_then(|s| Some(s.parse().unwrap()));

        let finalized = wallet.finalize_psbt(&mut psbt, assume_height)?;

        let mut res = String::new();
        res += &format!("PSBT: {}\n", base64::encode(&serialize(&psbt)));
        res += &format!("Finalized: {}", finalized);
        if finalized {
            res += &format!("\nExtracted: {}", serialize_hex(&psbt.extract_tx()));
        }

        Ok(Some(res))
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

        Ok(Some(format!(
            "PSBT: {}",
            base64::encode(&serialize(&final_psbt))
        )))
    } else {
        Ok(None)
    }
}
