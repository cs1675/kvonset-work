//! Command-line argument parser that will be compatible with the grading server.
//!
//! It is not required to use this module, but if you want to use it, you can by specifying:
//!
//! ```toml
//! kvonset-work = { git = "https://github.com/cs1675/kvonset-work", features = ["argparse"] }
//! ```
//!
//! in Cargo.toml and then `use kvonset_work::args::*;` in your implementation.

use clap::{Parser, ValueEnum};
use std::net::Ipv4Addr;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(about)]
pub struct KVonsetClientOpt {
    /// Provided hint for when to finish writing logs and exit before the runner script terminates
    /// this process.
    #[arg(short, long)]
    pub runtime_secs: u64,

    #[arg(long)]
    pub ip: Ipv4Addr,

    #[arg(short, long)]
    pub port: u16,

    /// PRNG seed to pass to [`GenerateRequests`]
    #[arg(short, long)]
    pub seed: Option<u64>,

    /// Attempted load to offer, in keys / second
    #[arg(short, long)]
    pub load_keys_attempted: u64,

    /// Only files written to this directory will be preserved; the runner script will delete all
    /// other files.
    #[arg(short, long)]
    pub outpath: PathBuf,
}

#[cfg(feature = "request-gen")]
impl KVonsetClientOpt {
    /// Calculate a target inter-arrival time
    /// given the provided number of keys per second of attempted load.
    ///
    /// The mixed workload averages ~7.3 keys / request
    /// (see `request_gen::test_workload::mixed_workload_distribution`), so at an attempted load of
    /// 10,000 keys / second, requests should be sent ~730µs apart.
    ///
    /// ```rust
    /// use clap::Parser;
    /// use kvonset_work::args::KVonsetClientOpt;
    ///
    /// let opt = |load: &str| {
    ///     KVonsetClientOpt::parse_from([
    ///         "client", "-r", "10", "--ip", "127.0.0.1", "-p", "4242", "-o", "out", "-l", load,
    ///     ])
    /// };
    ///
    /// // 2 keys / mget or mput * (252 / 256) + (1024 / 3) keys / range * (4 / 256)
    /// let keys_per_req = 2. * 252. / 256. + (1024. / 3.) * 4. / 256.;
    /// assert!((keys_per_req - 7.3_f64).abs() < 0.01);
    ///
    /// let got = opt("10000").target_interarrival().as_secs_f64();
    /// assert!((got - keys_per_req / 10_000.).abs() < 1e-9, "got {got}");
    ///
    /// // Doubling the attempted load halves the inter-arrival time.
    /// let doubled = opt("20000").target_interarrival().as_secs_f64();
    /// assert!((doubled - got / 2.).abs() < 1e-9, "got {doubled}");
    /// ```
    pub fn target_interarrival(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(
            // First calculate the number of keys / request
            const {
                use crate::request_gen::{GET, KEYSPACE, PUT, RANGE};
                // Why `2. *`?: The mean of a StandardGeometric distribution,
                //   which determines Mget and Mput request size, is 2.
                2. * (GET as f64 + PUT as f64)
                  / (u8::MAX as f64 + 1.)
                // Why `/ 3`?: The mean of the absolute difference of two
                //   values in KEYSPACE generated uniformly at random
                //   is a Triangular(a, b, c) distribution with mode c = 0,
                //   which has mean (a - b) / 3.
                + (KEYSPACE as f64 / 3.)
                      * RANGE as f64
                      / (u8::MAX as f64 + 1.)
            }
            // Why `/ self.load_keys_attempted`?: The above part calculates
            //   the average number of keys / request.
            //    seconds      key     seconds     key       key
            //    ------- =  ------- * ------- = ------- / -------
            //    request    request     key     request   seconds
            //   `self.load_keys_attempted` is key / second, so its inverse is seconds / key
            / self.load_keys_attempted as f64,
        )
    }
}

#[derive(Parser, Debug, Clone)]
#[command(about)]
pub struct KVonsetServerOpt {
    #[arg(short, long)]
    pub port: u16,

    /// Identify the implementation strategy the server should use.
    #[arg(short, long)]
    pub strategy: Strategy,

    /// Provided hint for when to finish writing logs and exit before the runner script terminates
    /// this process.
    #[arg(short, long)]
    pub runtime_secs: u64,

    /// Only files written to this directory will be preserved; the runner script will delete all
    /// other files.
    #[arg(short, long)]
    pub outpath: PathBuf,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum Strategy {
    /// Use the traditional send/recv socket API.
    SendRecv,
    /// Use vectored IO.
    VectoredIO,
    /// Use `io_uring`.
    IoUring,
    /// Target the latency-focused SLO.
    LatencySLO,
    /// Target the throughput-focused SLO.
    ThroughputSLO,
    /// (Optional) Target a submission to the class leaderboard.
    Leaderboard,
}

#[cfg(test)]
mod t {
    use super::{KVonsetClientOpt, KVonsetServerOpt, Strategy};
    use clap::{CommandFactory, Parser, error::ErrorKind};
    use std::net::Ipv4Addr;
    use std::path::PathBuf;

    const CLIENT_COMMON: &[&str] = &[
        "client",
        "--runtime-secs",
        "10",
        "--ip",
        "10.0.0.1",
        "--port",
        "4242",
        "--load-keys-attempted",
        "10000",
        "--outpath",
        "out.data",
    ];

    const SERVER_COMMON: &[&str] = &[
        "server",
        "--runtime-secs",
        "30",
        "--port",
        "4242",
        "--strategy",
        "send-recv",
        "--outpath",
        "server.data",
    ];

    fn with(base: &[&'static str], flag: &str, val: &'static str) -> Vec<&'static str> {
        let mut args = base.to_vec();
        let i = args.iter().position(|a| *a == flag).unwrap();
        args[i + 1] = val;
        args
    }

    fn without(base: &[&'static str], flag: &str) -> Vec<&'static str> {
        let mut args = base.to_vec();
        let i = args.iter().position(|a| *a == flag).unwrap();
        args.drain(i..i + 2);
        args
    }

    fn client_err(args: &[&str]) -> ErrorKind {
        KVonsetClientOpt::try_parse_from(args)
            .expect_err("parse should fail")
            .kind()
    }

    fn server_err(args: &[&str]) -> ErrorKind {
        KVonsetServerOpt::try_parse_from(args)
            .expect_err("parse should fail")
            .kind()
    }

    #[test]
    fn client_command_is_well_formed() {
        KVonsetClientOpt::command().debug_assert();
    }

    #[test]
    fn server_command_is_well_formed() {
        KVonsetServerOpt::command().debug_assert();
    }

    #[test]
    fn client_long_flags() {
        let opt = KVonsetClientOpt::try_parse_from(CLIENT_COMMON).expect("parse client");

        assert_eq!(opt.runtime_secs, 10);
        assert_eq!(opt.ip, Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(opt.port, 4242);
        assert_eq!(opt.load_keys_attempted, 10000);
        assert_eq!(opt.outpath, PathBuf::from("out.data"));
    }

    #[test]
    fn client_short_flags() {
        let opt = KVonsetClientOpt::try_parse_from([
            "client",
            "-r",
            "5",
            "--ip",
            "127.0.0.1",
            "-p",
            "8080",
            "-l",
            "500",
            "-o",
            "x.out",
        ])
        .expect("parse client");

        assert_eq!(opt.runtime_secs, 5);
        assert_eq!(opt.ip, Ipv4Addr::LOCALHOST);
        assert_eq!(opt.port, 8080);
        assert_eq!(opt.load_keys_attempted, 500);
        assert_eq!(opt.outpath, PathBuf::from("x.out"));
    }

    #[test]
    fn client_missing_args() {
        for flag in ["--runtime-secs", "--ip", "--port", "--outpath"] {
            assert_eq!(
                client_err(&without(CLIENT_COMMON, flag)),
                ErrorKind::MissingRequiredArgument,
                "missing {flag}"
            );
        }
    }

    #[test]
    fn client_invalid_values() {
        let err = |flag, val| client_err(&with(CLIENT_COMMON, flag, val));

        assert_eq!(err("--ip", "not-an-ip"), ErrorKind::ValueValidation);
        assert_eq!(err("--ip", "::1"), ErrorKind::ValueValidation);
        assert_eq!(err("--port", "70000"), ErrorKind::ValueValidation);
        assert_eq!(err("--runtime-secs", "ten"), ErrorKind::ValueValidation);
    }

    #[test]
    fn client_extra_args() {
        let mut args = CLIENT_COMMON.to_vec();
        args.push("closed-loop");
        assert_eq!(client_err(&args), ErrorKind::UnknownArgument);

        let mut args = CLIENT_COMMON.to_vec();
        args.extend(["--strategy", "send-recv"]);
        assert_eq!(client_err(&args), ErrorKind::UnknownArgument);

        // The client has no short flag for --ip.
        assert_eq!(
            client_err(&["client", "-r", "1", "-i", "1.2.3.4", "-p", "1", "-o", "c"]),
            ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn server_long_flags() {
        let opt = KVonsetServerOpt::try_parse_from(SERVER_COMMON).expect("parse server");

        assert_eq!(opt.runtime_secs, 30);
        assert_eq!(opt.port, 4242);
        assert!(matches!(opt.strategy, Strategy::SendRecv));
        assert_eq!(opt.outpath, PathBuf::from("server.data"));
    }

    #[test]
    fn server_short_flags() {
        let opt = KVonsetServerOpt::try_parse_from(["server", "-r", "1", "-p", "1", "-s", "io-uring", "-o", "s"])
            .expect("parse server short flags");

        assert_eq!(opt.runtime_secs, 1);
        assert_eq!(opt.port, 1);
        assert!(matches!(opt.strategy, Strategy::IoUring));
        assert_eq!(opt.outpath, PathBuf::from("s"));
    }

    #[test]
    fn server_strategies() {
        let parse = |val| {
            KVonsetServerOpt::try_parse_from(with(SERVER_COMMON, "--strategy", val))
                .unwrap_or_else(|e| panic!("parse strategy {val}: {e}"))
                .strategy
        };

        assert!(matches!(parse("send-recv"), Strategy::SendRecv));
        assert!(matches!(parse("vectored-io"), Strategy::VectoredIO));
        assert!(matches!(parse("io-uring"), Strategy::IoUring));
        assert!(matches!(parse("latency-slo"), Strategy::LatencySLO));
        assert!(matches!(parse("throughput-slo"), Strategy::ThroughputSLO));
        assert!(matches!(parse("leaderboard"), Strategy::Leaderboard));
    }

    #[test]
    fn server_missing_args() {
        for flag in ["--runtime-secs", "--port", "--strategy", "--outpath"] {
            assert_eq!(
                server_err(&without(SERVER_COMMON, flag)),
                ErrorKind::MissingRequiredArgument,
                "missing {flag}"
            );
        }
    }

    #[test]
    fn server_invalid_values() {
        let err = |flag, val| server_err(&with(SERVER_COMMON, flag, val));

        assert_eq!(err("--strategy", "epoll"), ErrorKind::InvalidValue);
        assert_eq!(err("--strategy", "SendRecv"), ErrorKind::InvalidValue);
        assert_eq!(err("--port", "70000"), ErrorKind::ValueValidation);
        assert_eq!(err("--runtime-secs", "ten"), ErrorKind::ValueValidation);
    }

    #[test]
    fn server_extra_args() {
        let mut args = SERVER_COMMON.to_vec();
        args.push("closed-loop");
        assert_eq!(server_err(&args), ErrorKind::UnknownArgument);

        let mut args = SERVER_COMMON.to_vec();
        args.extend(["--ip", "1.2.3.4"]);
        assert_eq!(server_err(&args), ErrorKind::UnknownArgument);
    }
}
