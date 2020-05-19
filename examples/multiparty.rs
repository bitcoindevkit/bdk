extern crate bitcoin;
extern crate clap;
extern crate log;
extern crate magical_bitcoin_wallet;
extern crate miniscript;
extern crate rand;
extern crate serde_json;
extern crate sled;

use std::str::FromStr;

use log::info;

use clap::{App, Arg};

use bitcoin::PublicKey;

use miniscript::policy::Concrete;
use miniscript::Descriptor;

use magical_bitcoin_wallet::multiparty::{Coordinator, Participant, Peer};

fn main() {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let matches = App::new("Multiparty Tools")
        .arg(
            Arg::with_name("POLICY")
                .help("Sets the spending policy to compile")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("TYPE")
                .help("Sets the script type used to embed the compiled policy")
                .required(true)
                .index(2)
                .possible_values(&["sh", "wsh", "sh-wsh"]),
        )
        .get_matches();

    let policy_str = matches.value_of("POLICY").unwrap();
    info!("Compiling policy: {}", policy_str);

    let policy = Concrete::<String>::from_str(&policy_str).unwrap();
    let compiled = policy.compile().unwrap();

    let descriptor = match matches.value_of("TYPE").unwrap() {
        "sh" => Descriptor::Sh(compiled),
        "wsh" => Descriptor::Wsh(compiled),
        "sh-wsh" => Descriptor::ShWsh(compiled),
        _ => panic!("Invalid type"),
    };

    info!("Descriptor: {}", descriptor);

    let mut coordinator: Participant<Coordinator> = Participant::new(descriptor).unwrap();
    /*let policy = coordinator.policy_for(vec![]).unwrap();
    info!(
        "Policy:\n{}",
        serde_json::to_string_pretty(&policy).unwrap()
    );*/

    let missing_keys = coordinator.missing_keys();
    info!("Missing keys: {:?}", missing_keys);

    let pk =
        PublicKey::from_str("02c65413e56b343a0a31c18d506f1502a17fc64dfbcef6bfb00d1c0d6229bb6f61")
            .unwrap();
    coordinator.add_key("Alice", pk.into()).unwrap();
    coordinator.add_key("Carol", pk.into()).unwrap();

    let for_bob = coordinator.descriptor_for("Bob").unwrap();
    info!("Descriptor for Bob: {}", for_bob);

    let mut bob_peer: Participant<Peer> = Participant::new(for_bob).unwrap();
    info!(
        "Bob's policy: {}",
        serde_json::to_string(&bob_peer.policy().unwrap().unwrap()).unwrap()
    );
    bob_peer.use_key(pk.into()).unwrap();
    info!("Bob's my_key: {}", bob_peer.my_key().unwrap());

    coordinator.add_key("Bob", pk.into()).unwrap();
    info!("Coordinator completed: {}", coordinator.completed());

    let coord_map = coordinator.get_map().unwrap();

    let finalized = coordinator.finalize().unwrap();
    info!("Coordinator final: {}", finalized);

    let bob_finalized = bob_peer.apply_map(coord_map).unwrap();
    info!("Bob final: {}", bob_finalized);
}
