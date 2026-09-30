//! Check the workload generators against the behavior described in `public/handout.md`.

use std::collections::HashSet;

use kvonset_work::{MixedWorkload, Request, WarmUpPutRequests};

const KEY_SPACE: u16 = 1024;
const SEED: u64 = 1675;
const NUM_SAMPLES: usize = 1_000_000;

/// Number of keys a request touches, as counted by the handout's keys / second load accounting.
fn num_keys(req: &Request) -> usize {
    match req {
        Request::Ping => 0,
        Request::Mput(kvs) => kvs.len(),
        Request::Mget(ks) => ks.len(),
        Request::Range { start, end } => (end - start) as usize + 1,
    }
}

#[test]
fn warmup_puts_span_key_space() {
    let reqs: Vec<_> = WarmUpPutRequests::new(Some(SEED)).collect();
    assert_eq!(reqs.len(), 32, "expected 32 warm-up requests");

    let mut seen = HashSet::new();
    for req in &reqs {
        let Request::Mput(kvs) = req else {
            panic!("warm-up request is not an mput: {req:?}");
        };
        assert_eq!(kvs.len(), 32, "expected 32 keys per warm-up request");
        for (k, v) in kvs {
            assert!(v.is_some(), "warm-up put of key {k} has no value");
            assert!(seen.insert(*k), "key {k} written more than once");
        }
    }

    // Every key the mixed workload can ask for must have been populated by the warm-up.
    let mixed_keys: HashSet<u16> = MixedWorkload::new(Some(SEED))
        .take(NUM_SAMPLES)
        .flat_map(|req| match req {
            Request::Mput(kvs) => kvs.into_iter().map(|(k, _)| k).collect(),
            Request::Mget(ks) => ks,
            Request::Range { start, end } => vec![start, end],
            Request::Ping => vec![],
        })
        .collect();
    let unwarmed: Vec<_> = mixed_keys.difference(&seen).collect();
    assert!(
        unwarmed.is_empty(),
        "mixed workload uses keys never written during warm-up: {unwarmed:?}"
    );

    assert_eq!(seen.len(), KEY_SPACE as usize);
}

#[test]
fn mixed_workload_distribution() {
    let (mut range, mut mput, mut mget) = (0usize, 0usize, 0usize);
    let (mut range_keys, mut multi_keys) = (0usize, 0usize);
    let mut total_keys = 0usize;

    for req in MixedWorkload::new(Some(SEED)).take(NUM_SAMPLES) {
        let n = num_keys(&req);
        total_keys += n;
        match req {
            Request::Range { start, end } => {
                assert!(start <= end, "range start {start} > end {end}");
                assert!(end < KEY_SPACE, "range end {end} outside key space");
                range += 1;
                range_keys += n;
            }
            Request::Mput(_) => {
                mput += 1;
                multi_keys += n;
            }
            Request::Mget(_) => {
                mget += 1;
                multi_keys += n;
            }
            Request::Ping => panic!("mixed workload generated a ping"),
        }
    }

    let frac = |c: usize| c as f64 / NUM_SAMPLES as f64;
    let assert_close = |what: &str, got: f64, want: f64, tol: f64| {
        assert!(
            (got - want).abs() <= tol,
            "{what}: got {got:.4}, expected {want:.4} +/- {tol}"
        );
    };

    // Request type mix: 4 / 256 range, 32 / 256 mput, remainder mget.
    assert_close("range fraction", frac(range), 4. / 256., 0.001);
    assert_close("mput fraction", frac(mput), 32. / 256., 0.002);
    assert_close("mget fraction", frac(mget), 220. / 256., 0.002);

    // Range width: difference of two uniform samples in the key space averages 1024 / 3.
    assert_close(
        "mean keys per range",
        range_keys as f64 / range as f64,
        1024. / 3.,
        5.,
    );

    // mget / mput sizes follow Geom(0.5), which the handout says has mean 2.
    assert_close(
        "mean keys per mget/mput",
        multi_keys as f64 / (mput + mget) as f64,
        2.,
        0.02,
    );

    // Overall, the average request has ~7.3 keys.
    assert_close(
        "mean keys per request",
        total_keys as f64 / NUM_SAMPLES as f64,
        7.3,
        0.1,
    );
}
