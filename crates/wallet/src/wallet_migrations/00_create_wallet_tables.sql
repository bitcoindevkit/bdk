-- Create bdk_txs table
CREATE TABLE bdk_wallet ( \
                id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
                descriptor TEXT, \
                change_descriptor TEXT, \
                network TEXT \
            )