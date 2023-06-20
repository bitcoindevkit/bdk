use std::collections::HashSet;

use bdk_chain::{
    bitcoin::{Block, Transaction, Txid},
    keychain::LocalUpdate,
    local_chain::CheckPoint,
    BlockId, ConfirmationHeightAnchor, ConfirmationTimeAnchor, TxGraph,
};
pub use bitcoincore_rpc;
use bitcoincore_rpc::{bitcoincore_rpc_json::GetBlockResult, Client, RpcApi};

#[derive(Debug, Clone)]
pub enum BitcoindRpcItem {
    Block {
        cp: CheckPoint,
        info: Box<GetBlockResult>,
        block: Box<Block>,
    },
    Mempool {
        cp: CheckPoint,
        txs: Vec<(Transaction, u64)>,
    },
}

pub fn confirmation_height_anchor(
    info: &GetBlockResult,
    _txid: Txid,
    _tx_pos: usize,
) -> ConfirmationHeightAnchor {
    ConfirmationHeightAnchor {
        anchor_block: BlockId {
            height: info.height as _,
            hash: info.hash,
        },
        confirmation_height: info.height as _,
    }
}

pub fn confirmation_time_anchor(
    info: &GetBlockResult,
    _txid: Txid,
    _tx_pos: usize,
) -> ConfirmationTimeAnchor {
    ConfirmationTimeAnchor {
        anchor_block: BlockId {
            height: info.height as _,
            hash: info.hash,
        },
        confirmation_height: info.height as _,
        confirmation_time: info.time as _,
    }
}

impl BitcoindRpcItem {
    pub fn is_mempool(&self) -> bool {
        matches!(self, Self::Mempool { .. })
    }

    pub fn into_update<K, A, F>(self, anchor: F) -> LocalUpdate<K, A>
    where
        A: Clone + Ord + PartialOrd,
        F: Fn(&GetBlockResult, Txid, usize) -> A,
    {
        match self {
            BitcoindRpcItem::Block { cp, info, block } => LocalUpdate {
                graph: {
                    let mut g = TxGraph::<A>::new(block.txdata);
                    for (tx_pos, &txid) in info.tx.iter().enumerate() {
                        let _ = g.insert_anchor(txid, anchor(&info, txid, tx_pos));
                    }
                    g
                },
                ..LocalUpdate::new(cp)
            },
            BitcoindRpcItem::Mempool { cp, txs } => LocalUpdate {
                graph: {
                    let mut last_seens = Vec::<(Txid, u64)>::with_capacity(txs.len());
                    let mut g = TxGraph::<A>::new(txs.into_iter().map(|(tx, last_seen)| {
                        last_seens.push((tx.txid(), last_seen));
                        tx
                    }));
                    for (txid, seen_at) in last_seens {
                        let _ = g.insert_seen_at(txid, seen_at);
                    }
                    g
                },
                ..LocalUpdate::new(cp)
            },
        }
    }
}

pub struct BitcoindRpcIter<'a> {
    client: &'a Client,
    fallback_height: u32,

    last_cp: Option<CheckPoint>,
    last_info: Option<GetBlockResult>,

    seen_txids: HashSet<Txid>,
}

impl<'a> Iterator for BitcoindRpcIter<'a> {
    type Item = Result<BitcoindRpcItem, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_emission().transpose()
    }
}

impl<'a> BitcoindRpcIter<'a> {
    pub fn new(client: &'a Client, fallback_height: u32, last_cp: Option<CheckPoint>) -> Self {
        Self {
            client,
            fallback_height,
            last_cp,
            last_info: None,
            seen_txids: HashSet::new(),
        }
    }

    fn next_emission(&mut self) -> Result<Option<BitcoindRpcItem>, bitcoincore_rpc::Error> {
        let client = self.client;

        'main_loop: loop {
            match (&mut self.last_cp, &mut self.last_info) {
                (last_cp @ None, last_info @ None) => {
                    // get first item at fallback_height
                    let info = client
                        .get_block_info(&client.get_block_hash(self.fallback_height as _)?)?;
                    let block = self.client.get_block(&info.hash)?;
                    let cp = CheckPoint::new(BlockId {
                        height: info.height as _,
                        hash: info.hash,
                    });
                    *last_info = Some(info.clone());
                    *last_cp = Some(cp.clone());
                    return Ok(Some(BitcoindRpcItem::Block {
                        cp,
                        info: Box::new(info),
                        block: Box::new(block),
                    }));
                }
                (last_cp @ Some(_), last_info @ None) => {
                    'cp_loop: for cp in last_cp.clone().iter().flat_map(CheckPoint::iter) {
                        let cp_block = cp.block_id();

                        let info = client.get_block_info(&cp_block.hash)?;
                        if info.confirmations < 0 {
                            // block is not in the main chain
                            continue 'cp_loop;
                        }

                        // agreement
                        *last_cp = Some(cp);
                        *last_info = Some(info);
                        continue 'main_loop;
                    }

                    // no point of agreement found
                    // next loop will emit block @ fallback height
                    *last_cp = None;
                    *last_info = None;
                }
                (Some(last_cp), last_info @ Some(_)) => {
                    // find next block
                    match last_info.as_ref().unwrap().nextblockhash {
                        Some(next_hash) => {
                            let info = self.client.get_block_info(&next_hash)?;

                            if info.confirmations < 0 {
                                *last_info = None;
                                continue 'main_loop;
                            }

                            let block = self.client.get_block(&info.hash)?;
                            let cp = last_cp
                                .clone()
                                .extend(BlockId {
                                    height: info.height as _,
                                    hash: info.hash,
                                })
                                .expect("must extend from checkpoint");

                            *last_cp = cp.clone();
                            *last_info = Some(info.clone());

                            return Ok(Some(BitcoindRpcItem::Block {
                                cp,
                                info: Box::new(info),
                                block: Box::new(block),
                            }));
                        }
                        None => {
                            // emit from mempool!
                            let mempool_txs = client
                                .get_raw_mempool()?
                                .into_iter()
                                .filter(|&txid| self.seen_txids.insert(txid))
                                .map(
                                    |txid| -> Result<(Transaction, u64), bitcoincore_rpc::Error> {
                                        let first_seen = client
                                            .get_mempool_entry(&txid)
                                            .map(|entry| entry.time)?;
                                        let tx = client.get_raw_transaction(&txid, None)?;
                                        Ok((tx, first_seen))
                                    },
                                )
                                .collect::<Result<Vec<_>, _>>()?;

                            // remove last info...
                            *last_info = None;

                            return Ok(Some(BitcoindRpcItem::Mempool {
                                txs: mempool_txs,
                                cp: last_cp.clone(),
                            }));
                        }
                    }
                }
                (None, Some(info)) => unreachable!("got info with no checkpoint? info={:#?}", info),
            }
        }
    }
}

pub trait BitcoindRpcErrorExt {
    fn is_not_found_error(&self) -> bool;
}

impl BitcoindRpcErrorExt for bitcoincore_rpc::Error {
    fn is_not_found_error(&self) -> bool {
        if let bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(rpc_err)) = self
        {
            rpc_err.code == -5
        } else {
            false
        }
    }
}
