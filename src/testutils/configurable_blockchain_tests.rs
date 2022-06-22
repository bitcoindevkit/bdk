use bitcoin::Network;

use crate::{
    blockchain::ConfigurableBlockchain, database::MemoryDatabase, testutils, wallet::AddressIndex,
    Wallet,
};

use super::blockchain_tests::TestClient;

/// Trait for testing [`ConfigurableBlockchain`] implementations.
pub trait ConfigurableBlockchainTester<B: ConfigurableBlockchain>: Sized {
    /// Blockchain name for logging.
    const BLOCKCHAIN_NAME: &'static str;

    /// Generates a blockchain config with a given stop_gap.
    ///
    /// If this returns [`Option::None`], then the associated tests will not run.
    fn config_with_stop_gap(
        &self,
        _test_client: &mut TestClient,
        _stop_gap: usize,
    ) -> Option<B::Config> {
        None
    }

    /// Runs all avaliable tests.
    fn run(&self) {
        let test_client = &mut TestClient::default();

        if self.config_with_stop_gap(test_client, 0).is_some() {
            test_wallet_sync_with_stop_gaps(test_client, self);
            test_wallet_sync_fulfills_missing_script_cache(test_client, self);
            test_wallet_sync_self_transfer_tx(test_client, self);
        } else {
            println!(
                "{}: Skipped tests requiring config_with_stop_gap.",
                Self::BLOCKCHAIN_NAME
            );
        }
    }
}

/// Test whether blockchain implementation syncs with expected behaviour given different `stop_gap`
/// parameters.
///
/// For each test vector:
/// * Fill wallet's derived addresses with balances (as specified by test vector).
///    * [0..addrs_before]          => 1000sats for each address
///    * [addrs_before..actual_gap] => empty addresses
///    * [actual_gap..addrs_after]  => 1000sats for each address
/// * Then, perform wallet sync and obtain wallet balance
/// * Check balance is within expected range (we can compare `stop_gap` and `actual_gap` to
///    determine this).
fn test_wallet_sync_with_stop_gaps<T, B>(test_client: &mut TestClient, tester: &T)
where
    T: ConfigurableBlockchainTester<B>,
    B: ConfigurableBlockchain,
{
    // Generates wallet descriptor
    let descriptor_of_account = |account_index: usize| -> String {
        format!("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/{account_index}/*)")
    };

    // Amount (in satoshis) provided to a single address (which expects to have a balance)
    const AMOUNT_PER_TX: u64 = 1000;

    // [stop_gap, actual_gap, addrs_before, addrs_after]
    //
    // [0]     stop_gap: Passed to [`ElectrumBlockchainConfig`]
    // [1]   actual_gap: Range size of address indexes without a balance
    // [2] addrs_before: Range size of address indexes (before gap) which contains a balance
    // [3]  addrs_after: Range size of address indexes (after gap) which contains a balance
    let test_vectors: Vec<[u64; 4]> = vec![
        [0, 0, 0, 5],
        [0, 0, 5, 5],
        [0, 1, 5, 5],
        [0, 2, 5, 5],
        [1, 0, 5, 5],
        [1, 1, 5, 5],
        [1, 2, 5, 5],
        [2, 1, 5, 5],
        [2, 2, 5, 5],
        [2, 3, 5, 5],
    ];

    for (account_index, vector) in test_vectors.into_iter().enumerate() {
        let [stop_gap, actual_gap, addrs_before, addrs_after] = vector;
        let descriptor = descriptor_of_account(account_index);

        let blockchain = B::from_config(
            &tester
                .config_with_stop_gap(test_client, stop_gap as _)
                .unwrap(),
        )
        .unwrap();

        let wallet =
            Wallet::new(&descriptor, None, Network::Regtest, MemoryDatabase::new()).unwrap();

        // fill server-side with txs to specified address indexes
        // return the max balance of the wallet (also the actual balance)
        let max_balance = (0..addrs_before)
            .chain(addrs_before + actual_gap..addrs_before + actual_gap + addrs_after)
            .fold(0_u64, |sum, i| {
                let address = wallet.get_address(AddressIndex::Peek(i as _)).unwrap();
                test_client.receive(testutils! {
                    @tx ( (@addr address.address) => AMOUNT_PER_TX )
                });
                sum + AMOUNT_PER_TX
            });

        // minimum allowed balance of wallet (based on stop gap)
        let min_balance = if actual_gap > stop_gap {
            addrs_before * AMOUNT_PER_TX
        } else {
            max_balance
        };
        let details = format!(
            "test_vector: [stop_gap: {}, actual_gap: {}, addrs_before: {}, addrs_after: {}]",
            stop_gap, actual_gap, addrs_before, addrs_after,
        );
        println!("{}", details);

        // perform wallet sync
        wallet.sync(&blockchain, Default::default()).unwrap();

        let wallet_balance = wallet.get_balance().unwrap().get_total();
        println!(
            "max: {}, min: {}, actual: {}",
            max_balance, min_balance, wallet_balance
        );

        assert!(
            wallet_balance <= max_balance,
            "wallet balance is greater than received amount: {}",
            details
        );
        assert!(
            wallet_balance >= min_balance,
            "wallet balance is smaller than expected: {}",
            details
        );

        // generate block to confirm new transactions
        test_client.generate(1, None);
    }
}

/// With a `stop_gap` of x and every x addresses having a balance of 1000 (for y addresses),
/// we expect `Wallet::sync` to correctly self-cache addresses, so that the resulting balance,
/// after sync, should be y * 1000.
fn test_wallet_sync_fulfills_missing_script_cache<T, B>(test_client: &mut TestClient, tester: &T)
where
    T: ConfigurableBlockchainTester<B>,
    B: ConfigurableBlockchain,
{
    // wallet descriptor
    let descriptor = "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/200/*)";

    // amount in sats per tx
    const AMOUNT_PER_TX: u64 = 1000;

    // addr constants
    const ADDR_COUNT: usize = 6;
    const ADDR_GAP: usize = 60;

    let blockchain =
        B::from_config(&tester.config_with_stop_gap(test_client, ADDR_GAP).unwrap()).unwrap();

    let wallet = Wallet::new(descriptor, None, Network::Regtest, MemoryDatabase::new()).unwrap();

    let expected_balance = (0..ADDR_COUNT).fold(0_u64, |sum, i| {
        let addr_i = i * ADDR_GAP;
        let address = wallet.get_address(AddressIndex::Peek(addr_i as _)).unwrap();

        println!(
            "tx: {} sats => [{}] {}",
            AMOUNT_PER_TX,
            addr_i,
            address.to_string()
        );

        test_client.receive(testutils! {
            @tx ( (@addr address.address) => AMOUNT_PER_TX )
        });
        test_client.generate(1, None);

        sum + AMOUNT_PER_TX
    });
    println!("expected balance: {}, syncing...", expected_balance);

    // perform sync
    wallet.sync(&blockchain, Default::default()).unwrap();
    println!("sync done!");

    let balance = wallet.get_balance().unwrap().get_total();
    assert_eq!(balance, expected_balance);
}

/// Given a `stop_gap`, a wallet with a 2 transactions, one sending to `scriptPubKey` at derivation
/// index of `stop_gap`, and the other spending from the same `scriptPubKey` into another
/// `scriptPubKey` at derivation index of `stop_gap * 2`, we expect `Wallet::sync` to perform
/// correctly, so that we detect the total balance.
fn test_wallet_sync_self_transfer_tx<T, B>(test_client: &mut TestClient, tester: &T)
where
    T: ConfigurableBlockchainTester<B>,
    B: ConfigurableBlockchain,
{
    const TRANSFER_AMOUNT: u64 = 10_000;
    const STOP_GAP: usize = 75;

    let descriptor = "wpkh(tprv8i8F4EhYDMquzqiecEX8SKYMXqfmmb1Sm7deoA1Hokxzn281XgTkwsd6gL8aJevLE4aJugfVf9MKMvrcRvPawGMenqMBA3bRRfp4s1V7Eg3/*)";

    let blockchain =
        B::from_config(&tester.config_with_stop_gap(test_client, STOP_GAP).unwrap()).unwrap();

    let wallet = Wallet::new(descriptor, None, Network::Regtest, MemoryDatabase::new()).unwrap();

    let address1 = wallet
        .get_address(AddressIndex::Peek(STOP_GAP as _))
        .unwrap();
    let address2 = wallet
        .get_address(AddressIndex::Peek((STOP_GAP * 2) as _))
        .unwrap();

    test_client.receive(testutils! {
        @tx ( (@addr address1.address) => TRANSFER_AMOUNT )
    });
    test_client.generate(1, None);

    wallet.sync(&blockchain, Default::default()).unwrap();

    let mut builder = wallet.build_tx();
    builder.add_recipient(address2.script_pubkey(), TRANSFER_AMOUNT / 2);
    let (mut psbt, details) = builder.finish().unwrap();
    assert!(wallet.sign(&mut psbt, Default::default()).unwrap());
    blockchain.broadcast(&psbt.extract_tx()).unwrap();

    test_client.generate(1, None);

    // obtain what is expected
    let fee = details.fee.unwrap();
    let expected_balance = TRANSFER_AMOUNT - fee;
    println!("fee={}, expected_balance={}", fee, expected_balance);

    // actually test the wallet
    wallet.sync(&blockchain, Default::default()).unwrap();
    let balance = wallet.get_balance().unwrap().get_total();
    assert_eq!(balance, expected_balance);

    // now try with a fresh wallet
    let fresh_wallet =
        Wallet::new(descriptor, None, Network::Regtest, MemoryDatabase::new()).unwrap();
    fresh_wallet.sync(&blockchain, Default::default()).unwrap();
    let fresh_balance = fresh_wallet.get_balance().unwrap().get_total();
    assert_eq!(fresh_balance, expected_balance);
}
