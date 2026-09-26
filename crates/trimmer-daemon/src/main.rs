//! `trimmerd` — the headless daemon.
//!
//! Started by hand or by a service manager. There is no configuration file: the three things
//! it needs are on the command line or in the environment, because a daemon that reads a
//! config file is a daemon whose refusal to start is somewhere the operator has to go looking
//! for.
//!
//! ```text
//! trimmerd --port 8787 --token <at-least-16-characters> [--bind 127.0.0.1] [--store <path>]
//! ```
//!
//! The port and the token and the store path may also come from `THE_TRIMMER_PORT`,
//! `THE_TRIMMER_TOKEN` and `THE_TRIMMER_STORE`, which is how a service unit passes them without
//! putting a secret in a process listing.

use std::process::ExitCode;

use trimmer_daemon::{serve, DaemonConfig};

/// What the command line asked for.
struct Args {
    port: Option<u16>,
    token: Option<String>,
    bind: Option<String>,
    store: Option<String>,
    help: bool,
}

/// The text `--help` prints.
const HELP: &str = "\
trimmerd — TheTrimmer's local HTTP/JSON API

USAGE:
    trimmerd [OPTIONS]

OPTIONS:
    --port <N>        The port to listen on. Default 8787, or THE_TRIMMER_PORT.
    --token <TEXT>    The bearer token every request must carry. At least 16 characters.
                      No default: the daemon refuses to start without one.
                      May also come from THE_TRIMMER_TOKEN.
    --bind <ADDR>     The address to listen on. Must be a loopback address; the daemon
                      refuses anything else, because this API can cut files, delete
                      projects. Default 127.0.0.1.
    --store <PATH>    The project database. Default is this machine's data directory,
                      or THE_TRIMMER_STORE.
    -h, --help        Print this.

The API is documented at GET /v1/openapi.json once it is running.
";

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("trimmerd: {message}");
            eprintln!("try `trimmerd --help`");
            return ExitCode::from(2);
        }
    };
    if args.help {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }

    init_tracing();

    let mut config = DaemonConfig::default();
    if let Ok(port) = std::env::var("THE_TRIMMER_PORT") {
        if let Ok(port) = port.trim().parse::<u16>() {
            config.port = port;
        }
    }
    if let Ok(token) = std::env::var("THE_TRIMMER_TOKEN") {
        config.token = token;
    }
    if let Ok(store) = std::env::var("THE_TRIMMER_STORE") {
        config.store_path = store.into();
    }
    if let Some(port) = args.port {
        config.port = port;
    }
    if let Some(token) = args.token {
        config.token = token;
    }
    if let Some(bind) = args.bind {
        config.bind = bind;
    }
    if let Some(store) = args.store {
        config.store_path = store.into();
    }

    if let Err(error) = config.validate() {
        eprintln!("trimmerd: {error}");
        return ExitCode::from(2);
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("trimmerd: could not start a runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("trimmerd: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `RUST_LOG` decides how much is said; `info` when it says nothing.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// Read the flags. Hand-rolled because the daemon has four of them and a parser dependency
/// would be four hundred kilobytes to parse four strings.
fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        port: None,
        token: None,
        bind: None,
        store: None,
        help: false,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => parsed.help = true,
            "--port" => {
                let text = value("--port")?;
                parsed.port = Some(
                    text.trim()
                        .parse()
                        .map_err(|_| format!("{text:?} is not a port number"))?,
                );
            }
            "--token" => parsed.token = Some(value("--token")?),
            "--bind" => parsed.bind = Some(value("--bind")?),
            "--store" => parsed.store = Some(value("--store")?),
            other => return Err(format!("{other:?} is not a flag this program knows")),
        }
    }
    Ok(parsed)
}
