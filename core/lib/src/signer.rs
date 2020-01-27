use bitcoin::util::bip32::{DerivationPath, Fingerprint};
use bitcoin::{PublicKey, Script, SigHashType};

use miniscript::miniscript::satisfy::BitcoinSig;

use crate::error::Error;

pub trait Signer {
    fn sig_legacy_from_fingerprint(
        &self,
        index: usize,
        sighash: SigHashType,
        fingerprint: &Fingerprint,
        path: &DerivationPath,
        script: &Script,
    ) -> Result<Option<BitcoinSig>, Error>;
    fn sig_legacy_from_pubkey(
        &self,
        index: usize,
        sighash: SigHashType,
        public_key: &PublicKey,
        script: &Script,
    ) -> Result<Option<BitcoinSig>, Error>;

    fn sig_segwit_from_fingerprint(
        &self,
        index: usize,
        sighash: SigHashType,
        fingerprint: &Fingerprint,
        path: &DerivationPath,
        script: &Script,
        value: u64,
    ) -> Result<Option<BitcoinSig>, Error>;
    fn sig_segwit_from_pubkey(
        &self,
        index: usize,
        sighash: SigHashType,
        public_key: &PublicKey,
        script: &Script,
        value: u64,
    ) -> Result<Option<BitcoinSig>, Error>;
}

#[allow(dead_code)]
impl dyn Signer {
    fn sig_legacy_from_fingerprint(
        &self,
        _index: usize,
        _sighash: SigHashType,
        _fingerprint: &Fingerprint,
        _path: &DerivationPath,
        _script: &Script,
    ) -> Result<Option<BitcoinSig>, Error> {
        Ok(None)
    }
    fn sig_legacy_from_pubkey(
        &self,
        _index: usize,
        _sighash: SigHashType,
        _public_key: &PublicKey,
        _script: &Script,
    ) -> Result<Option<BitcoinSig>, Error> {
        Ok(None)
    }

    fn sig_segwit_from_fingerprint(
        &self,
        _index: usize,
        _sighash: SigHashType,
        _fingerprint: &Fingerprint,
        _path: &DerivationPath,
        _script: &Script,
        _value: u64,
    ) -> Result<Option<BitcoinSig>, Error> {
        Ok(None)
    }
    fn sig_segwit_from_pubkey(
        &self,
        _index: usize,
        _sighash: SigHashType,
        _public_key: &PublicKey,
        _script: &Script,
        _value: u64,
    ) -> Result<Option<BitcoinSig>, Error> {
        Ok(None)
    }
}
