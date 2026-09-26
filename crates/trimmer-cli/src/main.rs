//! `thetrimmer` — the command line.
//!
//! Everything is in the library, so that `ttrim` is the same program under a shorter name
//! rather than a second implementation that will drift from this one.

fn main() {
    std::process::exit(trimmer_cli::run());
}
