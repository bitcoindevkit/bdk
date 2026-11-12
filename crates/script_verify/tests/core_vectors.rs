//! Runs Bitcoin Core's script and transaction test vectors (see `tests/data/README.md`).
//!
//! Core's own harness checks that dropping flags from a passing row keeps it passing. So a passing
//! row is run under its consensus flags only, while a failing row is run only when all its flags
//! are consensus flags, since a policy flag may be what fails it.

use std::collections::BTreeMap;

use bdk_script_verify::{Error, Flags};
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::taproot::{LeafVersion, TaprootBuilder};
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use serde_json::Value;

const OP_NAMES: [&str; 90] = [
    "NOP",
    "VER",
    "IF",
    "NOTIF",
    "VERIF",
    "VERNOTIF",
    "ELSE",
    "ENDIF",
    "VERIFY",
    "RETURN",
    "TOALTSTACK",
    "FROMALTSTACK",
    "2DROP",
    "2DUP",
    "3DUP",
    "2OVER",
    "2ROT",
    "2SWAP",
    "IFDUP",
    "DEPTH",
    "DROP",
    "DUP",
    "NIP",
    "OVER",
    "PICK",
    "ROLL",
    "ROT",
    "SWAP",
    "TUCK",
    "CAT",
    "SUBSTR",
    "LEFT",
    "RIGHT",
    "SIZE",
    "INVERT",
    "AND",
    "OR",
    "XOR",
    "EQUAL",
    "EQUALVERIFY",
    "RESERVED1",
    "RESERVED2",
    "1ADD",
    "1SUB",
    "2MUL",
    "2DIV",
    "NEGATE",
    "ABS",
    "NOT",
    "0NOTEQUAL",
    "ADD",
    "SUB",
    "MUL",
    "DIV",
    "MOD",
    "LSHIFT",
    "RSHIFT",
    "BOOLAND",
    "BOOLOR",
    "NUMEQUAL",
    "NUMEQUALVERIFY",
    "NUMNOTEQUAL",
    "LESSTHAN",
    "GREATERTHAN",
    "LESSTHANOREQUAL",
    "GREATERTHANOREQUAL",
    "MIN",
    "MAX",
    "WITHIN",
    "RIPEMD160",
    "SHA1",
    "SHA256",
    "HASH160",
    "HASH256",
    "CODESEPARATOR",
    "CHECKSIG",
    "CHECKSIGVERIFY",
    "CHECKMULTISIG",
    "CHECKMULTISIGVERIFY",
    "NOP1",
    "CHECKLOCKTIMEVERIFY",
    "CHECKSEQUENCEVERIFY",
    "NOP4",
    "NOP5",
    "NOP6",
    "NOP7",
    "NOP8",
    "NOP9",
    "NOP10",
    "CHECKSIGADD",
];

fn opcode(name: &str) -> u8 {
    let name = name.strip_prefix("OP_").unwrap_or(name);
    match name {
        "RESERVED" => 0x50,
        "INVALIDOPCODE" => 0xff,
        _ => {
            let i = OP_NAMES
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("unknown opcode {name}"));
            0x61 + i as u8
        }
    }
}

fn scriptnum(mut n: i64) -> Vec<u8> {
    let neg = n < 0;
    n = n.abs();
    let mut out = Vec::new();
    while n > 0 {
        out.push((n & 0xff) as u8);
        n >>= 8;
    }
    if out.last().is_some_and(|b| b & 0x80 != 0) {
        out.push(if neg { 0x80 } else { 0 });
    } else if neg {
        *out.last_mut().unwrap() |= 0x80;
    }
    out
}

fn push(out: &mut Vec<u8>, data: &[u8]) {
    let n = data.len();
    if n < 0x4c {
        out.push(n as u8);
    } else if n <= 0xff {
        out.extend([0x4c, n as u8]);
    } else if n <= 0xffff {
        out.push(0x4d);
        out.extend((n as u16).to_le_bytes());
    } else {
        out.push(0x4e);
        out.extend((n as u32).to_le_bytes());
    }
    out.extend_from_slice(data);
}

/// Bitcoin Core's `ParseScript`.
fn parse_script(asm: &str) -> ScriptBuf {
    let mut out = Vec::new();
    for word in asm.split_whitespace() {
        let is_num = word.bytes().all(|b| b.is_ascii_digit())
            || (word.len() > 1
                && word.starts_with('-')
                && word[1..].bytes().all(|b| b.is_ascii_digit()));
        if is_num {
            let n: i64 = word.parse().unwrap();
            match n {
                0 => out.push(0x00),
                -1 => out.push(0x4f),
                1..=16 => out.push(0x50 + n as u8),
                _ => push(&mut out, &scriptnum(n)),
            }
        } else if let Some(hex) = word.strip_prefix("0x").filter(|h| !h.is_empty()) {
            out.extend(decode_hex(hex));
        } else if word.len() >= 2 && word.starts_with('\'') && word.ends_with('\'') {
            push(&mut out, &word.as_bytes()[1..word.len() - 1]);
        } else {
            out.push(opcode(word));
        }
    }
    ScriptBuf::from_bytes(out)
}

fn decode_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// The consensus part of a Core flag list, and whether it also named policy flags.
fn parse_flags(list: &str) -> (Flags, bool) {
    let mut flags = Flags {
        p2sh: false,
        dersig: false,
        checklocktimeverify: false,
        checksequenceverify: false,
        witness: false,
        nulldummy: false,
        taproot: false,
    };
    let mut policy = false;
    for name in list.split(',').filter(|n| !n.is_empty() && *n != "NONE") {
        match name {
            "P2SH" => flags.p2sh = true,
            "DERSIG" => flags.dersig = true,
            "CHECKLOCKTIMEVERIFY" => flags.checklocktimeverify = true,
            "CHECKSEQUENCEVERIFY" => flags.checksequenceverify = true,
            "WITNESS" => flags.witness = true,
            "NULLDUMMY" => flags.nulldummy = true,
            "TAPROOT" => flags.taproot = true,
            _ => policy = true,
        }
    }
    (flags, policy)
}

/// Core's `#CONTROLBLOCK#` and `#TAPROOTOUTPUT#`: a tree whose only leaf is `leaf`. The internal
/// key differs from Core's, which no row depends on.
fn single_leaf_taproot(leaf: ScriptBuf) -> (ScriptBuf, Vec<u8>) {
    let secp = Secp256k1::new();
    let (internal_key, _) = SecretKey::from_slice(&[1; 32])
        .unwrap()
        .x_only_public_key(&secp);
    let info = TaprootBuilder::new()
        .add_leaf(0, leaf.clone())
        .unwrap()
        .finalize(&secp, internal_key)
        .unwrap();
    let control_block = info.control_block(&(leaf, LeafVersion::TapScript)).unwrap();
    (
        ScriptBuf::new_p2tr_tweaked(info.output_key()),
        control_block.serialize(),
    )
}

#[test]
fn core_script_tests() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("data/script_tests.json")).unwrap();
    let (mut ran, mut skipped) = (0, 0);
    for row in rows {
        let row = row.as_array().unwrap();
        if row.len() == 1 {
            continue;
        }
        let mut taproot_output = None;
        let (witness, amount, row) = match row[0].as_array() {
            Some(wit) => {
                let (amount, elements) = wit.split_last().unwrap();
                let mut items: Vec<Vec<u8>> = Vec::new();
                for element in elements {
                    let element = element.as_str().unwrap();
                    if let Some(asm) = element.strip_prefix("#SCRIPT#") {
                        items.push(parse_script(asm).into_bytes());
                    } else if element == "#CONTROLBLOCK#" {
                        let (output, control_block) = single_leaf_taproot(ScriptBuf::from_bytes(
                            items.last().unwrap().clone(),
                        ));
                        taproot_output = Some(output);
                        items.push(control_block);
                    } else {
                        items.push(decode_hex(element));
                    }
                }
                let sats = (amount.as_f64().unwrap() * 1e8).round() as u64;
                (
                    Witness::from_slice(&items),
                    Amount::from_sat(sats),
                    &row[1..],
                )
            }
            None => (Witness::new(), Amount::ZERO, &row[..]),
        };
        let strs = row.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        let (script_sig, spk, flag_list, expected) = (strs[0], strs[1], strs[2], strs[3]);
        let (flags, policy) = parse_flags(flag_list);
        let expect_ok = expected == "OK";
        if !expect_ok && policy {
            skipped += 1;
            continue;
        }

        let credit = Transaction {
            version: Version::ONE,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(vec![0x00, 0x00]),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: amount,
                script_pubkey: match spk {
                    "0x51 0x20 #TAPROOTOUTPUT#" => taproot_output.take().unwrap(),
                    _ => parse_script(spk),
                },
            }],
        };
        let spend = Transaction {
            version: Version::ONE,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(credit.compute_txid(), 0),
                script_sig: parse_script(script_sig),
                sequence: Sequence::MAX,
                witness: witness.clone(),
            }],
            output: vec![TxOut {
                value: amount,
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let v = bdk_script_verify::verify_input(
            &mut bdk_script_verify::VerificationCache::new(&spend, &[Some(&credit.output[0])]),
            0,
            flags,
        );
        match (&v.result, expect_ok) {
            (Ok(()), true) | (Err(Error::Script(_)), false) => {}
            _ => panic!("{row:?}: expected {expected}, got {:?}", v.result),
        }
        ran += 1;
    }
    assert!(ran >= 761, "ran {ran}, skipped {skipped}");
}

/// A `tx_valid.json` or `tx_invalid.json` row: the transaction, the outputs it spends and its
/// flag list.
fn parse_tx_row(row: &[Value]) -> (Transaction, Vec<TxOut>, &str) {
    let mut spent = BTreeMap::new();
    for p in row[0].as_array().unwrap() {
        let p = p.as_array().unwrap();
        let txid: Txid = p[0].as_str().unwrap().parse().unwrap();
        let vout = p[1].as_i64().unwrap() as u32;
        let outpoint = if vout == u32::MAX && txid == Txid::all_zeros() {
            OutPoint::null()
        } else {
            OutPoint::new(txid, vout)
        };
        let value = p.get(3).and_then(Value::as_u64).unwrap_or(0);
        spent.insert(
            outpoint,
            TxOut {
                value: Amount::from_sat(value),
                script_pubkey: parse_script(p[2].as_str().unwrap()),
            },
        );
    }
    let tx: Transaction =
        bitcoin::consensus::deserialize(&decode_hex(row[1].as_str().unwrap())).unwrap();
    let prevouts = tx
        .input
        .iter()
        .map(|txin| spent[&txin.previous_output].clone())
        .collect();
    (tx, prevouts, row[2].as_str().unwrap())
}

fn verify_tx(tx: &Transaction, prevouts: &[TxOut], flags: Flags) -> Vec<Result<(), Error>> {
    let prevouts = prevouts.iter().map(Some).collect::<Vec<_>>();
    bdk_script_verify::verify_transaction(tx, &prevouts, flags)
        .into_iter()
        .enumerate()
        .map(|(vin, verification)| {
            assert_eq!(
                verification,
                bdk_script_verify::verify_input(
                    &mut bdk_script_verify::VerificationCache::new(tx, &prevouts),
                    vin,
                    flags
                )
            );
            verification.result
        })
        .collect()
}

#[test]
fn core_tx_valid() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("data/tx_valid.json")).unwrap();
    let mut ran = 0;
    for row in rows {
        let row = row.as_array().unwrap();
        if !row[0].is_array() {
            continue;
        }
        let (tx, prevouts, flag_list) = parse_tx_row(row);
        let (excluded, _) = parse_flags(flag_list);
        let flags = Flags {
            p2sh: !excluded.p2sh,
            dersig: !excluded.dersig,
            checklocktimeverify: !excluded.checklocktimeverify,
            checksequenceverify: !excluded.checksequenceverify,
            witness: !excluded.witness,
            nulldummy: !excluded.nulldummy,
            taproot: !excluded.taproot,
        };
        for (vin, result) in verify_tx(&tx, &prevouts, flags).into_iter().enumerate() {
            assert_eq!(result, Ok(()), "{row:?} input {vin}");
        }
        ran += 1;
    }
    assert!(ran > 100, "ran {ran}");
}

/// Unlike `tx_valid.json`, a row's flags here are the ones applied, and Core checks they are
/// minimal: the row fails under them, still fails with any added, and passes with any one removed.
#[test]
fn core_tx_invalid() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("data/tx_invalid.json")).unwrap();
    let (mut ran, mut skipped) = (0, 0);
    for row in rows {
        let row = row.as_array().unwrap();
        if !row[0].is_array() {
            continue;
        }
        let (tx, prevouts, flag_list) = parse_tx_row(row);
        // BADTX rows fail `CheckTransaction`, which is not script validation.
        if flag_list == "BADTX" {
            skipped += 1;
            continue;
        }
        let (flags, policy) = parse_flags(flag_list);
        if policy {
            skipped += 1;
            continue;
        }

        for applied in [flags, Flags::CONSENSUS] {
            let results = verify_tx(&tx, &prevouts, applied);
            assert!(
                results.iter().any(|r| matches!(r, Err(Error::Script(_)))),
                "{row:?} under {applied:?}: {results:?}"
            );
        }
        for fewer in without_each_flag(flags) {
            let results = verify_tx(&tx, &prevouts, fewer);
            assert!(
                results.iter().all(Result::is_ok),
                "{row:?} under {fewer:?}: {results:?}"
            );
        }
        ran += 1;
    }
    assert!(ran >= 70, "ran {ran}, skipped {skipped}");
}

/// `flags` with each set flag removed in turn, dropping what then lacks its prerequisite as Core's
/// `TrimFlags` does.
fn without_each_flag(flags: Flags) -> Vec<Flags> {
    let mut out = Vec::new();
    let fields: [fn(&mut Flags) -> &mut bool; 7] = [
        |f| &mut f.p2sh,
        |f| &mut f.dersig,
        |f| &mut f.checklocktimeverify,
        |f| &mut f.checksequenceverify,
        |f| &mut f.witness,
        |f| &mut f.nulldummy,
        |f| &mut f.taproot,
    ];
    for field in fields {
        let mut fewer = flags;
        if !std::mem::take(field(&mut fewer)) {
            continue;
        }
        fewer.witness &= fewer.p2sh;
        if !out.contains(&fewer) {
            out.push(fewer);
        }
    }
    out
}

#[test]
fn bip341_key_path_vectors() {
    use bdk_script_verify::DependsOn;

    let vectors: Value =
        serde_json::from_str(include_str!("data/bip341_wallet_vectors.json")).unwrap();
    let case = &vectors["keyPathSpending"][0];
    let tx: Transaction = bitcoin::consensus::deserialize(&decode_hex(
        case["auxiliary"]["fullySignedTx"].as_str().unwrap(),
    ))
    .unwrap();
    let spent = case["given"]["utxosSpent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| TxOut {
            value: Amount::from_sat(u["amountSats"].as_u64().unwrap()),
            script_pubkey: ScriptBuf::from_bytes(decode_hex(u["scriptPubKey"].as_str().unwrap())),
        })
        .collect::<Vec<_>>();
    let all = spent.iter().map(Some).collect::<Vec<_>>();

    for input in case["inputSpending"].as_array().unwrap() {
        let vin = input["given"]["txinIndex"].as_u64().unwrap() as usize;
        let hash_type = input["given"]["hashType"].as_u64().unwrap();
        let anyone_can_pay = hash_type & 0x80 != 0;

        let v = bdk_script_verify::verify_input(
            &mut bdk_script_verify::VerificationCache::new(&tx, &all),
            vin,
            Flags::CONSENSUS,
        );
        assert_eq!(v.result, Ok(()), "input {vin}");

        // Without another input's prevout only ANYONECANPAY signatures are decided.
        let other = (vin + 1) % tx.input.len();
        let mut partial = all.clone();
        partial[other] = None;
        let v = bdk_script_verify::verify_input(
            &mut bdk_script_verify::VerificationCache::new(&tx, &partial),
            vin,
            Flags::CONSENSUS,
        );
        if anyone_can_pay {
            assert_eq!(v.result, Ok(()), "input {vin}");
            assert_eq!(v.depends_on, DependsOn::OwnPrevout);
        } else {
            assert_eq!(
                v.result,
                Err(Error::MissingPrevouts(vec![other])),
                "input {vin}"
            );
            assert_eq!(v.depends_on, DependsOn::AllPrevouts);
        }

        // A wrong amount for another input invalidates exactly the signatures committing to it.
        let mut wrong = spent.clone();
        wrong[other].value += Amount::from_sat(1);
        let wrong = wrong.iter().map(Some).collect::<Vec<_>>();
        let v = bdk_script_verify::verify_input(
            &mut bdk_script_verify::VerificationCache::new(&tx, &wrong),
            vin,
            Flags::CONSENSUS,
        );
        assert_eq!(v.result.is_ok(), anyone_can_pay, "input {vin}");

        let mut tampered = tx.clone();
        let mut sig = tampered.input[vin].witness.nth(0).unwrap().to_vec();
        sig[0] ^= 1;
        tampered.input[vin].witness = Witness::from_slice(&[sig]);
        let v = bdk_script_verify::verify_input(
            &mut bdk_script_verify::VerificationCache::new(&tampered, &all),
            vin,
            Flags::CONSENSUS,
        );
        assert!(matches!(v.result, Err(Error::Script(_))), "input {vin}");
    }
}
