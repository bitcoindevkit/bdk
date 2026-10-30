//! Shared test utilities: macros, helper functions, and test-vector constants.
//!
//! These helpers back BDK's own tests and downstream crates testing against a regtest
//! environment such as [`TestEnv`](crate::TestEnv). They are not intended for production use.
//!
//! - Macros: [`block_id!`](crate::block_id), [`hash!`](crate::hash),
//!   [`local_chain!`](crate::local_chain), [`chain_update!`](crate::chain_update)
//! - Functions: [`new_tx`], [`genesis_block_id`], [`tip_block_id`], [`spk_at_index`]
//! - Constants: [`DESCRIPTORS`]
//!
//! # Warning
//!
//! **Never use key material from this module on mainnet or with real funds.**
//! [`DESCRIPTORS`] embeds publicly-known extended private keys.
use core::str::FromStr;

#[cfg(feature = "miniscript")]
use bdk_chain::bitcoin::{key::Secp256k1, ScriptBuf};

#[cfg(feature = "miniscript")]
use bdk_chain::miniscript::{descriptor::KeyMap, Descriptor, DescriptorPublicKey};
use bdk_chain::{bitcoin, BlockId};
use bitcoin::{
    absolute, absolute::LockTime, constants, hashes::Hash, transaction, transaction::Version,
    Address, Amount, BlockHash, Network, OutPoint, Transaction, TxIn, TxOut, Txid,
};

/// Constructs a [`BlockId`](bdk_chain::BlockId) from a height and a label.
///
/// The label is a string literal whose *bytes are hashed* to produce a deterministic,
/// readable dummy block hash.
///
/// # Examples
///
/// ```rust
/// use bdk_testenv::block_id;
///
/// let block = block_id!(1, "B");
/// assert_eq!(block.height, 1);
/// ```
#[macro_export]
macro_rules! block_id {
    ($height:expr, $hash:literal) => {{
        bdk_chain::BlockId {
            height: $height,
            hash: bitcoin::hashes::Hash::hash($hash.as_bytes()),
        }
    }};
}

/// Hashes a string literal into a hash type (e.g. [`BlockHash`], [`Txid`](bitcoin::Txid)).
///
/// The literal's bytes are hashed, giving a deterministic value from a readable label.
///
/// # Examples
///
/// ```rust
/// use bdk_testenv::hash;
/// use bitcoin::BlockHash;
///
/// let a: BlockHash = hash!("block1");
/// let b: BlockHash = hash!("block1");
/// assert_eq!(a, b, "the same label always hashes the same");
/// ```
#[macro_export]
macro_rules! hash {
    ($index:literal) => {{
        bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

/// Builds a [`LocalChain`](bdk_chain::local_chain::LocalChain) from `(height, hash)` pairs.
///
/// The list must include the genesis block at height `0`, otherwise this panics.
///
/// # Examples
///
/// ```rust
/// use bdk_chain::local_chain::LocalChain;
/// use bdk_testenv::{hash, local_chain};
/// use bitcoin::BlockHash;
///
/// let chain: LocalChain<BlockHash> = local_chain![(0, hash!("genesis")), (1, hash!("block1"))];
/// assert_eq!(chain.tip().block_id().height, 1);
/// ```
#[macro_export]
macro_rules! local_chain {
    [ $(($height:expr, $hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
    }};
}

/// Builds a [`LocalChain`](bdk_chain::local_chain::LocalChain) from `(height, hash)` pairs
/// and returns its tip checkpoint.
///
/// Handy for constructing chain updates in tests. As with [`local_chain!`], the list must
/// include the genesis block at height `0`.
///
/// # Examples
///
/// ```rust
/// use bdk_chain::local_chain::CheckPoint;
/// use bdk_testenv::{chain_update, hash};
/// use bitcoin::BlockHash;
///
/// let update: CheckPoint<BlockHash> = chain_update![(0, hash!("genesis")), (1, hash!("block1"))];
/// assert_eq!(update.block_id().height, 1);
/// ```
#[macro_export]
macro_rules! chain_update {
    [ $(($height:expr, $hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
            .tip()
    }};
}

/// Creates a dummy [`Transaction`] whose only distinguishing feature is its lock time.
///
/// The transaction has no inputs and no outputs, so varying `lt` is enough to produce
/// distinct txids for tests that just need several unrelated transactions. Callers that
/// need a spendable output set `output` themselves via struct-update syntax.
///
/// # Examples
///
/// ```rust
/// use bdk_testenv::utils::new_tx;
///
/// assert_ne!(new_tx(0).compute_txid(), new_tx(1).compute_txid());
/// ```
pub fn new_tx(lt: u32) -> Transaction {
    Transaction {
        version: Version::non_standard(0),
        lock_time: LockTime::from_consensus(lt),
        input: vec![],
        output: vec![],
    }
}

/// Utility function to create a [`TxOut`] given amount (in satoshis) and address.
pub fn create_txout(sats: u64, addr: &str) -> TxOut {
    TxOut {
        value: Amount::from_sat(sats),
        script_pubkey: Address::from_str(addr)
            .unwrap()
            .assume_checked()
            .script_pubkey(),
    }
}

/// Utility function to create a transaction given txids, vouts of inputs and amounts (in satoshis),
/// addresses of outputs.
///
/// The locktime should be in the form given to `OP_CHECKLOCKTIMEVERIFY`.
pub fn create_test_tx<'a>(
    txids: impl IntoIterator<Item = Txid>,
    vouts: impl IntoIterator<Item = u32>,
    amounts: impl IntoIterator<Item = u64>,
    addrs: impl IntoIterator<Item = &'a str>,
    version: transaction::Version,
    locktime: u32,
) -> Transaction {
    let input_vec = core::iter::zip(txids, vouts)
        .map(|(txid, vout)| TxIn {
            previous_output: OutPoint::new(txid, vout),
            ..TxIn::default()
        })
        .collect();
    let output_vec = core::iter::zip(amounts, addrs)
        .map(|(amount, addr)| create_txout(amount, addr))
        .collect();
    let lock_time = absolute::LockTime::from_consensus(locktime);
    Transaction {
        version,
        lock_time,
        input: input_vec,
        output: output_vec,
    }
}

/// Returns the [`BlockId`] of the **regtest** genesis block.
///
/// The hash is Bitcoin's real regtest genesis hash, so chains built from this agree with
/// a `bitcoind -regtest` node. It does not match mainnet, testnet, or signet genesis.
///
/// # Examples
///
/// ```rust
/// use bdk_testenv::utils::genesis_block_id;
///
/// assert_eq!(genesis_block_id().height, 0);
/// ```
pub fn genesis_block_id() -> BlockId {
    BlockId {
        height: 0,
        hash: constants::genesis_block(Network::Regtest).block_hash(),
    }
}

/// Returns a placeholder [`BlockId`] at `height`, with an all-zero block hash.
///
/// # Examples
///
/// ```rust
/// use bdk_testenv::utils::tip_block_id;
///
/// assert_eq!(tip_block_id(42).height, 42);
/// ```
pub fn tip_block_id(height: u32) -> BlockId {
    BlockId {
        height,
        hash: BlockHash::all_zeros(),
    }
}

/// Derives the [`ScriptBuf`] (scriptPubkey) of `descriptor` at the given derivation index.
///
/// # Panics
///
/// Panics if the descriptor cannot be derived at `index`, likely when `index`
/// is hardened (`>= 2^31`), which a public descriptor cannot derive.
///
/// # Examples
///
/// ```rust
/// use bdk_chain::miniscript::Descriptor;
/// use bdk_testenv::utils::{spk_at_index, DESCRIPTORS};
///
/// let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
/// let (descriptor, _) = Descriptor::parse_descriptor(&secp, DESCRIPTORS[0]).unwrap();
///
/// assert_ne!(spk_at_index(&descriptor, 0), spk_at_index(&descriptor, 1));
/// ```
#[cfg(feature = "miniscript")]
pub fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

/// Parses a descriptor string.
#[cfg(feature = "miniscript")]
pub fn parse_descriptor(descriptor: &str) -> (Descriptor<DescriptorPublicKey>, KeyMap) {
    use bdk_chain::bitcoin::key::Secp256k1;
    let secp = Secp256k1::signing_only();
    Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, descriptor).unwrap()
}

#[allow(unused)]
/// Descriptors used by BDK's own tests.
///
/// # Warning
///
/// **Never use these on mainnet or with real funds.**
///
/// These embed extended private keys (`xprv`/`tprv`) that are well-known public test vectors,
/// anyone can derive the same keys and spend from them. Any funds sent to an address derived from
/// these descriptors can be swept by anyone.
pub const DESCRIPTORS: [&str; 7] = [
    "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)",
    "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)",
    "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0/*)",
    "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)",
    "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/1/*)",
    "wpkh(xprv9s21ZrQH143K4EXURwMHuLS469fFzZyXk7UUpdKfQwhoHcAiYTakpe8pMU2RiEdvrU9McyuE7YDoKcXkoAwEGoK53WBDnKKv2zZbb9BzttX/1/0/*)",
    // non-wildcard
    "wpkh([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/0)",
];
