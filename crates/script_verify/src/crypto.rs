// Adapted from the `crypto` module of rbitcoin `crates/rbitcoin-consensus/src/script/mod.rs` at
// 49210ea82130474a4df366e253d1346a16369fdb (MIT OR Apache-2.0, rbitcoin contributors).

use bitcoin::hashes::{hash160, sha1, sha256, Hash};
use bitcoin::secp256k1::{ecdsa, schnorr, Message, Parity, PublicKey, Secp256k1, VerifyOnly};
use bitcoin::taproot::{TapNodeHash, TapTweakHash};
use bitcoin::XOnlyPublicKey;

/// Splits a signature push into its DER part and the **raw** sighash byte.
///
/// The raw byte is kept because legacy and BIP143 sighashes commit to it unnormalized: mainnet has
/// spends signed with hashtype 0. DER is always parsed laxly, as Core does, because libsecp's
/// strict parser recovers the wrong (R, S) for some pre-BIP66 encodings. BIP66 strictness is
/// enforced separately by [`is_valid_signature_encoding`].
pub fn parse_der_sig(sig: &[u8]) -> Option<(ecdsa::Signature, u32)> {
    let (&sighash_byte, der) = sig.split_last()?;
    let sig = ecdsa::Signature::from_der_lax(der).ok()?;
    Some((sig, u32::from(sighash_byte)))
}

/// BIP66 strict DER encoding of a full signature push, including its trailing hashtype byte.
pub fn is_valid_signature_encoding(sig: &[u8]) -> bool {
    // 0x30 [total-length] 0x02 [R-length] [R] 0x02 [S-length] [S] [sighash]
    if sig.len() < 9 || sig.len() > 73 {
        return false;
    }
    if sig[0] != 0x30 {
        return false;
    }
    if sig[1] as usize != sig.len().wrapping_sub(3) {
        return false;
    }
    if sig[2] != 0x02 {
        return false;
    }
    let len_r = sig[3] as usize;
    if len_r == 0 {
        return false;
    }
    if 5 + len_r >= sig.len() {
        return false;
    }
    if sig[4 + len_r] != 0x02 {
        return false;
    }
    let len_s = sig[5 + len_r] as usize;
    if len_s == 0 {
        return false;
    }
    if len_r + len_s + 7 != sig.len() {
        return false;
    }
    if sig[4] & 0x80 != 0 {
        return false;
    }
    if len_r > 1 && sig[4] == 0x00 && (sig[5] & 0x80) == 0 {
        return false;
    }
    let s0 = 6 + len_r;
    if sig[s0] & 0x80 != 0 {
        return false;
    }
    if len_s > 1 && sig[s0] == 0x00 && (sig[s0 + 1] & 0x80) == 0 {
        return false;
    }
    true
}

/// Signature and key checks, sharing one secp256k1 verification context.
pub struct Verifier {
    secp: Secp256k1<VerifyOnly>,
}

impl Verifier {
    pub fn new() -> Self {
        Self {
            secp: Secp256k1::verification_only(),
        }
    }

    /// ECDSA verification under consensus rules.
    ///
    /// libsecp256k1 rejects high-S signatures but high S was never a consensus failure (BIP146 is
    /// unactivated), so S is normalized first as Core does.
    pub fn verify_ecdsa(
        &self,
        sighash: [u8; 32],
        sig: &ecdsa::Signature,
        pubkey: &PublicKey,
    ) -> bool {
        let mut normalized = *sig;
        normalized.normalize_s();
        self.secp
            .verify_ecdsa(&Message::from_digest(sighash), &normalized, pubkey)
            .is_ok()
    }

    /// BIP340 verification.
    pub fn verify_schnorr(
        &self,
        sighash: [u8; 32],
        sig: &schnorr::Signature,
        pubkey: &XOnlyPublicKey,
    ) -> bool {
        self.secp
            .verify_schnorr(sig, &Message::from_digest(sighash), pubkey)
            .is_ok()
    }

    /// BIP341: whether `output_key`, with `parity`, is `internal` tweaked by `merkle_root`.
    pub fn verify_taproot_tweak(
        &self,
        internal: &XOnlyPublicKey,
        merkle_root: TapNodeHash,
        output_key: &XOnlyPublicKey,
        parity: Parity,
    ) -> bool {
        let tweak = TapTweakHash::from_key_and_tweak(*internal, Some(merkle_root)).to_scalar();
        internal.tweak_add_check(&self.secp, output_key, parity, tweak)
    }
}

pub fn hash160(data: &[u8]) -> [u8; 20] {
    hash160::Hash::hash(data).to_byte_array()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    sha256::Hash::hash(data).to_byte_array()
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    sha1::Hash::hash(data).to_byte_array()
}
