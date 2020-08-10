#[macro_use]
extern crate quote;

use proc_macro::TokenStream;

use syn::spanned::Spanned;
use syn::{parse, parse2, Ident, ReturnType};

#[proc_macro_attribute]
pub fn magical_blockchain_tests(attr: TokenStream, item: TokenStream) -> TokenStream {
    let root_ident = if !attr.is_empty() {
        match parse::<syn::ExprPath>(attr) {
            Ok(parsed) => parsed,
            Err(e) => {
                let error_string = e.to_string();
                return (quote! {
                    compile_error!("Invalid crate path: {:?}", #error_string)
                })
                .into();
            }
        }
    } else {
        parse2::<syn::ExprPath>(quote! { magical_bitcoin_wallet }).unwrap()
    };

    match parse::<syn::ItemFn>(item) {
        Err(_) => (quote! {
            compile_error!("#[magical_blockchain_tests] can only be used on `fn`s")
        })
        .into(),
        Ok(parsed) => {
            let parsed_sig_ident = parsed.sig.ident.clone();
            let mod_name = Ident::new(
                &format!("generated_tests_{}", parsed_sig_ident.to_string()),
                parsed.span(),
            );

            let return_type = match parsed.sig.output {
                ReturnType::Type(_, ref t) => t.clone(),
                ReturnType::Default => {
                    return (quote! {
                        compile_error!("The tagged function must return a type that impl `OnlineBlockchain`")
                    }).into();
                }
            };

            let output = quote! {

            #parsed

            mod #mod_name {
                use bitcoin::Network;

                use miniscript::Descriptor;

                use testutils::{TestClient, serial};

                use #root_ident::blockchain::OnlineBlockchain;
                use #root_ident::descriptor::ExtendedDescriptor;
                use #root_ident::database::MemoryDatabase;
                use #root_ident::types::ScriptType;
                use #root_ident::{Wallet, TxBuilder};

                use super::*;

                fn get_blockchain() -> #return_type {
                    #parsed_sig_ident()
                }

                fn get_wallet_from_descriptors(descriptors: &(ExtendedDescriptor, Option<ExtendedDescriptor>)) -> Wallet<#return_type, MemoryDatabase> {
                    Wallet::new(&descriptors.0.to_string(), descriptors.1.as_ref().map(|d| d.to_string()).as_deref(), Network::Regtest, MemoryDatabase::new(), get_blockchain()).unwrap()
                }

                fn init_single_sig() -> (Wallet<#return_type, MemoryDatabase>, (ExtendedDescriptor, Option<ExtendedDescriptor>), TestClient) {
                    let descriptors = testutils! {
                        @descriptors ( "wpkh(Alice)" ) ( "wpkh(Alice)" ) ( @keys ( "Alice" => (@generate_xprv "/44'/0'/0'/0/*", "/44'/0'/0'/1/*") ) )
                    };

                    let test_client = TestClient::new();
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
                    let txid = test_client.receive(tx);

                    wallet.sync(None).unwrap();

                    assert_eq!(wallet.get_balance().unwrap(), 50_000);
                    assert_eq!(wallet.list_unspent().unwrap()[0].is_internal, false);

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

                    wallet.sync(None).unwrap();

                    assert_eq!(wallet.get_balance().unwrap(), 100_000);
                    assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
                }

                #[test]
                #[serial]
                fn test_sync_before_and_after_receive() {
                    let (wallet, descriptors, mut test_client) = init_single_sig();

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 0);

                    test_client.receive(testutils! {
                        @tx ( (@external descriptors, 0) => 50_000 )
                    });

                    wallet.sync(None).unwrap();

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

                    wallet.sync(None).unwrap();

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

                    wallet.sync(None).unwrap();

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

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 50_000);

                    test_client.receive(testutils! {
                        @tx ( (@external descriptors, 0) => 25_000 )
                    });

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 75_000);
                }

                #[test]
                #[serial]
                fn test_sync_receive_rbf_replaced() {
                    let (wallet, descriptors, mut test_client) = init_single_sig();

                    let txid = test_client.receive(testutils! {
                        @tx ( (@external descriptors, 0) => 50_000 ) ( @replaceable true )
                    });

                    wallet.sync(None).unwrap();

                    assert_eq!(wallet.get_balance().unwrap(), 50_000);
                    assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                    assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                    let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                    assert_eq!(list_tx_item.txid, txid);
                    assert_eq!(list_tx_item.received, 50_000);
                    assert_eq!(list_tx_item.sent, 0);
                    assert_eq!(list_tx_item.height, None);

                    let new_txid = test_client.bump_fee(&txid);

                    wallet.sync(None).unwrap();

                    assert_eq!(wallet.get_balance().unwrap(), 50_000);
                    assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                    assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                    let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                    assert_eq!(list_tx_item.txid, new_txid);
                    assert_eq!(list_tx_item.received, 50_000);
                    assert_eq!(list_tx_item.sent, 0);
                    assert_eq!(list_tx_item.height, None);
                }

                #[test]
                #[serial]
                fn test_sync_reorg_block() {
                    let (wallet, descriptors, mut test_client) = init_single_sig();

                    let txid = test_client.receive(testutils! {
                        @tx ( (@external descriptors, 0) => 50_000 ) ( @confirmations 1 ) ( @replaceable true )
                    });

                    wallet.sync(None).unwrap();

                    assert_eq!(wallet.get_balance().unwrap(), 50_000);
                    assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                    assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                    let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                    assert_eq!(list_tx_item.txid, txid);
                    assert!(list_tx_item.height.is_some());

                    // Invalidate 1 block
                    test_client.invalidate(1);

                    wallet.sync(None).unwrap();

                    assert_eq!(wallet.get_balance().unwrap(), 50_000);

                    let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                    assert_eq!(list_tx_item.txid, txid);
                    assert_eq!(list_tx_item.height, None);
                }

                #[test]
                #[serial]
                fn test_sync_after_send() {
                    let (wallet, descriptors, mut test_client) = init_single_sig();
                    let node_addr = test_client.get_node_address(None);

                    test_client.receive(testutils! {
                        @tx ( (@external descriptors, 0) => 50_000 )
                    });

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 50_000);

                    let (psbt, details) = wallet.create_tx(TxBuilder::from_addressees(vec![(node_addr, 25_000)])).unwrap();
                    let (psbt, finalized) = wallet.sign(psbt, None).unwrap();
                    assert!(finalized, "Cannot finalize transaction");
                    wallet.broadcast(psbt.extract_tx()).unwrap();

                    wallet.sync(None).unwrap();
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

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 50_000);

                    let (psbt, details) = wallet.create_tx(TxBuilder::from_addressees(vec![(node_addr, 25_000)])).unwrap();
                    let (psbt, finalized) = wallet.sign(psbt, None).unwrap();
                    assert!(finalized, "Cannot finalize transaction");
                    let sent_txid = wallet.broadcast(psbt.extract_tx()).unwrap();

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), details.received);

                    // empty wallet
                    let wallet = get_wallet_from_descriptors(&descriptors);
                    wallet.sync(None).unwrap();

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

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 50_000);

                    let mut total_sent = 0;
                    for _ in 0..5 {
                        let (psbt, details) = wallet.create_tx(TxBuilder::from_addressees(vec![(node_addr.clone(), 5_000)])).unwrap();
                        let (psbt, finalized) = wallet.sign(psbt, None).unwrap();
                        assert!(finalized, "Cannot finalize transaction");
                        wallet.broadcast(psbt.extract_tx()).unwrap();

                        wallet.sync(None).unwrap();

                        total_sent += 5_000 + details.fees;
                    }

                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 50_000 - total_sent);

                    // empty wallet
                    let wallet = get_wallet_from_descriptors(&descriptors);
                    wallet.sync(None).unwrap();
                    assert_eq!(wallet.get_balance().unwrap(), 50_000 - total_sent);
                }
            }

                        };

            output.into()
        }
    }
}
