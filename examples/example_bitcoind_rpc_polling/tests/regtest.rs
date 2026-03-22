use bdk_chain::bitcoin::Amount;
use bdk_testenv::TestEnv;
use std::process::Command;
use std::str::FromStr;

const DESCRIPTOR: &str = "wpkh(tprv8ZgxMBicQKsPfK9BTf82oQkHhawtZv19CorqQKPFeaHDMA4dXYX6eWsJGNJ7VTQXWmoHdrfjCYuDijcRmNFwSKcVhswzqs4fugE8turndGc/1/*)";

fn run_cmd(
    args: &[&str],
    rpc_url: &str,
    cookie: &str,
    workdir: &std::path::Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_example_bitcoind_rpc_polling"))
        .args(args)
        .env("RPC_URL", rpc_url)
        .env("RPC_COOKIE", cookie)
        .env("DESCRIPTOR", DESCRIPTOR)
        .current_dir(workdir)
        .output()
        .expect("failed to run example binary")
}

fn assert_cmd_success(out: &std::process::Output, label: &str) {
    assert!(
        out.status.success(),
        "{} failed:\nstdout: {}\nstderr: {}",
        label,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn test_sync_and_balance_regtest() {
    let env = TestEnv::new().expect("failed to create testenv");
    let tmp = tempfile::tempdir().expect("failed to create tempdir");

    let rpc_url = format!("127.0.0.1:{}", env.bitcoind.params.rpc_socket.port());
    let cookie = env
        .bitcoind
        .params
        .cookie_file
        .to_str()
        .expect("cookie path is valid utf8");

    // 1. Init wallet on regtest
    let out = run_cmd(
        &["init", "--network", "regtest"],
        &rpc_url,
        cookie,
        tmp.path(),
    );
    assert_cmd_success(&out, "init");

    // 2. Get next wallet address
    let out = run_cmd(&["address", "next"], &rpc_url, cookie, tmp.path());
    assert_cmd_success(&out, "address next");
    let address_output = String::from_utf8_lossy(&out.stdout);
    // Parse the address from output like: "[address @ 0] bcrt1q..."
    let address_str = address_output
        .split_whitespace()
        .last()
        .expect("address output should have at least one word");
    println!("wallet address: {}", address_str);

    // 3. Mine 101 blocks to make coinbase spendable
    env.mine_blocks(101, None).expect("failed to mine blocks");

    // 4. Send 0.05 BTC to our wallet address
    let wallet_address = bdk_chain::bitcoin::Address::from_str(address_str)
        .expect("valid address")
        .assume_checked();
    env.send(&wallet_address, Amount::from_btc(0.05).unwrap())
        .expect("failed to send to wallet");

    // 5. Sync - should see unconfirmed tx
    let out = run_cmd(&["sync"], &rpc_url, cookie, tmp.path());
    assert_cmd_success(&out, "sync (unconfirmed)");

    // 6. Check unconfirmed balance is 0.05 BTC
    let out = run_cmd(&["balance"], &rpc_url, cookie, tmp.path());
    assert_cmd_success(&out, "balance (unconfirmed)");
    let balance_str = String::from_utf8_lossy(&out.stdout);
    println!("balance (unconfirmed):\n{}", balance_str);
    assert!(
        balance_str.contains("5000000"),
        "expected 5000000 sats unconfirmed, got: {}",
        balance_str
    );

    // 7. Mine 1 block to confirm the tx
    env.mine_blocks(1, None)
        .expect("failed to mine confirming block");

    // 8. Sync again - should see confirmed tx
    let out = run_cmd(&["sync"], &rpc_url, cookie, tmp.path());
    assert_cmd_success(&out, "sync (confirmed)");

    // 9. Check confirmed balance is 0.05 BTC (5_000_000 sats)
    let out = run_cmd(&["balance"], &rpc_url, cookie, tmp.path());
    assert_cmd_success(&out, "balance (confirmed)");
    let balance_str = String::from_utf8_lossy(&out.stdout);
    println!("balance (confirmed):\n{}", balance_str);
    assert!(
        balance_str.contains("5000000"),
        "expected 5000000 sats confirmed, got: {}",
        balance_str
    );

    // 10. List txouts - should show our received utxo
    let out = run_cmd(&["txout", "list"], &rpc_url, cookie, tmp.path());
    assert_cmd_success(&out, "txout list");
    println!("txout list:\n{}", String::from_utf8_lossy(&out.stdout));
}
