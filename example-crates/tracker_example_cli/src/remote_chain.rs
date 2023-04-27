use bdk_chain::{Append, BlockId, ChainOracle};

pub type RemoteChainChangeSet = Option<u32>;

/// Contains a remote best-chain representation alongside the last-seen block's height.
///
/// The last-seen block height is persisted locally and can be used to determine which height to
/// start syncing from for block-by-block chain sources.
pub struct RemoteChain<O> {
    oracle: O,
    last_seen_height: Option<u32>,
}

impl<O> RemoteChain<O> {
    pub fn new(oracle: O) -> Self {
        Self {
            oracle,
            last_seen_height: None,
        }
    }

    pub fn inner(&self) -> &O {
        &self.oracle
    }

    pub fn last_seen_height(&self) -> Option<u32> {
        self.last_seen_height
    }

    pub fn update_last_seen_height(
        &mut self,
        last_seen_height: Option<u32>,
    ) -> RemoteChainChangeSet {
        if self.last_seen_height < last_seen_height {
            self.last_seen_height = last_seen_height;
            last_seen_height
        } else {
            None
        }
    }

    pub fn apply_changeset(&mut self, changeset: RemoteChainChangeSet) {
        Append::append(&mut self.last_seen_height, changeset)
    }
}

impl<O: ChainOracle> ChainOracle for RemoteChain<O> {
    type Error = O::Error;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        static_block: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        self.oracle.is_block_in_chain(block, static_block)
    }
}
