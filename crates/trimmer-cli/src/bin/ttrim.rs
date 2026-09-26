//! `ttrim` — the same program as `thetrimmer`, under a shorter name.
//!
//! A second binary rather than a symlink or a shell alias because Windows has neither in a form
//! that survives being installed by a studio's IT department. It calls the same [`run`], so the
//! two cannot disagree about anything.

fn main() {
    std::process::exit(trimmer_cli::run());
}
