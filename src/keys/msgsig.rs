/// Signing arbitrary messages and verify message signatures
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::ecdsa::RecoverableSignature;
use bitcoin::secp256k1::{All, Message, Secp256k1, SecretKey};
use bitcoin::util::misc::{signed_msg_hash, MessageSignature};
use bitcoin::{Address, Network, PrivateKey, PublicKey};

/// Trait for message signers
pub trait MessageSigner<S> {
    /// Sign an arbitrary message
    fn sign(&self, msg: &str) -> S;
}

/// Trait for message signature verifiers
pub trait MessageSignatureVerifier<S> {
    /// Verify a signature against an arbitrary message
    fn verify(&self, sig: S, msg: &str) -> bool;
}

/// A message signer using ECDSA
pub struct EcdsaMessageSigner {
    secp: Secp256k1<All>,
    secret_key: SecretKey,
}

impl EcdsaMessageSigner {
    /// Creates message signer from a bitcoin ECDSA private key
    pub fn from_prv(prv: PrivateKey) -> Self {
        Self::from_secret_key(prv.inner)
    }

    /// Creates message signer from an ECDSA secret key
    pub fn from_secret_key(secret_key: SecretKey) -> Self {
        EcdsaMessageSigner {
            secret_key,
            secp: Secp256k1::new(),
        }
    }
}

impl MessageSigner<RecoverableSignature> for EcdsaMessageSigner {
    fn sign(&self, message: &str) -> RecoverableSignature {
        let msg_hash = signed_msg_hash(message);
        self.secp.sign_ecdsa_recoverable(
            &Message::from_slice(&msg_hash.into_inner()[..])
                .expect("Message to be signed is not a valid Hash"),
            &self.secret_key,
        )
    }
}

/// A message signature verifier using ECDSA
pub struct EcdsaMessageSignatureVerifier {
    secp: Secp256k1<All>,
    address: Address,
}

impl EcdsaMessageSignatureVerifier {
    /// Creates a message signature verifier from a public key
    pub fn from_pub(public_key: PublicKey) -> Self {
        let address = Address::p2pkh(&public_key, Network::Bitcoin);
        Self::from_address(address)
    }

    /// Creates a message signature verifier from an address
    pub fn from_address(address: Address) -> Self {
        EcdsaMessageSignatureVerifier {
            address,
            secp: Secp256k1::new(),
        }
    }
}

impl MessageSignatureVerifier<RecoverableSignature> for EcdsaMessageSignatureVerifier {
    fn verify(&self, sig: RecoverableSignature, msg: &str) -> bool {
        let message_sig = MessageSignature::new(sig, false);
        let msg_hash = signed_msg_hash(msg);
        message_sig
            .is_signed_by_address(&self.secp, &self.address, msg_hash)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod ecdsa_msg_sign {
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::util::misc::MessageSignature;
    use bitcoin::{Address, Network};

    use super::*;

    pub const PRV_WIF: &str = "5KYZdUEo39z3FPrtuX2QbbwGnNP5zTd7yyr2SC1j299sBCnWjss";

    const MSG: &str = "fix the money";

    #[test]
    fn test_sign_message() {
        let secp = Secp256k1::new();
        let prv = PrivateKey::from_wif(PRV_WIF).unwrap();

        let signer = EcdsaMessageSigner::from_prv(prv);
        let sig = signer.sign(MSG);
        let pub_key = prv.public_key(&secp);
        let address = Address::p2pkh(&pub_key, Network::Bitcoin);
        let message_sig = MessageSignature::new(sig, false);

        assert_eq!(
            message_sig
                .is_signed_by_address(&secp, &address, signed_msg_hash(MSG))
                .unwrap(),
            true
        );
    }

    #[test]
    fn test_verify_signed_message() {
        let secp = Secp256k1::new();
        let prv = PrivateKey::from_wif(PRV_WIF).unwrap();
        let sig = EcdsaMessageSigner::from_prv(prv).sign(MSG);
        let verifier = EcdsaMessageSignatureVerifier::from_pub(prv.public_key(&secp));

        assert_eq!(verifier.verify(sig, MSG), true);
    }
}
