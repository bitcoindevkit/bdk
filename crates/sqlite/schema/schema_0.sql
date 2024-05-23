-- schema version control
CREATE TABLE version
(
    version INTEGER
) STRICT;
INSERT INTO version
VALUES (1);

-- network is the valid network for all other table data
CREATE TABLE network
(
    name TEXT UNIQUE NOT NULL
) STRICT;

-- keychain is the json serialized keychain structure as JSONB,
-- descriptor is the complete descriptor string,
-- descriptor_id is a sha256::Hash id of the descriptor string w/o the checksum,
-- last revealed index is a u32
CREATE TABLE keychain
(
    keychain      BLOB PRIMARY KEY NOT NULL,
    descriptor    TEXT             NOT NULL,
    descriptor_id BLOB             NOT NULL,
    last_revealed INTEGER
) STRICT;

-- hash is block hash hex string,
-- block height is a u32,
CREATE TABLE block
(
    hash   TEXT PRIMARY KEY NOT NULL,
    height INTEGER          NOT NULL
) STRICT;

-- txid is transaction hash hex string (reversed)
-- whole_tx is a consensus encoded transaction,
-- last seen is a u64 unix epoch seconds
CREATE TABLE tx
(
    txid      TEXT PRIMARY KEY NOT NULL,
    whole_tx  BLOB,
    last_seen INTEGER
) STRICT;

-- Outpoint txid hash hex string (reversed)
-- Outpoint vout
-- TxOut value as SATs
-- TxOut script consensus encoded
CREATE TABLE txout
(
    txid   TEXT    NOT NULL,
    vout   INTEGER NOT NULL,
    value  INTEGER NOT NULL,
    script BLOB    NOT NULL,
    PRIMARY KEY (txid, vout)
) STRICT;

-- join table between anchor and tx
-- block hash hex string
-- anchor is a json serialized Anchor structure as JSONB,
-- txid is transaction hash hex string (reversed)
CREATE TABLE anchor_tx
(
    block_hash          TEXT NOT NULL,
    anchor              BLOB NOT NULL,
    txid                TEXT NOT NULL REFERENCES tx (txid),
    UNIQUE (anchor, txid),
    FOREIGN KEY (block_hash) REFERENCES block(hash)
) STRICT;