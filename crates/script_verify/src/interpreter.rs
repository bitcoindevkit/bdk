// Adapted from rbitcoin `crates/rbitcoin-consensus/src/script/interpreter.rs` at
// 49210ea82130474a4df366e253d1346a16369fdb (MIT OR Apache-2.0, rbitcoin contributors).
//
// Consensus rules only. rbitcoin's policy flags (MINIMALDATA, NULLFAIL, LOW_S, STRICTENC,
// WITNESS_PUBKEYTYPE, CONST_SCRIPTCODE, v0 MINIMALIF) are removed.

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use bitcoin::hashes::{ripemd160, sha256d, Hash};
use bitcoin::script::{read_scriptbool, Instruction, Script, ScriptBuf};
use bitcoin::secp256k1::{schnorr, PublicKey};
use bitcoin::sighash::TapSighashType;
use bitcoin::{Amount, PubkeyHash, Sequence, TapLeafHash, TxOut, Witness, XOnlyPublicKey};

use crate::sighash::Sighasher;
use crate::{crypto, scriptnum, Fail, Flags};

const MAX_STACK_SIZE: usize = 1000;
const MAX_SCRIPT_ELEMENT_SIZE: usize = 520;
const MAX_SCRIPT_SIZE_LEGACY: usize = 10_000;
const MAX_OPS_LEGACY: usize = 201;
const MAX_PUBKEYS_PER_MULTISIG: i64 = 20;
const TAPSCRIPT_VALIDATION_WEIGHT_OFFSET: i64 = 50;
const TAPSCRIPT_VALIDATION_WEIGHT_PER_SIGOP: i64 = 50;

const OP_PUSHBYTES_75: u8 = 0x4b;
const OP_PUSHDATA1: u8 = 0x4c;
const OP_PUSHDATA2: u8 = 0x4d;
const OP_PUSHDATA4: u8 = 0x4e;
const OP_VERIF: u8 = 0x65;
const OP_VERNOTIF: u8 = 0x66;
const OP_CODESEPARATOR: u8 = 0xab;

type Stack = Vec<Vec<u8>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SigVersion {
    Base,
    WitnessV0,
    TapScript,
}

/// The facts a BIP342 signature hash needs beyond the transaction.
struct TapscriptData<'w> {
    leaf_hash: TapLeafHash,
    annex: Option<&'w [u8]>,
}

struct EvalContext<'a, 'c> {
    sighasher: &'c Sighasher<'a>,
    verifier: &'c crypto::Verifier,
    flags: Flags,
    input_index: usize,
    amount: Amount,
    script_code: &'c Script,
    sig_version: SigVersion,
    /// BIP342: instruction index (not byte offset) of the last executed OP_CODESEPARATOR.
    codeseparator_pos: Cell<u32>,
    /// Base and witness v0: byte offset just past the last executed OP_CODESEPARATOR, which starts
    /// the scriptCode that signatures commit to.
    codeseparator_script_off: Cell<Option<usize>>,
    validation_weight_left: Cell<i64>,
    tapscript: Option<TapscriptData<'c>>,
}

impl<'a, 'c> EvalContext<'a, 'c> {
    fn new(
        sighasher: &'c Sighasher<'a>,
        verifier: &'c crypto::Verifier,
        flags: Flags,
        input_index: usize,
        amount: Amount,
        script_code: &'c Script,
        sig_version: SigVersion,
    ) -> Self {
        let validation_weight = if sig_version == SigVersion::TapScript {
            let witness = &sighasher.tx().input[input_index].witness;
            TAPSCRIPT_VALIDATION_WEIGHT_OFFSET
                + bitcoin::consensus::encode::serialize(witness).len() as i64
        } else {
            0
        };
        Self {
            sighasher,
            verifier,
            flags,
            input_index,
            amount,
            script_code,
            sig_version,
            codeseparator_pos: Cell::new(0xFFFF_FFFF),
            codeseparator_script_off: Cell::new(None),
            validation_weight_left: Cell::new(validation_weight),
            tapscript: None,
        }
    }

    fn with_tapscript(mut self, data: TapscriptData<'c>) -> Self {
        self.tapscript = Some(data);
        self
    }
}

fn is_disabled_legacy(code: u8) -> bool {
    // OP_CAT, OP_SUBSTR, OP_LEFT, OP_RIGHT, OP_INVERT, OP_AND, OP_OR, OP_XOR, OP_2MUL, OP_2DIV,
    // OP_MUL, OP_DIV, OP_MOD, OP_LSHIFT, OP_RSHIFT
    matches!(
        code,
        0x7e | 0x7f | 0x80 | 0x81 | 0x83 | 0x84 | 0x85 | 0x86 | 0x8d | 0x8e | 0x95..=0x99
    )
}

fn is_op_success(code: u8) -> bool {
    matches!(
        code,
        80 | 98 | 126..=129 | 131..=134 | 137 | 138 | 141 | 142 | 149..=153 | 187..=254
    )
}

/// BIP342: an OP_SUCCESSx anywhere makes the script valid before any limit applies, so this is a
/// separate walk from evaluation. A truncated push stops the walk; evaluation then fails on it.
fn tapscript_has_op_success(script: &Script) -> bool {
    let bytes = script.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let op = bytes[i];
        if (1..=OP_PUSHBYTES_75).contains(&op) {
            i = i.saturating_add(1).saturating_add(op as usize);
            continue;
        }
        match op {
            OP_PUSHDATA1 => {
                if i + 1 >= bytes.len() {
                    break;
                }
                i = i.saturating_add(2).saturating_add(bytes[i + 1] as usize);
            }
            OP_PUSHDATA2 => {
                if i + 2 >= bytes.len() {
                    break;
                }
                let n = u16::from_le_bytes([bytes[i + 1], bytes[i + 2]]) as usize;
                i = i.saturating_add(3).saturating_add(n);
            }
            OP_PUSHDATA4 => {
                if i + 4 >= bytes.len() {
                    break;
                }
                let n = u32::from_le_bytes([bytes[i + 1], bytes[i + 2], bytes[i + 3], bytes[i + 4]])
                    as usize;
                i = i.saturating_add(5).saturating_add(n);
            }
            _ => {
                if is_op_success(op) {
                    return true;
                }
                i += 1;
            }
        }
    }
    false
}

/// Witness and tapscript: exactly one true element (implicit CLEANSTACK).
fn require_clean_true(stack: &[Vec<u8>]) -> Result<(), Fail> {
    if stack.len() != 1 {
        return Err(Fail::Script("CLEANSTACK"));
    }
    if !read_scriptbool(&stack[0]) {
        return Err(Fail::Script("EVAL_FALSE"));
    }
    Ok(())
}

/// Legacy: a non-empty stack with a true top. BIP62 CLEANSTACK never became consensus.
fn require_true_top(stack: &[Vec<u8>]) -> Result<(), Fail> {
    match stack.last() {
        Some(top) if read_scriptbool(top) => Ok(()),
        _ => Err(Fail::Script("EVAL_FALSE")),
    }
}

/// Core's `ConditionStack`: the depth plus the index of the first false entry, so that checking
/// whether we are executing is O(1). Tapscript has no opcode limit, so scanning a `Vec<bool>` per
/// opcode would be quadratic in nesting depth.
#[derive(Default)]
struct ConditionStack {
    size: usize,
    first_false: Option<usize>,
}

impl ConditionStack {
    fn is_empty(&self) -> bool {
        self.size == 0
    }

    fn all_true(&self) -> bool {
        self.first_false.is_none()
    }

    fn push(&mut self, value: bool) {
        if self.first_false.is_none() && !value {
            self.first_false = Some(self.size);
        }
        self.size += 1;
    }

    fn pop(&mut self) {
        self.size -= 1;
        if self.first_false == Some(self.size) {
            self.first_false = None;
        }
    }

    fn toggle_top(&mut self) {
        let top = self.size - 1;
        match self.first_false {
            None => self.first_false = Some(top),
            Some(pos) if pos == top => self.first_false = None,
            Some(_) => {}
        }
    }
}

fn eval_script(script: &Script, stack: &mut Stack, ctx: &EvalContext<'_, '_>) -> Result<(), Fail> {
    let tapscript = ctx.sig_version == SigVersion::TapScript;
    if !tapscript && script.len() > MAX_SCRIPT_SIZE_LEGACY {
        return Err(Fail::Script("SCRIPT_SIZE"));
    }

    let mut altstack: Stack = Vec::new();
    let mut cond_stack = ConditionStack::default();
    let mut op_count = 0usize;
    let mut opcode_pos: u32 = 0;
    for item in script.instruction_indices() {
        let (byte_index, ins) = item.map_err(|_| Fail::Script("BAD_OPCODE"))?;
        let this_pos = opcode_pos;
        opcode_pos = opcode_pos.saturating_add(1);
        let executing = cond_stack.all_true();

        let code = match ins {
            Instruction::PushBytes(b) => {
                // Core enforces the element size even in unexecuted branches.
                if b.len() > MAX_SCRIPT_ELEMENT_SIZE {
                    return Err(Fail::Script("PUSH_SIZE"));
                }
                if executing {
                    push(stack, altstack.len(), b.as_bytes().to_vec())?;
                }
                continue;
            }
            Instruction::Op(op) => op.to_u8(),
        };

        // Opcodes above OP_16 count toward the legacy limit even when skipped.
        if !tapscript && code > 0x60 {
            op_count += 1;
            if op_count > MAX_OPS_LEGACY {
                return Err(Fail::Script("OP_COUNT"));
            }
        }

        // Flow control runs even when skipped: it is structure, not value.
        match code {
            0x63 | 0x64 => {
                let mut cond = false;
                if executing {
                    let v = pop(stack)?;
                    if tapscript && !(v.is_empty() || v == [0x01]) {
                        return Err(Fail::Script("TAPSCRIPT_MINIMALIF"));
                    }
                    cond = read_scriptbool(&v) == (code == 0x63);
                }
                cond_stack.push(executing && cond);
                continue;
            }
            0x67 => {
                if cond_stack.is_empty() {
                    return Err(Fail::Script("UNBALANCED_CONDITIONAL"));
                }
                cond_stack.toggle_top();
                continue;
            }
            0x68 => {
                if cond_stack.is_empty() {
                    return Err(Fail::Script("UNBALANCED_CONDITIONAL"));
                }
                cond_stack.pop();
                continue;
            }
            _ => {}
        }

        // Core fails these even in unexecuted branches.
        if code == OP_VERIF || code == OP_VERNOTIF {
            return Err(Fail::Script("BAD_OPCODE"));
        }
        if !tapscript && is_disabled_legacy(code) {
            return Err(Fail::Script("DISABLED_OPCODE"));
        }

        if !executing {
            continue;
        }

        let alt = altstack.len();
        match code {
            0x4f => push(stack, alt, vec![0x81])?,
            n @ 0x51..=0x60 => push(stack, alt, vec![n - 0x50])?,
            0x61 => {}
            0x69 => {
                if !read_scriptbool(&pop(stack)?) {
                    return Err(Fail::Script("VERIFY"));
                }
            }
            0x6a => return Err(Fail::Script("OP_RETURN")),
            0x6b => {
                let v = pop(stack)?;
                altstack.push(v);
            }
            0x6c => {
                let v = altstack
                    .pop()
                    .ok_or(Fail::Script("INVALID_ALTSTACK_OPERATION"))?;
                push(stack, altstack.len(), v)?;
            }
            0x6d => {
                pop(stack)?;
                pop(stack)?;
            }
            0x6e => {
                require_n(stack, 2)?;
                let a = stack[stack.len() - 2].clone();
                let b = stack[stack.len() - 1].clone();
                push(stack, alt, a)?;
                push(stack, alt, b)?;
            }
            0x6f => {
                require_n(stack, 3)?;
                let a = stack[stack.len() - 3].clone();
                let b = stack[stack.len() - 2].clone();
                let c = stack[stack.len() - 1].clone();
                push(stack, alt, a)?;
                push(stack, alt, b)?;
                push(stack, alt, c)?;
            }
            0x70 => {
                require_n(stack, 4)?;
                let a = stack[stack.len() - 4].clone();
                let b = stack[stack.len() - 3].clone();
                push(stack, alt, a)?;
                push(stack, alt, b)?;
            }
            0x71 => {
                require_n(stack, 6)?;
                let n = stack.len();
                let x1 = stack[n - 6].clone();
                let x2 = stack[n - 5].clone();
                for i in 0..4 {
                    stack[n - 6 + i] = stack[n - 4 + i].clone();
                }
                stack[n - 2] = x1;
                stack[n - 1] = x2;
            }
            0x72 => {
                require_n(stack, 4)?;
                let n = stack.len();
                stack.swap(n - 4, n - 2);
                stack.swap(n - 3, n - 1);
            }
            0x73 => {
                require_n(stack, 1)?;
                let top = stack[stack.len() - 1].clone();
                if read_scriptbool(&top) {
                    push(stack, alt, top)?;
                }
            }
            0x74 => {
                let depth = stack.len() as i64;
                push(stack, alt, scriptnum::encode(depth))?;
            }
            0x75 => {
                pop(stack)?;
            }
            0x76 => {
                require_n(stack, 1)?;
                let v = stack[stack.len() - 1].clone();
                push(stack, alt, v)?;
            }
            0x77 => {
                require_n(stack, 2)?;
                let top = pop(stack)?;
                pop(stack)?;
                push(stack, alt, top)?;
            }
            0x78 => {
                require_n(stack, 2)?;
                let v = stack[stack.len() - 2].clone();
                push(stack, alt, v)?;
            }
            0x79 | 0x7a => {
                let n = scriptnum::decode(&pop(stack)?)?;
                if n < 0 || n as usize >= stack.len() {
                    return Err(Fail::Script("INVALID_STACK_OPERATION"));
                }
                let idx = stack.len() - 1 - n as usize;
                let v = if code == 0x79 {
                    stack[idx].clone()
                } else {
                    stack.remove(idx)
                };
                push(stack, altstack.len(), v)?;
            }
            0x7b => {
                require_n(stack, 3)?;
                let n = stack.len();
                stack.swap(n - 3, n - 2);
                stack.swap(n - 2, n - 1);
            }
            0x7c => {
                require_n(stack, 2)?;
                let n = stack.len();
                stack.swap(n - 1, n - 2);
            }
            0x7d => {
                require_n(stack, 2)?;
                let v = stack[stack.len() - 1].clone();
                push(stack, alt, v)?;
                let n = stack.len();
                stack.swap(n - 3, n - 2);
            }
            0x82 => {
                require_n(stack, 1)?;
                let size = stack[stack.len() - 1].len() as i64;
                push(stack, alt, scriptnum::encode(size))?;
            }
            0x87 | 0x88 => {
                let a = pop(stack)?;
                let b = pop(stack)?;
                if code == 0x87 {
                    push(stack, alt, bool_encode(a == b))?;
                } else if a != b {
                    return Err(Fail::Script("EQUALVERIFY"));
                }
            }
            0x8b | 0x8c | 0x8f | 0x90 | 0x91 | 0x92 => {
                let v = scriptnum::decode(&pop(stack)?)?;
                let out = match code {
                    0x8b => scriptnum::encode(v + 1),
                    0x8c => scriptnum::encode(v - 1),
                    0x8f => scriptnum::encode(-v),
                    0x90 => scriptnum::encode(v.abs()),
                    0x91 => bool_encode(v == 0),
                    _ => bool_encode(v != 0),
                };
                push(stack, alt, out)?;
            }
            0x93..=0x94 | 0x9a..=0xa4 => {
                let b = scriptnum::decode(&pop(stack)?)?;
                let a = scriptnum::decode(&pop(stack)?)?;
                let out = match code {
                    0x93 => scriptnum::encode(a + b),
                    0x94 => scriptnum::encode(a - b),
                    0x9a => bool_encode(a != 0 && b != 0),
                    0x9b => bool_encode(a != 0 || b != 0),
                    0x9c => bool_encode(a == b),
                    0x9d => {
                        if a != b {
                            return Err(Fail::Script("NUMEQUALVERIFY"));
                        }
                        continue;
                    }
                    0x9e => bool_encode(a != b),
                    0x9f => bool_encode(a < b),
                    0xa0 => bool_encode(a > b),
                    0xa1 => bool_encode(a <= b),
                    0xa2 => bool_encode(a >= b),
                    0xa3 => scriptnum::encode(a.min(b)),
                    _ => scriptnum::encode(a.max(b)),
                };
                push(stack, alt, out)?;
            }
            0xa5 => {
                let max = scriptnum::decode(&pop(stack)?)?;
                let min = scriptnum::decode(&pop(stack)?)?;
                let x = scriptnum::decode(&pop(stack)?)?;
                push(stack, alt, bool_encode(x >= min && x < max))?;
            }
            0xa6 => {
                let v = pop(stack)?;
                push(
                    stack,
                    alt,
                    ripemd160::Hash::hash(&v).to_byte_array().to_vec(),
                )?;
            }
            0xa7 => {
                let v = pop(stack)?;
                push(stack, alt, crypto::sha1(&v).to_vec())?;
            }
            0xa8 => {
                let v = pop(stack)?;
                push(stack, alt, crypto::sha256(&v).to_vec())?;
            }
            0xa9 => {
                let v = pop(stack)?;
                push(stack, alt, crypto::hash160(&v).to_vec())?;
            }
            0xaa => {
                let v = pop(stack)?;
                push(stack, alt, sha256d::Hash::hash(&v).to_byte_array().to_vec())?;
            }
            OP_CODESEPARATOR => {
                ctx.codeseparator_pos.set(this_pos);
                ctx.codeseparator_script_off
                    .set(Some(byte_index.saturating_add(1)));
            }
            0xac | 0xad => op_checksig(stack, alt, ctx, code == 0xad)?,
            0xae | 0xaf => {
                if tapscript {
                    return Err(Fail::Script("TAPSCRIPT_CHECKMULTISIG"));
                }
                op_checkmultisig(stack, alt, ctx, code == 0xaf, &mut op_count)?;
            }
            0xba if tapscript => op_checksigadd(stack, alt, ctx)?,
            0xb1 => {
                if !ctx.flags.checklocktimeverify {
                    continue;
                }
                require_n(stack, 1)?;
                let locktime = scriptnum::decode_width(&stack[stack.len() - 1], 5)?;
                check_locktime(ctx, locktime)?;
            }
            0xb2 => {
                if !ctx.flags.checksequenceverify {
                    continue;
                }
                require_n(stack, 1)?;
                let csv = scriptnum::decode_width(&stack[stack.len() - 1], 5)?;
                if csv < 0 {
                    return Err(Fail::Script("NEGATIVE_LOCKTIME"));
                }
                // The disable flag makes CSV a NOP, checked before the version gate.
                if csv as u32 & (1 << 31) != 0 {
                    continue;
                }
                check_sequence(ctx, csv as u32)?;
            }
            0xb0 | 0xb3..=0xb9 => {}
            _ => return Err(Fail::Script("BAD_OPCODE")),
        }
    }

    if !cond_stack.is_empty() {
        return Err(Fail::Script("UNBALANCED_CONDITIONAL"));
    }
    Ok(())
}

/// One input being verified, and the facts every script it runs shares.
struct Spend<'a, 'c> {
    sighasher: &'c Sighasher<'a>,
    verifier: &'c crypto::Verifier,
    input_index: usize,
    prevout: &'c TxOut,
    flags: Flags,
}

impl<'a, 'c> Spend<'a, 'c> {
    fn witness(&self) -> &'a Witness {
        &self.sighasher.tx().input[self.input_index].witness
    }

    fn ctx(&self, script_code: &'c Script, sig_version: SigVersion) -> EvalContext<'a, 'c> {
        EvalContext::new(
            self.sighasher,
            self.verifier,
            self.flags,
            self.input_index,
            self.prevout.value,
            script_code,
            sig_version,
        )
    }
}

/// Bitcoin Core's `VerifyScript`: runs input `input_index`'s scriptSig, the scriptPubKey of
/// `prevout` and its witness under `flags`.
pub fn verify_script(
    sighasher: &Sighasher<'_>,
    input_index: usize,
    prevout: &TxOut,
    flags: Flags,
) -> Result<(), Fail> {
    let verifier = crypto::Verifier::new();
    let spend = Spend {
        sighasher,
        verifier: &verifier,
        input_index,
        prevout,
        flags,
    };
    let txin = &sighasher.tx().input[input_index];
    let script_sig = txin.script_sig.as_script();
    let spk = prevout.script_pubkey.as_script();

    let mut stack = Stack::new();
    eval_script(
        script_sig,
        &mut stack,
        &spend.ctx(script_sig, SigVersion::Base),
    )?;
    let p2sh_stack = (flags.p2sh && spk.is_p2sh()).then(|| stack.clone());
    eval_script(spk, &mut stack, &spend.ctx(spk, SigVersion::Base))?;
    require_true_top(&stack)?;

    let mut had_witness = false;
    if flags.witness {
        if let Some((version, program)) = witness_program(spk) {
            had_witness = true;
            if !script_sig.is_empty() {
                return Err(Fail::Script("WITNESS_MALLEATED"));
            }
            verify_witness_program(&spend, version, program, false)?;
        }
    }

    if let Some(mut stack) = p2sh_stack {
        if !script_sig.is_push_only() {
            return Err(Fail::Script("SIG_PUSHONLY"));
        }
        let redeem = stack
            .pop()
            .expect("the scriptPubKey's hash check passed so the stack was not empty");
        let redeem = Script::from_bytes(&redeem);
        eval_script(redeem, &mut stack, &spend.ctx(redeem, SigVersion::Base))?;
        require_true_top(&stack)?;
        if flags.witness {
            if let Some((version, program)) = witness_program(redeem) {
                had_witness = true;
                if Some(script_sig.as_bytes()) != single_push(redeem.as_bytes()).as_deref() {
                    return Err(Fail::Script("WITNESS_MALLEATED_P2SH"));
                }
                verify_witness_program(&spend, version, program, true)?;
            }
        }
    }

    if flags.witness && !had_witness && !txin.witness.is_empty() {
        return Err(Fail::Script("WITNESS_UNEXPECTED"));
    }
    Ok(())
}

fn witness_program(script: &Script) -> Option<(u8, &[u8])> {
    let version = script.witness_version()?;
    Some((version.to_num(), &script.as_bytes()[2..]))
}

/// Bitcoin Core's `VerifyWitnessProgram`.
fn verify_witness_program(
    spend: &Spend<'_, '_>,
    version: u8,
    program: &[u8],
    is_p2sh: bool,
) -> Result<(), Fail> {
    let mut stack: Stack = spend.witness().iter().map(<[u8]>::to_vec).collect();
    match (version, program.len()) {
        (0, 32) => {
            let script = stack
                .pop()
                .ok_or(Fail::Script("WITNESS_PROGRAM_WITNESS_EMPTY"))?;
            if crypto::sha256(&script) != program {
                return Err(Fail::Script("WITNESS_PROGRAM_MISMATCH"));
            }
            let script = Script::from_bytes(&script);
            execute_witness_script(stack, script, &spend.ctx(script, SigVersion::WitnessV0))
        }
        (0, 20) => {
            if stack.len() != 2 {
                return Err(Fail::Script("WITNESS_PROGRAM_MISMATCH"));
            }
            let hash = PubkeyHash::from_slice(program).expect("20 bytes");
            let script = ScriptBuf::new_p2pkh(&hash);
            execute_witness_script(stack, &script, &spend.ctx(&script, SigVersion::WitnessV0))
        }
        (0, _) => Err(Fail::Script("WITNESS_PROGRAM_WRONG_LENGTH")),
        (1, 32) if !is_p2sh && spend.flags.taproot => verify_taproot(spend, program, stack),
        // Unknown witness versions and lengths, including pay-to-anchor, are reserved for
        // upgrades and pass.
        _ => Ok(()),
    }
}

fn verify_taproot(spend: &Spend<'_, '_>, output_key: &[u8], mut stack: Stack) -> Result<(), Fail> {
    let witness = spend.witness();
    if stack.is_empty() {
        return Err(Fail::Script("WITNESS_PROGRAM_WITNESS_EMPTY"));
    }
    let annex = match witness.last() {
        Some(last) if witness.len() >= 2 && last.first() == Some(&0x50) => {
            stack.pop();
            Some(last)
        }
        _ => None,
    };
    if stack.len() == 1 {
        return check_schnorr(
            spend.sighasher,
            spend.verifier,
            spend.input_index,
            &stack[0],
            output_key,
            annex,
            None,
        );
    }
    let control = stack.pop().expect("at least two elements");
    stack.pop().expect("at least two elements");
    // Borrow the script from the witness, not the stack, so it outlives the evaluation context.
    let script = Script::from_bytes(
        witness
            .nth(stack.len())
            .expect("the script is the element after the remaining stack"),
    );
    let (leaf_version, leaf_hash) =
        verify_taproot_commitment(spend.verifier, &control, output_key, script)?;
    // Unknown leaf versions are reserved for upgrades and pass without running the script.
    if leaf_version != 0xc0 {
        return Ok(());
    }
    let ctx = spend
        .ctx(script, SigVersion::TapScript)
        .with_tapscript(TapscriptData { leaf_hash, annex });
    execute_witness_script(stack, script, &ctx)
}

/// Checks the BIP341 commitment of a script path spend. Returns the leaf version and leaf hash.
///
/// Adapted from rbitcoin `p2tr.rs` (see the crate README). The leaf version byte is used raw
/// because rust-bitcoin's `LeafVersion` rejects 0x50, which consensus allows as a future version.
fn verify_taproot_commitment(
    verifier: &crypto::Verifier,
    control: &[u8],
    output_key: &[u8],
    script: &Script,
) -> Result<(u8, TapLeafHash), Fail> {
    use bitcoin::consensus::Encodable;
    use bitcoin::hashes::HashEngine;
    use bitcoin::secp256k1::Parity;
    use bitcoin::taproot::TapNodeHash;

    const BASE: usize = 33;
    const NODE: usize = 32;
    const MAX_NODES: usize = 128;
    let extra = control.len().checked_sub(BASE);
    let nodes = match extra {
        Some(extra) if extra % NODE == 0 && extra / NODE <= MAX_NODES => extra / NODE,
        _ => return Err(Fail::Script("TAPROOT_WRONG_CONTROL_SIZE")),
    };
    let leaf_version = control[0] & 0xfe;
    let parity = if control[0] & 1 == 0 {
        Parity::Even
    } else {
        Parity::Odd
    };
    let mismatch = || Fail::Script("WITNESS_PROGRAM_MISMATCH");
    let internal = XOnlyPublicKey::from_slice(&control[1..BASE]).map_err(|_| mismatch())?;
    let output_key = XOnlyPublicKey::from_slice(output_key).map_err(|_| mismatch())?;

    let mut eng = TapLeafHash::engine();
    eng.input(&[leaf_version]);
    script
        .consensus_encode(&mut eng)
        .expect("hash engines do not error");
    let leaf_hash = TapLeafHash::from_engine(eng);
    let mut node = TapNodeHash::from_byte_array(leaf_hash.to_byte_array());
    for sibling in control[BASE..].chunks_exact(NODE).take(nodes) {
        let sibling = TapNodeHash::from_byte_array(sibling.try_into().expect("32 bytes"));
        node = TapNodeHash::from_node_hashes(node, sibling);
    }
    if !verifier.verify_taproot_tweak(&internal, node, &output_key, parity) {
        return Err(mismatch());
    }
    Ok((leaf_version, leaf_hash))
}

/// Bitcoin Core's `ExecuteWitnessScript`.
fn execute_witness_script(
    mut stack: Stack,
    script: &Script,
    ctx: &EvalContext<'_, '_>,
) -> Result<(), Fail> {
    if ctx.sig_version == SigVersion::TapScript {
        if tapscript_has_op_success(script) {
            return Ok(());
        }
        if stack.len() > MAX_STACK_SIZE {
            return Err(Fail::Script("STACK_SIZE"));
        }
    }
    if stack
        .iter()
        .any(|item| item.len() > MAX_SCRIPT_ELEMENT_SIZE)
    {
        return Err(Fail::Script("PUSH_SIZE"));
    }
    eval_script(script, &mut stack, ctx)?;
    require_clean_true(&stack)
}

fn check_locktime(ctx: &EvalContext<'_, '_>, locktime: i64) -> Result<(), Fail> {
    const THRESHOLD: i64 = 500_000_000;
    if locktime < 0 {
        return Err(Fail::Script("NEGATIVE_LOCKTIME"));
    }
    let tx = ctx.sighasher.tx();
    let tx_lock = tx.lock_time.to_consensus_u32() as i64;
    if (locktime >= THRESHOLD) != (tx_lock >= THRESHOLD) || locktime > tx_lock {
        return Err(Fail::Script("UNSATISFIED_LOCKTIME"));
    }
    if tx.input[ctx.input_index].sequence.is_final() {
        return Err(Fail::Script("UNSATISFIED_LOCKTIME"));
    }
    Ok(())
}

fn check_sequence(ctx: &EvalContext<'_, '_>, csv: u32) -> Result<(), Fail> {
    const DISABLE: u32 = 1 << 31;
    const TYPE: u32 = 1 << 22;
    const MASK: u32 = 0x0000_ffff | TYPE;
    let tx = ctx.sighasher.tx();
    // Core compares the version unsigned.
    if (tx.version.0 as u32) < 2 {
        return Err(Fail::Script("UNSATISFIED_LOCKTIME"));
    }
    let seq = Sequence::to_consensus_u32(tx.input[ctx.input_index].sequence);
    if seq & DISABLE != 0 {
        return Err(Fail::Script("UNSATISFIED_LOCKTIME"));
    }
    let (seq, csv) = (seq & MASK, csv & MASK);
    if (seq ^ csv) & TYPE != 0 || (csv & 0xffff) > (seq & 0xffff) {
        return Err(Fail::Script("UNSATISFIED_LOCKTIME"));
    }
    Ok(())
}

fn op_checksig(
    stack: &mut Stack,
    alt_len: usize,
    ctx: &EvalContext<'_, '_>,
    verify: bool,
) -> Result<(), Fail> {
    let pubkey = pop(stack)?;
    let sig = pop(stack)?;
    if ctx.sig_version == SigVersion::TapScript {
        let success = tapscript_check_sig(&sig, &pubkey, ctx)?;
        if verify && !success {
            return Err(Fail::Script("CHECKSIGVERIFY"));
        }
        if !verify {
            // BIP342 pushes the empty vector on failure, not 0x00.
            push(stack, alt_len, bool_encode(success))?;
        }
        return Ok(());
    }
    let script_code = if ctx.sig_version == SigVersion::Base {
        legacy_script_code(script_code_bytes(ctx), &[&sig])
    } else {
        Cow::Borrowed(script_code_bytes(ctx))
    };
    let ok = ecdsa_sig_matches(&sig, &pubkey, ctx, &script_code)?;
    if verify {
        if !ok {
            return Err(Fail::Script("CHECKSIGVERIFY"));
        }
    } else {
        push(stack, alt_len, bool_encode(ok))?;
    }
    Ok(())
}

/// BIP342 OP_CHECKSIGADD: `sig n pubkey` becomes `n` or `n+1`.
fn op_checksigadd(
    stack: &mut Stack,
    alt_len: usize,
    ctx: &EvalContext<'_, '_>,
) -> Result<(), Fail> {
    require_n(stack, 3)?;
    let pubkey = pop(stack)?;
    let n = scriptnum::decode(&pop(stack)?)?;
    let sig = pop(stack)?;
    let success = tapscript_check_sig(&sig, &pubkey, ctx)?;
    push(stack, alt_len, scriptnum::encode(n + i64::from(success)))
}

/// Core's `EvalChecksigTapscript`. Returns whether the signature counts as a success. An empty
/// signature is a soft failure; any other failing signature fails the script.
fn tapscript_check_sig(sig: &[u8], pubkey: &[u8], ctx: &EvalContext<'_, '_>) -> Result<bool, Fail> {
    if !sig.is_empty() {
        let left = ctx.validation_weight_left.get() - TAPSCRIPT_VALIDATION_WEIGHT_PER_SIGOP;
        ctx.validation_weight_left.set(left);
        if left < 0 {
            return Err(Fail::Script("TAPSCRIPT_VALIDATION_WEIGHT"));
        }
    }
    if pubkey.is_empty() {
        return Err(Fail::Script("TAPSCRIPT_EMPTY_PUBKEY"));
    }
    if sig.is_empty() {
        return Ok(false);
    }
    // Unknown public key types succeed, reserved for soft forks.
    if pubkey.len() != 32 {
        return Ok(true);
    }
    let tapscript = ctx
        .tapscript
        .as_ref()
        .expect("tapscript evaluation always carries its leaf data");
    let leaf = Some((tapscript.leaf_hash, ctx.codeseparator_pos.get()));
    check_schnorr(
        ctx.sighasher,
        ctx.verifier,
        ctx.input_index,
        sig,
        pubkey,
        tapscript.annex,
        leaf,
    )?;
    Ok(true)
}

/// BIP340/341 Schnorr check shared by the key path and tapscript. Any failure fails the script.
fn check_schnorr(
    sighasher: &Sighasher<'_>,
    verifier: &crypto::Verifier,
    input_index: usize,
    sig: &[u8],
    pubkey: &[u8],
    annex: Option<&[u8]>,
    leaf: Option<(TapLeafHash, u32)>,
) -> Result<(), Fail> {
    let (sig, ty) = match sig.len() {
        64 => (sig, TapSighashType::Default),
        // An explicit 0x00 must use the 64-byte form.
        65 if sig[64] != 0x00 => {
            let ty = TapSighashType::from_consensus_u8(sig[64])
                .map_err(|_| Fail::Script("SCHNORR_SIG_HASHTYPE"))?;
            (&sig[..64], ty)
        }
        65 => return Err(Fail::Script("SCHNORR_SIG_HASHTYPE")),
        _ => return Err(Fail::Script("SCHNORR_SIG_SIZE")),
    };
    let pubkey = XOnlyPublicKey::from_slice(pubkey).map_err(|_| Fail::Script("SCHNORR_SIG"))?;
    let sig = schnorr::Signature::from_slice(sig).expect("64 bytes");
    let sighash = sighasher.taproot(input_index, annex, leaf, ty)?;
    if !verifier.verify_schnorr(sighash, &sig, &pubkey) {
        return Err(Fail::Script("SCHNORR_SIG"));
    }
    Ok(())
}

fn op_checkmultisig(
    stack: &mut Stack,
    alt_len: usize,
    ctx: &EvalContext<'_, '_>,
    verify: bool,
    op_count: &mut usize,
) -> Result<(), Fail> {
    // Pop order matches Core: n, n keys (top = last pushed), m, m sigs, dummy.
    let n = scriptnum::decode(&pop(stack)?)?;
    if !(0..=MAX_PUBKEYS_PER_MULTISIG).contains(&n) {
        return Err(Fail::Script("PUBKEY_COUNT"));
    }
    *op_count += n as usize;
    if *op_count > MAX_OPS_LEGACY {
        return Err(Fail::Script("OP_COUNT"));
    }
    let pubkeys = (0..n).map(|_| pop(stack)).collect::<Result<Vec<_>, _>>()?;
    let m = scriptnum::decode(&pop(stack)?)?;
    if m < 0 || m > n {
        return Err(Fail::Script("SIG_COUNT"));
    }
    let sigs = (0..m).map(|_| pop(stack)).collect::<Result<Vec<_>, _>>()?;
    let dummy = pop(stack)?;
    if ctx.flags.nulldummy && !dummy.is_empty() {
        return Err(Fail::Script("SIG_NULLDUMMY"));
    }

    let script_code = if ctx.sig_version == SigVersion::Base {
        let sig_refs = sigs.iter().map(Vec::as_slice).collect::<Vec<_>>();
        legacy_script_code(script_code_bytes(ctx), &sig_refs)
    } else {
        Cow::Borrowed(script_code_bytes(ctx))
    };

    // Start from the last pushed sig and key. The key always advances; the sig only on a match.
    // Encodings are only checked for pairs actually tried.
    let (mut isig, mut ikey) = (0usize, 0usize);
    let mut success = true;
    while success && isig < sigs.len() {
        if ecdsa_sig_matches(&sigs[isig], &pubkeys[ikey], ctx, &script_code)? {
            isig += 1;
        }
        ikey += 1;
        if sigs.len() - isig > pubkeys.len() - ikey {
            success = false;
        }
    }

    if verify {
        if !success {
            return Err(Fail::Script("CHECKMULTISIGVERIFY"));
        }
    } else {
        push(stack, alt_len, bool_encode(success))?;
    }
    Ok(())
}

/// Base and witness v0 scriptCode: what follows the last executed OP_CODESEPARATOR.
fn script_code_bytes<'s>(ctx: &'s EvalContext<'_, '_>) -> &'s [u8] {
    let full = ctx.script_code.as_bytes();
    match ctx.codeseparator_script_off.get() {
        Some(off) => &full[off..],
        None => full,
    }
}

/// Core's legacy scriptCode: FindAndDelete each signature push and drop every OP_CODESEPARATOR.
///
/// An empty signature is `CScript() << vchSig`, the push `0x00`, so it deletes every OP_0.
fn legacy_script_code<'s>(script: &'s [u8], sigs: &[&[u8]]) -> Cow<'s, [u8]> {
    let needles = sigs
        .iter()
        .map(|sig| {
            let mut needle = Vec::with_capacity(sig.len() + 5);
            encode_push(&mut needle, sig);
            needle
        })
        .collect::<Vec<_>>();
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0usize;
    while i < script.len() {
        if let Some(needle) = needles.iter().find(|n| script[i..].starts_with(n)) {
            out.get_or_insert_with(|| script[..i].to_vec());
            i += needle.len();
            continue;
        }
        let op = script[i];
        match instruction_end(script, i) {
            None => {
                if let Some(buf) = out.as_mut() {
                    buf.extend_from_slice(&script[i..]);
                }
                break;
            }
            Some(end) => {
                if op == OP_CODESEPARATOR {
                    out.get_or_insert_with(|| script[..i].to_vec());
                } else if let Some(buf) = out.as_mut() {
                    buf.extend_from_slice(&script[i..end]);
                }
                i = end;
            }
        }
    }
    out.map(Cow::Owned).unwrap_or(Cow::Borrowed(script))
}

fn encode_push(out: &mut Vec<u8>, data: &[u8]) {
    let n = data.len();
    if n < usize::from(OP_PUSHDATA1) {
        out.push(n as u8);
    } else if n <= 0xff {
        out.push(OP_PUSHDATA1);
        out.push(n as u8);
    } else if n <= 0xffff {
        out.push(OP_PUSHDATA2);
        out.extend_from_slice(&(n as u16).to_le_bytes());
    } else {
        out.push(OP_PUSHDATA4);
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
    out.extend_from_slice(data);
}

/// End of the instruction at `i`, or `None` when its push length is truncated. A short payload
/// ends at the last byte of the script.
fn instruction_end(script: &[u8], i: usize) -> Option<usize> {
    let op = script[i];
    let mut j = i + 1;
    let n = match op {
        1..=OP_PUSHBYTES_75 => op as usize,
        OP_PUSHDATA1 => {
            let n = *script.get(j)? as usize;
            j += 1;
            n
        }
        OP_PUSHDATA2 => {
            let b = script.get(j..j + 2)?;
            j += 2;
            u16::from_le_bytes([b[0], b[1]]) as usize
        }
        OP_PUSHDATA4 => {
            let b = script.get(j..j + 4)?;
            j += 4;
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
        }
        _ => 0,
    };
    Some((j + n).min(script.len()))
}

/// One ECDSA signature/key comparison. Under BIP66 a badly encoded signature fails the script;
/// otherwise anything that does not verify is just `false`.
fn ecdsa_sig_matches(
    sig: &[u8],
    pubkey: &[u8],
    ctx: &EvalContext<'_, '_>,
    script_code: &[u8],
) -> Result<bool, Fail> {
    if sig.is_empty() {
        return Ok(false);
    }
    if ctx.flags.dersig && !crypto::is_valid_signature_encoding(sig) {
        return Err(Fail::Script("SIG_DER"));
    }
    let Some((sig, raw_ty)) = crypto::parse_der_sig(sig) else {
        return Ok(false);
    };
    let Ok(pubkey) = PublicKey::from_slice(pubkey) else {
        return Ok(false);
    };
    let sighash = match ctx.sig_version {
        SigVersion::Base => ctx.sighasher.legacy(ctx.input_index, script_code, raw_ty),
        _ => ctx.sighasher.segwit_v0(
            ctx.input_index,
            Script::from_bytes(script_code),
            ctx.amount,
            raw_ty,
        ),
    };
    Ok(ctx.verifier.verify_ecdsa(sighash, &sig, &pubkey))
}

fn push(stack: &mut Stack, alt_len: usize, v: Vec<u8>) -> Result<(), Fail> {
    if stack.len() + alt_len + 1 > MAX_STACK_SIZE {
        return Err(Fail::Script("STACK_SIZE"));
    }
    stack.push(v);
    Ok(())
}

fn pop(stack: &mut Stack) -> Result<Vec<u8>, Fail> {
    stack.pop().ok_or(Fail::Script("INVALID_STACK_OPERATION"))
}

fn require_n(stack: &[Vec<u8>], n: usize) -> Result<(), Fail> {
    if stack.len() < n {
        return Err(Fail::Script("INVALID_STACK_OPERATION"));
    }
    Ok(())
}

fn bool_encode(b: bool) -> Vec<u8> {
    if b {
        vec![1]
    } else {
        vec![]
    }
}

/// Bytes of the minimal `CScript() << data` push for data up to 75 bytes.
fn single_push(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() > usize::from(OP_PUSHBYTES_75) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() + 1);
    encode_push(&mut out, data);
    Some(out)
}
