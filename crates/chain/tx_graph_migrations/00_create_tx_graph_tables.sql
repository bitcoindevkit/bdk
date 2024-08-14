-- Create bdk_txs table
CREATE TABLE bdk_txs (
    txid TEXT PRIMARY KEY NOT NULL,
    raw_tx BYTEA,
    last_seen BIGINT
);

-- Create bdk_txouts table
CREATE TABLE bdk_txouts (
    txid TEXT NOT NULL,
    vout INTEGER NOT NULL,
    value BIGINT NOT NULL,
    script BYTEA NOT NULL,
    PRIMARY KEY (txid, vout)
);

-- Create bdk_anchors table
CREATE TABLE bdk_anchors (
    txid TEXT NOT NULL REFERENCES bdk_txs (txid),
    block_height INTEGER NOT NULL,
    block_hash TEXT NOT NULL,
    anchor JSONB NOT NULL,
    PRIMARY KEY (txid, block_height, block_hash)
);