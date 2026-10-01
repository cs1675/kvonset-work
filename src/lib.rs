//! kvonset-work: Generate a key-value workload.
//!
//! This crate assumes a key space of size 1,024.
//!
//! The client sends [`Request`]s, and the server responds with [`Response`]s.
//!
//! The functions in [`GenerateRequests`], the workload generator return `impl Iterator<Item=Request>`.
//! It is possible to access the underlying iterators directly (e.g., for testing, or if you wish
//! to not use the `Iterator` trait: [`WarmUpPutRequests`] and [`MixedWorkload`].

#[cfg(feature = "argparse")]
pub mod args;

use rand::rngs::SmallRng;
use rand::{Rng, RngExt, SeedableRng};
use rand_distr::Zipf;
use rand_distr::{Distribution, StandardGeometric};
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

/// [`GenerateRequests`] manages generating a series of `Request`.
///
/// It supports two phases:
/// - [`GenerateRequests::warmup_puts`]:
///   write a value to each of the 1,024 keys to warm up the server.
/// - [`GenerateRequests::mixed_workload`]:
///   generate a mix of range (4 / 256), put (32 / 256), and get queries (rest).
pub struct GenerateRequests {
    rng: SmallRng,
}

impl Default for GenerateRequests {
    fn default() -> Self {
        Self::new(None)
    }
}

impl GenerateRequests {
    pub fn new(seed: Option<u64>) -> Self {
        Self::new_with_rng(
            seed.and_then(|s| Some(SmallRng::seed_from_u64(s)))
                .unwrap_or_else(|| rand::make_rng()),
        )
    }

    pub fn new_with_rng(rng: SmallRng) -> Self {
        Self { rng }
    }

    /// Return an iterator which will fill the key space with PUTs.
    pub fn warmup_puts(&mut self) -> impl Iterator<Item = Request> {
        WarmUpPutRequests::new_with_rng(self.rng.clone())
    }

    /// Return an iterator which generates an infinite stream of mixed workload requests.
    pub fn mixed_workload(&self) -> impl Iterator<Item = Request> {
        MixedWorkload::new_with_rng(self.rng.clone())
    }
}

/// Generate PUT requests to (a) populate the kv store and (b) warm up the server.
///
/// Iterator generates 32 `mput` requests, each with 32 keys and values.
/// In total, each of the 1,024 keys in the key space will get written.
pub struct WarmUpPutRequests {
    curr_req_num: u16,
    rng: SmallRng,
}

impl WarmUpPutRequests {
    pub fn new(seed: Option<u64>) -> Self {
        Self::new_with_rng(
            seed.and_then(|s| Some(SmallRng::seed_from_u64(s)))
                .unwrap_or_else(|| rand::make_rng()),
        )
    }

    pub fn new_with_rng(rng: SmallRng) -> Self {
        Self {
            curr_req_num: 0,
            rng,
        }
    }
}

// `i`: the `i`th request
fn warmup_put(i: u16, rng: &mut impl Rng) -> Request {
    Request::Mput(
        (i * 32..(i + 1) * 32)
            .map(|i| (i, Some(rng.random())))
            .collect(),
    )
}

impl Iterator for WarmUpPutRequests {
    type Item = Request;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr_req_num >= 32 {
            None
        } else {
            let v = Some(warmup_put(self.curr_req_num, &mut self.rng));
            self.curr_req_num += 1;
            v
        }
    }
}

/// Generate a mix of range (4 / 256), put (32 / 256), and get queries (rest).
///
/// The iterator will never return `None`.
pub struct MixedWorkload {
    key_distr: Zipf<f64>,
    rng: SmallRng,
}

impl Default for MixedWorkload {
    fn default() -> Self {
        Self::new(None)
    }
}

impl MixedWorkload {
    pub fn new(seed: Option<u64>) -> Self {
        Self::new_with_rng(
            seed.and_then(|s| Some(SmallRng::seed_from_u64(s)))
                .unwrap_or_else(|| rand::make_rng()),
        )
    }

    pub fn new_with_rng(rng: SmallRng) -> Self {
        Self {
            key_distr: Zipf::new(1024.0, 1.)
                .expect("unable to initialize Zipf distribution"),
            rng,
        }
    }

    /// Make a `Request`.
    ///
    /// The workload is for a keyspace of size 1024.
    /// Each key in a request is Zipf distributed in the keyspace.
    ///
    /// There are three types of requests:
    /// - Range (4 / 256): `key_start`, `key_end` uniformly distributed in [0, 1023].
    /// - Mput (32 / 256) and Mget (rest): Generate a `StandardGeometric`-distributed number of items per request.
    pub fn gen_request(&mut self) -> Request {
        let req_type: u8 = self.rng.random();
        const RANGE: u8 = 4;
        const PUT: u8 = 32;

        if req_type < RANGE {
            // range
            let range: u32 = self.rng.random();
            // lower 10 bits
            let lower = range & 0x3ff;
            // next 10 bits
            let upper = (range >> 10) & 0x3ff;
            let start = lower.min(upper) as _;
            let end = lower.max(upper) as _;
            Request::Range { start, end }
        } else {
            // +1: `StandardGeometric` is the number of failed coin flips before a success, which could be 0. Add 1 to have at least one request.
            let items = StandardGeometric.sample(&mut self.rng) + 1;
            let key_distr = self.key_distr;
            // -1: `Zipf` generates [1, 1024] and we want [0, 1023].
            let sample_key =
                |rng: &mut SmallRng| rng.sample(key_distr) as u16 - 1;
            if req_type < const { RANGE + PUT } {
                Request::Mput(
                    (0..items)
                        .map(|_| {
                            let key = sample_key(&mut self.rng);
                            let del = self.rng.random_bool(0.05);
                            if del {
                                (key, None)
                            } else {
                                let mut v = [0u8; 8];
                                self.rng.fill_bytes(&mut v);
                                (key, Some(v))
                            }
                        })
                        .collect(),
                )
            } else {
                Request::Mget(
                    (0..items).map(|_| sample_key(&mut self.rng)).collect(),
                )
            }
        }
    }
}

impl Iterator for MixedWorkload {
    type Item = Request;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.gen_request())
    }
}
