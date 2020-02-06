use bitcoin::blockdata::block;
use bitcoin::hashes::hex::FromHex;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{Script, Txid};

use serde::{de, Deserialize, Serialize};

static JSONRPC_2_0: &str = "2.0";

#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum Param {
    Usize(usize),
    String(String),
    Bool(bool),
}

#[derive(Serialize, Clone)]
pub struct Request<'a> {
    jsonrpc: &'static str,

    pub id: usize,
    pub method: &'a str,
    pub params: Vec<Param>,
}

impl<'a> Request<'a> {
    pub fn new(method: &'a str, params: Vec<Param>) -> Self {
        Self {
            id: 0,
            jsonrpc: JSONRPC_2_0,
            method,
            params,
        }
    }

    pub fn new_id(id: usize, method: &'a str, params: Vec<Param>) -> Self {
        let mut instance = Self::new(method, params);
        instance.id = id;

        instance
    }
}

pub type ScriptHash = [u8; 32];
pub type ScriptStatus = [u8; 32];

pub trait ToElectrumScriptHash {
    fn to_electrum_scripthash(&self) -> ScriptHash;
}

impl ToElectrumScriptHash for Script {
    fn to_electrum_scripthash(&self) -> ScriptHash {
        let mut result = sha256::Hash::hash(self.as_bytes()).into_inner();
        result.reverse();

        result
    }
}

fn from_hex<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: FromHex,
    D: de::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    T::from_hex(&s).map_err(de::Error::custom)
}

fn from_hex_array<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    T: FromHex + std::fmt::Debug,
    D: de::Deserializer<'de>,
{
    let arr = Vec::<String>::deserialize(deserializer)?;

    let results: Vec<Result<T, _>> = arr
        .into_iter()
        .map(|s| T::from_hex(&s).map_err(de::Error::custom))
        .collect();

    let mut answer = Vec::new();
    for x in results.into_iter() {
        answer.push(x?);
    }

    Ok(answer)
}

#[derive(Debug, Deserialize)]
pub struct GetHistoryRes {
    pub height: i32,
    pub tx_hash: Txid,
}

#[derive(Debug, Deserialize)]
pub struct ListUnspentRes {
    pub height: usize,
    pub tx_pos: usize,
    pub value: u64,
    pub tx_hash: Txid,
}

#[derive(Debug, Deserialize)]
pub struct ServerFeaturesRes {
    pub server_version: String,
    #[serde(deserialize_with = "from_hex")]
    pub genesis_hash: [u8; 32],
    pub protocol_min: String,
    pub protocol_max: String,
    pub hash_function: Option<String>,
    pub pruning: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct GetHeadersRes {
    pub max: usize,
    pub count: usize,
    #[serde(rename(deserialize = "hex"), deserialize_with = "from_hex")]
    pub raw_headers: Vec<u8>,
    #[serde(skip)]
    pub headers: Vec<block::BlockHeader>,
}

#[derive(Debug, Deserialize)]
pub struct GetBalanceRes {
    pub confirmed: u64,
    pub unconfirmed: u64,
}

#[derive(Debug, Deserialize)]
pub struct GetMempoolRes {
    pub fee: u64,
    pub height: i32,
    pub tx_hash: Txid,
}

#[derive(Debug, Deserialize)]
pub struct GetMerkleRes {
    pub block_height: usize,
    pub pos: usize,
    #[serde(deserialize_with = "from_hex_array")]
    pub merkle: Vec<[u8; 32]>,
}

#[derive(Debug, Deserialize)]
pub struct HeaderNotification {
    pub height: usize,
    #[serde(rename(serialize = "hex"))]
    pub header: block::BlockHeader,
}

#[derive(Debug, Deserialize)]
pub struct ScriptNotification {
    pub scripthash: ScriptHash,
    pub status: ScriptStatus,
}

#[derive(Debug)]
pub enum Error {
    IOError(std::io::Error),
    JSON(serde_json::error::Error),
    Hex(bitcoin::hashes::hex::Error),
    Protocol(serde_json::Value),
    Bitcoin(bitcoin::consensus::encode::Error),
    AlreadySubscribed(ScriptHash),
    NotSubscribed(ScriptHash),
    InvalidResponse(serde_json::Value),
    Message(String),
    InvalidDNSNameError(String),

    #[cfg(feature = "use-openssl")]
    InvalidSslMethod(openssl::error::ErrorStack),
    #[cfg(feature = "use-openssl")]
    SslHandshakeError(openssl::ssl::HandshakeError<std::net::TcpStream>),
}

macro_rules! impl_error {
    ( $from:ty, $to:ident ) => {
        impl std::convert::From<$from> for Error {
            fn from(err: $from) -> Self {
                Error::$to(err)
            }
        }
    };
}

impl_error!(std::io::Error, IOError);
impl_error!(serde_json::Error, JSON);
impl_error!(bitcoin::hashes::hex::Error, Hex);
impl_error!(bitcoin::consensus::encode::Error, Bitcoin);
