/// This macro runs blockchain tests against a `Blockchain` implementation. It requires access to a
/// Bitcoin core wallet via RPC. At the moment you have to dig into the code yourself and look at
/// the setup required to run the tests yourself.
#[macro_export]
macro_rules! bdk_blockchain_tests {
    (
     fn test_instance() -> $blockchain:ty $block:block) => {
        #[cfg(test)]
        mod bdk_blockchain_tests {
            use $crate::bitcoin::Network;
            use $crate::testutils::{TestClient};
            use $crate::blockchain::noop_progress;
            use $crate::database::MemoryDatabase;
            use $crate::types::KeychainKind;
            use $crate::{Wallet, FeeRate};
            use $crate::wallet::AddressIndex::New;
            use $crate::testutils;
            use $crate::serial_test::serial;

            use super::*;

            fn get_blockchain() -> $blockchain {
                $block
            }

            fn get_wallet_from_descriptors(descriptors: &(String, Option<String>)) -> Wallet<$blockchain, MemoryDatabase> {
                Wallet::new(&descriptors.0.to_string(), descriptors.1.as_ref(), Network::Regtest, MemoryDatabase::new(), get_blockchain()).unwrap()
            }

            fn init_single_sig() -> (Wallet<$blockchain, MemoryDatabase>, (String, Option<String>), TestClient) {
                let descriptors = testutils! {
                    @descriptors ( "wpkh(Alice)" ) ( "wpkh(Alice)" ) ( @keys ( "Alice" => (@generate_xprv "/44'/0'/0'/0/*", "/44'/0'/0'/1/*") ) )
                };

                let test_client = TestClient::default();
                let wallet = get_wallet_from_descriptors(&descriptors);

                (wallet, descriptors, test_client)
            }

            #[test]
            #[serial]
            fn test_sync_simple() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let tx = testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                };
                println!("{:?}", tx);
                let txid = test_client.receive(tx);

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_unspent().unwrap()[0].keychain, KeychainKind::External);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.received, 50_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);
            }

            #[test]
            #[serial]
            fn test_sync_stop_gap_20() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 5) => 50_000 )
                });
                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 25) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 100_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
            }

            #[test]
            #[serial]
            fn test_sync_before_and_after_receive() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 0);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
            }

            #[test]
            #[serial]
            fn test_sync_multiple_outputs_same_tx() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000, (@external descriptors, 5) => 30_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 105_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 3);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.received, 105_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);
            }

            #[test]
            #[serial]
            fn test_sync_receive_multi() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });
                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 5) => 25_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 75_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
                assert_eq!(wallet.list_unspent().unwrap().len(), 2);
            }

            #[test]
            #[serial]
            fn test_sync_address_reuse() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 25_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 75_000);
            }

            #[test]
            #[serial]
            fn test_sync_receive_rbf_replaced() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) ( @replaceable true )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.received, 50_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);

                let new_txid = test_client.bump_fee(&txid);

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, new_txid);
                assert_eq!(list_tx_item.received, 50_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);
            }

            // FIXME: I would like this to be cfg_attr(not(feature = "test-esplora"), ignore) but it
            // doesn't work for some reason.
            #[cfg(not(feature = "esplora"))]
            #[test]
            #[serial]
            fn test_sync_reorg_block() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) ( @confirmations 1 ) ( @replaceable true )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert!(list_tx_item.height.is_some());

                // Invalidate 1 block
                test_client.invalidate(1);

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.height, None);
            }

            #[test]
            #[serial]
            fn test_sync_after_send() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                println!("{}", descriptors.0);
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey(), 25_000);
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                let tx = psbt.extract_tx();
                println!("{}", bitcoin::consensus::encode::serialize_hex(&tx));
                wallet.broadcast(tx).unwrap();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);
            }

            #[test]
            #[serial]
            fn test_sync_outgoing_from_scratch() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                let received_txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey(), 25_000);
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                let sent_txid = wallet.broadcast(psbt.extract_tx()).unwrap();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                // empty wallet
                let wallet = get_wallet_from_descriptors(&descriptors);
                wallet.sync(noop_progress(), None).unwrap();

                let tx_map = wallet.list_transactions(false).unwrap().into_iter().map(|tx| (tx.txid, tx)).collect::<std::collections::HashMap<_, _>>();

                let received = tx_map.get(&received_txid).unwrap();
                assert_eq!(received.received, 50_000);
                assert_eq!(received.sent, 0);

                let sent = tx_map.get(&sent_txid).unwrap();
                assert_eq!(sent.received, details.received);
                assert_eq!(sent.sent, details.sent);
                assert_eq!(sent.fees, details.fees);
            }

            #[test]
            #[serial]
            fn test_sync_long_change_chain() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut total_sent = 0;
                for _ in 0..5 {
                    let mut builder = wallet.build_tx();
                    builder.add_recipient(node_addr.script_pubkey(), 5_000);
                    let (mut psbt, details) = builder.finish().unwrap();
                    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                    assert!(finalized, "Cannot finalize transaction");
                    wallet.broadcast(psbt.extract_tx()).unwrap();

                    wallet.sync(noop_progress(), None).unwrap();

                    total_sent += 5_000 + details.fees;
                }

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - total_sent);

                // empty wallet
                let wallet = get_wallet_from_descriptors(&descriptors);
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - total_sent);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 5_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - details.fees - 5_000);
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(2.1));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - new_details.fees - 5_000);
                assert_eq!(wallet.get_balance().unwrap(), new_details.received);

                assert!(new_details.fees > details.fees);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee_remove_change() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 49_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 1_000 - details.fees);
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(5.0));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 0);
                assert_eq!(new_details.received, 0);

                assert!(new_details.fees > details.fees);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee_add_input() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 75_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 49_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 26_000 - details.fees);
                assert_eq!(details.received, 1_000 - details.fees);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(10.0));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(new_details.sent, 75_000);
                assert_eq!(wallet.get_balance().unwrap(), new_details.received);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee_add_input_no_change() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 75_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 49_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 26_000 - details.fees);
                assert_eq!(details.received, 1_000 - details.fees);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(123.0));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                println!("{:#?}", new_details);

                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(new_details.sent, 75_000);
                assert_eq!(wallet.get_balance().unwrap(), 0);
                assert_eq!(new_details.received, 0);
            }

            #[test]
            #[serial]
            fn test_sync_receive_coinbase() {
                let (wallet, _, mut test_client) = init_single_sig();
                let wallet_addr = wallet.get_address(New).unwrap();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 0);

                test_client.generate(1, Some(wallet_addr));

                wallet.sync(noop_progress(), None).unwrap();
                assert!(wallet.get_balance().unwrap() > 0);
            }
        }
    }
}
