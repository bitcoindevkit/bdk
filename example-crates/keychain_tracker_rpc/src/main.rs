use keychain_tracker_example_cli::{
    anyhow,
    clap::{self, Subcommand},
    init,
};

use bdk_chain::{
    bitcoin::{consensus::deserialize, Transaction},
    TxHeight,
};

use bdk_rpc_wallet::{
    bitcoincore_rpc::{Auth, RpcApi},
    into_keychain_scan, missing_full_txs, RpcClient, RpcWalletExt,
};

use std::fmt::Debug;

#[derive(Subcommand, Debug, Clone)]
enum RpcCommands {
    /// Scans for transactions related spks in the tracker
    Scan,
}

/// This is a binary cli application that simulates a command line wallet with [`bdk_chain::keychain::KeychainTracker`]
/// and syncing it with [`bdk_rpc_wallet::RpcClient`].
///
/// use `keychain_tracker_rpc help` to see all the commands and options for the app.
///
/// The `main` code below demonstrates a basic workflow of syncing the keychain tracker.
///
/// The example work with a local signet node with RPC and Wallet enabled. If you need to change the node credential
/// update the `client` construction segment below with your own details (RPC Port, username:password)
///
/// Note: Currently the RPC syncing works with legacy wallet, with BDB support. Which is a non-default core configuration.
/// Follow the below instruction to compile bitcoind with BDB support.
/// https://github.com/bitcoin/bitcoin/blob/master/doc/build-unix.md#berkeley-db

fn main() -> anyhow::Result<()> {
    let (args, keymap, mut tracker, mut db) = init::<RpcCommands, TxHeight>()?;

    let client = {
        let rpc_url = "127.0.0.1:38332".to_string();
        // Change the auth below to match your node config before running the example
        let rpc_auth = Auth::UserPass("user".to_string(), "password".to_string());
        RpcClient::new("wallet-name".to_string(), rpc_url, rpc_auth)?
    };

    let (spk_iterator, local_chain) = {
        let tracker = &*tracker.lock().unwrap();
        let spk_iterator = tracker
            .txout_index
            .inner()
            .all_spks()
            .iter()
            .map(|(_, s)| s.clone())
            .collect::<Vec<_>>();
        let local_chain = tracker.chain().checkpoints().clone();
        (spk_iterator, local_chain)
    };

    match args.command {
        keychain_tracker_example_cli::Commands::ChainSpecific(RpcCommands::Scan) => {
            // Get the initial update sparsechain. This contains all the new txids to be added.
            let response = client.scan(&local_chain, spk_iterator)?;

            // Find the missing transactions that we don't have in the tracker
            let new_txids = {
                let tracker = &*tracker.lock().unwrap();
                missing_full_txs(&response, &tracker)
            };

            // Fetch the missing full transactions
            let new_txs = new_txids
                .iter()
                .map(|txid| {
                    let tx_data = client.get_transaction(&txid, Some(true))?.hex;
                    let tx: Transaction = deserialize(&tx_data)?;
                    Ok(tx)
                })
                .collect::<Result<Vec<_>, anyhow::Error>>()?;

            // Compile the full update with fetched transaction. Take a short lock on the tracker to apply the updates.
            {
                let mut tracker = tracker.lock().unwrap();
                let changeset = {
                    let scan = into_keychain_scan(response, new_txs, tracker.chain_graph())?;
                    tracker.determine_changeset(&scan)?
                };
                db.lock().unwrap().append_changeset(&changeset)?;
                tracker.apply_changeset(changeset);
            }
        }
        general_command => {
            return keychain_tracker_example_cli::handle_commands(
                general_command,
                |transaction| {
                    let _txid = client.send_raw_transaction(transaction)?;
                    Ok(())
                },
                &mut tracker,
                &mut db,
                args.network,
                &keymap,
            )
        }
    };

    Ok(())
}
