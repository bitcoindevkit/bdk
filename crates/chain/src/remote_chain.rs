use crate::{tracker::LastSeenBlock, Append, BlockId, ChainOracle};

pub struct RemoteChain<O> {
    oracle: O,
    last_seen: Option<BlockId>,
}

impl<O> LastSeenBlock for RemoteChain<O> {
    fn last_seen_block(&self) -> Option<BlockId> {
        self.last_seen
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

impl<O> RemoteChain<O> {
    pub fn new(oracle: O) -> Self {
        Self {
            oracle,
            last_seen: None,
        }
    }

    pub fn inner(&self) -> &O {
        &self.oracle
    }

    pub fn last_seen_block(&self) -> Option<BlockId> {
        self.last_seen
    }

    pub fn update_last_seen_block(&mut self, last_seen_block: BlockId) -> ChangeSet {
        let update = match self.last_seen {
            Some(original_ls) => {
                original_ls.height < last_seen_block.height || original_ls == last_seen_block
            }
            None => true,
        };
        if update {
            let changeset = Some(last_seen_block);
            self.last_seen = changeset;
            changeset
        } else {
            None
        }
    }

    pub fn apply_changeset(&mut self, changeset: ChangeSet) {
        Append::append(&mut self.last_seen, changeset)
    }
}

pub type ChangeSet = Option<BlockId>;

impl Append for ChangeSet {
    fn append(&mut self, other: Self) {
        if *self != other && self.map(|b| b.height) <= other.map(|b| b.height) {
            *self = other;
        }
    }
}
