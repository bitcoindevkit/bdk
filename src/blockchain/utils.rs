// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use std::collections::{HashMap, HashSet};

#[allow(unused_imports)]
use log::{debug, error, info, trace};
use rand::seq::SliceRandom;
use rand::thread_rng;

use bitcoin::{BlockHeader, OutPoint, Script, Transaction, Txid};

use super::*;
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{ConfirmationTime, KeychainKind, LocalUtxo, TransactionDetails};
use crate::wallet::time::Instant;
