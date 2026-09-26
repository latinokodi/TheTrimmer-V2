//! # trimmer-daemon
//!
//! A local HTTP/JSON control surface over the store, the queue and the verifier, so a studio's
//! own tooling — a render manager, a web dashboard, a shell script in a watch folder — can drive
//! TheTrimmer without a person clicking anything.
//!
//! ## Why it binds to `127.0.0.1` and never to `0.0.0.0`
//!
//! **This API can cut files and read every project on the machine.** It has no
//! user accounts, no rate limiting, no TLS and no audit of *who* is asking beyond a shared
//! token. It is a **local control surface, not a service**: the process that owns the port owns
//! the machine's media.
//!
//! So the bind address is not a default that a configuration can change — [`serve`] refuses any
//! address that is not a loopback address, by name, with a sentence saying why. A studio that
//! wants remote access should put a real reverse proxy in front of it, terminate TLS there, and
//! decide for itself what authentication means; none of those decisions belongs in this crate,
//! and all of them are worse if this crate silently pretends to make them.
//!
//! ## The token
//!
//! Every request carries `Authorization: Bearer <token>`. A missing or wrong token is `401`
//! with a JSON body, and the comparison is constant-time, so the time a refusal takes says
//! nothing about how much of a guess was right. [`serve`] refuses to start at all with a token
//! shorter than [`MIN_TOKEN_LEN`] characters: a four-character token on a port that can delete
//! a project is worse than no daemon, because it looks protected.
//!
//! ## The run registry is bounded
//!
//! A daemon runs for weeks. `Arc<Mutex<HashMap<Uuid, RunRecord>>>` with no bound is a leak that
//! only shows up in week three, so the registry holds at most [`MAX_RUNS`] records and evicts
//! the oldest **finished** run to make room. A run that is still going is never evicted; if
//! every record is a running one, the daemon refuses the new run with `409` rather than
//! discarding the record of work in flight.
//!
//! ## What is honest about the edges
//!
//! * `trimmer_app::QueuePreview` has no `serde` derive, so the dry run is rendered into JSON by
//!   hand in [`routes`]. That is a real coupling to that type's fields and it is noted where it
//!   happens.
//! * A transcript search for a video with no caption file answers `200` with an empty list, not
//!   `404`. "There is no transcript here" is a fact about the file, not a missing resource, and
//!   a client that has to tell those apart can look at the file it asked about.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::doc_markdown,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::ref_option,
    clippy::items_after_statements,
    clippy::redundant_closure,
    clippy::large_futures
)]

pub mod config;
pub mod openapi;
pub mod routes;
pub mod state;

pub use config::{DaemonConfig, MIN_TOKEN_LEN};
pub use routes::{constant_time_eq, router, serve, SERVICE_NAME};
pub use state::{DaemonState, ProbeMeasurer, RunItem, RunOutcome, RunRecord, RunState, MAX_RUNS};

/// Set up `tracing`, honouring `RUST_LOG`, with `info` when it says nothing.
///
/// Called by the `trimmerd` binary and by `trimmer-cli`'s `daemon` subcommand, so a request log
/// looks the same however the daemon was started. `try_init` is used rather than `init`: a
/// second call is a no-op rather than a panic, which matters in a process that has already set
/// a subscriber up for another purpose.
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
