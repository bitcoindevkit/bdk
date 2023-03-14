#[allow(unused_macros)]
macro_rules! h {
    ($index:literal) => {{
        bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

#[allow(unused_macros)]
macro_rules! chain {
    ($([$($tt:tt)*]),*) => { chain!( checkpoints: [$([$($tt)*]),*] ) };
    (checkpoints: $($tail:tt)*) => { chain!( index: TxHeight, checkpoints: $($tail)*) };
    (index: $ind:ty, checkpoints: [ $([$height:expr, $block_hash:expr]),* ] $(,txids: [$(($txid:expr, $tx_height:expr)),*])?) => {{
        #[allow(unused_mut)]
        let mut chain = bdk_chain::sparse_chain::SparseChain::<$ind>::from_checkpoints([$(($height, $block_hash).into()),*]);

        $(
            $(
                let _ = chain.insert_tx($txid, $tx_height).expect("should succeed");
            )*
        )?

        chain
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
        version: 0x00,
        lock_time: bitcoin::PackedLockTime(lt),
        input: vec![],
        output: vec![],
    }
}
