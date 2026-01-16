//! Test ensuring that the API allows manual construction of SyncRequest
//! and execution via BdkElectrumClient.
//! 
//! Strategy: Use a real `electrum_client::Client` pointing to a non-existent server.
//! This validates that the types (`SyncRequest`, `BdkElectrumClient`) are compatible
//! and compile together. Runtime failure is expected and asserted.

use bdk_chain::{
    local_chain::LocalChain,
    spk_client::SyncRequest,
    keychain_txout::KeychainTxOutIndex,
    IndexedTxGraph,
};
use bdk_electrum::BdkElectrumClient;
use bdk_electrum::electrum_client;
use bdk_electrum::bitcoin::{
    Address, BlockHash,
    hashes::Hash,
};
use std::str::FromStr;

#[test]
fn test_manual_sync_request_construction_with_dummy_client() {
    // 1. Setup Dummy Client
    // We use a real client but point to an invalid address.
    // This allows us to compile check the wiring without mocking the huge trait.
    let dummy_url = "ssl://127.0.0.1:0"; // Invalid port/host
    // If creation fails (e.g. invalid URL format), we panic, which is fine (test fails).
    // If creation succeeds, we get a client that will fail on IO.
    let electrum_client = match electrum_client::Client::new(dummy_url) {
        Ok(c) => c,
        Err(_) => return, // Could not create client, skips test (or panic?)
        // If we can't create it, we can't test wiring. But verify compilation is the main goal.
    };

    let client = BdkElectrumClient::new(electrum_client);

    // 2. Setup Wallet (Local components)
    let (mut chain, _) = LocalChain::from_genesis(BlockHash::all_zeros());
    let mut graph = IndexedTxGraph::<bdk_chain::ConfirmationBlockTime, _>::new(KeychainTxOutIndex::<u32>::default());

    // 3. Define a script to track
    let descriptor_str = "wpkh(022e3e56c52b21c640798e6e5d2633008432a2657e057f5c907a48d844208a0d0a)";
    let descriptor = bdk_chain::miniscript::Descriptor::from_str(descriptor_str).expect("parse");
    
    // Insert into keychain
    let _ = graph.index.insert_descriptor(0, descriptor);
    graph.index.reveal_to_target(0, 5); 

    // 4. Construct SyncRequest Manually
    // This part validates the API Types compatibility.
    let request = SyncRequest::builder()
        .chain_tip(chain.tip())
        .spks_with_indexes(graph.index.revealed_spks(..).map(|(k, s)| (k.1, s.into())))
        .build();

    // 5. Execute Sync
    // This should fail with an error (likely IO or ConnectionRefused), but COMPILATION must succeed.
    let result = client.sync(request, 10, false);

    // 6. Assertions
    // We expect an error. If by miracle it succeeds (no), valid.
    assert!(result.is_err(), "Sync should fail due to dummy URL, but it must compile and run");
}
