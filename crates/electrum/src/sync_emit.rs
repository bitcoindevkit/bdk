use bdk_core::{
    spk_client::{FullScanRequestEventSink, SyncRequestEventSink},
    ConfirmationBlockTime, TxUpdate, TxUpdateCursor,
};

/// Sink for emitting incremental [`TxUpdate`]s during sync or full scan.
pub(crate) trait PartialUpdateSink {
    fn emit_partial(&mut self, update: TxUpdate<ConfirmationBlockTime>);
    fn emit_anchors(&mut self, update: TxUpdate<ConfirmationBlockTime>);
}

impl<I, D> PartialUpdateSink for SyncRequestEventSink<'_, I, D> {
    fn emit_partial(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        self.emit_partial_update(update);
    }

    fn emit_anchors(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        self.emit_anchors_resolved(update);
    }
}

impl<K: Ord + Clone, D> PartialUpdateSink for FullScanRequestEventSink<'_, K, D> {
    fn emit_partial(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        self.emit_partial_update(update);
    }

    fn emit_anchors(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        self.emit_anchors_resolved(update);
    }
}

pub(crate) fn drain_and_emit_partial(
    sink: &mut dyn PartialUpdateSink,
    cursor: &mut TxUpdateCursor<ConfirmationBlockTime>,
    tx_update: &mut TxUpdate<ConfirmationBlockTime>,
) {
    let delta = tx_update.drain_since(cursor);
    sink.emit_partial(delta);
}

pub(crate) fn drain_and_emit_anchors(
    sink: &mut dyn PartialUpdateSink,
    cursor: &mut TxUpdateCursor<ConfirmationBlockTime>,
    tx_update: &mut TxUpdate<ConfirmationBlockTime>,
) {
    let delta = tx_update.drain_since(cursor);
    sink.emit_anchors(delta);
}

pub(crate) fn maybe_drain_and_emit_partial(
    sink: &mut Option<&mut dyn PartialUpdateSink>,
    cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
    tx_update: &mut TxUpdate<ConfirmationBlockTime>,
) {
    if let (Some(sink), Some(cursor)) = (sink.as_deref_mut(), cursor.as_mut()) {
        drain_and_emit_partial(sink, cursor, tx_update);
    }
}

pub(crate) fn maybe_drain_and_emit_anchors(
    sink: &mut Option<&mut dyn PartialUpdateSink>,
    cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
    tx_update: &mut TxUpdate<ConfirmationBlockTime>,
) {
    if let (Some(sink), Some(cursor)) = (sink.as_deref_mut(), cursor.as_mut()) {
        drain_and_emit_anchors(sink, cursor, tx_update);
    }
}
