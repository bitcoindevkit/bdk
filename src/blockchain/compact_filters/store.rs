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

use std::convert::TryInto;
use std::fmt;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::RwLock;

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

use rocksdb::{Direction, IteratorMode, ReadOptions, WriteBatch, DB};

use bitcoin::consensus::{deserialize, encode::VarInt, serialize, Decodable, Encodable};
use bitcoin::hash_types::{FilterHash, FilterHeader};
use bitcoin::hashes::hex::FromHex;
use bitcoin::hashes::Hash;
use bitcoin::util::bip158::BlockFilter;
use bitcoin::util::uint::Uint256;
use bitcoin::Block;
use bitcoin::BlockHash;
use bitcoin::BlockHeader;
use bitcoin::Network;

use lazy_static::lazy_static;

use super::CompactFiltersError;

lazy_static! {
    static ref MAINNET_GENESIS: Block = deserialize(&Vec::<u8>::from_hex("0100000000000000000000000000000000000000000000000000000000000000000000003BA3EDFD7A7B12B27AC72C3E67768F617FC81BC3888A51323A9FB8AA4B1E5E4A29AB5F49FFFF001D1DAC2B7C0101000000010000000000000000000000000000000000000000000000000000000000000000FFFFFFFF4D04FFFF001D0104455468652054696D65732030332F4A616E2F32303039204368616E63656C6C6F72206F6E206272696E6B206F66207365636F6E64206261696C6F757420666F722062616E6B73FFFFFFFF0100F2052A01000000434104678AFDB0FE5548271967F1A67130B7105CD6A828E03909A67962E0EA1F61DEB649F6BC3F4CEF38C4F35504E51EC112DE5C384DF7BA0B8D578A4C702B6BF11D5FAC00000000").unwrap()).unwrap();
    static ref TESTNET_GENESIS: Block = deserialize(&Vec::<u8>::from_hex("0100000000000000000000000000000000000000000000000000000000000000000000003BA3EDFD7A7B12B27AC72C3E67768F617FC81BC3888A51323A9FB8AA4B1E5E4ADAE5494DFFFF001D1AA4AE180101000000010000000000000000000000000000000000000000000000000000000000000000FFFFFFFF4D04FFFF001D0104455468652054696D65732030332F4A616E2F32303039204368616E63656C6C6F72206F6E206272696E6B206F66207365636F6E64206261696C6F757420666F722062616E6B73FFFFFFFF0100F2052A01000000434104678AFDB0FE5548271967F1A67130B7105CD6A828E03909A67962E0EA1F61DEB649F6BC3F4CEF38C4F35504E51EC112DE5C384DF7BA0B8D578A4C702B6BF11D5FAC00000000").unwrap()).unwrap();
    static ref REGTEST_GENESIS: Block = deserialize(&Vec::<u8>::from_hex("0100000000000000000000000000000000000000000000000000000000000000000000003BA3EDFD7A7B12B27AC72C3E67768F617FC81BC3888A51323A9FB8AA4B1E5E4ADAE5494DFFFF7F20020000000101000000010000000000000000000000000000000000000000000000000000000000000000FFFFFFFF4D04FFFF001D0104455468652054696D65732030332F4A616E2F32303039204368616E63656C6C6F72206F6E206272696E6B206F66207365636F6E64206261696C6F757420666F722062616E6B73FFFFFFFF0100F2052A01000000434104678AFDB0FE5548271967F1A67130B7105CD6A828E03909A67962E0EA1F61DEB649F6BC3F4CEF38C4F35504E51EC112DE5C384DF7BA0B8D578A4C702B6BF11D5FAC00000000").unwrap()).unwrap();
    static ref SIGNET_GENESIS: Block = deserialize(&Vec::<u8>::from_hex("0100000000000000000000000000000000000000000000000000000000000000000000003BA3EDFD7A7B12B27AC72C3E67768F617FC81BC3888A51323A9FB8AA4B1E5E4A008F4D5FAE77031E8AD222030101000000010000000000000000000000000000000000000000000000000000000000000000FFFFFFFF4D04FFFF001D0104455468652054696D65732030332F4A616E2F32303039204368616E63656C6C6F72206F6E206272696E6B206F66207365636F6E64206261696C6F757420666F722062616E6B73FFFFFFFF0100F2052A01000000434104678AFDB0FE5548271967F1A67130B7105CD6A828E03909A67962E0EA1F61DEB649F6BC3F4CEF38C4F35504E51EC112DE5C384DF7BA0B8D578A4C702B6BF11D5FAC00000000").unwrap()).unwrap();
}

pub trait StoreType: Default + fmt::Debug {}

#[derive(Default, Debug)]
pub struct Full;
impl StoreType for Full {}
#[derive(Default, Debug)]
pub struct Snapshot;
impl StoreType for Snapshot {}

pub enum StoreEntry {
    BlockHeader(Option<usize>),
    Block(Option<usize>),
    BlockHeaderIndex(Option<BlockHash>),
    CFilterTable((u8, Option<usize>)),
}

impl StoreEntry {
    pub fn get_prefix(&self) -> Vec<u8> {
        match self {
            StoreEntry::BlockHeader(_) => b"z",
            StoreEntry::Block(_) => b"x",
            StoreEntry::BlockHeaderIndex(_) => b"i",
            StoreEntry::CFilterTable(_) => b"t",
        }
        .to_vec()
    }

    pub fn get_key(&self) -> Vec<u8> {
        let mut prefix = self.get_prefix();
        match self {
            StoreEntry::BlockHeader(Some(height)) => {
                prefix.extend_from_slice(&height.to_be_bytes())
            }
            StoreEntry::Block(Some(height)) => prefix.extend_from_slice(&height.to_be_bytes()),
            StoreEntry::BlockHeaderIndex(Some(hash)) => {
                prefix.extend_from_slice(&hash.into_inner())
            }
            StoreEntry::CFilterTable((filter_type, bundle_index)) => {
                prefix.push(*filter_type);
                if let Some(bundle_index) = bundle_index {
                    prefix.extend_from_slice(&bundle_index.to_be_bytes());
                }
            }
            _ => {}
        }

        prefix
    }
}

pub trait SerializeDb: Sized {
    fn serialize(&self) -> Vec<u8>;
    fn deserialize(data: &[u8]) -> Result<Self, CompactFiltersError>;
}

impl<T> SerializeDb for T
where
    T: Encodable + Decodable,
{
    fn serialize(&self) -> Vec<u8> {
        serialize(self)
    }

    fn deserialize(data: &[u8]) -> Result<Self, CompactFiltersError> {
        deserialize(data).map_err(|_| CompactFiltersError::DataCorruption)
    }
}

impl Encodable for BundleStatus {
    fn consensus_encode<W: Write>(&self, mut e: W) -> Result<usize, std::io::Error> {
        let mut written = 0;

        match self {
            BundleStatus::Init => {
                written += 0x00u8.consensus_encode(&mut e)?;
            }
            BundleStatus::CFHeaders { cf_headers } => {
                written += 0x01u8.consensus_encode(&mut e)?;
                written += VarInt(cf_headers.len() as u64).consensus_encode(&mut e)?;
                for header in cf_headers {
                    written += header.consensus_encode(&mut e)?;
                }
            }
            BundleStatus::CFilters { cf_filters } => {
                written += 0x02u8.consensus_encode(&mut e)?;
                written += VarInt(cf_filters.len() as u64).consensus_encode(&mut e)?;
                for filter in cf_filters {
                    written += filter.consensus_encode(&mut e)?;
                }
            }
            BundleStatus::Processed { cf_filters } => {
                written += 0x03u8.consensus_encode(&mut e)?;
                written += VarInt(cf_filters.len() as u64).consensus_encode(&mut e)?;
                for filter in cf_filters {
                    written += filter.consensus_encode(&mut e)?;
                }
            }
            BundleStatus::Pruned => {
                written += 0x04u8.consensus_encode(&mut e)?;
            }
            BundleStatus::Tip { cf_filters } => {
                written += 0x05u8.consensus_encode(&mut e)?;
                written += VarInt(cf_filters.len() as u64).consensus_encode(&mut e)?;
                for filter in cf_filters {
                    written += filter.consensus_encode(&mut e)?;
                }
            }
        }

        Ok(written)
    }
}

impl Decodable for BundleStatus {
    fn consensus_decode<D: Read>(mut d: D) -> Result<Self, bitcoin::consensus::encode::Error> {
        let byte_type = u8::consensus_decode(&mut d)?;
        match byte_type {
            0x00 => Ok(BundleStatus::Init),
            0x01 => {
                let num = VarInt::consensus_decode(&mut d)?;
                let num = num.0 as usize;

                let mut cf_headers = Vec::with_capacity(num);
                for _ in 0..num {
                    cf_headers.push(FilterHeader::consensus_decode(&mut d)?);
                }

                Ok(BundleStatus::CFHeaders { cf_headers })
            }
            0x02 => {
                let num = VarInt::consensus_decode(&mut d)?;
                let num = num.0 as usize;

                let mut cf_filters = Vec::with_capacity(num);
                for _ in 0..num {
                    cf_filters.push(Vec::<u8>::consensus_decode(&mut d)?);
                }

                Ok(BundleStatus::CFilters { cf_filters })
            }
            0x03 => {
                let num = VarInt::consensus_decode(&mut d)?;
                let num = num.0 as usize;

                let mut cf_filters = Vec::with_capacity(num);
                for _ in 0..num {
                    cf_filters.push(Vec::<u8>::consensus_decode(&mut d)?);
                }

                Ok(BundleStatus::Processed { cf_filters })
            }
            0x04 => Ok(BundleStatus::Pruned),
            0x05 => {
                let num = VarInt::consensus_decode(&mut d)?;
                let num = num.0 as usize;

                let mut cf_filters = Vec::with_capacity(num);
                for _ in 0..num {
                    cf_filters.push(Vec::<u8>::consensus_decode(&mut d)?);
                }

                Ok(BundleStatus::Tip { cf_filters })
            }
            _ => Err(bitcoin::consensus::encode::Error::ParseFailed(
                "Invalid byte type",
            )),
        }
    }
}

pub struct ChainStore<T: StoreType> {
    store: Arc<RwLock<DB>>,
    cf_name: String,
    min_height: usize,
    network: Network,
    phantom: PhantomData<T>,
}

impl ChainStore<Full> {
    pub fn new(store: DB, network: Network) -> Result<Self, CompactFiltersError> {
        let genesis = match network {
            Network::Bitcoin => MAINNET_GENESIS.deref(),
            Network::Testnet => TESTNET_GENESIS.deref(),
            Network::Regtest => REGTEST_GENESIS.deref(),
            Network::Signet => SIGNET_GENESIS.deref(),
        };

        let cf_name = "default".to_string();
        let cf_handle = store.cf_handle(&cf_name).unwrap();

        let genesis_key = StoreEntry::BlockHeader(Some(0)).get_key();

        if store.get_pinned_cf(cf_handle, &genesis_key)?.is_none() {
            let mut batch = WriteBatch::default();
            batch.put_cf(
                cf_handle,
                genesis_key,
                (genesis.header, genesis.header.work()).serialize(),
            );
            batch.put_cf(
                cf_handle,
                StoreEntry::BlockHeaderIndex(Some(genesis.block_hash())).get_key(),
                &0usize.to_be_bytes(),
            );
            store.write(batch)?;
        }

        Ok(ChainStore {
            store: Arc::new(RwLock::new(store)),
            cf_name,
            min_height: 0,
            network,
            phantom: PhantomData,
        })
    }

    pub fn get_locators(&self) -> Result<Vec<(BlockHash, usize)>, CompactFiltersError> {
        let mut step = 1;
        let mut index = self.get_height()?;
        let mut answer = Vec::new();

        let store_read = self.store.read().unwrap();
        let cf_handle = store_read.cf_handle(&self.cf_name).unwrap();

        loop {
            if answer.len() > 10 {
                step *= 2;
            }

            let (header, _): (BlockHeader, Uint256) = SerializeDb::deserialize(
                &store_read
                    .get_pinned_cf(cf_handle, StoreEntry::BlockHeader(Some(index)).get_key())?
                    .unwrap(),
            )?;
            answer.push((header.block_hash(), index));

            if let Some(new_index) = index.checked_sub(step) {
                index = new_index;
            } else {
                break;
            }
        }

        Ok(answer)
    }

    pub fn start_snapshot(&self, from: usize) -> Result<ChainStore<Snapshot>, CompactFiltersError> {
        let new_cf_name: String = thread_rng().sample_iter(&Alphanumeric).take(16).collect();
        let new_cf_name = format!("_headers:{}", new_cf_name);

        let mut write_store = self.store.write().unwrap();

        write_store.create_cf(&new_cf_name, &Default::default())?;

        let cf_handle = write_store.cf_handle(&self.cf_name).unwrap();
        let new_cf_handle = write_store.cf_handle(&new_cf_name).unwrap();

        let (header, work): (BlockHeader, Uint256) = SerializeDb::deserialize(
            &write_store
                .get_pinned_cf(cf_handle, StoreEntry::BlockHeader(Some(from)).get_key())?
                .ok_or(CompactFiltersError::DataCorruption)?,
        )?;

        let mut batch = WriteBatch::default();
        batch.put_cf(
            new_cf_handle,
            StoreEntry::BlockHeaderIndex(Some(header.block_hash())).get_key(),
            &from.to_be_bytes(),
        );
        batch.put_cf(
            new_cf_handle,
            StoreEntry::BlockHeader(Some(from)).get_key(),
            (header, work).serialize(),
        );
        write_store.write(batch)?;

        let store = Arc::clone(&self.store);
        Ok(ChainStore {
            store,
            cf_name: new_cf_name,
            min_height: from,
            network: self.network,
            phantom: PhantomData,
        })
    }

    pub fn recover_snapshot(&self, cf_name: &str) -> Result<(), CompactFiltersError> {
        let mut write_store = self.store.write().unwrap();
        let snapshot_cf_handle = write_store.cf_handle(cf_name).unwrap();

        let prefix = StoreEntry::BlockHeader(None).get_key();
        let mut iterator = write_store.prefix_iterator_cf(snapshot_cf_handle, prefix);

        let min_height = match iterator
            .next()
            .and_then(|(k, _)| k[1..].try_into().ok())
            .map(usize::from_be_bytes)
        {
            None => {
                std::mem::drop(iterator);
                write_store.drop_cf(cf_name).ok();

                return Ok(());
            }
            Some(x) => x,
        };
        std::mem::drop(iterator);
        std::mem::drop(write_store);

        let snapshot = ChainStore {
            store: Arc::clone(&self.store),
            cf_name: cf_name.into(),
            min_height,
            network: self.network,
            phantom: PhantomData,
        };
        if snapshot.work()? > self.work()? {
            self.apply_snapshot(snapshot)?;
        }

        Ok(())
    }

    pub fn apply_snapshot(
        &self,
        snaphost: ChainStore<Snapshot>,
    ) -> Result<(), CompactFiltersError> {
        let mut batch = WriteBatch::default();

        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();
        let snapshot_cf_handle = read_store.cf_handle(&snaphost.cf_name).unwrap();

        let from_key = StoreEntry::BlockHeader(Some(snaphost.min_height)).get_key();
        let to_key = StoreEntry::BlockHeader(Some(usize::MAX)).get_key();

        let mut opts = ReadOptions::default();
        opts.set_iterate_upper_bound(to_key.clone());

        log::debug!("Removing items");
        batch.delete_range_cf(cf_handle, &from_key, &to_key);
        for (_, v) in read_store.iterator_cf_opt(
            cf_handle,
            opts,
            IteratorMode::From(&from_key, Direction::Forward),
        ) {
            let (header, _): (BlockHeader, Uint256) = SerializeDb::deserialize(&v)?;

            batch.delete_cf(
                cf_handle,
                StoreEntry::BlockHeaderIndex(Some(header.block_hash())).get_key(),
            );
        }

        // Delete full blocks overriden by snapshot
        let from_key = StoreEntry::Block(Some(snaphost.min_height)).get_key();
        let to_key = StoreEntry::Block(Some(usize::MAX)).get_key();
        batch.delete_range(&from_key, &to_key);

        log::debug!("Copying over new items");
        for (k, v) in read_store.iterator_cf(snapshot_cf_handle, IteratorMode::Start) {
            batch.put_cf(cf_handle, k, v);
        }

        read_store.write(batch)?;
        std::mem::drop(read_store);

        self.store.write().unwrap().drop_cf(&snaphost.cf_name)?;

        Ok(())
    }

    pub fn get_height_for(
        &self,
        block_hash: &BlockHash,
    ) -> Result<Option<usize>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();

        let key = StoreEntry::BlockHeaderIndex(Some(*block_hash)).get_key();
        let data = read_store.get_pinned_cf(cf_handle, key)?;
        data.map(|data| {
            Ok::<_, CompactFiltersError>(usize::from_be_bytes(
                data.as_ref()
                    .try_into()
                    .map_err(|_| CompactFiltersError::DataCorruption)?,
            ))
        })
        .transpose()
    }

    pub fn get_block_hash(&self, height: usize) -> Result<Option<BlockHash>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();

        let key = StoreEntry::BlockHeader(Some(height)).get_key();
        let data = read_store.get_pinned_cf(cf_handle, key)?;
        data.map(|data| {
            let (header, _): (BlockHeader, Uint256) =
                deserialize(&data).map_err(|_| CompactFiltersError::DataCorruption)?;
            Ok::<_, CompactFiltersError>(header.block_hash())
        })
        .transpose()
    }

    pub fn save_full_block(&self, block: &Block, height: usize) -> Result<(), CompactFiltersError> {
        let key = StoreEntry::Block(Some(height)).get_key();
        self.store.read().unwrap().put(key, block.serialize())?;

        Ok(())
    }

    pub fn get_full_block(&self, height: usize) -> Result<Option<Block>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();

        let key = StoreEntry::Block(Some(height)).get_key();
        let opt_block = read_store.get_pinned(key)?;

        opt_block
            .map(|data| deserialize(&data))
            .transpose()
            .map_err(|_| CompactFiltersError::DataCorruption)
    }

    pub fn delete_blocks_until(&self, height: usize) -> Result<(), CompactFiltersError> {
        let from_key = StoreEntry::Block(Some(0)).get_key();
        let to_key = StoreEntry::Block(Some(height)).get_key();

        let mut batch = WriteBatch::default();
        batch.delete_range(&from_key, &to_key);

        self.store.read().unwrap().write(batch)?;

        Ok(())
    }

    pub fn iter_full_blocks(&self) -> Result<Vec<(usize, Block)>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();

        let prefix = StoreEntry::Block(None).get_key();

        let iterator = read_store.prefix_iterator(&prefix);
        // FIXME: we have to filter manually because rocksdb sometimes returns stuff that doesn't
        // have the right prefix
        iterator
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, v)| {
                let height: usize = usize::from_be_bytes(
                    k[1..]
                        .try_into()
                        .map_err(|_| CompactFiltersError::DataCorruption)?,
                );
                let block = SerializeDb::deserialize(&v)?;

                Ok((height, block))
            })
            .collect::<Result<_, _>>()
    }
}

impl<T: StoreType> ChainStore<T> {
    pub fn work(&self) -> Result<Uint256, CompactFiltersError> {
        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();

        let prefix = StoreEntry::BlockHeader(None).get_key();
        let iterator = read_store.prefix_iterator_cf(cf_handle, prefix);

        Ok(iterator
            .last()
            .map(|(_, v)| -> Result<_, CompactFiltersError> {
                let (_, work): (BlockHeader, Uint256) = SerializeDb::deserialize(&v)?;

                Ok(work)
            })
            .transpose()?
            .unwrap_or_default())
    }

    pub fn get_height(&self) -> Result<usize, CompactFiltersError> {
        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();

        let prefix = StoreEntry::BlockHeader(None).get_key();
        let iterator = read_store.prefix_iterator_cf(cf_handle, prefix);

        Ok(iterator
            .last()
            .map(|(k, _)| -> Result<_, CompactFiltersError> {
                let height = usize::from_be_bytes(
                    k[1..]
                        .try_into()
                        .map_err(|_| CompactFiltersError::DataCorruption)?,
                );

                Ok(height)
            })
            .transpose()?
            .unwrap_or_default())
    }

    pub fn get_tip_hash(&self) -> Result<Option<BlockHash>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();

        let prefix = StoreEntry::BlockHeader(None).get_key();
        let iterator = read_store.prefix_iterator_cf(cf_handle, prefix);

        iterator
            .last()
            .map(|(_, v)| -> Result<_, CompactFiltersError> {
                let (header, _): (BlockHeader, Uint256) = SerializeDb::deserialize(&v)?;

                Ok(header.block_hash())
            })
            .transpose()
    }

    pub fn apply(
        &mut self,
        from: usize,
        headers: Vec<BlockHeader>,
    ) -> Result<BlockHash, CompactFiltersError> {
        let mut batch = WriteBatch::default();

        let read_store = self.store.read().unwrap();
        let cf_handle = read_store.cf_handle(&self.cf_name).unwrap();

        let (mut last_hash, mut accumulated_work) = read_store
            .get_pinned_cf(cf_handle, StoreEntry::BlockHeader(Some(from)).get_key())?
            .map(|result| {
                let (header, work): (BlockHeader, Uint256) = SerializeDb::deserialize(&result)?;
                Ok::<_, CompactFiltersError>((header.block_hash(), work))
            })
            .transpose()?
            .ok_or(CompactFiltersError::DataCorruption)?;

        for (index, header) in headers.into_iter().enumerate() {
            if header.prev_blockhash != last_hash {
                return Err(CompactFiltersError::InvalidHeaders);
            }

            last_hash = header.block_hash();
            accumulated_work = accumulated_work + header.work();

            let height = from + index + 1;
            batch.put_cf(
                cf_handle,
                StoreEntry::BlockHeaderIndex(Some(header.block_hash())).get_key(),
                &(height).to_be_bytes(),
            );
            batch.put_cf(
                cf_handle,
                StoreEntry::BlockHeader(Some(height)).get_key(),
                (header, accumulated_work).serialize(),
            );
        }

        std::mem::drop(read_store);

        self.store.write().unwrap().write(batch)?;
        Ok(last_hash)
    }
}

impl<T: StoreType> fmt::Debug for ChainStore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("ChainStore<{:?}>", T::default()))
            .field("cf_name", &self.cf_name)
            .field("min_height", &self.min_height)
            .field("network", &self.network)
            .field("headers_height", &self.get_height())
            .field("tip_hash", &self.get_tip_hash())
            .finish()
    }
}

pub enum BundleStatus {
    Init,
    CFHeaders { cf_headers: Vec<FilterHeader> },
    CFilters { cf_filters: Vec<Vec<u8>> },
    Processed { cf_filters: Vec<Vec<u8>> },
    Tip { cf_filters: Vec<Vec<u8>> },
    Pruned,
}

pub struct CFStore {
    store: Arc<RwLock<DB>>,
    filter_type: u8,
}

type BundleEntry = (BundleStatus, FilterHeader);

impl CFStore {
    pub fn new(
        headers_store: &ChainStore<Full>,
        filter_type: u8,
    ) -> Result<Self, CompactFiltersError> {
        let cf_store = CFStore {
            store: Arc::clone(&headers_store.store),
            filter_type,
        };

        let genesis = match headers_store.network {
            Network::Bitcoin => MAINNET_GENESIS.deref(),
            Network::Testnet => TESTNET_GENESIS.deref(),
            Network::Regtest => REGTEST_GENESIS.deref(),
            Network::Signet => SIGNET_GENESIS.deref(),
        };

        let filter = BlockFilter::new_script_filter(genesis, |utxo| {
            Err(bitcoin::util::bip158::Error::UtxoMissing(*utxo))
        })?;
        let first_key = StoreEntry::CFilterTable((filter_type, Some(0))).get_key();

        // Add the genesis' filter
        {
            let read_store = cf_store.store.read().unwrap();
            if read_store.get_pinned(&first_key)?.is_none() {
                read_store.put(
                    &first_key,
                    (
                        BundleStatus::Init,
                        filter.filter_header(&FilterHeader::from_hash(Default::default())),
                    )
                        .serialize(),
                )?;
            }
        }

        Ok(cf_store)
    }

    pub fn get_filter_type(&self) -> u8 {
        self.filter_type
    }

    pub fn get_bundles(&self) -> Result<Vec<BundleEntry>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();

        let prefix = StoreEntry::CFilterTable((self.filter_type, None)).get_key();
        let iterator = read_store.prefix_iterator(&prefix);

        // FIXME: we have to filter manually because rocksdb sometimes returns stuff that doesn't
        // have the right prefix
        iterator
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, data)| BundleEntry::deserialize(&data))
            .collect::<Result<_, _>>()
    }

    pub fn get_checkpoints(&self) -> Result<Vec<FilterHeader>, CompactFiltersError> {
        let read_store = self.store.read().unwrap();

        let prefix = StoreEntry::CFilterTable((self.filter_type, None)).get_key();
        let iterator = read_store.prefix_iterator(&prefix);

        // FIXME: we have to filter manually because rocksdb sometimes returns stuff that doesn't
        // have the right prefix
        iterator
            .filter(|(k, _)| k.starts_with(&prefix))
            .skip(1)
            .map(|(_, data)| Ok::<_, CompactFiltersError>(BundleEntry::deserialize(&data)?.1))
            .collect::<Result<_, _>>()
    }

    pub fn replace_checkpoints(
        &self,
        checkpoints: Vec<FilterHeader>,
    ) -> Result<(), CompactFiltersError> {
        let current_checkpoints = self.get_checkpoints()?;

        let mut equal_bundles = 0;
        for (index, (our, their)) in current_checkpoints
            .iter()
            .zip(checkpoints.iter())
            .enumerate()
        {
            equal_bundles = index;

            if our != their {
                break;
            }
        }

        let read_store = self.store.read().unwrap();
        let mut batch = WriteBatch::default();

        for (index, filter_hash) in checkpoints.iter().enumerate().skip(equal_bundles) {
            let key = StoreEntry::CFilterTable((self.filter_type, Some(index + 1))).get_key(); // +1 to skip the genesis' filter

            if let Some((BundleStatus::Tip { .. }, _)) = read_store
                .get_pinned(&key)?
                .map(|data| BundleEntry::deserialize(&data))
                .transpose()?
            {
                println!("Keeping bundle #{} as Tip", index);
            } else {
                batch.put(&key, (BundleStatus::Init, *filter_hash).serialize());
            }
        }

        read_store.write(batch)?;

        Ok(())
    }

    pub fn advance_to_cf_headers(
        &self,
        bundle: usize,
        checkpoint: FilterHeader,
        filter_hashes: Vec<FilterHash>,
    ) -> Result<BundleStatus, CompactFiltersError> {
        let cf_headers: Vec<FilterHeader> = filter_hashes
            .into_iter()
            .scan(checkpoint, |prev_header, filter_hash| {
                let filter_header = filter_hash.filter_header(&prev_header);
                *prev_header = filter_header;

                Some(filter_header)
            })
            .collect();

        let read_store = self.store.read().unwrap();

        let next_key = StoreEntry::CFilterTable((self.filter_type, Some(bundle + 1))).get_key(); // +1 to skip the genesis' filter
        if let Some((_, next_checkpoint)) = read_store
            .get_pinned(&next_key)?
            .map(|data| BundleEntry::deserialize(&data))
            .transpose()?
        {
            // check connection with the next bundle if present
            if cf_headers.iter().last() != Some(&next_checkpoint) {
                return Err(CompactFiltersError::InvalidFilterHeader);
            }
        }

        let key = StoreEntry::CFilterTable((self.filter_type, Some(bundle))).get_key();
        let value = (BundleStatus::CFHeaders { cf_headers }, checkpoint);

        read_store.put(key, value.serialize())?;

        Ok(value.0)
    }

    pub fn advance_to_cf_filters(
        &self,
        bundle: usize,
        checkpoint: FilterHeader,
        headers: Vec<FilterHeader>,
        filters: Vec<(usize, Vec<u8>)>,
    ) -> Result<BundleStatus, CompactFiltersError> {
        let cf_filters = filters
            .into_iter()
            .zip(headers.into_iter())
            .scan(checkpoint, |prev_header, ((_, filter_content), header)| {
                let filter = BlockFilter::new(&filter_content);
                if header != filter.filter_header(&prev_header) {
                    return Some(Err(CompactFiltersError::InvalidFilter));
                }
                *prev_header = header;

                Some(Ok::<_, CompactFiltersError>(filter_content))
            })
            .collect::<Result<_, _>>()?;

        let key = StoreEntry::CFilterTable((self.filter_type, Some(bundle))).get_key();
        let value = (BundleStatus::CFilters { cf_filters }, checkpoint);

        let read_store = self.store.read().unwrap();
        read_store.put(key, value.serialize())?;

        Ok(value.0)
    }

    pub fn prune_filters(
        &self,
        bundle: usize,
        checkpoint: FilterHeader,
    ) -> Result<BundleStatus, CompactFiltersError> {
        let key = StoreEntry::CFilterTable((self.filter_type, Some(bundle))).get_key();
        let value = (BundleStatus::Pruned, checkpoint);

        let read_store = self.store.read().unwrap();
        read_store.put(key, value.serialize())?;

        Ok(value.0)
    }

    pub fn mark_as_tip(
        &self,
        bundle: usize,
        cf_filters: Vec<Vec<u8>>,
        checkpoint: FilterHeader,
    ) -> Result<BundleStatus, CompactFiltersError> {
        let key = StoreEntry::CFilterTable((self.filter_type, Some(bundle))).get_key();
        let value = (BundleStatus::Tip { cf_filters }, checkpoint);

        let read_store = self.store.read().unwrap();
        read_store.put(key, value.serialize())?;

        Ok(value.0)
    }
}
