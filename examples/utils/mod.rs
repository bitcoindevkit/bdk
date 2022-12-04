pub(crate) mod tx {

    use std::str::FromStr;

    use bdk::{database::BatchDatabase, SignOptions, Wallet};
    use bitcoin::{Address, Transaction};

    pub fn build_signed_tx<D: BatchDatabase>(
        wallet: &Wallet<D>,
        recipient_address: &str,
        amount: u64,
    ) -> Transaction {
        // Create a transaction builder
        let mut tx_builder = wallet.build_tx();

        let to_address = Address::from_str(recipient_address).unwrap();

        // Set recipient of the transaction
        tx_builder.set_recipients(vec![(to_address.script_pubkey(), amount)]);

        // Finalise the transaction and extract PSBT
        let (mut psbt, _) = tx_builder.finish().unwrap();

        // Sign the above psbt with signing option
        wallet.sign(&mut psbt, SignOptions::default()).unwrap();

        // Extract the final transaction
        psbt.extract_tx()
    }
}

pub(crate) mod tor {
    use std::fs::File;
    use std::io::prelude::*;
    use std::thread;
    use std::time::Duration;

    use libtor::LogDestination;
    use libtor::LogLevel;
    use libtor::{HiddenServiceVersion, Tor, TorAddress, TorFlag};

    use std::env;

    pub struct TorAddresses {
        pub socks: String,
        pub hidden_service: Option<String>,
    }

    pub fn use_tor() -> bool {
        match env::var("TOR") {
            Ok(val) => val == "1" || val == "true",
            _ => false,
        }
    }

    pub fn start_tor(hidden_service_port: Option<u16>) -> TorAddresses {
        let socks_port = 19050;

        let data_dir = format!("{}/{}", env::temp_dir().display(), "bdk-tor-example");
        let log_file_name = format!("{}/{}", &data_dir, "log");
        let hidden_service_dir =
            hidden_service_port.map(|port| format!("{}/{}-{}", &data_dir, "hs-dir", port));

        println!("Staring Tor in {}", &data_dir);

        truncate_log(&log_file_name);

        if let Some(hs_port) = hidden_service_port {
            Tor::new()
                .flag(TorFlag::DataDirectory(data_dir.into()))
                .flag(TorFlag::LogTo(
                    LogLevel::Notice,
                    LogDestination::File(log_file_name.as_str().into()),
                ))
                .flag(TorFlag::SocksPort(socks_port))
                .flag(TorFlag::Custom("ExitPolicy reject *:*".into()))
                .flag(TorFlag::Custom("BridgeRelay 0".into()))
                .flag(TorFlag::HiddenServiceDir(
                    hidden_service_dir.as_ref().unwrap().into(),
                ))
                .flag(TorFlag::HiddenServiceVersion(HiddenServiceVersion::V3))
                .flag(TorFlag::HiddenServicePort(
                    TorAddress::Port(hs_port),
                    None.into(),
                ))
                .start_background()
        } else {
            Tor::new()
                .flag(TorFlag::DataDirectory(data_dir.into()))
                .flag(TorFlag::LogTo(
                    LogLevel::Notice,
                    LogDestination::File(log_file_name.as_str().into()),
                ))
                .flag(TorFlag::SocksPort(socks_port))
                .flag(TorFlag::Custom("ExitPolicy reject *:*".into()))
                .flag(TorFlag::Custom("BridgeRelay 0".into()))
                .start_background()
        };

        let mut started = false;
        let mut tries = 0;
        while !started {
            tries += 1;
            if tries > 120 {
                panic!(
                    "It took too long to start Tor. See {} for details",
                    &log_file_name
                );
            }

            thread::sleep(Duration::from_millis(1000));
            started = find_string_in_log(&log_file_name, &"Bootstrapped 100%".into());
        }

        println!("Tor started");

        TorAddresses {
            socks: format!("127.0.0.1:{}", socks_port),
            hidden_service: hidden_service_port.map(|port| {
                format!(
                    "{}:{}",
                    get_onion_address(&hidden_service_dir.unwrap()),
                    port
                )
            }),
        }
    }

    fn truncate_log(filename: &String) {
        let path = std::path::Path::new(filename);
        if path.exists() {
            let file = File::options()
                .write(true)
                .open(path)
                .expect("no such file");
            file.set_len(0).expect("cannot set size to 0");
        }
    }

    fn find_string_in_log(filename: &String, str: &String) -> bool {
        let path = std::path::Path::new(filename);
        if path.exists() {
            let mut file = File::open(path).expect("cannot open log file");
            let mut buf = String::new();
            file.read_to_string(&mut buf).expect("cannot read log file");
            buf.contains(str)
        } else {
            false
        }
    }

    fn get_onion_address(hidden_service_dir: &String) -> String {
        let filename = format!("{}/{}", hidden_service_dir, "hostname");
        let path = std::path::Path::new(&filename);
        let mut file = File::open(path).expect("cannot open hostname file");
        let mut buf = String::new();
        file.read_to_string(&mut buf).expect("cannot read log file");
        buf.replace("\n", "")
    }
}
