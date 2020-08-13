use crate::database::Database;
use crate::error::Error;
use crate::types::*;

/// Filters unspent utxos
pub(super) fn filter_available<I: Iterator<Item = UTXO>, D: Database>(
    database: &D,
    iter: I,
) -> Result<Vec<UTXO>, Error> {
    Ok(iter
        .map(|utxo| {
            Ok(match database.get_tx(&utxo.outpoint.txid, true)? {
                None => None,
                Some(tx) if tx.height.is_none() => None,
                Some(_) => Some(utxo),
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

        let filtered =
            filter_available(&database, database.iter_utxos().unwrap().into_iter()).unwrap();
        assert_eq!(filtered, &[]);
    }
}
