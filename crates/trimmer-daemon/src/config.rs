//! What the daemon is told when it starts.

use std::net::IpAddr;
use std::path::PathBuf;

/// The shortest token [`crate::serve`] will start with.
///
/// Sixteen characters is not a security proof, it is a floor: a token short enough to guess by
/// hand turns a protected port into an unprotected one that *looks* protected, and the failure
/// is silent. A caller who wants a real secret should generate 32 bytes of entropy and encode
/// them — the daemon neither generates nor stores the token, because it must not have an
/// opinion about where a studio keeps its secrets.
pub const MIN_TOKEN_LEN: usize = 16;

/// Everything the daemon needs in order to run.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// The port to listen on.
    pub port: u16,
    /// The bearer token every request must carry.
    pub token: String,
    /// Where the project database lives.
    pub store_path: PathBuf,
    /// The address to bind. Must be a loopback address; see the crate docs.
    pub bind: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: 8787,
            token: String::new(),
            store_path: trimmer_store::default_store_path()
                .unwrap_or_else(|_| PathBuf::from("projects.sqlite")),
            // The only address this daemon is willing to listen on.
            bind: "127.0.0.1".to_owned(),
        }
    }
}

impl DaemonConfig {
    /// A configuration with a token, a port and a store.
    #[must_use]
    pub fn new(port: u16, token: impl Into<String>, store_path: impl Into<PathBuf>) -> Self {
        Self {
            port,
            token: token.into(),
            store_path: store_path.into(),
            bind: "127.0.0.1".to_owned(),
        }
    }

    /// Refuse a configuration the daemon must not run with.
    ///
    /// # Errors
    ///
    /// Returns a sentence when the token is too short, or when `bind` is not a loopback
    /// address. Both refusals name the value and the reason.
    pub fn validate(&self) -> Result<(), String> {
        if self.token.chars().count() < MIN_TOKEN_LEN {
            return Err(format!(
                "the token is {} characters; a daemon that can delete a project needs at least \
                 {MIN_TOKEN_LEN}. Generate one with, for example, `openssl rand -hex 24`",
                self.token.chars().count()
            ));
        }
        if self.token.chars().any(char::is_whitespace) {
            return Err(
                "the token contains whitespace, which cannot survive an Authorization header"
                    .to_owned(),
            );
        }
        let address: IpAddr = self.bind.parse().map_err(|_| {
            format!(
                "bind address {:?} is not an IP address; the daemon binds to 127.0.0.1 by design",
                self.bind
            )
        })?;
        if !address.is_loopback() {
            return Err(format!(
                "refusing to bind to {address}: this API can cut files and delete projects, so it \
                 is a control surface for one machine rather than a service. Bind 127.0.0.1, and \
                 put a reverse proxy in front of it if remote access is wanted"
            ));
        }
        Ok(())
    }
}
