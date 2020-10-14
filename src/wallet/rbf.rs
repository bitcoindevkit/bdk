// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::database::Database;
use crate::error::Error;
use crate::types::*;

/// Filters unspent utxos
pub(super) fn filter_available<I: Iterator<Item = (UTXO, usize)>, D: Database>(
    database: &D,
    iter: I,
) -> Result<Vec<(UTXO, usize)>, Error> {
    Ok(iter
        .map(|(utxo, weight)| {
            Ok(match database.get_tx(&utxo.outpoint.txid, true)? {
                None => None,
                Some(tx) if tx.height.is_none() => None,
                Some(_) => Some((utxo, weight)),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?
        .into_iter()
        .filter_map(|x| x)
        .collect())
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::{OutPoint, Transaction, TxIn, TxOut, Txid};

    use super::*;
    use crate::database::{BatchOperations, MemoryDatabase};

    fn add_transaction(
        database: &mut MemoryDatabase,
        spend: Vec<OutPoint>,
        outputs: Vec<u64>,
    ) -> Txid {
        let tx = Transaction {
            version: 1,
            lock_time: 0,
            input: spend
                .iter()
                .cloned()
                .map(|previous_output| TxIn {
                    previous_output,
                    ..Default::default()
                })
                .collect(),
            output: outputs
                .iter()
                .cloned()
                .map(|value| TxOut {
                    value,
                    ..Default::default()
                })
                .collect(),
        };
        let txid = tx.txid();

        for input in &spend {
            database.del_utxo(input).unwrap();
        }
        for vout in 0..outputs.len() {
            database
                .set_utxo(&UTXO {
                    txout: tx.output[vout].clone(),
                    outpoint: OutPoint {
                        txid,
                        vout: vout as u32,
                    },
                    is_internal: true,
                })
                .unwrap();
        }
        database
            .set_tx(&TransactionDetails {
                txid,
                transaction: Some(tx),
                height: None,
                ..Default::default()
            })
            .unwrap();

        txid
    }

    #[test]
    fn test_filter_available() {
        let mut database = MemoryDatabase::new();
        add_transaction(
            &mut database,
            vec![OutPoint::from_str(
                "aad194c72fd5cfd16d23da9462930ca91e35df1cfee05242b62f4034f50c3d41:5",
            )
            .unwrap()],
            vec![50_000],
        );

        let filtered = filter_available(
            &database,
            database
                .iter_utxos()
                .unwrap()
                .into_iter()
                .map(|utxo| (utxo, 0)),
        )
        .unwrap();
        assert_eq!(filtered, &[]);
    }
}
