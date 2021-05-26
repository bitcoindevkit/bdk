// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.
#![allow(missing_docs)]

#[cfg(feature = "test-blockchains")]
pub mod blockchain_tests;

use bitcoin::secp256k1::{Secp256k1, Verification};
use bitcoin::{Address, PublicKey};

use miniscript::descriptor::DescriptorPublicKey;
use miniscript::{Descriptor, MiniscriptKey, TranslatePk};

#[derive(Clone, Debug)]
pub struct TestIncomingOutput {
    pub value: u64,
    pub to_address: String,
}

impl TestIncomingOutput {
    pub fn new(value: u64, to_address: Address) -> Self {
        Self {
            value,
            to_address: to_address.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestIncomingTx {
    pub output: Vec<TestIncomingOutput>,
    pub min_confirmations: Option<u64>,
    pub locktime: Option<i64>,
    pub replaceable: Option<bool>,
}

impl TestIncomingTx {
    pub fn new(
        output: Vec<TestIncomingOutput>,
        min_confirmations: Option<u64>,
        locktime: Option<i64>,
        replaceable: Option<bool>,
    ) -> Self {
        Self {
            output,
            min_confirmations,
            locktime,
            replaceable,
        }
    }

    pub fn add_output(&mut self, output: TestIncomingOutput) {
        self.output.push(output);
    }
}

#[doc(hidden)]
pub trait TranslateDescriptor {
    // derive and translate a `Descriptor<DescriptorPublicKey>` into a `Descriptor<PublicKey>`
    fn derive_translated<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        index: u32,
    ) -> Descriptor<PublicKey>;
}

impl TranslateDescriptor for Descriptor<DescriptorPublicKey> {
    fn derive_translated<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        index: u32,
    ) -> Descriptor<PublicKey> {
        let translate = |key: &DescriptorPublicKey| -> PublicKey {
            match key {
                DescriptorPublicKey::XPub(xpub) => {
                    xpub.xkey
                        .derive_pub(secp, &xpub.derivation_path)
                        .expect("hardened derivation steps")
                        .public_key
                }
                DescriptorPublicKey::SinglePub(key) => key.key,
            }
        };

        self.derive(index)
            .translate_pk_infallible(|pk| translate(pk), |pkh| translate(pkh).to_pubkeyhash())
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! testutils {
    ( @external $descriptors:expr, $child:expr ) => ({
        use bitcoin::secp256k1::Secp256k1;
        use miniscript::descriptor::{Descriptor, DescriptorPublicKey, DescriptorTrait};

        use $crate::testutils::TranslateDescriptor;

        let secp = Secp256k1::new();

        let parsed = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &$descriptors.0).expect("Failed to parse descriptor in `testutils!(@external)`").0;
        parsed.derive_translated(&secp, $child).address(bitcoin::Network::Regtest).expect("No address form")
    });
    ( @internal $descriptors:expr, $child:expr ) => ({
        use bitcoin::secp256k1::Secp256k1;
        use miniscript::descriptor::{Descriptor, DescriptorPublicKey, DescriptorTrait};

        use $crate::testutils::TranslateDescriptor;

        let secp = Secp256k1::new();

        let parsed = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &$descriptors.1.expect("Missing internal descriptor")).expect("Failed to parse descriptor in `testutils!(@internal)`").0;
        parsed.derive_translated(&secp, $child).address(bitcoin::Network::Regtest).expect("No address form")
    });
    ( @e $descriptors:expr, $child:expr ) => ({ testutils!(@external $descriptors, $child) });
    ( @i $descriptors:expr, $child:expr ) => ({ testutils!(@internal $descriptors, $child) });

    ( @tx ( $( ( $( $addr:tt )* ) => $amount:expr ),+ ) $( ( @locktime $locktime:expr ) )? $( ( @confirmations $confirmations:expr ) )? $( ( @replaceable $replaceable:expr ) )? ) => ({
        let outs = vec![$( $crate::testutils::TestIncomingOutput::new($amount, testutils!( $($addr)* ))),+];

        let locktime = None::<i64>$(.or(Some($locktime)))?;

        let min_confirmations = None::<u64>$(.or(Some($confirmations)))?;
        let replaceable = None::<bool>$(.or(Some($replaceable)))?;

        $crate::testutils::TestIncomingTx::new(outs, min_confirmations, locktime, replaceable)
    });

    ( @literal $key:expr ) => ({
        let key = $key.to_string();
        (key, None::<String>, None::<String>)
    });
    ( @generate_xprv $( $external_path:expr )? $( ,$internal_path:expr )? ) => ({
        use rand::Rng;

        let mut seed = [0u8; 32];
        rand::thread_rng().fill(&mut seed[..]);

        let key = bitcoin::util::bip32::ExtendedPrivKey::new_master(
            bitcoin::Network::Testnet,
            &seed,
        );

        let external_path = None::<String>$(.or(Some($external_path.to_string())))?;
        let internal_path = None::<String>$(.or(Some($internal_path.to_string())))?;

        (key.unwrap().to_string(), external_path, internal_path)
    });
    ( @generate_wif ) => ({
        use rand::Rng;

        let mut key = [0u8; bitcoin::secp256k1::constants::SECRET_KEY_SIZE];
        rand::thread_rng().fill(&mut key[..]);

        (bitcoin::PrivateKey {
            compressed: true,
            network: bitcoin::Network::Testnet,
            key: bitcoin::secp256k1::SecretKey::from_slice(&key).unwrap(),
        }.to_string(), None::<String>, None::<String>)
    });

    ( @keys ( $( $alias:expr => ( $( $key_type:tt )* ) ),+ ) ) => ({
        let mut map = std::collections::HashMap::new();
        $(
            let alias: &str = $alias;
            map.insert(alias, testutils!( $($key_type)* ));
        )+

        map
    });

    ( @descriptors ( $external_descriptor:expr ) $( ( $internal_descriptor:expr ) )? $( ( @keys $( $keys:tt )* ) )* ) => ({
        use std::str::FromStr;
        use std::collections::HashMap;
        use miniscript::descriptor::Descriptor;
        use miniscript::TranslatePk;

        #[allow(unused_assignments, unused_mut)]
        let mut keys: HashMap<&'static str, (String, Option<String>, Option<String>)> = HashMap::new();
        $(
            keys = testutils!{ @keys $( $keys )* };
        )*

        let external: Descriptor<String> = FromStr::from_str($external_descriptor).unwrap();
        let external: Descriptor<String> = external.translate_pk_infallible::<_, _>(|k| {
            if let Some((key, ext_path, _)) = keys.get(&k.as_str()) {
                format!("{}{}", key, ext_path.as_ref().unwrap_or(&"".into()))
            } else {
                k.clone()
            }
        }, |kh| {
            if let Some((key, ext_path, _)) = keys.get(&kh.as_str()) {
                format!("{}{}", key, ext_path.as_ref().unwrap_or(&"".into()))
            } else {
                kh.clone()
            }

        });
        let external = external.to_string();

        let internal = None::<String>$(.or({
            let string_internal: Descriptor<String> = FromStr::from_str($internal_descriptor).unwrap();

            let string_internal: Descriptor<String> = string_internal.translate_pk_infallible::<_, _>(|k| {
                if let Some((key, _, int_path)) = keys.get(&k.as_str()) {
                    format!("{}{}", key, int_path.as_ref().unwrap_or(&"".into()))
                } else {
                    k.clone()
                }
            }, |kh| {
                if let Some((key, _, int_path)) = keys.get(&kh.as_str()) {
                    format!("{}{}", key, int_path.as_ref().unwrap_or(&"".into()))
                } else {
                    kh.clone()
                }
            });
            Some(string_internal.to_string())
        }))?;

        (external, internal)
    })
}
