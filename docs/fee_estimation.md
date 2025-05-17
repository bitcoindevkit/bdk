# Fee Estimation Examples in BDK

This document provides real-world examples of how to estimate transaction fees using different blockchain backends supported by BDK: Electrum, Esplora (blocking and async), and bitcoind RPC.

---

## Electrum

To estimate the fee rate for a target confirmation in blocks using Electrum:

```rust
use electrum_client::Client;
use bdk_chain::FeeRate;

fn estimate_fee_electrum(target_blocks: usize) -> Result<FeeRate, electrum_client::Error> {
    let client = Client::new("ssl://electrum.blockstream.info:60002")?;
    // Returns BTC/kvB, convert to sat/vB
    let btc_per_kvb = client.estimate_fee(target_blocks)? as f32;
    Ok(FeeRate::from_btc_per_kvb(btc_per_kvb))
}
```

---

## Esplora (Blocking)

To estimate the fee rate using Esplora's blocking client:

```rust
use bdk_esplora::esplora_client::BlockingClient;
use bdk_chain::FeeRate;

fn estimate_fee_esplora(target_blocks: usize) -> Result<FeeRate, esplora_client::Error> {
    let client = BlockingClient::new("https://blockstream.info/api");
    let estimates = client.get_fee_estimates()?;
    let sat_per_vb = bdk_esplora::esplora_client::convert_fee_rate(target_blocks, &estimates)?;
    Ok(FeeRate::from_sat_per_vb(sat_per_vb))
}
```

---

## Esplora (Async)

To estimate the fee rate using Esplora's async client:

```rust
use bdk_esplora::esplora_client::AsyncClient;
use bdk_chain::FeeRate;

async fn estimate_fee_esplora_async(target_blocks: usize) -> Result<FeeRate, esplora_client::Error> {
    let client = AsyncClient::new("https://blockstream.info/api");
    let estimates = client.get_fee_estimates().await?;
    let sat_per_vb = bdk_esplora::esplora_client::convert_fee_rate(target_blocks, &estimates)?;
    Ok(FeeRate::from_sat_per_vb(sat_per_vb))
}
```

---

## bitcoind RPC

To estimate the fee rate using bitcoind's RPC interface:

```rust
use bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_chain::FeeRate;

fn estimate_fee_bitcoind(target_blocks: usize) -> Result<FeeRate, bitcoincore_rpc::Error> {
    let client = Client::new(
        "http://localhost:8332",
        Auth::UserPass("user".into(), "password".into()),
    )?;
    let sat_per_kb = client
        .estimate_smart_fee(target_blocks as u16, None)?
        .fee_rate
        .ok_or(bitcoincore_rpc::Error::UnexpectedStructure(
            "FeeRateUnavailable".to_string(),
        ))?
        .to_sat() as f64;
    Ok(FeeRate::from_sat_per_vb((sat_per_kb / 1000f64) as f32))
}
```

---

## Notes
- Always check the actual API and error handling for your version of the client libraries.
- For more details, see the [discussion in bdk_wallet#140](https://github.com/bitcoindevkit/bdk_wallet/issues/140) and [bdk#289](https://github.com/bitcoindevkit/bdk/issues/289).
- These examples are minimal and meant for illustration. In production, handle errors and edge cases appropriately. 
