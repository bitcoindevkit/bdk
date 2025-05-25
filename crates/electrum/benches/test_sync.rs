use bdk_chain::{
    bitcoin::{Address, Amount, ScriptBuf},
    local_chain::LocalChain,
    spk_client::{SyncRequest, SyncResponse},
    spk_txout::SpkTxOutIndex,
    ConfirmationBlockTime, IndexedTxGraph, Indexer, Merge,
};
use bdk_core::bitcoin::{
    key::{Secp256k1, UntweakedPublicKey},
    Network,
};
use bdk_electrum::BdkElectrumClient;
use bdk_testenv::{anyhow, bitcoincore_rpc::RpcApi, TestEnv};
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

// Batch size for `sync_with_electrum`.
const BATCH_SIZE: usize = 5;

pub fn get_test_spk() -> ScriptBuf {
    const PK_BYTES: &[u8] = &[
        12, 244, 72, 4, 163, 4, 211, 81, 159, 82, 153, 123, 125, 74, 142, 40, 55, 237, 191, 231,
        31, 114, 89, 165, 83, 141, 8, 203, 93, 240, 53, 101,
    ];
    let secp = Secp256k1::new();
    let pk = UntweakedPublicKey::from_slice(PK_BYTES).expect("Must be valid PK");
    ScriptBuf::new_p2tr(&secp, pk, None)
}

fn sync_with_electrum<I, Spks>(
    client: &BdkElectrumClient<electrum_client::Client>,
    spks: Spks,
    chain: &mut LocalChain,
    graph: &mut IndexedTxGraph<ConfirmationBlockTime, I>,
) -> anyhow::Result<SyncResponse>
where
    I: Indexer,
    I::ChangeSet: Default + Merge,
    Spks: IntoIterator<Item = ScriptBuf>,
    Spks::IntoIter: ExactSizeIterator + Send + 'static,
{
    let update = client.sync(
        SyncRequest::builder().chain_tip(chain.tip()).spks(spks),
        BATCH_SIZE,
        true,
    )?;

    assert!(
        !update.tx_update.txs.is_empty(),
        "expected some transactions from sync, but got none"
    );

    if let Some(chain_update) = update.chain_update.clone() {
        let _ = chain
            .apply_update(chain_update)
            .map_err(|err| anyhow::anyhow!("LocalChain update error: {:?}", err))?;
    }
    let _ = graph.apply_update(update.tx_update.clone());

    Ok(update)
}

pub fn test_sync_performance(c: &mut Criterion) {
    let env = TestEnv::new().unwrap();
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str()).unwrap();
    let client = BdkElectrumClient::new(electrum_client);

    const NUM_BLOCKS: usize = 100;
    let mut spks = Vec::with_capacity(NUM_BLOCKS);

    // Mine some blocks and send transactions.
    env.mine_blocks(101, None).unwrap();

    // Scatter UTXOs across many blocks.
    for _ in 0..NUM_BLOCKS {
        let spk = get_test_spk();
        let addr = Address::from_script(&spk, Network::Regtest).unwrap();
        env.send(&addr, Amount::from_sat(10_000)).unwrap();
        env.mine_blocks(1, None).unwrap();

        spks.push(spk);
    }
    let _ = env.wait_until_electrum_sees_block(Duration::from_secs(6));

    // Setup receiver.
    let genesis = env.bitcoind.client.get_block_hash(0).unwrap();
    let (chain, _) = LocalChain::from_genesis_hash(genesis);
    let graph = IndexedTxGraph::<ConfirmationBlockTime, _>::new({
        let mut idx = SpkTxOutIndex::default();
        idx.insert_spk((), spks[0].clone());
        idx
    });

    c.bench_function("sync_with_electrum", move |b| {
        b.iter(|| {
            let spks = spks.clone();
            let mut recv_chain = chain.clone();
            let mut recv_graph = graph.clone();

            let _ = sync_with_electrum(&client, spks, &mut recv_chain, &mut recv_graph);
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10);
    targets = test_sync_performance
}
criterion_main!(benches);
