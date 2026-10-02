//! Request generator for the KVonset client to use.
//!
//! This module is feature-gated because only the client needs it.
//! Of course, the server still needs [`crate::Request`] and [`crate::Response`], so those types are not feature-gated.
//!
//! This crate assumes a key space of size 1,024.
//!
//! The functions in [`GenerateRequests`], the workload generator, return `impl Iterator<Item=Request>`.
//! It is possible to access the underlying iterators directly (e.g., for testing, or if you wish
//! to not use the `Iterator` trait: [`WarmUpPutRequests`] and [`MixedWorkload`].

use std::iter::FusedIterator;
use std::ops::{Deref, DerefMut};

use crate::Request;
use rand::rngs::SmallRng;
use rand::{Rng, RngExt, SeedableRng};
use rand_distr::Zipf;
use rand_distr::{Distribution, StandardGeometric};

/// [`GenerateRequests`] manages generating a series of `Request`.
///
/// It supports two phases:
/// - [`GenerateRequests::warmup_puts`]:
///   write a value to each of the 1,024 keys to warm up the server.
/// - [`GenerateRequests::mixed_workload`]:
///   generate a mix of range (4 / 256), put (32 / 256), and get queries (rest).
///
/// To help constrain nondeterminism, it is possible to pass a `seed` to
/// [`GenerateRequests::new`].
/// This will initialize a single PRNG, which both
/// `warmup_puts` and `mixed_workload` will use.
pub struct GenerateRequests {
    rng: SmallRng,
}

impl Default for GenerateRequests {
    fn default() -> Self {
        Self::new(None)
    }
}

impl GenerateRequests {
    /// Create a new `GenerateRequests`.
    ///
    /// `seed`: Optionally pass in a fixed seed.
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
        WarmUpPutRequests::new_with_rng(RngHolder::Borrowed(&mut self.rng))
    }

    /// Return an iterator which generates an infinite stream of mixed workload requests.
    pub fn mixed_workload(&mut self) -> impl Iterator<Item = Request> {
        MixedWorkload::new_with_rng(RngHolder::Borrowed(&mut self.rng))
    }
}

// similar to `std::borrow::Cow`, but instead of cloning on write we just have a &mut
enum RngHolder<'r> {
    Borrowed(&'r mut SmallRng),
    Owned(SmallRng),
}

impl<'r> Deref for RngHolder<'r> {
    type Target = SmallRng;
    fn deref(&self) -> &Self::Target {
        match self {
            RngHolder::Borrowed(v) => v,
            RngHolder::Owned(v) => v,
        }
    }
}

impl<'r> DerefMut for RngHolder<'r> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            RngHolder::Borrowed(v) => v,
            RngHolder::Owned(v) => v,
        }
    }
}

/// Generate PUT requests to (a) populate the kv store and (b) warm up the server.
///
/// Iterator generates 32 `mput` requests, each with 32 keys and values.
/// In total, each of the 1,024 keys in the key space will get written.
///
/// After this, the iterator will always return `None`.
pub struct WarmUpPutRequests<'a> {
    curr_req_num: u16,
    rng: RngHolder<'a>,
}

impl Default for WarmUpPutRequests<'static> {
    fn default() -> Self {
        Self::new(None)
    }
}

impl WarmUpPutRequests<'static> {
    pub fn new(seed: Option<u64>) -> Self {
        Self::new_with_rng(RngHolder::Owned(
            seed.and_then(|s| Some(SmallRng::seed_from_u64(s)))
                .unwrap_or_else(|| rand::make_rng()),
        ))
    }
}

impl<'a> WarmUpPutRequests<'a> {
    fn new_with_rng(rng: RngHolder<'a>) -> Self {
        Self { curr_req_num: 0, rng }
    }
}

// `i`: the `i`th request
fn warmup_put(i: u16, rng: &mut impl Rng) -> Request {
    Request::Mput((i * 32..(i + 1) * 32).map(|i| (i, Some(rng.random()))).collect())
}

impl<'a> Iterator for WarmUpPutRequests<'a> {
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

impl<'a> FusedIterator for WarmUpPutRequests<'a> {}

/// Generate a mix of range (4 / 256), put (32 / 256), and get queries (rest).
///
/// The iterator will never return `None`.
pub struct MixedWorkload<'r> {
    key_distr: Zipf<f64>,
    rng: RngHolder<'r>,
}

impl Default for MixedWorkload<'static> {
    fn default() -> Self {
        Self::new(None)
    }
}

/// The number of requests per 256 requests that should be [`Request::Range`].
pub const RANGE: u8 = 4;
/// The number of requests per 256 requests that should be [`Request::Mput`].
pub const PUT: u8 = 32;
/// The number of requests per 256 requests that should be [`Request::Mget`].
pub const GET: u8 = (u8::MAX - u8::MIN) - RANGE - PUT + 1;
/// Number of possible keys
pub const KEYSPACE: u16 = 1024;

impl MixedWorkload<'static> {
    pub fn new(seed: Option<u64>) -> Self {
        Self::new_with_rng(RngHolder::Owned(
            seed.and_then(|s| Some(SmallRng::seed_from_u64(s)))
                .unwrap_or_else(|| rand::make_rng()),
        ))
    }
}

impl<'r> MixedWorkload<'r> {
    fn new_with_rng(rng: RngHolder<'r>) -> Self {
        Self {
            key_distr: Zipf::new(KEYSPACE as _, 1.).expect("unable to initialize Zipf distribution"),
            rng,
        }
    }

    /// Make a `Request`.
    ///
    /// The workload is for a keyspace of size [`KEYSPACE`].
    /// Each key in a request is Zipf distributed in the keyspace.
    ///
    /// There are three types of requests:
    /// - Range (4 / 256): `key_start`, `key_end` uniformly distributed in [0, 1023].
    /// - Mput (32 / 256) and Mget (rest): Generate a `StandardGeometric`-distributed number of items per request.
    pub fn gen_request(&mut self) -> Request {
        let req_type: u8 = self.rng.random();

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
            // -1: `Zipf` generates [1, KEYSPACE] and we want [0, KEYSPACE -1].
            let sample_key = |rng: &mut SmallRng| rng.sample(key_distr) as u16 - 1;
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
                Request::Mget((0..items).map(|_| sample_key(&mut self.rng)).collect())
            }
        }
    }
}

impl<'r> Iterator for MixedWorkload<'r> {
    type Item = Request;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.gen_request())
    }
}

#[cfg(test)]
mod test_workload;
