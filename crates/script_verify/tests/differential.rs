//! Differential tests: `bdk_script_verify` must reach Bitcoin Core's verdict on every input of
//! every generated transaction. Core is libbitcoinkernel, through the `bitcoinkernel` crate pinned
//! at 0.3.2. That builds Bitcoin Core master at `70d9ec7f` (2026-07-14) from source, since every
//! published `bitcoinkernel` bundles a master snapshot rather than a release. Its script
//! interpreter differs from v32.1 only by refactors. Building it needs CMake, a C++ compiler and
//! Boost's headers.
//!
//! Cases are generated as plain descriptions (a seed spend and a list of mutations) rather than as
//! bytes, so that proptest shrinks a disagreement to a readable spend.
//!
//! The case count is proptest's, `PROPTEST_CASES` (256 by default). A long local run:
//! `PROPTEST_CASES=100000 cargo test --release -p bdk_script_verify --test differential`.

use std::sync::LazyLock;

use bdk_script_verify::{Error, Flags};
use bitcoin::absolute::LockTime;
use bitcoin::consensus::encode::{serialize, serialize_hex};
use bitcoin::hashes::{hash160, ripemd160, sha256, sha256d, Hash};
use bitcoin::key::{Keypair, TapTweak};
use bitcoin::opcodes::all::*;
use bitcoin::opcodes::Opcode;
use bitcoin::secp256k1::{All, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use bitcoin::sighash::Annex;
use bitcoin::sighash::{EcdsaSighashType, Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot::{LeafVersion, TapLeafHash, TapNodeHash, TaprootBuilder};
use bitcoin::transaction::Version;
use bitcoin::{
    relative, Amount, OutPoint, PublicKey, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness,
};
use miniscript::policy::Concrete;
use miniscript::{Descriptor, Satisfier, Segwitv0};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

static SECP: LazyLock<Secp256k1<All>> = LazyLock::new(Secp256k1::new);

/// An unspendable internal key, so a taproot output with no key in its policy has no key path.
const NUMS: &str = "50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";

const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

#[derive(Debug, Clone)]
struct Case {
    tx: Transaction,
    prevouts: Vec<TxOut>,
    flags: Flags,
}

fn core_flags(flags: Flags) -> bitcoinkernel::ScriptVerificationFlags {
    use bitcoinkernel as k;
    [
        (flags.p2sh, k::VERIFY_P2SH),
        (flags.dersig, k::VERIFY_DERSIG),
        (flags.checklocktimeverify, k::VERIFY_CHECKLOCKTIMEVERIFY),
        (flags.checksequenceverify, k::VERIFY_CHECKSEQUENCEVERIFY),
        (flags.witness, k::VERIFY_WITNESS),
        (flags.nulldummy, k::VERIFY_NULLDUMMY),
        (flags.taproot, k::VERIFY_TAPROOT),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(k::VERIFY_NONE, |acc, (_, bit)| acc | bit)
}

/// Only a script failure is invalid. The kernel's other errors mean the case itself is malformed,
/// which must not be counted as agreement, so they panic.
fn core_verdict(case: &Case, tx_bytes: &[u8], vin: usize) -> bool {
    use bitcoinkernel as k;
    let malformed = |e: k::KernelError| -> ! {
        panic!(
            "malformed case, the kernel says {e:?}:\n{}",
            report(case, vin)
        )
    };
    let tx = k::Transaction::new(tx_bytes).unwrap_or_else(|e| malformed(e));
    let spent = case
        .prevouts
        .iter()
        .map(|txout| {
            let script = k::ScriptPubkey::new(txout.script_pubkey.as_bytes())
                .unwrap_or_else(|e| malformed(e));
            k::TxOut::new(&script, txout.value.to_sat() as i64)
        })
        .collect::<Vec<_>>();
    let tx_data = k::PrecomputedTransactionData::new(&tx, &spent).unwrap_or_else(|e| malformed(e));
    let prevout = &case.prevouts[vin];
    let script =
        k::ScriptPubkey::new(prevout.script_pubkey.as_bytes()).unwrap_or_else(|e| malformed(e));
    match k::verify(
        &script,
        Some(prevout.value.to_sat() as i64),
        &tx,
        vin,
        Some(core_flags(case.flags)),
        &tx_data,
    ) {
        Ok(()) => true,
        Err(k::KernelError::ScriptVerify(k::ScriptVerifyError::Invalid)) => false,
        Err(e) => malformed(e),
    }
}

fn report(case: &Case, vin: usize) -> String {
    let prevouts = case
        .prevouts
        .iter()
        .map(|txout| {
            format!(
                "  {} {}",
                txout.value.to_sat(),
                txout.script_pubkey.to_hex_string()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "input {vin}, flags {:?}\ntx {}\nprevouts (sats, script):\n{prevouts}",
        case.flags,
        serialize_hex(&case.tx)
    )
}

/// Each input's verdict, failing the test case where we and Core differ.
fn verdicts(case: &Case) -> Result<Vec<bool>, TestCaseError> {
    let tx_bytes = serialize(&case.tx);
    let prevouts = case.prevouts.iter().map(Some).collect::<Vec<_>>();
    let shared = bdk_script_verify::verify_transaction(&case.tx, &prevouts, case.flags);
    let mut out = Vec::new();
    for (vin, verification) in shared.into_iter().enumerate() {
        prop_assert_eq!(
            &verification,
            &bdk_script_verify::verify_input(
                &mut bdk_script_verify::VerificationCache::new(&case.tx, &prevouts),
                vin,
                case.flags
            ),
            "shared and independent verification differ: {}",
            report(case, vin)
        );
        let ours = verification.result;
        if let Err(Error::MissingPrevouts(missing)) = &ours {
            panic!("every prevout was given, yet {missing:?} are reported missing");
        }
        let core = core_verdict(case, &tx_bytes, vin);
        prop_assert_eq!(
            ours.is_ok(),
            core,
            "we say {:?}, Core says {}\n{}",
            ours,
            if core { "valid" } else { "invalid" },
            report(case, vin)
        );
        out.push(core);
    }
    Ok(out)
}

fn assert_all_valid(case: &Case) -> Result<(), TestCaseError> {
    let verdicts = verdicts(case)?;
    prop_assert!(
        verdicts.iter().all(|v| *v),
        "generator built an invalid spend: {:?}\n{}",
        verdicts,
        report(case, 0)
    );
    Ok(())
}

fn push(out: &mut Vec<u8>, data: &[u8]) {
    let n = data.len();
    if n < usize::from(OP_PUSHDATA1.to_u8()) {
        out.push(n as u8);
    } else if n <= 0xff {
        out.extend([OP_PUSHDATA1.to_u8(), n as u8]);
    } else if n <= 0xffff {
        out.push(OP_PUSHDATA2.to_u8());
        out.extend((n as u16).to_le_bytes());
    } else {
        out.push(OP_PUSHDATA4.to_u8());
        out.extend((n as u32).to_le_bytes());
    }
    out.extend_from_slice(data);
}

fn scriptnum(n: i64) -> Vec<u8> {
    let neg = n < 0;
    let mut abs = n.unsigned_abs();
    let mut out = Vec::new();
    while abs > 0 {
        out.push((abs & 0xff) as u8);
        abs >>= 8;
    }
    if out.last().is_some_and(|b| b & 0x80 != 0) {
        out.push(if neg { 0x80 } else { 0 });
    } else if neg {
        *out.last_mut().unwrap() |= 0x80;
    }
    out
}

#[derive(Debug, Clone)]
enum Op {
    Code(Opcode),
    Push(Vec<u8>),
    Num(i64),
    /// Bytes as they are, which may be a truncated or non-minimal push.
    Raw(Vec<u8>),
    /// A push through OP_PUSHDATA1, 2 or 4, whatever the length.
    PushData(u8, Vec<u8>),
}

fn encode(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            Op::Code(code) => out.push(code.to_u8()),
            Op::Push(data) => push(&mut out, data),
            Op::Num(0) => out.push(OP_PUSHBYTES_0.to_u8()),
            Op::Num(-1) => out.push(OP_PUSHNUM_NEG1.to_u8()),
            Op::Num(n @ 1..=16) => out.push(OP_PUSHNUM_1.to_u8() + *n as u8 - 1),
            Op::Num(n) => push(&mut out, &scriptnum(*n)),
            Op::Raw(bytes) => out.extend_from_slice(bytes),
            Op::PushData(width, data) => {
                match width {
                    1 => out.extend([OP_PUSHDATA1.to_u8(), data.len() as u8]),
                    2 => {
                        out.push(OP_PUSHDATA2.to_u8());
                        out.extend((data.len() as u16).to_le_bytes());
                    }
                    _ => {
                        out.push(OP_PUSHDATA4.to_u8());
                        out.extend((data.len() as u32).to_le_bytes());
                    }
                }
                out.extend_from_slice(data);
            }
        }
    }
    out
}

fn secret(i: u64) -> SecretKey {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&(i + 1).to_be_bytes());
    SecretKey::from_slice(&bytes).unwrap()
}

fn public(sk: &SecretKey) -> PublicKey {
    PublicKey::new(sk.public_key(&SECP))
}

fn x_only(sk: &SecretKey) -> XOnlyPublicKey {
    sk.x_only_public_key(&SECP).0
}

fn single_leaf_taproot(leaf: ScriptBuf, version: LeafVersion) -> (ScriptBuf, Vec<u8>) {
    let info = TaprootBuilder::new()
        .add_leaf_with_ver(0, leaf.clone(), version)
        .unwrap()
        .finalize(&SECP, x_only(&secret(0)))
        .unwrap();
    let control_block = info.control_block(&(leaf, version)).unwrap();
    (
        ScriptBuf::new_p2tr_tweaked(info.output_key()),
        control_block.serialize(),
    )
}

fn prevout_txid(i: usize) -> Txid {
    Txid::from_byte_array([i as u8 + 1; 32])
}

fn unsigned_tx(version: i32, lock_time: u32, sequences: &[u32], outputs: usize) -> Transaction {
    Transaction {
        version: Version(version),
        lock_time: LockTime::from_consensus(lock_time),
        input: sequences
            .iter()
            .enumerate()
            .map(|(i, seq)| TxIn {
                previous_output: OutPoint::new(prevout_txid(i), i as u32),
                script_sig: ScriptBuf::new(),
                sequence: Sequence(*seq),
                witness: Witness::new(),
            })
            .collect(),
        output: (0..outputs)
            .map(|i| TxOut {
                value: Amount::from_sat(1000 + i as u64),
                script_pubkey: ScriptBuf::new_p2wpkh(
                    &public(&secret(90 + i as u64)).wpubkey_hash().unwrap(),
                ),
            })
            .collect(),
    }
}

fn any_flags() -> impl Strategy<Value = Flags> {
    any::<[bool; 7]>().prop_map(|b| Flags {
        p2sh: b[0],
        dersig: b[1],
        checklocktimeverify: b[2],
        checksequenceverify: b[3],
        // Core requires these flags' prerequisites, as `Flags` documents.
        witness: b[4] && b[0],
        nulldummy: b[5],
        taproot: b[6] && b[4] && b[0],
    })
}

fn flags() -> impl Strategy<Value = Flags> {
    prop_oneof![3 => Just(Flags::CONSENSUS), 1 => any_flags()]
}

fn stack_item() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        Just(vec![]),
        Just(vec![0x00]),
        Just(vec![0x01]),
        Just(vec![0x80]),
        Just(vec![0x81]),
        Just(vec![0x00, 0x80]),
        vec(any::<u8>(), 1..5),
        vec(any::<u8>(), 20..=20),
        vec(any::<u8>(), 32..=33),
        vec(any::<u8>(), 64..=65),
        vec(any::<u8>(), 70..=73),
        vec(any::<u8>(), 519..=521),
    ]
}

fn number() -> impl Strategy<Value = i64> {
    prop_oneof![
        4 => -1..=16i64,
        1 => Just(0x7fff_ffff),
        1 => Just(-0x7fff_ffff),
        1 => Just(0x8000_0000),
        1 => Just(0xff_ffff_ffff),
        1 => prop_oneof![Just(127), Just(128), Just(-128), Just(255), Just(256)],
        2 => any::<i32>().prop_map(i64::from),
        2 => 0..1000i64,
    ]
}

fn opcode() -> impl Strategy<Value = Opcode> {
    prop_oneof![
        // Everything that is not a data push, including disabled opcodes and OP_SUCCESSx.
        4 => (OP_PUSHNUM_NEG1.to_u8()..=u8::MAX).prop_map(Opcode::from),
        2 => prop_oneof![
            Just(OP_IF),
            Just(OP_NOTIF),
            Just(OP_ELSE),
            Just(OP_ENDIF),
            Just(OP_VERIFY),
            Just(OP_RETURN)
        ],
        2 => prop_oneof![
            Just(OP_CODESEPARATOR),
            Just(OP_CHECKSIG),
            Just(OP_CHECKSIGVERIFY),
            Just(OP_CHECKMULTISIG),
            Just(OP_CHECKMULTISIGVERIFY),
            Just(OP_CHECKSIGADD),
            Just(OP_CLTV),
            Just(OP_CSV)
        ],
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => opcode().prop_map(Op::Code),
        2 => stack_item().prop_map(Op::Push),
        2 => number().prop_map(Op::Num),
        1 => vec(any::<u8>(), 1..4).prop_map(Op::Raw),
        1 => (prop_oneof![Just(1u8), Just(2), Just(4)], vec(any::<u8>(), 0..4))
            .prop_map(|(width, data)| Op::PushData(width, data)),
    ]
}

fn sequence() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(u32::MAX),
        Just(0xffff_fffe),
        0..1000u32,
        (0..1000u32).prop_map(|n| n | (1 << 22)),
        any::<u32>(),
    ]
}

fn lock_time() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(0),
        0..1000u32,
        500_000_000..500_001_000u32,
        any::<u32>()
    ]
}

fn version() -> impl Strategy<Value = i32> {
    prop_oneof![Just(1), Just(2), any::<i32>()]
}

fn amount() -> impl Strategy<Value = u64> {
    prop_oneof![Just(0), 1..100_000u64, 0..=MAX_MONEY]
}

/// Indices are taken modulo what they index when applied, so shrinking keeps them meaningful.
#[derive(Debug, Clone)]
enum Mutation {
    FlipWitnessBit {
        input: usize,
        elem: usize,
        bit: usize,
    },
    FlipScriptSigBit {
        input: usize,
        bit: usize,
    },
    ReplaceWitnessElem {
        input: usize,
        elem: usize,
        data: Vec<u8>,
    },
    DropWitnessElem {
        input: usize,
        elem: usize,
    },
    DupWitnessElem {
        input: usize,
        elem: usize,
    },
    SwapWitnessElems {
        input: usize,
        a: usize,
        b: usize,
    },
    TruncateWitness {
        input: usize,
        len: usize,
    },
    /// Replaces the last byte of an element: a signature's sighash type.
    SetLastByte {
        input: usize,
        elem: usize,
        byte: u8,
    },
    /// Turns a 64-byte taproot signature into a 65-byte one with an explicit type.
    AppendByte {
        input: usize,
        elem: usize,
        byte: u8,
    },
    TrimLastByte {
        input: usize,
        elem: usize,
    },
    /// Gives the first 64-byte element, a default-type taproot signature, an explicit type.
    TypeSchnorrSig {
        input: usize,
        byte: u8,
    },
    /// Edits the DER structure of the first ECDSA signature found in the scriptSig or witness.
    MangleDer {
        input: usize,
        edit: DerEdit,
    },
    /// Toggles a small element between empty and `0x01`: an IF selector or a dummy.
    ToggleSelector {
        input: usize,
        elem: usize,
    },
    AddAnnex {
        input: usize,
        data: Vec<u8>,
    },
    BumpAmount {
        input: usize,
        delta: i64,
    },
    FlipPrevoutScriptBit {
        input: usize,
        bit: usize,
    },
    SetWitnessVersion {
        input: usize,
        version: u8,
    },
    TruncateWitnessProgram {
        input: usize,
    },
    SetLockTime(u32),
    SetSequence {
        input: usize,
        sequence: u32,
    },
    SetVersion(i32),
    BumpOutput {
        output: usize,
        delta: i64,
    },
    FlipOutputScriptBit {
        output: usize,
        bit: usize,
    },
    PushOutput,
    PopOutput,
}

fn mutation() -> impl Strategy<Value = Mutation> {
    let i = || 0..8usize;
    prop_oneof![
        3 => (i(), i(), any::<usize>()).prop_map(|(input, elem, bit)| Mutation::FlipWitnessBit { input, elem, bit }),
        2 => (i(), any::<usize>()).prop_map(|(input, bit)| Mutation::FlipScriptSigBit { input, bit }),
        1 => (i(), i(), stack_item()).prop_map(|(input, elem, data)| Mutation::ReplaceWitnessElem { input, elem, data }),
        1 => (i(), i()).prop_map(|(input, elem)| Mutation::DropWitnessElem { input, elem }),
        1 => (i(), i()).prop_map(|(input, elem)| Mutation::DupWitnessElem { input, elem }),
        1 => (i(), i(), i()).prop_map(|(input, a, b)| Mutation::SwapWitnessElems { input, a, b }),
        1 => (i(), 0..4usize).prop_map(|(input, len)| Mutation::TruncateWitness { input, len }),
        2 => (i(), i(), sighash_byte()).prop_map(|(input, elem, byte)| Mutation::SetLastByte { input, elem, byte }),
        1 => (i(), i(), sighash_byte()).prop_map(|(input, elem, byte)| Mutation::AppendByte { input, elem, byte }),
        1 => (i(), i()).prop_map(|(input, elem)| Mutation::TrimLastByte { input, elem }),
        2 => (i(), prop_oneof![Just(0x00), sighash_byte()]).prop_map(|(input, byte)| Mutation::TypeSchnorrSig { input, byte }),
        3 => (i(), der_edit()).prop_map(|(input, edit)| Mutation::MangleDer { input, edit }),
        2 => (i(), i()).prop_map(|(input, elem)| Mutation::ToggleSelector { input, elem }),
        1 => (i(), vec(any::<u8>(), 0..4)).prop_map(|(input, data)| Mutation::AddAnnex { input, data }),
        2 => (i(), prop_oneof![Just(1i64), Just(-1), any::<i32>().prop_map(i64::from)]).prop_map(|(input, delta)| Mutation::BumpAmount { input, delta }),
        1 => (i(), any::<usize>()).prop_map(|(input, bit)| Mutation::FlipPrevoutScriptBit { input, bit }),
        1 => (i(), prop_oneof![Just(0u8), 0x51..=0x60u8]).prop_map(|(input, version)| Mutation::SetWitnessVersion { input, version }),
        1 => i().prop_map(|input| Mutation::TruncateWitnessProgram { input }),
        1 => lock_time().prop_map(Mutation::SetLockTime),
        1 => (i(), sequence()).prop_map(|(input, sequence)| Mutation::SetSequence { input, sequence }),
        1 => version().prop_map(Mutation::SetVersion),
        1 => (i(), prop_oneof![Just(1i64), Just(-1)]).prop_map(|(output, delta)| Mutation::BumpOutput { output, delta }),
        1 => (i(), any::<usize>()).prop_map(|(output, bit)| Mutation::FlipOutputScriptBit { output, bit }),
        1 => Just(Mutation::PushOutput),
        1 => Just(Mutation::PopOutput),
    ]
}

fn sighash_byte() -> impl Strategy<Value = u8> {
    prop_oneof![
        3 => prop_oneof![Just(0x00), Just(0x01), Just(0x02), Just(0x03), Just(0x81), Just(0x82), Just(0x83)],
        1 => any::<u8>(),
    ]
}

#[derive(Debug, Clone, Copy)]
enum DerEdit {
    /// Sets one header byte: 0 is the sequence tag, 1 its length, 2 R's tag, 3 R's length, and 4
    /// and 5 S's tag and length.
    Header { field: u8, value: u8 },
    /// Pads R or S with a zero byte, which keeps its value: only strict DER rejects it.
    Pad { s: bool },
    /// Sets the top bit of R or S, making it negative.
    Negate { s: bool },
}

fn der_edit() -> impl Strategy<Value = DerEdit> {
    prop_oneof![
        (0..6u8, any::<u8>()).prop_map(|(field, value)| DerEdit::Header { field, value }),
        any::<bool>().prop_map(|s| DerEdit::Pad { s }),
        any::<bool>().prop_map(|s| DerEdit::Negate { s }),
    ]
}

/// The R and S lengths of a well-formed DER signature with its type byte. An earlier edit can
/// leave a signature malformed, and later edits index by these lengths, so they only apply while
/// they hold.
fn der_layout(e: &[u8]) -> Option<(usize, usize)> {
    let n = e.len();
    if n < 9 || e[0] != 0x30 || e[1] as usize != n - 3 || e[2] != 0x02 {
        return None;
    }
    let len_r = e[3] as usize;
    let s_tag = 4 + len_r;
    if len_r == 0 || s_tag + 2 >= n || e[s_tag] != 0x02 {
        return None;
    }
    let len_s = e[s_tag + 1] as usize;
    (len_s > 0 && s_tag + 2 + len_s == n - 1).then_some((len_r, len_s))
}

fn is_der_like(e: &[u8]) -> bool {
    der_layout(e).is_some()
}

fn mangle_der(sig: &mut Vec<u8>, edit: DerEdit) {
    let Some((len_r, _)) = der_layout(sig) else {
        return;
    };
    let s_tag = 4 + len_r;
    match edit {
        DerEdit::Header { field, value } => {
            let at = [0, 1, 2, 3, s_tag, s_tag + 1][field as usize];
            sig[at] = value;
        }
        DerEdit::Pad { s } => {
            let (len_at, start) = if s { (s_tag + 1, s_tag + 2) } else { (3, 4) };
            let (Some(len), Some(total)) = (sig[len_at].checked_add(1), sig[1].checked_add(1))
            else {
                return;
            };
            sig.insert(start, 0x00);
            sig[len_at] = len;
            sig[1] = total;
        }
        DerEdit::Negate { s } => {
            let start = if s { s_tag + 2 } else { 4 };
            sig[start] |= 0x80;
        }
    }
}

fn flip_bit(bytes: &mut [u8], bit: usize) {
    if !bytes.is_empty() {
        let bit = bit % (bytes.len() * 8);
        bytes[bit / 8] ^= 1 << (bit % 8);
    }
}

fn edit_witness(tx: &mut Transaction, input: usize, edit: impl FnOnce(&mut Vec<Vec<u8>>)) {
    let n = tx.input.len();
    let txin = &mut tx.input[input % n];
    let mut items = txin.witness.to_vec();
    edit(&mut items);
    txin.witness = Witness::from_slice(&items);
}

fn edit_elem(tx: &mut Transaction, input: usize, elem: usize, edit: impl FnOnce(&mut Vec<u8>)) {
    edit_witness(tx, input, |items| {
        if !items.is_empty() {
            let n = items.len();
            edit(&mut items[elem % n]);
        }
    });
}

fn bump(value: u64, delta: i64) -> u64 {
    value.saturating_add_signed(delta).min(MAX_MONEY)
}

fn apply(mutation: &Mutation, case: &mut Case) {
    let tx = &mut case.tx;
    let n = tx.input.len();
    match mutation.clone() {
        Mutation::FlipWitnessBit { input, elem, bit } => {
            edit_elem(tx, input, elem, |e| flip_bit(e, bit))
        }
        Mutation::FlipScriptSigBit { input, bit } => {
            let mut bytes = tx.input[input % n].script_sig.to_bytes();
            flip_bit(&mut bytes, bit);
            tx.input[input % n].script_sig = ScriptBuf::from_bytes(bytes);
        }
        Mutation::ReplaceWitnessElem { input, elem, data } => {
            edit_elem(tx, input, elem, |e| *e = data)
        }
        Mutation::DropWitnessElem { input, elem } => edit_witness(tx, input, |items| {
            if !items.is_empty() {
                let n = items.len();
                items.remove(elem % n);
            }
        }),
        Mutation::DupWitnessElem { input, elem } => edit_witness(tx, input, |items| {
            if !items.is_empty() {
                let at = elem % items.len();
                items.insert(at, items[at].clone());
            }
        }),
        Mutation::SwapWitnessElems { input, a, b } => edit_witness(tx, input, |items| {
            if !items.is_empty() {
                let n = items.len();
                items.swap(a % n, b % n);
            }
        }),
        Mutation::TruncateWitness { input, len } => {
            edit_witness(tx, input, |items| items.truncate(len))
        }
        Mutation::SetLastByte { input, elem, byte } => edit_elem(tx, input, elem, |e| {
            if let Some(last) = e.last_mut() {
                *last = byte;
            }
        }),
        Mutation::AppendByte { input, elem, byte } => edit_elem(tx, input, elem, |e| e.push(byte)),
        Mutation::MangleDer { input, edit } => {
            let txin = &mut tx.input[input % n];
            let mut items = txin.witness.to_vec();
            if let Some(sig) = items.iter_mut().find(|e| is_der_like(e)) {
                mangle_der(sig, edit);
                txin.witness = Witness::from_slice(&items);
            } else {
                let mut pushes = txin
                    .script_sig
                    .instructions()
                    .map(|i| match i {
                        Ok(bitcoin::script::Instruction::PushBytes(b)) => {
                            Some(b.as_bytes().to_vec())
                        }
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .unwrap_or_default();
                if let Some(sig) = pushes.iter_mut().find(|e| is_der_like(e)) {
                    mangle_der(sig, edit);
                    let mut bytes = Vec::new();
                    for p in &pushes {
                        if p.is_empty() {
                            bytes.push(OP_PUSHBYTES_0.to_u8());
                        } else {
                            push(&mut bytes, p);
                        }
                    }
                    txin.script_sig = ScriptBuf::from_bytes(bytes);
                }
            }
        }
        Mutation::TypeSchnorrSig { input, byte } => edit_witness(tx, input, |items| {
            if let Some(sig) = items.iter_mut().find(|e| e.len() == 64) {
                sig.push(byte);
            }
        }),
        Mutation::TrimLastByte { input, elem } => edit_elem(tx, input, elem, |e| {
            e.pop();
        }),
        Mutation::ToggleSelector { input, elem } => edit_witness(tx, input, |items| {
            let small = (0..items.len())
                .filter(|i| items[*i].len() <= 1)
                .collect::<Vec<_>>();
            if !small.is_empty() {
                let e = &mut items[small[elem % small.len()]];
                *e = if e.is_empty() { vec![1] } else { vec![] };
            }
        }),
        Mutation::AddAnnex { input, data } => edit_witness(tx, input, |items| {
            let mut annex = vec![0x50];
            annex.extend(data);
            items.push(annex);
        }),
        Mutation::BumpAmount { input, delta } => {
            let txout = &mut case.prevouts[input % n];
            txout.value = Amount::from_sat(bump(txout.value.to_sat(), delta));
        }
        Mutation::FlipPrevoutScriptBit { input, bit } => {
            let mut bytes = case.prevouts[input % n].script_pubkey.to_bytes();
            flip_bit(&mut bytes, bit);
            case.prevouts[input % n].script_pubkey = ScriptBuf::from_bytes(bytes);
        }
        Mutation::SetWitnessVersion { input, version } => {
            let spk = &mut case.prevouts[input % n].script_pubkey;
            if spk.is_witness_program() {
                let mut bytes = spk.to_bytes();
                bytes[0] = version;
                *spk = ScriptBuf::from_bytes(bytes);
            }
        }
        Mutation::TruncateWitnessProgram { input } => {
            let spk = &mut case.prevouts[input % n].script_pubkey;
            if spk.is_witness_program() {
                let mut bytes = spk.to_bytes();
                bytes.pop();
                bytes[1] -= 1;
                *spk = ScriptBuf::from_bytes(bytes);
            }
        }
        Mutation::SetLockTime(lock_time) => tx.lock_time = LockTime::from_consensus(lock_time),
        Mutation::SetSequence { input, sequence } => {
            tx.input[input % n].sequence = Sequence(sequence)
        }
        Mutation::SetVersion(version) => tx.version = Version(version),
        Mutation::BumpOutput { output, delta } => {
            let m = tx.output.len();
            let txout = &mut tx.output[output % m];
            txout.value = Amount::from_sat(bump(txout.value.to_sat(), delta));
        }
        Mutation::FlipOutputScriptBit { output, bit } => {
            let m = tx.output.len();
            let mut bytes = tx.output[output % m].script_pubkey.to_bytes();
            flip_bit(&mut bytes, bit);
            tx.output[output % m].script_pubkey = ScriptBuf::from_bytes(bytes);
        }
        Mutation::PushOutput => tx.output.push(TxOut {
            value: Amount::from_sat(1),
            script_pubkey: ScriptBuf::new(),
        }),
        // An output is kept so the transaction stays unambiguous to deserialize.
        Mutation::PopOutput => {
            if tx.output.len() > 1 {
                tx.output.pop();
            }
        }
    }
}

#[derive(Debug, Clone)]
enum Placement {
    Bare,
    P2sh,
    P2wsh,
    P2shP2wsh,
    Tapscript {
        leaf_version: u8,
    },
    /// No script: the output is a witness program of this version and length.
    WitnessProgram {
        version: u8,
        len: usize,
    },
}

fn placement() -> impl Strategy<Value = Placement> {
    prop_oneof![
        2 => Just(Placement::Bare),
        2 => Just(Placement::P2sh),
        2 => Just(Placement::P2wsh),
        2 => Just(Placement::P2shP2wsh),
        2 => Just(Placement::Tapscript { leaf_version: 0xc0 }),
        // An unknown leaf version succeeds unconditionally; 0x50 would be an annex.
        1 => (0..0x80u8).prop_filter("not annex", |v| *v != 0x28).prop_map(|v| Placement::Tapscript { leaf_version: v * 2 }),
        1 => (prop_oneof![Just(0u8), 1..=16u8], prop_oneof![Just(20usize), Just(32), 0..=41usize])
            .prop_map(|(version, len)| Placement::WitnessProgram { version, len }),
    ]
}

#[derive(Debug, Clone)]
struct ScriptSpec {
    placement: Placement,
    script: Vec<Op>,
    script_sig: Vec<Op>,
    /// The stack the script starts with: pushed in the scriptSig of a legacy placement, and the
    /// witness of a segwit one.
    stack: Vec<Vec<u8>>,
    /// Witness items below the stack, so a legacy placement gets an unexpected witness.
    witness: Vec<Vec<u8>>,
    /// Appended after the control block, so only a tapscript spend has one.
    annex: Option<Vec<u8>>,
    flags: Flags,
    version: i32,
    lock_time: u32,
    sequence: u32,
    amount: u64,
}

/// The parts of a [`ScriptSpec`] that shape the script: script, scriptSig, stack and witness.
type Shape = (Vec<Op>, Vec<Op>, Vec<Vec<u8>>, Vec<Vec<u8>>);

fn script_spec(shape: impl Strategy<Value = Shape>) -> impl Strategy<Value = ScriptSpec> {
    (
        placement(),
        shape,
        proptest::option::weighted(0.1, vec(any::<u8>(), 0..4)),
        flags(),
        version(),
        lock_time(),
        sequence(),
        amount(),
    )
        .prop_map(
            |(
                placement,
                (script, script_sig, stack, witness),
                annex,
                flags,
                version,
                lock_time,
                sequence,
                amount,
            )| ScriptSpec {
                placement,
                script,
                script_sig,
                stack,
                witness,
                annex,
                flags,
                version,
                lock_time,
                sequence,
                amount,
            },
        )
}

fn random_shape() -> impl Strategy<Value = Shape> {
    (
        vec(op(), 0..=12),
        vec(op(), 0..=4),
        vec(stack_item(), 0..=4),
        prop_oneof![3 => Just(vec![]), 1 => vec(stack_item(), 1..=2)],
    )
}

/// Simple ops for the bodies of shaped scripts, so that they often run to completion.
fn plain_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        number().prop_map(Op::Num),
        Just(Op::Code(OP_DROP)),
        Just(Op::Code(OP_DUP)),
        Just(Op::Code(OP_EQUAL)),
    ]
}

fn op_success() -> impl Strategy<Value = u8> {
    prop_oneof![
        // The ends of each range, where an off-by-one would be.
        prop_oneof![
            Just(126u8),
            Just(129),
            Just(131),
            Just(134),
            Just(149),
            Just(153),
            Just(187),
            Just(254)
        ],
        Just(80u8),
        Just(98),
        126..=129u8,
        131..=134u8,
        137..=138u8,
        141..=142u8,
        149..=153u8,
        187..=254u8,
    ]
}

/// Scripts shaped to reach one rule each, leaving the transaction fields and the items they
/// consume random.
fn shaped() -> impl Strategy<Value = Shape> {
    let conditional = (
        prop_oneof![Just(OP_IF), Just(OP_NOTIF)],
        vec(plain_op(), 0..3),
        proptest::option::of(vec(plain_op(), 0..3)),
        stack_item(),
    )
        .prop_map(|(code, then, otherwise, condition)| {
            let mut ops = vec![Op::Code(code)];
            ops.extend(then);
            if let Some(otherwise) = otherwise {
                ops.push(Op::Code(OP_ELSE));
                ops.extend(otherwise);
            }
            ops.extend([Op::Code(OP_ENDIF), Op::Num(1)]);
            (ops, vec![], vec![condition], vec![])
        });
    let timelock = (
        prop_oneof![number(), (0..1000i64).prop_map(|n| n | (1 << 22))],
        prop_oneof![Just(OP_CLTV), Just(OP_CSV)],
    )
        .prop_map(|(n, code)| {
            let ops = vec![Op::Num(n), Op::Code(code), Op::Code(OP_DROP), Op::Num(1)];
            (ops, vec![], vec![], vec![])
        });
    let checksig = (
        prop_oneof![
            Just(vec![]),
            vec(any::<u8>(), 1..5),
            vec(any::<u8>(), 32..=32),
            vec(any::<u8>(), 33..=33),
        ],
        stack_item(),
        0..4u8,
    )
        .prop_map(|(pubkey, sig, form)| {
            let ops = match form {
                0 => vec![Op::Push(pubkey), Op::Code(OP_CHECKSIG)],
                1 => vec![Op::Push(pubkey), Op::Code(OP_CHECKSIGVERIFY), Op::Num(1)],
                // A check that fails softly passes here, so a hard failure is observable.
                2 => vec![Op::Push(pubkey), Op::Code(OP_CHECKSIG), Op::Code(OP_NOT)],
                _ => vec![Op::Num(0), Op::Push(pubkey), Op::Code(OP_CHECKSIGADD)],
            };
            (ops, vec![], vec![sig], vec![])
        });
    // A script that passes, so only the scriptSig's own rules decide.
    let pushes = || vec(stack_item().prop_map(Op::Push), 0..=2);
    let harmless = prop_oneof![
        Just(OP_NOP),
        Just(OP_NOP1),
        (OP_NOP4.to_u8()..=OP_NOP10.to_u8()).prop_map(Opcode::from)
    ]
    .prop_map(Op::Code);
    let trivial = prop_oneof![
        vec(op(), 0..=4),
        pushes(),
        (pushes(), harmless, any::<usize>()).prop_map(|(mut ops, op, at)| {
            let at = at % (ops.len() + 1);
            ops.insert(at, op);
            ops
        }),
    ]
    .prop_map(|script_sig| (vec![Op::Num(1)], script_sig, vec![], vec![]));
    let success =
        (vec(op(), 0..=6), any::<usize>(), op_success()).prop_map(|(mut ops, at, code)| {
            let at = at % (ops.len() + 1);
            ops.insert(at, Op::Code(Opcode::from(code)));
            (ops, vec![], vec![], vec![])
        });
    prop_oneof![conditional, timelock, checksig, trivial, success]
}

fn build_script_case(spec: &ScriptSpec) -> Case {
    let script = ScriptBuf::from_bytes(encode(&spec.script));
    let mut script_sig = encode(&spec.script_sig);
    let mut witness = spec.witness.clone();
    let legacy = matches!(spec.placement, Placement::Bare | Placement::P2sh);
    if legacy {
        for item in &spec.stack {
            push(&mut script_sig, item);
        }
    } else {
        witness.extend(spec.stack.iter().cloned());
    }
    let script_pubkey = match spec.placement {
        Placement::Bare => script,
        Placement::P2sh => {
            push(&mut script_sig, script.as_bytes());
            ScriptBuf::new_p2sh(&script.script_hash())
        }
        Placement::P2wsh => {
            witness.push(script.to_bytes());
            ScriptBuf::new_p2wsh(&script.wscript_hash())
        }
        Placement::P2shP2wsh => {
            let program = ScriptBuf::new_p2wsh(&script.wscript_hash());
            push(&mut script_sig, program.as_bytes());
            witness.push(script.to_bytes());
            ScriptBuf::new_p2sh(&program.script_hash())
        }
        Placement::Tapscript { leaf_version } => {
            let version = LeafVersion::from_consensus(leaf_version).unwrap();
            let (output, control_block) = single_leaf_taproot(script.clone(), version);
            witness.push(script.to_bytes());
            witness.push(control_block);
            if let Some(data) = &spec.annex {
                witness.push([&[0x50], &data[..]].concat());
            }
            output
        }
        Placement::WitnessProgram { version, len } => {
            let mut bytes = vec![if version == 0 { 0 } else { 0x50 + version }];
            push(&mut bytes, &vec![0x42; len]);
            ScriptBuf::from_bytes(bytes)
        }
    };
    let mut tx = unsigned_tx(spec.version, spec.lock_time, &[spec.sequence], 1);
    tx.input[0].script_sig = ScriptBuf::from_bytes(script_sig);
    tx.input[0].witness = Witness::from_slice(&witness);
    Case {
        tx,
        prevouts: vec![TxOut {
            value: Amount::from_sat(spec.amount),
            script_pubkey,
        }],
        flags: spec.flags,
    }
}

/// Scripts built to sit on either side of an interpreter limit.
#[derive(Debug, Clone)]
enum Limit {
    /// `n` non-push opcodes, against the 201-opcode limit.
    OpCount,
    /// `n` stack items, dropped again, against the 1000-item limit.
    StackSize,
    /// `n` items moved to the altstack, which counts toward the same limit.
    AltStack,
    /// `n` initial witness stack items, against tapscript's 1000-item limit before execution.
    WitnessItems,
    /// A push of `n` bytes, against the 520-byte element limit.
    PushSize,
    /// A script of about `n` bytes, against the 10,000-byte limit.
    ScriptSize,
    /// A CHECKMULTISIG with `n` keys, against the 20-key limit and the opcode count it adds to.
    MultisigKeys,
    /// `n` opcodes before a 20-key CHECKMULTISIG, whose keys count toward the opcode limit.
    MultisigOpCount,
}

#[derive(Debug, Clone)]
struct LimitSpec {
    limit: Limit,
    delta: i64,
    placement: Placement,
    flags: Flags,
}

const LIMITS: [Limit; 8] = [
    Limit::OpCount,
    Limit::StackSize,
    Limit::AltStack,
    Limit::WitnessItems,
    Limit::PushSize,
    Limit::ScriptSize,
    Limit::MultisigKeys,
    Limit::MultisigOpCount,
];

const LIMIT_DELTAS: core::ops::RangeInclusive<i64> = -3..=3;

const LIMIT_PLACEMENTS: [Placement; 4] = [
    Placement::Bare,
    Placement::P2sh,
    Placement::P2wsh,
    Placement::Tapscript { leaf_version: 0xc0 },
];

fn limit_spec() -> impl Strategy<Value = LimitSpec> {
    (
        proptest::sample::select(LIMITS.to_vec()),
        LIMIT_DELTAS,
        proptest::sample::select(LIMIT_PLACEMENTS.to_vec()),
        flags(),
    )
        .prop_map(|(limit, delta, placement, flags)| LimitSpec {
            limit,
            delta,
            placement,
            flags,
        })
}

fn build_limit_case(spec: &LimitSpec) -> Case {
    const ONE: Op = Op::Num(1);
    const DROP: Op = Op::Code(OP_DROP);
    let n = |base: i64| (base + spec.delta) as usize;
    let mut witness = Vec::new();
    let script = match spec.limit {
        Limit::OpCount => {
            let mut ops = vec![Op::Code(OP_NOP); n(201)];
            ops.push(ONE);
            ops
        }
        Limit::StackSize => {
            let k = n(1000);
            let mut ops = vec![ONE; k];
            ops.extend(vec![DROP; k - 1]);
            ops
        }
        Limit::AltStack => {
            let mut ops = Vec::new();
            for _ in 0..n(500) {
                ops.extend([ONE, ONE, Op::Code(OP_TOALTSTACK)]);
            }
            ops.push(ONE);
            ops
        }
        Limit::WitnessItems => {
            let k = n(1000);
            witness = vec![vec![]; k];
            let mut ops = vec![Op::Code(OP_2DROP); k / 2];
            if k % 2 == 1 {
                ops.push(DROP);
            }
            ops.push(ONE);
            ops
        }
        Limit::PushSize => vec![Op::Push(vec![0x42; n(520)]), DROP, ONE],
        Limit::ScriptSize => {
            let mut ops = Vec::new();
            for _ in 0..19 {
                ops.extend([Op::Push(vec![0x42; 520]), DROP]);
            }
            ops.extend([Op::Push(vec![0x42; n(40)]), DROP, ONE]);
            ops
        }
        Limit::MultisigKeys | Limit::MultisigOpCount => {
            let (nops, keys) = match spec.limit {
                Limit::MultisigKeys => (0, n(20)),
                _ => (n(201 - 21), 20),
            };
            let mut ops = vec![Op::Code(OP_NOP); nops];
            ops.extend([Op::Num(0), Op::Num(0)]);
            ops.extend(vec![Op::Push(public(&secret(1)).to_bytes()); keys]);
            ops.extend([Op::Num(keys as i64), Op::Code(OP_CHECKMULTISIG)]);
            ops
        }
    };
    build_script_case(&ScriptSpec {
        placement: spec.placement.clone(),
        script,
        script_sig: vec![],
        stack: vec![],
        witness,
        annex: None,
        flags: spec.flags,
        version: 2,
        lock_time: 0,
        sequence: u32::MAX,
        amount: 1000,
    })
}

/// A tapscript leaf; every one is in each script-path tree, so the others are its siblings.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Leaf {
    Single,
    Multi,
    /// `<k1> CODESEPARATOR CHECKSIG`, whose signature commits to the separator's position.
    CodeSep,
    /// `n - 1` repeats of `DUP <k1> CHECKSIGVERIFY`, then `<k1> CHECKSIG`, all on one signature,
    /// so that `n` crosses the sigop budget that the witness size buys.
    Repeat(u8),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    P2pkh,
    P2shMultisig,
    P2wpkh,
    P2shP2wpkh,
    P2wshMultisig,
    /// `<k0> CHECKSIGVERIFY CODESEPARATOR <k1> CHECKSIG`, in P2SH or P2WSH, so the two
    /// signatures commit to different script codes.
    CodeSep {
        witness: bool,
    },
    /// P2SH `<sig> DROP <k0> CHECKSIG` carrying its own signature, which FindAndDelete removes
    /// from the script code it signs.
    FindAndDelete,
    P2trKey,
    P2trScript(Leaf),
}

impl Kind {
    fn is_taproot(self) -> bool {
        matches!(self, Kind::P2trKey | Kind::P2trScript(_))
    }
}

#[derive(Debug, Clone, Copy)]
enum Sighash {
    All,
    None,
    Single,
    AcpAll,
    AcpNone,
    AcpSingle,
    /// Taproot's implicit ALL; ALL elsewhere.
    Default,
    /// A non-standard type byte, which ECDSA signatures commit to as is; ALL for taproot.
    Raw(u8),
    /// A taproot signature over the default type that also spells it out as 0x00, which BIP341
    /// forbids; ALL elsewhere.
    ExplicitDefault,
}

impl Sighash {
    fn ecdsa(self) -> EcdsaSighashType {
        match self {
            Sighash::None => EcdsaSighashType::None,
            Sighash::Single => EcdsaSighashType::Single,
            Sighash::AcpAll => EcdsaSighashType::AllPlusAnyoneCanPay,
            Sighash::AcpNone => EcdsaSighashType::NonePlusAnyoneCanPay,
            Sighash::AcpSingle => EcdsaSighashType::SinglePlusAnyoneCanPay,
            Sighash::All | Sighash::Default | Sighash::Raw(_) | Sighash::ExplicitDefault => {
                EcdsaSighashType::All
            }
        }
    }

    fn raw(self) -> u8 {
        match self {
            Sighash::Raw(byte) => byte,
            other => other.ecdsa() as u8,
        }
    }

    fn taproot(self) -> TapSighashType {
        match self {
            Sighash::Default | Sighash::ExplicitDefault => TapSighashType::Default,
            Sighash::Raw(_) => TapSighashType::All,
            other => TapSighashType::from_consensus_u8(other.ecdsa() as u8).unwrap(),
        }
    }

    fn is_single(self) -> bool {
        matches!(self, Sighash::Single | Sighash::AcpSingle)
    }
}

fn sighash() -> impl Strategy<Value = Sighash> {
    prop_oneof![
        3 => Just(Sighash::All),
        1 => Just(Sighash::None),
        1 => Just(Sighash::Single),
        1 => Just(Sighash::AcpAll),
        1 => Just(Sighash::AcpNone),
        1 => Just(Sighash::AcpSingle),
        2 => Just(Sighash::Default),
        2 => any::<u8>().prop_map(Sighash::Raw),
        1 => Just(Sighash::ExplicitDefault),
    ]
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        1 => Just(Kind::P2pkh),
        1 => Just(Kind::P2shMultisig),
        1 => Just(Kind::P2wpkh),
        1 => Just(Kind::P2shP2wpkh),
        1 => Just(Kind::P2wshMultisig),
        1 => Just(Kind::CodeSep { witness: false }),
        1 => Just(Kind::CodeSep { witness: true }),
        1 => Just(Kind::FindAndDelete),
        1 => Just(Kind::P2trKey),
        1 => Just(Kind::P2trScript(Leaf::Single)),
        1 => Just(Kind::P2trScript(Leaf::Multi)),
        1 => Just(Kind::P2trScript(Leaf::CodeSep)),
        // Around where the sigop budget runs out.
        3 => (8..=20u8).prop_map(|n| Kind::P2trScript(Leaf::Repeat(n))),
    ]
}

#[derive(Debug, Clone)]
struct InputSpec {
    kind: Kind,
    key: u8,
    sighash: Sighash,
    amount: u64,
    sequence: u32,
    /// A taproot input's annex, without its 0x50 tag, which the signature commits to.
    annex: Option<Vec<u8>>,
}

fn input_spec() -> impl Strategy<Value = InputSpec> {
    (
        kind(),
        any::<u8>(),
        sighash(),
        amount(),
        sequence(),
        proptest::option::weighted(0.2, vec(any::<u8>(), 0..4)),
    )
        .prop_map(|(kind, key, sighash, amount, sequence, annex)| InputSpec {
            kind,
            key,
            sighash,
            amount,
            sequence,
            annex,
        })
}

/// Replaces byte `pos` of a tapscript leaf before signing, with the tree rebuilt to commit to it,
/// so that the edited script is what executes.
#[derive(Debug, Clone)]
struct LeafEdit {
    input: usize,
    pos: usize,
    byte: u8,
}

#[derive(Debug, Clone)]
struct SignedSpec {
    inputs: Vec<InputSpec>,
    /// One output only, so a SIGHASH_SINGLE input past it hits the legacy "one" hash.
    one_output: bool,
    version: i32,
    lock_time: u32,
    leaf_edit: Option<LeafEdit>,
    mutations: Vec<Mutation>,
    flags: Flags,
}

fn signed_spec() -> impl Strategy<Value = SignedSpec> {
    (
        vec(input_spec(), 1..=3),
        any::<bool>(),
        version(),
        lock_time(),
        proptest::option::weighted(
            0.2,
            (0..3usize, any::<usize>(), any::<u8>()).prop_map(|(input, pos, byte)| LeafEdit {
                input,
                pos,
                byte,
            }),
        ),
        vec(mutation(), 0..=3),
        flags(),
    )
        .prop_map(
            |(inputs, one_output, version, lock_time, leaf_edit, mutations, flags)| SignedSpec {
                inputs,
                one_output,
                version,
                lock_time,
                leaf_edit,
                mutations,
                flags,
            },
        )
}

fn ecdsa_sig(sighash: [u8; 32], sk: &SecretKey, type_byte: u8) -> Vec<u8> {
    let sig = SECP.sign_ecdsa(&Message::from_digest(sighash), sk);
    let mut out = sig.serialize_der().to_vec();
    out.push(type_byte);
    out
}

fn schnorr_sig(sighash: [u8; 32], keypair: &Keypair, ty: TapSighashType) -> Vec<u8> {
    let sig = SECP.sign_schnorr_no_aux_rand(&Message::from_digest(sighash), keypair);
    let mut out = sig.serialize().to_vec();
    if ty != TapSighashType::Default {
        out.push(ty as u8);
    }
    out
}

/// BIP143 over the raw type byte, which rust-bitcoin's typed API cannot express. Written from the
/// BIP rather than shared with the verifier, so that Core is the only judge of both.
fn bip143_sighash(
    tx: &Transaction,
    vin: usize,
    script_code: &[u8],
    value: Amount,
    raw: u32,
) -> [u8; 32] {
    let dsha = |data: &[u8]| sha256d::Hash::hash(data).to_byte_array();
    let base = raw & 0x1f;
    let anyone_can_pay = raw & 0x80 != 0;
    let (none, single) = (base == 2, base == 3);
    let prevouts = if anyone_can_pay {
        [0; 32]
    } else {
        dsha(
            &tx.input
                .iter()
                .flat_map(|i| serialize(&i.previous_output))
                .collect::<Vec<_>>(),
        )
    };
    let sequences = if anyone_can_pay || none || single {
        [0; 32]
    } else {
        dsha(
            &tx.input
                .iter()
                .flat_map(|i| i.sequence.0.to_le_bytes())
                .collect::<Vec<_>>(),
        )
    };
    let outputs = if !none && !single {
        dsha(&tx.output.iter().flat_map(serialize).collect::<Vec<_>>())
    } else if single && vin < tx.output.len() {
        dsha(&serialize(&tx.output[vin]))
    } else {
        [0; 32]
    };
    let mut data = Vec::new();
    data.extend(tx.version.0.to_le_bytes());
    data.extend(prevouts);
    data.extend(sequences);
    data.extend(serialize(&tx.input[vin].previous_output));
    data.extend(serialize(&ScriptBuf::from_bytes(script_code.to_vec())));
    data.extend(value.to_sat().to_le_bytes());
    data.extend(tx.input[vin].sequence.0.to_le_bytes());
    data.extend(outputs);
    data.extend(tx.lock_time.to_consensus_u32().to_le_bytes());
    data.extend(raw.to_le_bytes());
    dsha(&data)
}

fn multisig_script(keys: &[SecretKey]) -> ScriptBuf {
    let mut bytes = vec![OP_PUSHNUM_2.to_u8()];
    for sk in keys {
        push(&mut bytes, &public(sk).to_bytes());
    }
    bytes.extend([OP_PUSHNUM_3.to_u8(), OP_CHECKMULTISIG.to_u8()]);
    ScriptBuf::from_bytes(bytes)
}

fn codesep_tail(keys: &[SecretKey]) -> Vec<u8> {
    let mut tail = Vec::new();
    push(&mut tail, &public(&keys[1]).to_bytes());
    tail.push(OP_CHECKSIG.to_u8());
    tail
}

fn codesep_script(keys: &[SecretKey], with_separator: bool) -> Vec<u8> {
    let mut script = Vec::new();
    push(&mut script, &public(&keys[0]).to_bytes());
    script.push(OP_CHECKSIGVERIFY.to_u8());
    if with_separator {
        script.push(OP_CODESEPARATOR.to_u8());
    }
    script.extend(codesep_tail(keys));
    script
}

fn leaf_script(leaf: Leaf, keys: &[SecretKey]) -> Vec<u8> {
    let k1 = x_only(&keys[1]).serialize();
    let mut out = Vec::new();
    match leaf {
        Leaf::Single => {
            push(&mut out, &k1);
            out.push(OP_CHECKSIG.to_u8());
        }
        Leaf::Multi => {
            push(&mut out, &k1);
            out.push(OP_CHECKSIG.to_u8());
            push(&mut out, &x_only(&keys[2]).serialize());
            out.extend([OP_CHECKSIGADD, OP_PUSHNUM_2, OP_NUMEQUAL].map(Opcode::to_u8));
        }
        Leaf::CodeSep => {
            push(&mut out, &k1);
            out.extend([OP_CODESEPARATOR, OP_CHECKSIG].map(Opcode::to_u8));
        }
        Leaf::Repeat(n) => {
            for _ in 1..n {
                out.push(OP_DUP.to_u8());
                push(&mut out, &k1);
                out.push(OP_CHECKSIGVERIFY.to_u8());
            }
            push(&mut out, &k1);
            out.push(OP_CHECKSIG.to_u8());
        }
    }
    out
}

struct Prepared {
    script_pubkey: ScriptBuf,
    keys: Vec<SecretKey>,
    /// The redeem or witness script.
    script: Option<ScriptBuf>,
    leaf: Option<ScriptBuf>,
    control_block: Option<Vec<u8>>,
    merkle_root: Option<TapNodeHash>,
}

fn prepare(input: &InputSpec, index: usize, edit: Option<&LeafEdit>) -> Prepared {
    let base = u64::from(input.key) * 4 + index as u64 * 1024;
    let keys = (0..3).map(|i| secret(base + i)).collect::<Vec<_>>();
    let mut prepared = Prepared {
        script_pubkey: ScriptBuf::new(),
        keys: keys.clone(),
        script: None,
        leaf: None,
        control_block: None,
        merkle_root: None,
    };
    let wpkh = public(&keys[0]).wpubkey_hash().unwrap();
    match input.kind {
        Kind::P2pkh => {
            prepared.script_pubkey = ScriptBuf::new_p2pkh(&public(&keys[0]).pubkey_hash())
        }
        Kind::P2wpkh => prepared.script_pubkey = ScriptBuf::new_p2wpkh(&wpkh),
        Kind::P2shP2wpkh => {
            let program = ScriptBuf::new_p2wpkh(&wpkh);
            prepared.script_pubkey = ScriptBuf::new_p2sh(&program.script_hash());
            prepared.script = Some(program);
        }
        Kind::P2shMultisig | Kind::CodeSep { witness: false } => {
            let redeem = match input.kind {
                Kind::P2shMultisig => multisig_script(&keys),
                _ => ScriptBuf::from_bytes(codesep_script(&keys, true)),
            };
            prepared.script_pubkey = ScriptBuf::new_p2sh(&redeem.script_hash());
            prepared.script = Some(redeem);
        }
        Kind::P2wshMultisig | Kind::CodeSep { witness: true } => {
            let ws = match input.kind {
                Kind::P2wshMultisig => multisig_script(&keys),
                _ => ScriptBuf::from_bytes(codesep_script(&keys, true)),
            };
            prepared.script_pubkey = ScriptBuf::new_p2wsh(&ws.wscript_hash());
            prepared.script = Some(ws);
        }
        // Its output script contains its signature, so it is known only after signing.
        Kind::FindAndDelete => {}
        Kind::P2trKey => {
            let info = TaprootBuilder::new()
                .finalize(&SECP, x_only(&keys[0]))
                .unwrap();
            prepared.script_pubkey = ScriptBuf::new_p2tr_tweaked(info.output_key());
        }
        Kind::P2trScript(spent) => {
            let sibling_repeat = match spent {
                Leaf::Repeat(n) => n,
                _ => 2,
            };
            let mut leaves = [
                Leaf::Single,
                Leaf::Multi,
                Leaf::CodeSep,
                Leaf::Repeat(sibling_repeat),
            ]
            .map(|leaf| (leaf, leaf_script(leaf, &keys)));
            let spent_at = leaves.iter().position(|(leaf, _)| *leaf == spent).unwrap();
            if let Some(edit) = edit.filter(|e| e.input == index) {
                let leaf = &mut leaves[spent_at].1;
                let n = leaf.len();
                leaf[edit.pos % n] = edit.byte;
            }
            let leaves = leaves.map(|(_, script)| ScriptBuf::from_bytes(script));
            let info = leaves
                .iter()
                .fold(TaprootBuilder::new(), |builder, leaf| {
                    builder.add_leaf(2, leaf.clone()).unwrap()
                })
                .finalize(&SECP, x_only(&keys[0]))
                .unwrap();
            let leaf = leaves[spent_at].clone();
            prepared.control_block = Some(
                info.control_block(&(leaf.clone(), LeafVersion::TapScript))
                    .unwrap()
                    .serialize(),
            );
            prepared.leaf = Some(leaf);
            prepared.merkle_root = info.merkle_root();
            prepared.script_pubkey = ScriptBuf::new_p2tr_tweaked(info.output_key());
        }
    }
    prepared
}

/// Signs a non-taproot input, whose signatures commit to no other input's spent output.
fn sign_ecdsa_input(
    unsigned: &Transaction,
    vin: usize,
    input: &InputSpec,
    p: &mut Prepared,
    txin: &mut TxIn,
) {
    let ty = input.sighash.raw();
    let amount = Amount::from_sat(input.amount);
    let legacy = |code: &[u8], sk: &SecretKey| {
        let hash = SighashCache::new(unsigned)
            .legacy_signature_hash(vin, bitcoin::Script::from_bytes(code), ty.into())
            .unwrap();
        ecdsa_sig(hash.to_byte_array(), sk, ty)
    };
    let segwit = |code: &[u8], sk: &SecretKey| {
        ecdsa_sig(
            bip143_sighash(unsigned, vin, code, amount, ty.into()),
            sk,
            ty,
        )
    };
    let pk = public(&p.keys[0]).to_bytes();
    let mut script_sig = Vec::new();
    let mut witness = Vec::new();
    match input.kind {
        Kind::P2pkh => {
            push(
                &mut script_sig,
                &legacy(p.script_pubkey.as_bytes(), &p.keys[0]),
            );
            push(&mut script_sig, &pk);
        }
        Kind::P2shMultisig => {
            let redeem = p.script.as_ref().unwrap();
            script_sig.push(OP_PUSHBYTES_0.to_u8());
            for sk in &p.keys[..2] {
                push(&mut script_sig, &legacy(redeem.as_bytes(), sk));
            }
            push(&mut script_sig, redeem.as_bytes());
        }
        Kind::P2wpkh | Kind::P2shP2wpkh => {
            let code = ScriptBuf::new_p2pkh(&public(&p.keys[0]).pubkey_hash());
            witness = vec![segwit(code.as_bytes(), &p.keys[0]), pk];
            if let Some(program) = &p.script {
                push(&mut script_sig, program.as_bytes());
            }
        }
        Kind::P2wshMultisig => {
            let ws = p.script.as_ref().unwrap();
            witness.push(vec![]);
            for sk in &p.keys[..2] {
                witness.push(segwit(ws.as_bytes(), sk));
            }
            witness.push(ws.to_bytes());
        }
        Kind::CodeSep { witness: false } => {
            // Legacy script codes drop every separator; the second also starts after it.
            let first = legacy(&codesep_script(&p.keys, false), &p.keys[0]);
            let second = legacy(&codesep_tail(&p.keys), &p.keys[1]);
            push(&mut script_sig, &second);
            push(&mut script_sig, &first);
            push(&mut script_sig, p.script.as_ref().unwrap().as_bytes());
        }
        Kind::CodeSep { witness: true } => {
            // Segwit v0 keeps the separators, only starting the second code after the one it ran.
            let ws = p.script.as_ref().unwrap();
            let first = segwit(ws.as_bytes(), &p.keys[0]);
            let second = segwit(&codesep_tail(&p.keys), &p.keys[1]);
            witness = vec![second, first, ws.to_bytes()];
        }
        Kind::FindAndDelete => {
            let mut code = vec![OP_DROP.to_u8()];
            push(&mut code, &pk);
            code.push(OP_CHECKSIG.to_u8());
            let sig = legacy(&code, &p.keys[0]);
            let mut redeem = Vec::new();
            push(&mut redeem, &sig);
            redeem.extend(code);
            let redeem = ScriptBuf::from_bytes(redeem);
            p.script_pubkey = ScriptBuf::new_p2sh(&redeem.script_hash());
            push(&mut script_sig, &sig);
            push(&mut script_sig, redeem.as_bytes());
        }
        Kind::P2trKey | Kind::P2trScript(_) => unreachable!("taproot inputs are signed after"),
    }
    txin.script_sig = ScriptBuf::from_bytes(script_sig);
    txin.witness = Witness::from_slice(&witness);
}

/// The spend, correctly signed, before any mutation.
fn build_signed_case(spec: &SignedSpec) -> Case {
    let edit = spec.leaf_edit.as_ref();
    let mut prepared = spec
        .inputs
        .iter()
        .enumerate()
        .map(|(i, input)| prepare(input, i, edit))
        .collect::<Vec<_>>();
    let any_taproot = spec.inputs.iter().any(|i| i.kind.is_taproot());
    let outputs = if spec.one_output && !any_taproot {
        1
    } else {
        spec.inputs.len()
    };
    let sequences = spec.inputs.iter().map(|i| i.sequence).collect::<Vec<_>>();
    let mut tx = unsigned_tx(spec.version, spec.lock_time, &sequences, outputs);
    let unsigned = tx.clone();

    for (vin, (input, p)) in spec.inputs.iter().zip(&mut prepared).enumerate() {
        if !input.kind.is_taproot() {
            sign_ecdsa_input(&unsigned, vin, input, p, &mut tx.input[vin]);
        }
    }

    let prevouts = spec
        .inputs
        .iter()
        .zip(&prepared)
        .map(|(input, p)| TxOut {
            value: Amount::from_sat(input.amount),
            script_pubkey: p.script_pubkey.clone(),
        })
        .collect::<Vec<_>>();
    let mut cache = SighashCache::new(&unsigned);
    for (vin, (input, p)) in spec.inputs.iter().zip(&prepared).enumerate() {
        if !input.kind.is_taproot() {
            continue;
        }
        let mut ty = input.sighash.taproot();
        // A taproot SIGHASH_SINGLE with no matching output has no signature hash at all.
        if input.sighash.is_single() && vin >= outputs {
            ty = TapSighashType::All;
        }
        let all = Prevouts::All(&prevouts);
        let annex = input
            .annex
            .as_ref()
            .map(|data| [&[0x50], &data[..]].concat());
        let mut sighash = |leaf: Option<(TapLeafHash, u32)>| {
            let annex = annex.as_deref().map(|a| Annex::new(a).unwrap());
            cache
                .taproot_signature_hash(vin, &all, annex, leaf, ty)
                .unwrap()
                .to_byte_array()
        };
        let mut items = match (&p.leaf, input.kind) {
            (Some(leaf), Kind::P2trScript(spent)) => {
                let leaf_hash = TapLeafHash::from_script(leaf, LeafVersion::TapScript);
                // The separator is opcode 1 of its leaf; no other leaf has one.
                let codesep_pos = if spent == Leaf::CodeSep { 1 } else { u32::MAX };
                let hash = sighash(Some((leaf_hash, codesep_pos)));
                let sign =
                    |sk: &SecretKey| schnorr_sig(hash, &Keypair::from_secret_key(&SECP, sk), ty);
                let mut items = Vec::new();
                // The 2-of-2 leaf checks key 1 first, which is the top of the stack.
                if spent == Leaf::Multi {
                    items.push(sign(&p.keys[2]));
                }
                items.push(sign(&p.keys[1]));
                items.push(leaf.to_bytes());
                items.push(p.control_block.clone().unwrap());
                items
            }
            _ => {
                let keypair = Keypair::from_secret_key(&SECP, &p.keys[0])
                    .tap_tweak(&SECP, p.merkle_root)
                    .to_keypair();
                vec![schnorr_sig(sighash(None), &keypair, ty)]
            }
        };
        if matches!(input.sighash, Sighash::ExplicitDefault) {
            items[0].push(0x00);
        }
        items.extend(annex);
        tx.input[vin].witness = Witness::from_slice(&items);
    }
    Case {
        tx,
        prevouts,
        flags: spec.flags,
    }
}

#[derive(Debug, Clone)]
enum Policy {
    Pk,
    Sha256,
    Hash256,
    Ripemd160,
    Hash160,
    After(u32),
    Older(u16),
    And(Box<Policy>, Box<Policy>),
    Or(u8, Box<Policy>, u8, Box<Policy>),
    Thresh(usize, Vec<Policy>),
}

fn policy() -> impl Strategy<Value = Policy> {
    let leaf = prop_oneof![
        5 => Just(Policy::Pk),
        1 => Just(Policy::Sha256),
        1 => Just(Policy::Hash256),
        1 => Just(Policy::Ripemd160),
        1 => Just(Policy::Hash160),
        1 => (1..=500u32).prop_map(Policy::After),
        1 => (1..=100u16).prop_map(Policy::Older),
    ];
    let inner =
        leaf.prop_recursive(3, 12, 4, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone())
                    .prop_map(|(a, b)| Policy::And(Box::new(a), Box::new(b))),
                (1..=4u8, inner.clone(), 1..=4u8, inner.clone())
                    .prop_map(|(wa, a, wb, b)| Policy::Or(wa, Box::new(a), wb, Box::new(b))),
                vec(inner, 2..=4)
                    .prop_flat_map(|subs| (1..=subs.len(), Just(subs)))
                    .prop_map(|(k, subs)| Policy::Thresh(k, subs)),
            ]
        });
    // Wrapping in a key keeps most policies safe (every spend needs a signature), which the
    // compiler requires.
    prop_oneof![
        2 => inner.clone().prop_map(|p| Policy::And(Box::new(Policy::Pk), Box::new(p))),
        1 => inner.clone().prop_map(|p| Policy::Or(1, Box::new(Policy::Pk), 1, Box::new(Policy::And(Box::new(Policy::Pk), Box::new(p))))),
        1 => inner,
    ]
}

/// The keys and preimages a rendered policy uses, in order of appearance.
#[derive(Default)]
struct Secrets {
    keys: Vec<SecretKey>,
    preimages: Vec<[u8; 32]>,
    max_after: u32,
    max_older: u16,
}

fn render(policy: &Policy, x_only_keys: bool, secrets: &mut Secrets) -> String {
    let preimage = |secrets: &mut Secrets| {
        let p = [0x10 + secrets.preimages.len() as u8; 32];
        secrets.preimages.push(p);
        p
    };
    match policy {
        Policy::Pk => {
            let sk = secret(500 + secrets.keys.len() as u64);
            secrets.keys.push(sk);
            if x_only_keys {
                format!("pk({})", x_only(&sk))
            } else {
                format!("pk({})", public(&sk))
            }
        }
        Policy::Sha256 => format!("sha256({})", sha256::Hash::hash(&preimage(secrets))),
        Policy::Hash256 => format!(
            "hash256({})",
            miniscript::hash256::Hash::hash(&preimage(secrets))
        ),
        Policy::Ripemd160 => format!("ripemd160({})", ripemd160::Hash::hash(&preimage(secrets))),
        Policy::Hash160 => format!("hash160({})", hash160::Hash::hash(&preimage(secrets))),
        Policy::After(n) => {
            secrets.max_after = secrets.max_after.max(*n);
            format!("after({n})")
        }
        Policy::Older(n) => {
            secrets.max_older = secrets.max_older.max(*n);
            format!("older({n})")
        }
        Policy::And(a, b) => format!(
            "and({},{})",
            render(a, x_only_keys, secrets),
            render(b, x_only_keys, secrets)
        ),
        Policy::Or(wa, a, wb, b) => format!(
            "or({wa}@{},{wb}@{})",
            render(a, x_only_keys, secrets),
            render(b, x_only_keys, secrets)
        ),
        Policy::Thresh(k, subs) => {
            let subs = subs
                .iter()
                .map(|p| render(p, x_only_keys, secrets))
                .collect::<Vec<_>>();
            format!("thresh({k},{})", subs.join(","))
        }
    }
}

/// Changes made before signing, so the signatures stay valid and only the policy can fail.
#[derive(Debug, Clone)]
enum PreSign {
    /// The lock time one below what `after` needs, while the satisfier still claims it is met.
    LockTimeBelow,
    /// The sequence one below what `older` needs, likewise.
    SequenceBelow,
    /// The satisfier gives a wrong preimage for the `n`th hash.
    WrongPreimage(usize),
    /// A final sequence, which disables `after`.
    FinalSequence,
    /// Version 1, which disables `older`.
    VersionOne,
}

#[derive(Debug, Clone)]
struct MiniscriptSpec {
    taproot: bool,
    policy: Policy,
    key_path: bool,
    sighash: Sighash,
    amount: u64,
    pre_sign: Vec<PreSign>,
    mutations: Vec<Mutation>,
}

fn miniscript_spec(mutate: bool) -> impl Strategy<Value = MiniscriptSpec> {
    let (pre_sign, mutations) = if mutate {
        (
            vec(
                prop_oneof![
                    Just(PreSign::LockTimeBelow),
                    Just(PreSign::SequenceBelow),
                    (0..4usize).prop_map(PreSign::WrongPreimage),
                    Just(PreSign::FinalSequence),
                    Just(PreSign::VersionOne),
                ],
                0..=2,
            )
            .boxed(),
            vec(mutation(), 0..=3).boxed(),
        )
    } else {
        (Just(vec![]).boxed(), Just(vec![]).boxed())
    };
    (
        any::<bool>(),
        policy(),
        proptest::bool::weighted(0.2),
        prop_oneof![3 => Just(Sighash::All), 1 => Just(Sighash::Default), 1 => sighash()],
        amount(),
        pre_sign,
        mutations,
    )
        .prop_map(
            |(taproot, policy, key_path, sighash, amount, pre_sign, mutations)| MiniscriptSpec {
                taproot,
                policy,
                key_path,
                sighash,
                amount,
                pre_sign,
                mutations,
            },
        )
}

struct Signer<'a> {
    tx: &'a Transaction,
    prevouts: &'a [TxOut],
    secrets: &'a Secrets,
    witness_script: Option<ScriptBuf>,
    merkle_root: Option<TapNodeHash>,
    key_path: bool,
    sighash: Sighash,
    wrong_preimage: Option<usize>,
    lock_time: LockTime,
    sequence: Sequence,
}

impl Signer<'_> {
    fn secret_for(&self, pk: &XOnlyPublicKey) -> Option<&SecretKey> {
        self.secrets.keys.iter().find(|sk| x_only(sk) == *pk)
    }

    fn preimage(&self, matches: impl Fn(&[u8; 32]) -> bool) -> Option<[u8; 32]> {
        let i = self.secrets.preimages.iter().position(matches)?;
        Some(if self.wrong_preimage == Some(i) {
            [0xee; 32]
        } else {
            self.secrets.preimages[i]
        })
    }

    fn older_met(&self, n: relative::LockTime) -> bool {
        relative::LockTime::from_sequence(self.sequence).is_ok_and(|have| n.is_implied_by(have))
    }

    fn after_met(&self, n: LockTime) -> bool {
        n.is_implied_by(self.lock_time)
    }
}

macro_rules! signer_lookups {
    () => {
        fn lookup_sha256(&self, h: &sha256::Hash) -> Option<miniscript::Preimage32> {
            self.preimage(|p| sha256::Hash::hash(p) == *h)
        }

        fn lookup_hash256(&self, h: &miniscript::hash256::Hash) -> Option<miniscript::Preimage32> {
            self.preimage(|p| miniscript::hash256::Hash::hash(p) == *h)
        }

        fn lookup_ripemd160(&self, h: &ripemd160::Hash) -> Option<miniscript::Preimage32> {
            self.preimage(|p| ripemd160::Hash::hash(p) == *h)
        }

        fn lookup_hash160(&self, h: &hash160::Hash) -> Option<miniscript::Preimage32> {
            self.preimage(|p| hash160::Hash::hash(p) == *h)
        }

        fn check_older(&self, n: relative::LockTime) -> bool {
            self.older_met(n)
        }

        fn check_after(&self, n: LockTime) -> bool {
            self.after_met(n)
        }
    };
}

impl Satisfier<PublicKey> for Signer<'_> {
    fn lookup_ecdsa_sig(&self, pk: &PublicKey) -> Option<bitcoin::ecdsa::Signature> {
        let sk = self.secret_for(&pk.inner.x_only_public_key().0)?;
        let ty = self.sighash.ecdsa();
        let hash = SighashCache::new(self.tx)
            .p2wsh_signature_hash(0, self.witness_script.as_ref()?, self.prevouts[0].value, ty)
            .unwrap();
        Some(bitcoin::ecdsa::Signature {
            signature: SECP.sign_ecdsa(&Message::from_digest(hash.to_byte_array()), sk),
            sighash_type: ty,
        })
    }

    signer_lookups!();
}

impl Satisfier<XOnlyPublicKey> for Signer<'_> {
    fn lookup_tap_key_spend_sig(&self, pk: &XOnlyPublicKey) -> Option<bitcoin::taproot::Signature> {
        if !self.key_path {
            return None;
        }
        let sk = self.secret_for(pk)?;
        let ty = self.sighash.taproot();
        let hash = SighashCache::new(self.tx)
            .taproot_key_spend_signature_hash(0, &Prevouts::All(self.prevouts), ty)
            .unwrap();
        let keypair = Keypair::from_secret_key(&SECP, sk)
            .tap_tweak(&SECP, self.merkle_root)
            .to_keypair();
        Some(bitcoin::taproot::Signature {
            signature: SECP
                .sign_schnorr_no_aux_rand(&Message::from_digest(hash.to_byte_array()), &keypair),
            sighash_type: ty,
        })
    }

    fn lookup_tap_leaf_script_sig(
        &self,
        pk: &XOnlyPublicKey,
        leaf_hash: &TapLeafHash,
    ) -> Option<bitcoin::taproot::Signature> {
        let sk = self.secret_for(pk)?;
        let ty = self.sighash.taproot();
        let hash = SighashCache::new(self.tx)
            .taproot_script_spend_signature_hash(0, &Prevouts::All(self.prevouts), *leaf_hash, ty)
            .unwrap();
        Some(bitcoin::taproot::Signature {
            signature: SECP.sign_schnorr_no_aux_rand(
                &Message::from_digest(hash.to_byte_array()),
                &Keypair::from_secret_key(&SECP, sk),
            ),
            sighash_type: ty,
        })
    }

    signer_lookups!();
}

/// The satisfied spend, or `None` if the policy does not compile.
fn build_miniscript_case(spec: &MiniscriptSpec) -> Option<Case> {
    let mut secrets = Secrets::default();
    let rendered = render(&spec.policy, spec.taproot, &mut secrets);
    let lock_time = LockTime::from_consensus(secrets.max_after);
    let sequence = if secrets.max_older > 0 {
        Sequence::from_height(secrets.max_older)
    } else {
        Sequence::ENABLE_LOCKTIME_NO_RBF
    };
    let has = |f: fn(&PreSign) -> bool| spec.pre_sign.iter().any(f);
    let tx_lock_time = if has(|p| matches!(p, PreSign::LockTimeBelow)) && secrets.max_after > 0 {
        secrets.max_after - 1
    } else {
        secrets.max_after
    };
    let tx_sequence = if has(|p| matches!(p, PreSign::FinalSequence)) {
        u32::MAX
    } else if has(|p| matches!(p, PreSign::SequenceBelow)) && secrets.max_older > 0 {
        Sequence::from_height(secrets.max_older - 1).0
    } else {
        sequence.0
    };
    let tx_version = if has(|p| matches!(p, PreSign::VersionOne)) {
        1
    } else {
        2
    };
    let wrong_preimage = spec.pre_sign.iter().find_map(|p| match p {
        PreSign::WrongPreimage(i) => Some(*i),
        _ => None,
    });

    let mut tx = unsigned_tx(tx_version, tx_lock_time, &[tx_sequence], 1);
    let (script_pubkey, satisfied) = if spec.taproot {
        let policy = rendered.parse::<Concrete<XOnlyPublicKey>>().ok()?;
        let descriptor = policy.compile_tr(Some(NUMS.parse().unwrap())).ok()?;
        let Descriptor::Tr(tr) = &descriptor else {
            unreachable!()
        };
        let merkle_root = tr.spend_info().merkle_root();
        let prevouts = [TxOut {
            value: Amount::from_sat(spec.amount),
            script_pubkey: descriptor.script_pubkey(),
        }];
        let signer = Signer {
            tx: &tx,
            prevouts: &prevouts,
            secrets: &secrets,
            witness_script: None,
            merkle_root,
            key_path: spec.key_path,
            sighash: spec.sighash,
            wrong_preimage,
            lock_time,
            sequence,
        };
        let mut txin = tx.input[0].clone();
        descriptor.satisfy(&mut txin, &signer).ok()?;
        (descriptor.script_pubkey(), txin)
    } else {
        let policy = rendered.parse::<Concrete<PublicKey>>().ok()?;
        let ms = policy.compile::<Segwitv0>().ok()?;
        let descriptor = Descriptor::new_wsh(ms).ok()?;
        let prevouts = [TxOut {
            value: Amount::from_sat(spec.amount),
            script_pubkey: descriptor.script_pubkey(),
        }];
        let signer = Signer {
            tx: &tx,
            prevouts: &prevouts,
            secrets: &secrets,
            witness_script: Some(descriptor.explicit_script().unwrap()),
            merkle_root: None,
            key_path: false,
            sighash: spec.sighash,
            wrong_preimage,
            lock_time,
            sequence,
        };
        let mut txin = tx.input[0].clone();
        descriptor.satisfy(&mut txin, &signer).ok()?;
        (descriptor.script_pubkey(), txin)
    };
    tx.input[0] = satisfied;
    Some(Case {
        tx,
        prevouts: vec![TxOut {
            value: Amount::from_sat(spec.amount),
            script_pubkey,
        }],
        flags: Flags::CONSENSUS,
    })
}

fn config() -> ProptestConfig {
    ProptestConfig {
        max_global_rejects: 1 << 20,
        ..ProptestConfig::default()
    }
}

#[test]
fn every_opcode() {
    let placements = [
        Placement::Bare,
        Placement::P2sh,
        Placement::P2wsh,
        Placement::Tapscript { leaf_version: 0xc0 },
    ];
    // A strictly encoded signature that verifies against nothing.
    let der_sig = vec![0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x01];
    let stacks = [
        vec![],
        vec![vec![]],
        vec![vec![1]],
        vec![vec![0xff]],
        vec![vec![1], vec![2]],
        vec![vec![0x81]; 3],
        (1..=6).map(|i| vec![i]).collect(),
        vec![der_sig.clone(), vec![0x02; 33]],
        vec![der_sig.clone(), vec![0x05; 33]],
        // An empty signature, which fails softly.
        vec![vec![], vec![0x02; 32]],
        // CHECKMULTISIG with an empty signature, and with more signatures than keys.
        vec![vec![], vec![], vec![1], vec![0x02; 33], vec![1]],
        vec![vec![], vec![5], vec![0x02; 33], vec![1]],
        // CHECKMULTISIG of nothing, with a dummy that NULLDUMMY forbids.
        vec![vec![1], vec![], vec![]],
    ];
    for code in 0..=255u8 {
        for script in [
            vec![Op::Raw(vec![code])],
            vec![Op::Raw(vec![code]), Op::Num(1)],
        ] {
            for placement in &placements {
                for stack in &stacks {
                    let case = build_script_case(&ScriptSpec {
                        placement: placement.clone(),
                        script: script.clone(),
                        script_sig: vec![],
                        stack: stack.clone(),
                        witness: vec![],
                        annex: None,
                        flags: Flags::CONSENSUS,
                        version: 2,
                        lock_time: 0,
                        sequence: 0,
                        amount: 1000,
                    });
                    if let Err(e) = verdicts(&case) {
                        panic!("opcode {code:#04x}: {e}");
                    }
                }
            }
        }
    }
}

#[test]
fn timelock_edges() {
    let args = [
        -1i64,
        0,
        1,
        0xffff,
        0x1_0000,
        1 << 22,
        (1 << 22) | 1,
        499_999_999,
        500_000_000,
        500_000_001,
        1 << 31,
    ];
    let lock_times = [0u32, 1, 499_999_999, 500_000_000, 500_000_001];
    let sequences = [
        0u32,
        1,
        0xffff,
        1 << 22,
        (1 << 22) | 1,
        1 << 31,
        0xffff_fffe,
        u32::MAX,
    ];
    for code in [OP_CLTV, OP_CSV] {
        for arg in args {
            for lock_time in lock_times {
                for sequence in sequences {
                    for version in [1, 2, -1] {
                        for placement in [Placement::Bare, Placement::P2wsh] {
                            let case = build_script_case(&ScriptSpec {
                                placement,
                                script: vec![
                                    Op::Num(arg),
                                    Op::Code(code),
                                    Op::Code(OP_DROP),
                                    Op::Num(1),
                                ],
                                script_sig: vec![],
                                stack: vec![],
                                witness: vec![],
                                annex: None,
                                flags: Flags::CONSENSUS,
                                version,
                                lock_time,
                                sequence,
                                amount: 1000,
                            });
                            if let Err(e) = verdicts(&case) {
                                panic!("{e}");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn unexpected_witness() {
    let no_witness = Flags {
        witness: false,
        taproot: false,
        ..Flags::CONSENSUS
    };
    for placement in [Placement::Bare, Placement::P2sh] {
        for witness in [vec![], vec![vec![]], vec![vec![1]], vec![vec![], vec![1]]] {
            for flags in [Flags::CONSENSUS, no_witness] {
                let case = build_script_case(&ScriptSpec {
                    placement: placement.clone(),
                    script: vec![Op::Num(1)],
                    script_sig: vec![],
                    stack: vec![],
                    witness: witness.clone(),
                    annex: None,
                    flags,
                    version: 2,
                    lock_time: 0,
                    sequence: 0,
                    amount: 1000,
                });
                if let Err(e) = verdicts(&case) {
                    panic!("{e}");
                }
            }
        }
    }
}

/// The generated limit family under consensus flags, enumerated, since an off-by-one limit fails
/// at a single delta, which 256 random cases can miss.
#[test]
fn every_limit_edge() {
    for limit in LIMITS {
        for delta in LIMIT_DELTAS {
            for placement in LIMIT_PLACEMENTS {
                let spec = LimitSpec {
                    limit: limit.clone(),
                    delta,
                    placement,
                    flags: Flags::CONSENSUS,
                };
                if let Err(e) = verdicts(&build_limit_case(&spec)) {
                    panic!("{e}");
                }
            }
        }
    }
}

/// Mutations compose, so a DER edit must stay well-defined on a signature an earlier one broke.
#[test]
fn der_edits_compose() {
    let mut edits = vec![
        DerEdit::Pad { s: false },
        DerEdit::Pad { s: true },
        DerEdit::Negate { s: false },
        DerEdit::Negate { s: true },
    ];
    for field in 0..6 {
        for value in [0x00, 0x01, 0x02, 0x30, 0x7f, 0xff] {
            edits.push(DerEdit::Header { field, value });
        }
    }
    for kind in [Kind::P2pkh, Kind::P2wpkh] {
        let spec = SignedSpec {
            inputs: vec![InputSpec {
                kind,
                key: 0,
                sighash: Sighash::All,
                amount: 1000,
                sequence: u32::MAX,
                annex: None,
            }],
            one_output: false,
            version: 2,
            lock_time: 0,
            leaf_edit: None,
            mutations: vec![],
            flags: Flags::CONSENSUS,
        };
        let signed = build_signed_case(&spec);
        for first in &edits {
            for second in &edits {
                let mut case = signed.clone();
                for edit in [first, second] {
                    apply(
                        &Mutation::MangleDer {
                            input: 0,
                            edit: *edit,
                        },
                        &mut case,
                    );
                }
                if let Err(e) = verdicts(&case) {
                    panic!("{kind:?} {first:?} then {second:?}: {e}");
                }
            }
        }
    }
}

/// Legacy script codes are rebuilt around each signature check, walking every push encoding.
#[test]
fn script_code_walks() {
    let der_sig = vec![0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x01];
    let mut long_sig = vec![0x30, 0x4d, 0x02, 0x01, 0x01, 0x02, 0x48];
    long_sig.extend(vec![0x01; 0x48]);
    long_sig.push(0x01);
    let checksig = Op::Code(OP_CHECKSIG);
    let scripts = [
        vec![
            Op::PushData(1, vec![1]),
            Op::Code(OP_DROP),
            checksig.clone(),
        ],
        vec![
            Op::PushData(2, vec![1]),
            Op::Code(OP_DROP),
            checksig.clone(),
        ],
        vec![
            Op::PushData(4, vec![1]),
            Op::Code(OP_DROP),
            checksig.clone(),
        ],
        vec![checksig.clone(), Op::Raw(vec![OP_PUSHDATA2.to_u8(), 0x05])],
        vec![
            checksig.clone(),
            Op::Raw(vec![OP_PUSHDATA4.to_u8(), 0x05, 0x00]),
        ],
        vec![checksig.clone(), Op::Raw(vec![OP_PUSHDATA1.to_u8()])],
        vec![checksig.clone(), Op::Code(OP_NOT)],
    ];
    let no_dersig = Flags {
        dersig: false,
        ..Flags::CONSENSUS
    };
    for script in &scripts {
        for sig in [&der_sig, &long_sig] {
            for flags in [Flags::CONSENSUS, no_dersig] {
                for placement in [Placement::Bare, Placement::P2wsh] {
                    let case = build_script_case(&ScriptSpec {
                        placement,
                        script: script.clone(),
                        script_sig: vec![],
                        stack: vec![sig.clone(), public(&secret(1)).to_bytes()],
                        witness: vec![],
                        annex: None,
                        flags,
                        version: 2,
                        lock_time: 0,
                        sequence: 0,
                        amount: 1000,
                    });
                    if let Err(e) = verdicts(&case) {
                        panic!("{e}");
                    }
                }
            }
        }
    }
}

/// Control blocks either side of the 128-hash depth limit, and a byte off a whole number of hashes.
/// Each output key commits to the leaf through every sibling, so only the size rule can fail it.
#[test]
fn control_block_sizes() {
    let leaf = ScriptBuf::from_bytes(encode(&[Op::Num(1)]));
    let internal = x_only(&secret(0));
    let mut decided = Vec::new();
    for depth in [0usize, 1, 127, 128, 129] {
        let siblings = (0..depth)
            .map(|i| TapNodeHash::from_byte_array([i as u8 + 1; 32]))
            .collect::<Vec<_>>();
        let mut node = TapNodeHash::from(TapLeafHash::from_script(&leaf, LeafVersion::TapScript));
        for sibling in &siblings {
            node = TapNodeHash::from_node_hashes(node, *sibling);
        }
        let (output, parity) = internal.tap_tweak(&SECP, Some(node));
        let mut control_block = vec![LeafVersion::TapScript.to_consensus() | parity.to_u8()];
        control_block.extend(internal.serialize());
        for sibling in &siblings {
            control_block.extend(sibling.to_byte_array());
        }
        for extra in [-1i64, 0, 1] {
            let mut control_block = control_block.clone();
            match extra {
                -1 => {
                    control_block.pop();
                }
                1 => control_block.push(0),
                _ => {}
            }
            let mut tx = unsigned_tx(2, 0, &[u32::MAX], 1);
            tx.input[0].witness = Witness::from_slice(&[leaf.to_bytes(), control_block]);
            let case = Case {
                tx,
                prevouts: vec![TxOut {
                    value: Amount::from_sat(1000),
                    script_pubkey: ScriptBuf::new_p2tr_tweaked(output),
                }],
                flags: Flags::CONSENSUS,
            };
            match verdicts(&case) {
                Ok(v) => decided.push(((depth, extra), v[0])),
                Err(e) => panic!("depth {depth}, {extra:+} bytes: {e}"),
            }
        }
    }
    for ((depth, extra), valid) in decided {
        assert_eq!(
            valid,
            extra == 0 && depth <= 128,
            "depth {depth}, {extra:+} bytes"
        );
    }
}

/// The budget is 50 plus the witness size, and each signature costs 50, so a long enough repeat
/// leaf runs out. Every length is tried, so both sides of the boundary are.
#[test]
fn sigop_budget_crossover() {
    let mut outcomes = Vec::new();
    for n in 1..=24 {
        let case = build_signed_case(&SignedSpec {
            inputs: vec![InputSpec {
                kind: Kind::P2trScript(Leaf::Repeat(n)),
                key: 0,
                sighash: Sighash::Default,
                amount: 1000,
                sequence: u32::MAX,
                annex: None,
            }],
            one_output: false,
            version: 2,
            lock_time: 0,
            leaf_edit: None,
            mutations: vec![],
            flags: Flags::CONSENSUS,
        });
        match verdicts(&case) {
            Ok(v) => outcomes.push(v[0]),
            Err(e) => panic!("{n} signatures: {e}"),
        }
    }
    let first_invalid = outcomes
        .iter()
        .position(|v| !v)
        .expect("the budget is exceeded");
    assert!(first_invalid > 1, "{outcomes:?}");
    assert!(outcomes[first_invalid..].iter().all(|v| !v), "{outcomes:?}");
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn random_scripts(spec in script_spec(random_shape())) {
        verdicts(&build_script_case(&spec))?;
    }

    #[test]
    fn shaped_scripts(spec in script_spec(shaped())) {
        verdicts(&build_script_case(&spec))?;
    }

    #[test]
    fn scripts_at_interpreter_limits(spec in limit_spec()) {
        verdicts(&build_limit_case(&spec))?;
    }

    #[test]
    fn mutated_signed_spends(spec in signed_spec()) {
        let mut clean = spec.clone();
        clean.leaf_edit = None;
        clean.mutations.clear();
        clean.flags = Flags::CONSENSUS;
        for input in &mut clean.inputs {
            if matches!(input.sighash, Sighash::ExplicitDefault) {
                input.sighash = Sighash::Default;
            }
        }
        let clean = build_signed_case(&clean);
        // A long enough repeat leaf exceeds the sigop budget, and is meant to.
        if spec.inputs.iter().any(|i| matches!(i.kind, Kind::P2trScript(Leaf::Repeat(_)))) {
            verdicts(&clean)?;
        } else {
            assert_all_valid(&clean)?;
        }

        let mut case = build_signed_case(&spec);
        for mutation in &spec.mutations {
            apply(mutation, &mut case);
        }
        verdicts(&case)?;
    }

    #[test]
    fn miniscript_spends(spec in miniscript_spec(false)) {
        let case = build_miniscript_case(&spec);
        prop_assume!(case.is_some());
        assert_all_valid(&case.unwrap())?;
    }

    #[test]
    fn mutated_miniscript_spends(spec in miniscript_spec(true)) {
        let mut clean = spec.clone();
        clean.pre_sign.clear();
        clean.mutations.clear();
        let clean = build_miniscript_case(&clean);
        prop_assume!(clean.is_some());
        assert_all_valid(&clean.unwrap())?;

        let mut case = build_miniscript_case(&spec).unwrap();
        for mutation in &spec.mutations {
            apply(mutation, &mut case);
        }
        verdicts(&case)?;
    }
}
