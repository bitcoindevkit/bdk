use bdk_core::{
    spk_client::{FullScanRequest, SyncRequest},
    ConfirmationBlockTime, TxUpdate, TxUpdateCursor,
};

/// Emit partial [`TxUpdate`]s during Esplora fetch loops.
pub(crate) trait EmitPartial {
    fn emit_partial(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    );
}

impl<I, D> EmitPartial for SyncRequest<I, D> {
    fn emit_partial(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) {
        self.try_emit_partial_update(cursor, tx_update);
    }
}

impl<K: Ord + Clone, D> EmitPartial for FullScanRequest<K, D> {
    fn emit_partial(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) {
        self.try_emit_partial_update(cursor, tx_update);
    }
}
