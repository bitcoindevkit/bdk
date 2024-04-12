mod tx_template;
#[allow(unused_imports)]
pub use tx_template::*;

#[allow(unused_macros)]
macro_rules! block_id {
    ($height:expr, $hash:literal) => {{
        bdk_chain::BlockId {
            height: $height,
            hash: bitcoin::hashes::Hash::hash($hash.as_bytes()),
        }
    }};
}

#[allow(unused_macros)]
macro_rules! h {
    ($index:literal) => {{
        bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

#[allow(unused_macros)]
macro_rules! local_chain {
    [ $(($height:expr, $block_hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $block_hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
    }};
}

#[allow(unused_macros)]
macro_rules! chain_update {
    [ $(($height:expr, $hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::Update {
            tip: bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $hash).into()),*].into_iter().collect())
                .expect("chain must have genesis block")
                .tip(),
            introduce_older_blocks: true,
        }
    }};
}

#[allow(unused_macros)]
macro_rules! changeset {
    (checkpoints: $($tail:tt)*) => { changeset!(index: TxHeight, checkpoints: $($tail)*) };
    (
        index: $ind:ty,
        checkpoints: [ $(( $height:expr, $cp_to:expr )),* ]
        $(,txids: [ $(( $txid:expr, $tx_to:expr )),* ])?
    ) => {{
        use bdk_chain::collections::BTreeMap;

        #[allow(unused_mut)]
        bdk_chain::sparse_chain::ChangeSet::<$ind> {
            checkpoints: {
                let mut changes = BTreeMap::default();
                $(changes.insert($height, $cp_to);)*
                changes
            },
            txids: {
                let mut changes = BTreeMap::default();
                $($(changes.insert($txid, $tx_to.map(|h: TxHeight| h.into()));)*)?
                changes
            }
        }
    }};
}

#[allow(unused)]
pub fn new_tx(lt: u32) -> bitcoin::Transaction {
    bitcoin::Transaction {
        version: bitcoin::transaction::Version::non_standard(0x00),
        lock_time: bitcoin::absolute::LockTime::from_consensus(lt),
        input: vec![],
        output: vec![],
    }
}
