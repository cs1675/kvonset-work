# kvonset-work

kvonset-work: Express a key-value workload.

The client sends [`Request`]s, and the server responds with [`Response`]s.

### Modules
- [`args`] (feature `argparse`):
  Command-line argument parsing logic that is compatible with the run script.
- [`request_gen`] (feature `request-gen`):
  Request generator for the client to use.
- [`stats`] (feature `stats`):
  Helper functions for calculating summary statistics
  and writing `leaderboard.csv`.
