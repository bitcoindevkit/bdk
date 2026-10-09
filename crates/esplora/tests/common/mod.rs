#![allow(dead_code)]

use bdk_core::bitcoin::key::{Secp256k1, UntweakedPublicKey};
use bdk_core::bitcoin::{Address, ScriptBuf};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::str::FromStr;

const PK_BYTES: &[u8] = &[
    12, 244, 72, 4, 163, 4, 211, 81, 159, 82, 153, 123, 125, 74, 142, 40, 55, 237, 191, 231, 31,
    114, 89, 165, 83, 141, 8, 203, 93, 240, 53, 101,
];

pub fn get_test_spk() -> ScriptBuf {
    let secp = Secp256k1::new();
    let pk = UntweakedPublicKey::from_slice(PK_BYTES).expect("Must be valid PK");
    ScriptBuf::new_p2tr(&secp, pk, None)
}

pub fn test_addresses() -> Vec<Address> {
    [
        "bcrt1qj9f7r8r3p2y0sqf4r3r62qysmkuh0fzep473d2ar7rcz64wqvhssjgf0z4",
        "bcrt1qmm5t0ch7vh2hryx9ctq3mswexcugqe4atkpkl2tetm8merqkthas3w7q30",
        "bcrt1qut9p7ej7l7lhyvekj28xknn8gnugtym4d5qvnp5shrsr4nksmfqsmyn87g",
        "bcrt1qqz0xtn3m235p2k96f5wa2dqukg6shxn9n3txe8arlrhjh5p744hsd957ww",
        "bcrt1q9c0t62a8l6wfytmf2t9lfj35avadk3mm8g4p3l84tp6rl66m48sqrme7wu",
        "bcrt1qkmh8yrk2v47cklt8dytk8f3ammcwa4q7dzattedzfhqzvfwwgyzsg59zrh",
        "bcrt1qvgrsrzy07gjkkfr5luplt0azxtfwmwq5t62gum5jr7zwcvep2acs8hhnp2",
        "bcrt1qw57edarcg50ansq8mk3guyrk78rk0fwvrds5xvqeupteu848zayq549av8",
        "bcrt1qvtve5ekf6e5kzs68knvnt2phfw6a0yjqrlgat392m6zt9jsvyxhqfx67ef",
        "bcrt1qw03ddumfs9z0kcu76ln7jrjfdwam20qtffmkcral3qtza90sp9kqm787uk",
    ]
    .into_iter()
    .map(|s| Address::from_str(s).unwrap().assume_checked())
    .collect()
}

/// Number of transactions per page requested by `bdk_esplora`.
const PAGE_SIZE: u32 = 25;

/// A full page of transactions, always ending on the same txid.
fn non_advancing_page() -> String {
    let txs = (0..PAGE_SIZE)
        .map(|i| {
            serde_json::json!({
                "txid": format!("{i:064x}"),
                "version": 2,
                "locktime": 0,
                "vin": [],
                "vout": [],
                "size": 100,
                "weight": 400,
                "status": {
                    "confirmed": false,
                    "block_height": null,
                    "block_hash": null,
                    "block_time": null
                },
                "fee": 0
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&txs).expect("page must serialize")
}

/// Spawns a stub Esplora server which answers every request with the same full page of
/// transactions, emulating a server that never pages forward.
///
/// Returns the base URL to point an Esplora client at. The listener is served until the test
/// binary exits.
pub fn spawn_non_advancing_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("must bind to loopback");
    let base_url = format!(
        "http://{}",
        listener.local_addr().expect("must have local addr")
    );
    let body = non_advancing_page();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Every path gets the same answer, so there is nothing to parse. We still consume the
            // request head, otherwise closing the connection could abort the client's write.
            let mut reader = BufReader::new(stream.try_clone().expect("must clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\n\
                 content-type: application/json\r\n\
                 content-length: {}\r\n\
                 connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
        }
    });

    base_url
}
