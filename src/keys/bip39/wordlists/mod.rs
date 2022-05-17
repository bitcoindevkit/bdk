mod english;
mod japanese;
mod korean;
mod spanish;
mod chinese_simplified;
mod chinese_traditional;
mod french;
mod italian;
mod czech;
mod portuguese;

/// List of supported languages for mnemonics
pub enum Language {
    /// The English language
    English,
    /// The Japanese language
    Japanese,
    /// The Korean language
    Korean,
    /// The Spanish language
    Spanish,
    /// The Chinese (Simplified) language
    ChineseSimplified,
    /// The Chinese (Traditional) language
    ChineseTraditional,
    /// The French language
    French,
    /// The Italian language
    Italian,
    /// The Czech language
    Czech,
    /// The Portuguese language
    Portuguese,
}

impl Language {
    /// Fetch the wordlist for a specific language
    pub fn wordlist(&self) -> &[&str; 2048] {
        match *self {
            Language::English => &english::WORDS,
            Language::Japanese => &japanese::WORDS,
            Language::Korean => &korean::WORDS,
            Language::Spanish => &spanish::WORDS,
            Language::ChineseSimplified => &chinese_simplified::WORDS,
            Language::ChineseTraditional => &chinese_traditional::WORDS,
            Language::French => &french::WORDS,
            Language::Italian => &italian::WORDS,
            Language::Czech => &czech::WORDS,
            Language::Portuguese => &portuguese::WORDS,
        }
    }
}

#[cfg(test)]
mod test {
    use super::Language;
    use bitcoin::hashes::{sha256, Hash, HashEngine};

    /// Wordlists are sourced from BIP39. This test is to ensure they have not been tampered with
    /// by checking the SHA256 of the wordlist txt file matches the SHA256 of the data in our
    /// codebase.
    #[test]
    fn test_wordlist_hash() {
        // The sha256 is calculated from the raw txt file of each language wordlist sourced from
        // [BIP39](https://github.com/bitcoin/bips/blob/master/bip-0039/bip-0039-wordlists.md).
        let checksum = vec![
            ("2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda", Language::English),
            ("2eed0aef492291e061633d7ad8117f1a2b03eb80a29d0e4e3117ac2528d05ffd", Language::Japanese),
            ("9e95f86c167de88f450f0aaf89e87f6624a57f973c67b516e338e8e8b8897f60", Language::Korean),
            ("46846a5a0139d1e3cb77293e521c2865f7bcdb82c44e8d0a06a2cd0ecba48c0b", Language::Spanish),
            ("5c5942792bd8340cb8b27cd592f1015edf56a8c5b26276ee18a482428e7c5726", Language::ChineseSimplified),
            ("417b26b3d8500a4ae3d59717d7011952db6fc2fb84b807f3f94ac734e89c1b5f", Language::ChineseTraditional),
            ("ebc3959ab7801a1df6bac4fa7d970652f1df76b683cd2f4003c941c63d517e59", Language::French),
            ("d392c49fdb700a24cd1fceb237c1f65dcc128f6b34a8aacb58b59384b5c648c2", Language::Italian),
            ("7e80e161c3e93d9554c2efb78d4e3cebf8fc727e9c52e03b83b94406bdcc95fc", Language::Czech),
            ("2685e9c194c82ae67e10ba59d9ea5345a23dc093e92276fc5361f6667d79cd3f", Language::Portuguese),
        ];

        for (hash, language) in checksum {
            let wordlist = language.wordlist();
            let mut digest = sha256::Hash::engine();
            for word in wordlist {
                digest.input(word.as_bytes());
                digest.input(b"\n");
            }

            assert_eq!(sha256::Hash::from_engine(digest).to_string(), hash);
        }
    }
}
