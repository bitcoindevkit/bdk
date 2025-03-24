//! The BIP-322 module provides functionality for message signing
//! according to the BIP-322 standard.

pub mod sign;
pub mod utils;
pub mod verify;
use alloc::{string::String, vec::Vec};
use bitcoin::{OutPoint, ScriptBuf, Sequence, Witness};
use core::fmt;

/// BIP322Signature encapsulates all the data and functionality required to sign a message
/// according to the BIP322 specification. It supports multiple signature formats:
/// - **Legacy:** Produces a standard ECDSA signature for P2PKH addresses.
/// - **Simple:** Creates a simplified signature that encodes witness data.
/// - **Full:** Constructs a complete transaction with witness details and signs it.
///
/// # Fields
/// - `private_key_str`: A WIF-encoded private key as a `String`.  
/// - `message`: The message to be signed.
/// - `address_str`: The Bitcoin address associated with the signing process.
/// - `signature_type`: The signature format to use, defined by `Bip322SignatureFormat`.
/// - `proof_of_funds`: An optional vector of tuples providing additional UTXO information
///   (proof of funds) used in advanced signing scenarios.
pub struct BIP322Signature {
    private_key_str: String,
    message: String,
    address_str: String,
    signature_type: Bip322SignatureFormat,
    proof_of_funds: Option<Vec<(OutPoint, ScriptBuf, Witness, Sequence)>>,
}

/// BIP322Verification encapsulates the data and functionality required to verify a message
/// signature according to the BIP322 protocol. It supports verifying signatures produced
/// using different signature formats:
/// - **Legacy:** Standard ECDSA signatures.
/// - **Simple:** Simplified signatures that encapsulate witness data.
/// - **Full:** Fully signed transactions with witness details.
///
/// # Fields
/// - `address_str`: The Bitcoin address as a string against which the signature will be verified.
/// - `signature`: A Base64-encoded signature string.
/// - `message`: The original message that was signed.
/// - `signature_type`: The signature format used during signing, defined by `Bip322SignatureFormat`.
/// - `priv_key`: An optional private key string. Required for verifying legacy signatures.
pub struct BIP322Verification {
    address_str: String,
    signature: String,
    message: String,
    signature_type: Bip322SignatureFormat,
    private_key_str: Option<String>,
}

/// Represents the different formats supported by the BIP322 message signing protocol.
///
/// BIP322 defines multiple formats for signatures to accommodate different use cases
/// and maintain backward compatibility with legacy signing methods.
#[derive(Debug, PartialEq)]
pub enum Bip322SignatureFormat {
    /// The legacy Bitcoin message signature format used before BIP322.
    Legacy,
    /// A simplified version of the BIP322 format that includes only essential data.
    Simple,
    /// The Full BIP322 format that includes all signature data.
    Full,
}

/// Error types for BIP322 message signing and verification operations.
///
/// This enum encompasses all possible errors that can occur during the BIP322
/// message signing or verification process.
#[derive(Debug)]
pub enum BIP322Error {
    /// Error encountered when extracting data, such as from a PSBT
    ExtractionError(String),
    /// Unable to compute the signature hash for signing
    SighashError,
    /// The message does not meet requirements
    InvalidMessage,
    /// The script or address type is not supported
    UnsupportedType,
    /// The format of the data is invalid for the given context
    InvalidFormat(String),
    /// The provided private key is invalid
    InvalidPrivateKey,
    /// The provided public key is invalid
    InvalidPublicKey(String),
    /// The provided Bitcoin address is invalid
    InvalidAddress,
    /// The address is not a Segwit address
    NotSegwitAddress,
    /// The Segwit version is not supported for the given context
    UnsupportedSegwitVersion(String),
    /// The digital signature is invalid
    InvalidSignature(String),
    /// The transaction witness data is invalid
    InvalidWitness(String),
    /// Error encountered when decoding Bitcoin consensus data
    DecodeError(String),
    /// Error encountered when decoding Base64 data
    Base64DecodeError,
    /// The provided sighash type is invalid for this context
    InvalidSighashType,
}

impl std::error::Error for BIP322Error {}

impl std::fmt::Display for BIP322Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BIP322Error::ExtractionError(e) => write!(f, "Unable to extract {}", e),
            BIP322Error::SighashError => write!(f, "Unable to compute signature hash"),
            BIP322Error::InvalidMessage => write!(f, "Message hash is not secure"),
            BIP322Error::UnsupportedType => write!(f, "Type is not supported"),
            BIP322Error::InvalidFormat(e) => write!(f, "Only valid for {} format", e),
            BIP322Error::InvalidPrivateKey => write!(f, "Invalid private key"),
            BIP322Error::InvalidPublicKey(e) => write!(f, "Invalid public key {}", e),
            BIP322Error::InvalidAddress => write!(f, "Invalid address"),
            BIP322Error::NotSegwitAddress => write!(f, "Not a Segwit address"),
            BIP322Error::UnsupportedSegwitVersion(e) => write!(f, "Only Segwit {} is supported", e),
            BIP322Error::InvalidSignature(e) => write!(f, "Invalid Signature - {}", e),
            BIP322Error::InvalidWitness(e) => write!(f, "Invalid Witness - {}", e),
            BIP322Error::InvalidSighashType => write!(f, "Sighash type is invalid"),
            BIP322Error::DecodeError(e) => write!(f, "Consensus decode error - {}", e),
            BIP322Error::Base64DecodeError => write!(f, "Base64 decoding failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bip322::utils::{tagged_message_hash, to_sign, to_spend};

    use super::*;
    use bitcoin::Address;
    use core::str::FromStr;
    use std::string::ToString;

    // TEST VECTORS FROM
    // https://github.com/bitcoin/bips/blob/master/bip-0322.mediawiki#user-content-Test_vectors
    // https://github.com/Peach2Peach/bip322-js/tree/main/test

    const PRIVATE_KEY: &str = "L3VFeEujGtevx9w18HD1fhRbCH67Az2dpCymeRE1SoPK6XQtaN2k";
    const PRIVATE_KEY_TESTNET: &str = "cTrF79uahxMC7bQGWh2931vepWPWqS8KtF8EkqgWwv3KMGZNJ2yP";

    const SEGWIT_ADDRESS: &str = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
    const SEGWIT_TESTNET_ADDRESS: &str = "tb1q9vza2e8x573nczrlzms0wvx3gsqjx7vaxwd45v";
    const TAPROOT_ADDRESS: &str = "bc1ppv609nr0vr25u07u95waq5lucwfm6tde4nydujnu8npg4q75mr5sxq8lt3";
    const TAPROOT_TESTNET_ADDRESS: &str =
        "tb1ppv609nr0vr25u07u95waq5lucwfm6tde4nydujnu8npg4q75mr5s3g3s37";
    const LEGACY_ADDRESS: &str = "14vV3aCHBeStb5bkenkNHbe2YAFinYdXgc";
    const LEGACY_ADDRESS_TESTNET: &str = "mjSSLdHFzft9NC5NNMik7WrMQ9rRhMhNpT";

    const HELLO_WORLD_MESSAGE: &str = "Hello World";

    const NESTED_SEGWIT_PRIVATE_KEY: &str = "KwTbAxmBXjoZM3bzbXixEr9nxLhyYSM4vp2swet58i19bw9sqk5z";
    const NESTED_SEGWIT_TESTNET_PRIVATE_KEY: &str =
        "cMpadsm2xoVpWV5FywY5cAeraa1PCtSkzrBM45Ladpf9rgDu6cMz";
    const NESTED_SEGWIT_ADDRESS: &str = "3HSVzEhCFuH9Z3wvoWTexy7BMVVp3PjS6f";
    const NESTED_SEGWIT_TESTNET_ADDRESS: &str = "2N8zi3ydDsMnVkqaUUe5Xav6SZqhyqEduap";

    #[test]
    fn test_message_hashing() {
        let empty_hash = tagged_message_hash(b"");
        let hello_world_hash = tagged_message_hash(b"Hello World");

        assert_eq!(
            empty_hash.to_string(),
            "c90c269c4f8fcbe6880f72a721ddfbf1914268a794cbb21cfafee13770ae19f1"
        );
        assert_eq!(
            hello_world_hash.to_string(),
            "f0eb03b1a75ac6d9847f55c624a99169b5dccba2a31f5b23bea77ba270de0a7a"
        );
    }

    #[test]
    fn test_to_spend_and_to_sign() {
        let script_pubkey = Address::from_str(SEGWIT_ADDRESS)
            .unwrap()
            .assume_checked()
            .script_pubkey();

        // Test case for empty message - to_spend
        let tx_spend_empty_msg = to_spend(&script_pubkey, "");
        assert_eq!(
            tx_spend_empty_msg.compute_txid().to_string(),
            "c5680aa69bb8d860bf82d4e9cd3504b55dde018de765a91bb566283c545a99a7"
        );

        // Test case for "Hello World" - to_spend
        let tx_spend_hello_world_msg = to_spend(&script_pubkey, HELLO_WORLD_MESSAGE);
        assert_eq!(
            tx_spend_hello_world_msg.compute_txid().to_string(),
            "b79d196740ad5217771c1098fc4a4b51e0535c32236c71f1ea4d61a2d603352b"
        );

        // Test case for empty message - to_sign
        let tx_sign_empty_msg = to_sign(
            &tx_spend_empty_msg.output[0].script_pubkey,
            tx_spend_empty_msg.compute_txid(),
            tx_spend_empty_msg.lock_time,
            tx_spend_empty_msg.input[0].sequence,
            Some(tx_spend_empty_msg.input[0].witness.clone()),
        )
        .unwrap();
        assert_eq!(
            tx_sign_empty_msg.unsigned_tx.compute_txid().to_string(),
            "1e9654e951a5ba44c8604c4de6c67fd78a27e81dcadcfe1edf638ba3aaebaed6"
        );

        // Test case for HELLO_WORLD_MESSAGE - to_sign
        let tx_sign_hw_msg = to_sign(
            &tx_spend_hello_world_msg.output[0].script_pubkey,
            tx_spend_hello_world_msg.compute_txid(),
            tx_spend_hello_world_msg.lock_time,
            tx_spend_hello_world_msg.input[0].sequence,
            Some(tx_spend_hello_world_msg.input[0].witness.clone()),
        )
        .unwrap();

        assert_eq!(
            tx_sign_hw_msg.unsigned_tx.compute_txid().to_string(),
            "88737ae86f2077145f93cc4b153ae9a1cb8d56afa511988c149c5c8c9d93bddf"
        );
    }

    #[test]
    fn sign_and_verify_legacy_signature() {
        let legacy_sign = BIP322Signature::new(
            PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            LEGACY_ADDRESS.to_string(),
            Bip322SignatureFormat::Legacy,
            None,
        );
        let sign_message = legacy_sign.sign().unwrap();

        let legacy_sign_testnet = BIP322Signature::new(
            PRIVATE_KEY_TESTNET.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            LEGACY_ADDRESS_TESTNET.to_string(),
            Bip322SignatureFormat::Legacy,
            None,
        );
        let sign_message_testnet = legacy_sign_testnet.sign().unwrap();

        let legacy_verify = BIP322Verification::new(
            LEGACY_ADDRESS.to_string(),
            sign_message,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Legacy,
            Some(PRIVATE_KEY.to_string()),
        );

        let verify_message = legacy_verify.verify().unwrap();

        let legacy_verify_testnet = BIP322Verification::new(
            LEGACY_ADDRESS.to_string(),
            sign_message_testnet,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Legacy,
            Some(PRIVATE_KEY_TESTNET.to_string()),
        );

        let verify_message_testnet = legacy_verify_testnet.verify().unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn sign_and_verify_legacy_signature_with_wrong_message() {
        let legacy_sign = BIP322Signature::new(
            PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            LEGACY_ADDRESS.to_string(),
            Bip322SignatureFormat::Legacy,
            None,
        );
        let sign_message = legacy_sign.sign().unwrap();

        let legacy_verify = BIP322Verification::new(
            LEGACY_ADDRESS.to_string(),
            sign_message,
            "".to_string(),
            Bip322SignatureFormat::Legacy,
            Some(PRIVATE_KEY.to_string()),
        );

        let verify_message = legacy_verify.verify().unwrap();

        assert_eq!(verify_message, false);
    }

    #[test]
    fn test_sign_and_verify_nested_segwit_address() {
        let nested_segwit_simple_sign = BIP322Signature::new(
            NESTED_SEGWIT_PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            NESTED_SEGWIT_ADDRESS.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );

        let sign_message = nested_segwit_simple_sign.sign().unwrap();
        assert_eq!(sign_message.clone(), "AkgwRQIhAMd2wZSY3x0V9Kr/NClochoTXcgDaGl3OObOR17yx3QQAiBVWxqNSS+CKen7bmJTG6YfJjsggQ4Fa2RHKgBKrdQQ+gEhAxa5UDdQCHSQHfKQv14ybcYm1C9y6b12xAuukWzSnS+w");

        let nested_segwit_full_sign_testnet = BIP322Signature::new(
            NESTED_SEGWIT_TESTNET_PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            NESTED_SEGWIT_TESTNET_ADDRESS.to_string(),
            Bip322SignatureFormat::Full,
            None,
        );

        let sign_message_testnet = nested_segwit_full_sign_testnet.sign().unwrap();
        assert_eq!(sign_message_testnet.clone(), "AAAAAAABAVuR8vsJiiYj9+vO+8l7Ol3wt3Frz7SVyVSxn0ehOUb+AAAAAAAAAAAAAQAAAAAAAAAAAWoCSDBFAiEAx3bBlJjfHRX0qv80KWhyGhNdyANoaXc45s5HXvLHdBACIFVbGo1JL4Ip6ftuYlMbph8mOyCBDgVrZEcqAEqt1BD6ASEDFrlQN1AIdJAd8pC/XjJtxibUL3LpvXbEC66RbNKdL7AAAAAA");

        let nested_segwit_full_verify = BIP322Verification::new(
            NESTED_SEGWIT_ADDRESS.to_string(),
            sign_message,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );

        let verify_message = nested_segwit_full_verify.verify().unwrap();

        let nested_segwit_full_verify_testnet = BIP322Verification::new(
            NESTED_SEGWIT_TESTNET_ADDRESS.to_string(),
            sign_message_testnet,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Full,
            None,
        );

        let verify_message_testnet = nested_segwit_full_verify_testnet.verify().unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn test_sign_and_verify_segwit_address() {
        let full_sign = BIP322Signature::new(
            PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            SEGWIT_ADDRESS.to_string(),
            Bip322SignatureFormat::Full,
            None,
        );
        let sign_message = full_sign.sign().unwrap();

        let simple_sign = BIP322Signature::new(
            PRIVATE_KEY_TESTNET.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            SEGWIT_TESTNET_ADDRESS.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );
        let sign_message_testnet = simple_sign.sign().unwrap();

        let full_verify = BIP322Verification::new(
            SEGWIT_ADDRESS.to_string(),
            sign_message,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Full,
            None,
        );

        let verify_message = full_verify.verify().unwrap();

        let simple_verify = BIP322Verification::new(
            SEGWIT_TESTNET_ADDRESS.to_string(),
            sign_message_testnet,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );

        let verify_message_testnet = simple_verify.verify().unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn test_sign_and_verify_taproot_address() {
        let full_sign = BIP322Signature::new(
            PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            TAPROOT_ADDRESS.to_string(),
            Bip322SignatureFormat::Full,
            None,
        );
        let sign_message = full_sign.sign().unwrap();

        let simple_sign = BIP322Signature::new(
            PRIVATE_KEY.to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            TAPROOT_TESTNET_ADDRESS.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );

        let sign_message_testnet = simple_sign.sign().unwrap();

        let full_verify = BIP322Verification::new(
            TAPROOT_ADDRESS.to_string(),
            sign_message,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Full,
            None,
        );

        let verify_message = full_verify.verify().unwrap();

        let simple_verify = BIP322Verification::new(
            TAPROOT_TESTNET_ADDRESS.to_string(),
            sign_message_testnet,
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );

        let verify_message_testnet = simple_verify.verify().unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn test_simple_segwit_verification() {
        let simple_verify = BIP322Verification::new(
            SEGWIT_ADDRESS.to_string(),
            "AkcwRAIgZRfIY3p7/DoVTty6YZbWS71bc5Vct9p9Fia83eRmw2QCICK/ENGfwLtptFluMGs2KsqoNSk89pO7F29zJLUx9a/sASECx/EgAxlkQpQ9hYjgGu6EBCPMVPwVIVJqO4XCsMvViHI=".to_string(),
            HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );
        assert!(simple_verify.verify().unwrap());

        let simple_verify_2 = BIP322Verification::new(
            SEGWIT_ADDRESS.to_string(),
            "AkgwRQIhAOzyynlqt93lOKJr+wmmxIens//zPzl9tqIOua93wO6MAiBi5n5EyAcPScOjf1lAqIUIQtr3zKNeavYabHyR8eGhowEhAsfxIAMZZEKUPYWI4BruhAQjzFT8FSFSajuFwrDL1Yhy".to_string(),
                HELLO_WORLD_MESSAGE.to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );
        assert!(simple_verify_2.verify().unwrap());

        let simple_verify_empty_message = BIP322Verification::new(
            SEGWIT_ADDRESS.to_string(),
            "AkgwRQIhAPkJ1Q4oYS0htvyuSFHLxRQpFAY56b70UvE7Dxazen0ZAiAtZfFz1S6T6I23MWI2lK/pcNTWncuyL8UL+oMdydVgzAEhAsfxIAMZZEKUPYWI4BruhAQjzFT8FSFSajuFwrDL1Yhy".to_string(),
                "".to_string(),
            Bip322SignatureFormat::Simple,
            None,
        );
        assert!(simple_verify_empty_message.verify().unwrap());

        let simple_verify_empty_message_2 = BIP322Verification::new(
                SEGWIT_ADDRESS.to_string(),
                "AkcwRAIgM2gBAQqvZX15ZiysmKmQpDrG83avLIT492QBzLnQIxYCIBaTpOaD20qRlEylyxFSeEA2ba9YOixpX8z46TSDtS40ASECx/EgAxlkQpQ9hYjgGu6EBCPMVPwVIVJqO4XCsMvViHI=".to_string(),
                    "".to_string(),
                Bip322SignatureFormat::Simple,
                None,
            );
        assert!(simple_verify_empty_message_2.verify().unwrap());
    }
}
