// Bitcoin Dev Kit
// Written in 2021 by Rajarshi Maitra <rajarshi149@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! A SOCKS5 proxy transport implementation for RPC blockchain.
//!
//! This is currently internal to the lib and only compatible with
//! Bitcoin Core RPC.

use super::rpc::Auth;
use bitcoin::base64;
use bitcoincore_rpc::jsonrpc::Error as JSONRPC_Error;
use bitcoincore_rpc::jsonrpc::{Request, Response, Transport};
use bitcoincore_rpc::Error as RPC_Error;
use socks::Socks5Stream;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

/// Errors that can be thrown by [`ProxyTransport`](crate::blockchain::rpc_proxy::ProxyTransport)
#[derive(Debug)]
pub enum RpcProxyError {
    /// Bitcoin core rpc error
    CoreRpc(bitcoincore_rpc::Error),
    /// IO error
    Io(std::io::Error),
    /// Invalid RPC url
    InvalidUrl,
    /// Error serializing or deserializing JSON data
    Json(serde_json::Error),
    /// RPC timeout error
    RpcTimeout,
    /// Http Parsing Error
    HttpParsing,
    /// Http Timeout Error
    HttpTimeout,
    /// Http Response Code
    HttpResponseCode(u16),
}

impl std::fmt::Display for RpcProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for RpcProxyError {}

impl_error!(bitcoincore_rpc::Error, CoreRpc, RpcProxyError);
impl_error!(std::io::Error, Io, RpcProxyError);
impl_error!(serde_json::Error, Json, RpcProxyError);

// We need to backport RpcProxyError to satisfy Transport trait bound
impl From<RpcProxyError> for JSONRPC_Error {
    fn from(e: RpcProxyError) -> Self {
        Self::Transport(Box::new(e))
    }
}

/// SOCKS5 proxy transport
/// This is currently designed to work only with Bitcoin Core RPC
pub(crate) struct ProxyTransport {
    proxy_addr: String,
    target_addr: String,
    proxy_credential: Option<(String, String)>,
    wallet_path: String,
    rpc_auth: Option<String>,
    timeout: Duration,
}

impl ProxyTransport {
    /// Create a new ProxyTransport
    pub(crate) fn new(
        proxy_addr: &str,
        rpc_url: &str,
        proxy_credential: Option<(String, String)>,
        rpc_auth: &Auth,
    ) -> Result<Self, RpcProxyError> {
        // Fetch the RPC address:port and wallet path from url
        let (target_addr, wallet_path) = {
            // the url will be of form "http://<rpc-host>:<rpc-port>/wallet/<wallet-file-name>"
            (rpc_url[7..22].to_owned(), rpc_url[22..].to_owned())
        };

        // fetch username password from rpc authentication
        let rpc_auth = {
            if let (Some(user), Some(pass)) = Self::get_user_pass(rpc_auth)? {
                let mut auth = user;
                auth.push(':');
                auth.push_str(&pass[..]);
                Some(format!("Basic {}", &base64::encode(auth.as_bytes())))
            } else {
                None
            }
        };

        Ok(ProxyTransport {
            proxy_addr: proxy_addr.to_owned(),
            target_addr,
            proxy_credential,
            wallet_path,
            rpc_auth,
            timeout: Duration::from_secs(15), // Same as regular RPC default
        })
    }

    // Helper function to parse username:password pair for rpc Auth
    fn get_user_pass(auth: &Auth) -> Result<(Option<String>, Option<String>), RpcProxyError> {
        use std::io::Read;
        match auth {
            Auth::None => Ok((None, None)),
            Auth::UserPass { username, password } => {
                Ok((Some(username.clone()), Some(password.clone())))
            }
            Auth::Cookie { file } => {
                let mut file = File::open(file)?;
                let mut contents = String::new();
                file.read_to_string(&mut contents)?;
                let mut split = contents.splitn(2, ':');
                Ok((
                    Some(split.next().ok_or(RPC_Error::InvalidCookieFile)?.into()),
                    Some(split.next().ok_or(RPC_Error::InvalidCookieFile)?.into()),
                ))
            }
        }
    }

    // Try to read a line from a buffered reader. If no line can be read till the deadline is reached
    // return a timeout error.
    fn get_line<R: BufRead>(reader: &mut R, deadline: Instant) -> Result<String, RpcProxyError> {
        let mut line = String::new();
        while deadline > Instant::now() {
            match reader.read_line(&mut line) {
                // EOF reached for now, try again later
                Ok(0) => std::thread::sleep(Duration::from_millis(5)),
                // received useful data, return it
                Ok(_) => return Ok(line),
                // io error occurred, abort
                Err(e) => return Err(e.into()),
            }
        }
        Err(RpcProxyError::RpcTimeout)
    }

    // Http request and response over SOCKS5
    fn request<R>(&self, req: impl serde::Serialize) -> Result<R, RpcProxyError>
    where
        R: for<'a> serde::de::Deserialize<'a>,
    {
        let request_deadline = Instant::now() + self.timeout;

        // Open connection
        let mut socks_stream = if let Some((username, password)) = &self.proxy_credential {
            Socks5Stream::connect_with_password(
                &self.proxy_addr[..],
                &self.target_addr[..],
                &username[..],
                &password[..],
            )?
        } else {
            Socks5Stream::connect(&self.proxy_addr[..], &self.target_addr[..])?
        };

        let socks_stream = socks_stream.get_mut();

        // Serialize the body first so we can set the Content-Length header.
        let body = serde_json::to_vec(&req)?;

        // Send HTTP request
        socks_stream.write_all(b"POST ")?;
        socks_stream.write_all(self.wallet_path.as_bytes())?;
        socks_stream.write_all(b" HTTP/1.1\r\n")?;
        // Write headers
        socks_stream.write_all(b"Content-Type: application/json-rpc\r\n")?;
        socks_stream.write_all(b"Content-Length: ")?;
        socks_stream.write_all(body.len().to_string().as_bytes())?;
        socks_stream.write_all(b"\r\n")?;
        if let Some(ref auth) = self.rpc_auth {
            socks_stream.write_all(b"Authorization: ")?;
            socks_stream.write_all(auth.as_ref())?;
            socks_stream.write_all(b"\r\n")?;
        }
        // Write body
        socks_stream.write_all(b"\r\n")?;
        socks_stream.write_all(&body)?;
        socks_stream.flush()?;

        // Receive response
        let mut reader = BufReader::new(socks_stream);

        // Parse first HTTP response header line
        let http_response = Self::get_line(&mut reader, request_deadline)?;
        if http_response.len() < 12 || !http_response.starts_with("HTTP/1.1 ") {
            return Err(RpcProxyError::HttpParsing);
        }
        let response_code = match http_response[9..12].parse::<u16>() {
            Ok(n) => n,
            Err(_) => return Err(RpcProxyError::HttpParsing),
        };

        // Skip response header fields
        while Self::get_line(&mut reader, request_deadline)? != "\r\n" {}

        // Even if it's != 200, we parse the response as we may get a JSONRPC error instead
        // of the less meaningful HTTP error code.
        let resp_body = Self::get_line(&mut reader, request_deadline)?;
        match serde_json::from_str(&resp_body) {
            Ok(s) => Ok(s),
            Err(e) => {
                if response_code != 200 {
                    Err(RpcProxyError::HttpResponseCode(response_code))
                } else {
                    // If it was 200 then probably it was legitimately a parse error
                    Err(e.into())
                }
            }
        }
    }
}

// Make ProxyTransport usable in bitcoin_core::rpc::Client
impl Transport for ProxyTransport {
    fn send_request(&self, req: Request) -> Result<Response, JSONRPC_Error> {
        Ok(self.request(req)?)
    }

    fn send_batch(&self, reqs: &[Request]) -> Result<Vec<Response>, JSONRPC_Error> {
        Ok(self.request(reqs)?)
    }

    fn fmt_target(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}/{}", self.target_addr, self.wallet_path)
    }
}
