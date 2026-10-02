//! kvonset-work: Express a key-value workload.
//!
//! The client sends [`Request`]s, and the server responds with [`Response`]s.
//!
//! ## Modules
//! - [`args`] (feature `argparse`):
//!   Command-line argument parsing logic that is compatible with the run script.
//! - [`request_gen`] (feature `request-gen`):
//!   Request generator for the client to use.

#[cfg(feature = "argparse")]
pub mod args;

#[cfg(feature = "request-gen")]
pub mod request_gen;

use serde::*;

/// A KVonset request.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Request {
    /// For local testing only.
    Ping,
    /// A list of (key, value) tuples to set.
    ///
    /// To delete a key, use key <- None.
    Mput(Vec<(u16, Option<[u8; 8]>)>),
    /// A list of keys to fetch.
    Mget(Vec<u16>),
    /// A range of keys to fetch.
    Range { start: u16, end: u16 },
}

/// A KVonset response.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Response {
    /// For local testing only.
    Ping,
    /// A list of set keys and their previously set values.
    Mput(Vec<(u16, Option<[u8; 8]>)>),
    /// Currently set values for the given keys.
    Mget(Vec<(u16, Option<[u8; 8]>)>),
    /// A list of set keys and their currently set values.
    Range(Vec<(u16, [u8; 8])>),
}
