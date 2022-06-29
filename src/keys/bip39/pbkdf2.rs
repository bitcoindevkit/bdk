// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2022 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use std::borrow::Cow;

use bitcoin::hashes::{sha512, Hash, HashEngine, Hmac, HmacEngine};
use unicode_normalization::UnicodeNormalization;

const SALT_PREFIX: &str = "mnemonic"; // BIP-0039 seed's salt prefix
const ITERATION_COUNT: u32 = 2048; // PBKDF2 iteration count
const BLOCK_LEN: usize = sha512::Hash::LEN; // PBKDF2 block size

/// Hmac engine used as the pseudo-random function.
type HmacPRF = HmacEngine<sha512::Hash>;

/// Make hmac-sha512 engine from password.
/// The hmac engine is used as the pseudo-random function.
fn make_prf(password: &str) -> HmacPRF {
    HmacEngine::new(password.as_bytes())
}

/// Generate block (of given block_index) by calculating xor sum of iterations of PRF.
fn xor_sum(hmac_prf: &HmacPRF, salt: &str, iter_count: u32, block_index: u32, block: &mut [u8]) {
    // for the first iteration, we concat: salt + block_index (as big-endian bytes)
    let mut prev_u = Vec::with_capacity(salt.len() + 4);
    prev_u.extend_from_slice(salt.as_bytes());
    prev_u.extend_from_slice(&block_index.to_be_bytes());

    for _ in 0..iter_count {
        let mut prf = hmac_prf.clone(); // fresh hmac engine
        prf.input(&prev_u);
        let u = Hmac::from_engine(prf).into_inner();

        prev_u.clone_from(&u.to_vec());

        // perform xor sum into `block`
        block.iter_mut().zip(&u).for_each(|(a, b)| *a ^= b);
    }
}

/// Generate `derived_key` via PBKDF2-HMAC-SHA512.
fn pbkdf2_hmac_sha512(password: &str, salt: &str, iter_count: u32, derived_key: &mut [u8]) {
    let hmac_prf = make_prf(password);

    for (i, block) in derived_key.chunks_mut(BLOCK_LEN).enumerate() {
        // block indexes starts at 1
        let block_index = (i + 1) as u32;

        xor_sum(&hmac_prf, salt, iter_count, block_index, block);
    }
}

/// Password is the UTF8-NFKD-normalized result of mnemonic words separated by space.
fn make_password<'a, W>(words: W) -> String
where
    W: Iterator<Item = &'a str>,
{
    words.collect::<Vec<&str>>().join(" ").nfkd().to_string()
}

/// Salt is the UTF8-NFKD-normalized result of (SALT_PREFIX + passphrase).
fn make_salt(passphrase: Option<&str>) -> Cow<'static, str> {
    let mut salt = Cow::from(SALT_PREFIX);
    if let Some(passphrase) = passphrase {
        salt.to_mut().push_str(&passphrase.nfkd().to_string());
    }
    salt
}

/// Generate BIP-0039 seed.
pub(crate) fn generate_seed<'a, W>(words: W, passphrase: Option<&str>, seed: &mut [u8])
where
    W: Iterator<Item = &'a str>,
{
    let password = make_password(words);
    let salt = make_salt(passphrase);

    pbkdf2_hmac_sha512(&password, &salt, ITERATION_COUNT, seed);
}
