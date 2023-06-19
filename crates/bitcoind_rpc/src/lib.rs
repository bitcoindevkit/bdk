use std::collections::HashSet;

use bdk_chain::{
    bitcoin::{Transaction, Txid},
    local_chain::CheckPoint,
    BlockId,
};
use bitcoincore_rpc::{bitcoincore_rpc_json::GetBlockResult, Client, RpcApi};

#[derive(Debug, Clone)]
pub enum BitcoindRpcItem {
    Block {
        cp: CheckPoint,
        info: Box<GetBlockResult>,
    },
    Mempool {
        cp: CheckPoint,
        txs: Vec<(Transaction, u64)>,
    },
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
                    let cp = CheckPoint::new(BlockId {
                        height: info.height as _,
                        hash: info.hash,
                    });
                    *last_info = Some(info.clone());
                    *last_cp = Some(cp.clone());
                    return Ok(Some(BitcoindRpcItem::Block {
                        cp,
                        info: Box::new(info),
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
                        // next loop
                        *last_cp = Some(cp);
                        *last_info = Some(info);
                    }

                    // no point of agreement found
                    // next loop will emit block @ fallback height
                    *last_cp = None;
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

                            let cp = CheckPoint::new_with_prev(
                                BlockId {
                                    height: info.height as _,
                                    hash: info.hash,
                                },
                                Some(last_cp.clone()),
                            )
                            .expect("must create valid checkpoint");

                            *last_cp = cp.clone();
                            *last_info = Some(info.clone());

                            return Ok(Some(BitcoindRpcItem::Block {
                                cp,
                                info: Box::new(info),
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
                (None, Some(_)) => unreachable!(),
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
