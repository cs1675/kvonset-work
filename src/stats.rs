//! Helper module for writing the KVonset client.

use std::time::Duration;

use crate::{Request, Response};

#[derive(Clone, Debug)]
pub struct ExperimentStats {
    /// For each connection, every request it sent. Requests that got a response are
    /// [`RequestState::Summarized`]; the rest are [`RequestState::Sent`].
    pub conns: Vec<Vec<RequestState>>,
    /// When the open-loop phase started. Send times are relative to this.
    pub start: quanta::Instant,
    /// How long the senders were sending for. Offered and achieved load are relative to this.
    pub send_duration: Duration,
    pub attempted_interarrival: Duration,
}

/// Summary statistics from a client.
#[derive(Clone, Debug)]
pub struct SummaryStats {
    /// Attempted request inter-arrival time:
    pub attempted_interarrival_requests: std::time::Duration,

    /// Avg. request inter-arrival time:
    pub mean_request_inter_arrival: std::time::Duration,

    /// Experiment time during which the offered load was offered.
    ///
    /// Removes warmup and cooldown, which are times during which
    /// the offered load is lower than the target.
    ///
    /// - warmup:   Time between connection establishment
    ///             and the first response `r` such that
    ///             at least one response was received before
    ///             the request for `r` was sent.
    /// - cooldown: Time between
    ///             the send time of the last request
    ///             for which a response was received
    ///             and the client shutdown
    pub active_experiment_duration: std::time::Duration,

    pub sent_requests: usize,
    pub sent_mget_reqs: usize,
    pub sent_mput_reqs: usize,
    pub sent_range_reqs: usize,
    pub completed_requests: usize,
    pub attempted_reqs_per_sec: f64,
    pub offered_reqs_per_sec: f64,
    pub achieved_reqs_per_sec: f64,

    pub sent_keys: usize,
    pub completed_keys: usize,
    pub attempted_keys_per_sec: f64,
    pub offered_keys_per_sec: f64,
    pub achieved_keys_per_sec: f64,

    /// Histogram of latencies per connection, with units microseconds
    pub overall_latency_us_hist: Vec<hdrhistogram::Histogram<u64>>,
    /// Histogram of mget latencies per connection, with units microseconds
    pub mget_latency_us_hist: Vec<hdrhistogram::Histogram<u64>>,
    /// Histogram of mput latencies per connection, with units microseconds
    pub mput_latency_us_hist: Vec<hdrhistogram::Histogram<u64>>,
    /// Histogram of range latencies per connection, with units microseconds
    pub range_latency_us_hist: Vec<hdrhistogram::Histogram<u64>>,
}

impl SummaryStats {
    /// Retrieve the given `quantile` for the given `Op` per connection,
    /// and return the median value across connections.
    ///
    /// # Requirements
    ///
    /// - `quantile` must be between 0 and 1
    /// - `Op` must not be `Ping`
    ///
    /// Otherwise, returns `None`.
    pub fn latency_quantile_us(&self, op: Option<Op>, quantile: f64) -> Option<u64> {
        if quantile < 0. || quantile > 1. {
            return None;
        }

        let mut per_conn: Vec<_> = match op {
            None => &self.overall_latency_us_hist,
            Some(Op::Ping) => return None,
            Some(Op::Mget) => &self.mget_latency_us_hist,
            Some(Op::Mput) => &self.mput_latency_us_hist,
            Some(Op::Range) => &self.range_latency_us_hist,
        }
        .iter()
        .map(|h| h.value_at_quantile(quantile))
        .collect();
        per_conn.sort();
        if per_conn.is_empty() {
            None
        } else {
            Some(per_conn[per_conn.len() / 2])
        }
    }

    /// Given `outdir`, will append to `[outdir]/leaderboard.csv`
    /// a row representing this `SummaryStats`.
    ///
    /// A latency value of 0 indicates an error.
    pub fn write_leaderboard(&self, outdir: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;

        // why 0.999? otherwise, it would be possible to just drop range requests
        if self.achieved_keys_per_sec < self.offered_keys_per_sec * 0.999 {
            // don't write anything, but this is not an error
            return Ok(());
        }

        let outpath = outdir.join("leaderboard.csv");
        let mut f = if outpath.try_exists()? {
            std::fs::File::options().append(true).open(outpath)?
        } else {
            let mut f = std::fs::File::create(outpath)?;
            writeln!(
                &mut f,
                "achieved_load_keys_per_sec,latency_p5,latency_p25,latency_p50,latency_p75,latency_p95,latency_p999"
            )?;
            f
        };

        writeln!(
            &mut f,
            "{achieved},{p5},{p25},{p50},{p75},{p95},{p999}",
            achieved = self.achieved_keys_per_sec,
            p5 = self.latency_quantile_us(None, 0.05).unwrap_or(0),
            p25 = self.latency_quantile_us(None, 0.25).unwrap_or(0),
            p50 = self.latency_quantile_us(None, 0.50).unwrap_or(0),
            p75 = self.latency_quantile_us(None, 0.75).unwrap_or(0),
            p95 = self.latency_quantile_us(None, 0.95).unwrap_or(0),
            p999 = self.latency_quantile_us(None, 0.999).unwrap_or(0),
        )?;

        Ok(())
    }
}

impl ExperimentStats {
    /// Summarize the experiment.
    ///
    /// Request and key counts (`sent_*`, `completed_*`) cover the whole experiment. Offered and
    /// achieved loads only count the active part of the experiment (see
    /// [`SummaryStats::active_experiment_duration`]): offered load counts requests sent during it,
    /// and achieved load counts responses received during it.
    pub fn get_summary_stats(&self) -> SummaryStats {
        // (op, keys, send time, completion time if a response was received)
        let sent: Vec<(Op, usize, quanta::Instant, Option<quanta::Instant>)> = self
            .conns
            .iter()
            .flatten()
            .filter_map(|s| match s {
                RequestState::Sent { req, send_time, .. } => Some((req.into(), req.num_keys(), *send_time, None)),
                RequestState::Summarized {
                    op,
                    num_keys,
                    send_time,
                    complete_time,
                    ..
                } => Some((*op, *num_keys, *send_time, Some(*complete_time))),
                // Neither sent nor checked.
                RequestState::NotSent { .. } | RequestState::Received { .. } => None,
            })
            .collect();
        let completed = || {
            sent.iter()
                .filter_map(|&(op, keys, send, done)| Some((op, keys, send, done?)))
        };

        let sent_ops = |want: Op| sent.iter().filter(|(op, ..)| *op == want).count();
        let sent_keys = sent.iter().map(|(_, keys, ..)| keys).sum();
        let completed_requests = completed().count();
        let completed_keys = completed().map(|(_, keys, ..)| keys).sum();

        // Mean gap between consecutive send times.
        let mean_request_inter_arrival = match (
            sent.iter().map(|(_, _, send, _)| send).min(),
            sent.iter().map(|(_, _, send, _)| send).max(),
        ) {
            (Some(first), Some(last)) if sent.len() > 1 => last.duration_since(*first) / (sent.len() - 1) as u32,
            _ => Duration::ZERO,
        };

        // Warmup ends with the first response to a request sent after some response was received;
        // cooldown starts with the send time of the last request that got a response.
        let first_response = completed().map(|(.., done)| done).min();
        let active_start = first_response.and_then(|first| {
            completed()
                .filter(|&(_, _, send, _)| send > first)
                .map(|(.., done)| done)
                .min()
        });
        let active_end = completed().map(|(_, _, send, _)| send).max();
        let (active_experiment_duration, offered, achieved) = match (active_start, active_end) {
            (Some(active_start), Some(active_end)) if active_start < active_end => {
                let active = active_start..=active_end;
                let offered = sent.iter().filter(|(_, _, send, _)| active.contains(send));
                let achieved = completed().filter(|(.., done)| active.contains(done));
                (
                    active_end.duration_since(active_start),
                    (offered.clone().count(), offered.map(|(_, keys, ..)| keys).sum()),
                    (achieved.clone().count(), achieved.map(|(_, keys, ..)| keys).sum()),
                )
            }
            _ => (Duration::ZERO, (0, 0), (0, 0)),
        };
        let per_sec = |n: usize| {
            if active_experiment_duration.is_zero() {
                0.
            } else {
                n as f64 / active_experiment_duration.as_secs_f64()
            }
        };

        let attempted_reqs_per_sec = 1. / self.attempted_interarrival.as_secs_f64();

        // One histogram of each kind per connection, in the same order as `self.conns`.
        let mut overall_latency_us_hist = Vec::with_capacity(self.conns.len());
        let mut mget_latency_us_hist = Vec::with_capacity(self.conns.len());
        let mut mput_latency_us_hist = Vec::with_capacity(self.conns.len());
        let mut range_latency_us_hist = Vec::with_capacity(self.conns.len());
        for conn in &self.conns {
            let new_hist = || hdrhistogram::Histogram::<u64>::new(3).expect("3 significant figures is valid");
            let (mut overall, mut mget, mut mput, mut range) = (new_hist(), new_hist(), new_hist(), new_hist());
            for s in conn {
                let RequestState::Summarized {
                    op,
                    send_time,
                    complete_time,
                    ..
                } = s
                else {
                    continue;
                };

                let latency_us = complete_time.duration_since(*send_time).as_micros() as u64;
                overall.saturating_record(latency_us);
                match op {
                    Op::Mget => mget.saturating_record(latency_us),
                    Op::Mput => mput.saturating_record(latency_us),
                    Op::Range => range.saturating_record(latency_us),
                    Op::Ping => {}
                }
            }

            overall_latency_us_hist.push(overall);
            mget_latency_us_hist.push(mget);
            mput_latency_us_hist.push(mput);
            range_latency_us_hist.push(range);
        }

        SummaryStats {
            attempted_interarrival_requests: self.attempted_interarrival,
            mean_request_inter_arrival,
            active_experiment_duration,
            sent_requests: sent.len(),
            sent_mget_reqs: sent_ops(Op::Mget),
            sent_mput_reqs: sent_ops(Op::Mput),
            sent_range_reqs: sent_ops(Op::Range),
            completed_requests,
            attempted_reqs_per_sec,
            offered_reqs_per_sec: per_sec(offered.0),
            achieved_reqs_per_sec: per_sec(achieved.0),
            sent_keys,
            completed_keys,
            attempted_keys_per_sec: attempted_reqs_per_sec * crate::request_gen::MEAN_KEYS_PER_REQUEST,
            offered_keys_per_sec: per_sec(offered.1),
            achieved_keys_per_sec: per_sec(achieved.1),
            overall_latency_us_hist,
            mget_latency_us_hist,
            mput_latency_us_hist,
            range_latency_us_hist,
        }
    }
}

/// Keep track of requests we sent
#[derive(Debug, Clone)]
pub enum RequestState {
    /// Request hasn't been sent yet
    NotSent { id: usize, req: Request },
    /// Request has been sent, but response not received
    Sent {
        id: usize,
        req: Request,
        send_time: quanta::Instant,
    },
    /// Request sent, and response received
    Received {
        id: usize,
        req: Request,
        resp: Response,
        send_time: quanta::Instant,
        complete_time: quanta::Instant,
    },
    /// Pack down to the data we actually care about
    Summarized {
        id: usize,
        op: Op,
        num_keys: usize,
        send_time: quanta::Instant,
        complete_time: quanta::Instant,
    },
}

impl RequestState {
    pub fn new(id: usize, req: Request) -> Self {
        RequestState::NotSent { id, req }
    }

    /// If the current value is `NotSent`, record the current time as the send time.
    /// Otherwise, return `self` as the `Err`
    pub fn sent(self) -> Result<Self, Self> {
        match self {
            RequestState::NotSent { id, req } => Ok(RequestState::Sent {
                id,
                req,
                send_time: quanta::Instant::now(),
            }),
            x => Err(x),
        }
    }

    /// If the current value is `Sent`, record `resp` and the current time as the completion time.
    /// Otherwise, return `self` as the `Err`
    pub fn received(self, resp: Response) -> Result<Self, Self> {
        match self {
            RequestState::Sent { id, req, send_time } => Ok(RequestState::Received {
                id,
                req,
                resp,
                send_time,
                complete_time: quanta::Instant::now(),
            }),
            x => Err(x),
        }
    }

    /// If the current value is `Received`, drop the request and response contents, keeping only
    /// the operation type and the number of keys in the request (see [`Request::num_keys`]).
    ///
    /// Also check that the response is a plausible one for this request.
    pub fn summarize(self) -> Result<Self, KVonsetError> {
        match self {
            RequestState::Received {
                id,
                req,
                resp,
                send_time,
                complete_time,
            } => {
                check_response(&req, &resp)?;
                let op = Op::from(&req);
                let num_keys = req.num_keys();
                Ok(RequestState::Summarized {
                    id,
                    op,
                    num_keys,
                    send_time,
                    complete_time,
                })
            }
            r @ RequestState::Summarized { .. } => Ok(r),
            r => Err(KVonsetError::NotReceived(r)),
        }
    }
}

/// Types of requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Ping,
    Mget,
    Mput,
    Range,
}

impl std::fmt::Display for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Op::Ping => "ping",
            Op::Mput => "mput",
            Op::Mget => "mget",
            Op::Range => "range",
        })
    }
}

impl From<&Request> for Op {
    fn from(value: &Request) -> Self {
        match value {
            Request::Ping => Op::Ping,
            Request::Mget(_) => Op::Mget,
            Request::Mput(_) => Op::Mput,
            Request::Range { .. } => Op::Range,
        }
    }
}

impl From<&Response> for Op {
    fn from(value: &Response) -> Self {
        match value {
            Response::Pong => Op::Ping,
            Response::Mget(_) => Op::Mget,
            Response::Mput(_) => Op::Mput,
            Response::Range(_) => Op::Range,
        }
    }
}

/// Check that `resp` is a plausible response to `req`.
fn check_response(req: &Request, resp: &Response) -> Result<(), KVonsetError> {
    match (req, resp) {
        (Request::Ping, Response::Pong) => {}
        (Request::Mget(ks), Response::Mget(got)) => {
            if let Some((index, expected, got)) = first_mismatch(ks.iter().copied(), got.iter().map(|(k, _)| *k)) {
                return Err(KVonsetError::MGetKeys { index, expected, got });
            }
        }
        (Request::Mput(kvs), Response::Mput(got)) => {
            if let Some((index, expected, got)) =
                first_mismatch(kvs.iter().map(|(k, _)| *k), got.iter().map(|(k, _)| *k))
            {
                return Err(KVonsetError::MPutKeys { index, expected, got });
            }
        }
        (Request::Range { start, end }, Response::Range(got)) => {
            if let Some((k, _)) = got.iter().find(|(k, _)| k < start || k > end) {
                return Err(KVonsetError::RangeKeys {
                    start: *start,
                    end: *end,
                    key: *k,
                });
            }
            if let Some((i, w)) = got.windows(2).enumerate().find(|(_, w)| w[0].0 > w[1].0) {
                return Err(KVonsetError::RangeOrder {
                    index: i + 1,
                    prev: w[0].0,
                    key: w[1].0,
                });
            }
        }
        _ => {
            return Err(KVonsetError::TypeMismatch {
                request: req.into(),
                response: resp.into(),
            });
        }
    }

    Ok(())
}

/// Something that went wrong
#[derive(Debug, Clone)]
pub enum KVonsetError {
    /// mget response keys do not match request
    MGetKeys {
        /// position of the first mismatched key
        index: usize,
        /// key in the request at `index`, if any
        expected: Option<u16>,
        /// key in the response at `index`, if any
        got: Option<u16>,
    },
    /// mput response keys do not match request
    MPutKeys {
        /// position of the first mismatched key
        index: usize,
        /// key in the request at `index`, if any
        expected: Option<u16>,
        /// key in the response at `index`, if any
        got: Option<u16>,
    },
    /// range response contains keys outside [{start}, {end}]
    RangeKeys { start: u16, end: u16, key: u16 },
    /// range response is not sorted by key
    RangeOrder {
        /// position of the out-of-order key
        index: usize,
        /// key at `index - 1`
        prev: u16,
        /// key at `index`
        key: u16,
    },
    /// response type does not match request type
    TypeMismatch { request: Op, response: Op },
    /// tried to summarize a request that is not in the `Received` state
    NotReceived(RequestState),
}

impl std::fmt::Display for KVonsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MGetKeys { index, expected, got } => write!(
                f,
                "mget response keys do not match request: at index {index}, expected {expected:?}, got {got:?}"
            ),
            Self::MPutKeys { index, expected, got } => write!(
                f,
                "mput response keys do not match request: at index {index}, expected {expected:?}, got {got:?}"
            ),
            Self::RangeKeys { start, end, key } => {
                write!(f, "range response contains key {key} outside [{start}, {end}]")
            }
            Self::RangeOrder { index, prev, key } => write!(
                f,
                "range response is not sorted by key: key {key} at index {index} follows {prev}"
            ),
            Self::TypeMismatch { request, response } => write!(
                f,
                "response type does not match request type: {request} request, {response} response"
            ),
            Self::NotReceived(RequestState::NotSent { id, .. }) => {
                write!(f, "cannot summarize request {id}: was never sent")
            }
            Self::NotReceived(RequestState::Sent { id, .. }) => {
                write!(f, "cannot summarize request {id}: was never received")
            }
            Self::NotReceived(_) => unreachable!(),
        }
    }
}

impl std::error::Error for KVonsetError {}

/// Find the first position at which `expected` and `got` differ, including length mismatches.
fn first_mismatch(
    expected: impl IntoIterator<Item = u16>,
    got: impl IntoIterator<Item = u16>,
) -> Option<(usize, Option<u16>, Option<u16>)> {
    let mut expected = expected.into_iter();
    let mut got = got.into_iter();
    let mut index = 0;
    loop {
        match (expected.next(), got.next()) {
            (None, None) => return None,
            (e, g) if e != g => return Some((index, e, g)),
            _ => index += 1,
        }
    }
}
