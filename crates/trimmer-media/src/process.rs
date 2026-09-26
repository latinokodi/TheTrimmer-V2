//! Running a child process: argv arrays, no shell, cancellation, and a heartbeat.
//!
//! Every call in this crate goes through [`ProcessRunner`], so there is a single place where a
//! process is started — and therefore a single place to audit.
//!
//! Three properties matter and each has a reason:
//!
//! * **No shell.** [`std::process::Command`] is given an argument array. Nothing is ever
//!   concatenated into a command line, so no file name can become syntax.
//! * **No inherited window.** On Windows the child gets `CREATE_NO_WINDOW`. The GUI runs under
//!   the Windows subsystem, so without the flag each of a trim's dozen calls flashes its own
//!   console window.
//! * **cancellable between polls, never blocked on.** The child's stdout is polled with a
//!   timeout rather than awaited outright, so a cancellation is noticed within
//!   [`PollPolicy::interval`] instead of after ffmpeg has finished a ten-minute copy.

use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::MediaError;

/// Windows hands every console child of a windowed process its own console, which makes a trim
/// look like a sequence of command windows opening and closing. The child's output is read
/// through pipes either way, so there is nothing to show. The constant does not exist off
/// Windows, where this is 0 and `Command` ignores it.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How the runner waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPolicy {
    /// How often the runner checks for completion and for cancellation.
    ///
    /// Long enough to be free, short enough that Cancel feels like a button rather than a
    /// request.
    pub interval: Duration,
    /// How often elapsed time is reported while a long step runs. `None` switches the
    /// heartbeat off, which is what the unit tests want.
    pub heartbeat: Option<Duration>,
    /// Refuse to wait longer than this. `None` waits forever, which is right for a trim of an
    /// unknown-length master and wrong for a probe that should take 100 ms.
    pub timeout: Option<Duration>,
}

impl Default for PollPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(150),
            heartbeat: Some(Duration::from_secs(15)),
            timeout: None,
        }
    }
}

impl PollPolicy {
    /// Probe and other short calls: fail fast rather than hang a user interface.
    #[must_use]
    pub const fn quick() -> Self {
        Self {
            interval: Duration::from_millis(50),
            heartbeat: None,
            timeout: Some(Duration::from_secs(60)),
        }
    }

    /// A long encode or copy: no timeout, and a heartbeat so silence is not mistaken for a hang.
    #[must_use]
    pub const fn long() -> Self {
        Self {
            interval: Duration::from_millis(200),
            heartbeat: Some(Duration::from_secs(10)),
            timeout: None,
        }
    }

    /// With the heartbeat off, for tests and for a caller that renders its own progress.
    #[must_use]
    pub const fn silent() -> Self {
        Self {
            interval: Duration::from_millis(10),
            heartbeat: None,
            timeout: None,
        }
    }
}

/// A one-shot cancellation flag shared between a user interface and a running job.
///
/// A separate type from an error on purpose: a user pressing Cancel is not a failure, and code
/// that has to distinguish "the operator stopped this" from "this went wrong" needs a value it
/// can test, not a message it has to read.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    /// A fresh, uncancelled flag.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask for cancellation. Safe to call from any thread, and idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Fail with [`MediaError::Cancelled`] when cancellation has been requested.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::Cancelled`] when the flag is set.
    pub fn check(&self) -> Result<(), MediaError> {
        if self.is_cancelled() {
            Err(MediaError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Something a running job wants to say.
///
/// `Eq` is deliberately not derived: [`Progress::Elapsed`] and [`Progress::Finished`] carry
/// seconds as `f64`, and claiming total equality for a float would be a lie. `Serialize` is
/// derived because progress is forwarded to the interface as an event.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Progress {
    /// A new step has begun.
    Step {
        /// A short label, e.g. `head encode`.
        label: String,
    },
    /// The exact command line about to run, as a single display string.
    ///
    /// Display only. The command is *executed* as an array; this string exists so the log can
    /// show what ran.
    Command {
        /// The rendered command line.
        text: String,
        /// Its arguments, for a caller that wants to show them individually.
        args: Vec<String>,
    },
    /// The current step is still running, and has been for this long.
    Elapsed {
        /// Seconds since the step started.
        seconds: f64,
    },
    /// A step finished.
    Finished {
        /// The step's label.
        label: String,
        /// How long it took.
        seconds: f64,
        /// True when it exited zero.
        ok: bool,
    },
    /// A line of the child's error output, when it said something worth showing.
    Message {
        /// The line.
        text: String,
    },
    /// Where the running step has got to, as ffmpeg reported it.
    ///
    /// Emitted every `-stats_period` seconds while a step runs, but **only for a run that asked for
    /// it** — see [`Watch`]. The fraction is `out_seconds / expected_seconds` when the caller knew the
    /// expectation, and a caller that did not still gets the counters.
    Ticks {
        /// What ffmpeg said.
        ticks: ProgressTicks,
    },
}

/// Where a running step has got to, as **ffmpeg itself** reports it.
///
/// ## Why this exists
///
/// Until this was added, a four-minute cut reported a step label and a heartbeat and nothing else: no
/// fraction, no rate, no estimate. The interface showed an indeterminate sweep, correctly, because
/// there was genuinely no number to show — and "we are doing something" is not progress.
///
/// ffmpeg will say exactly how far along it is, if asked. `-progress pipe:1` makes it write a block of
/// `key=value` lines to standard output every `-stats_period` seconds and a final one after
/// `progress=end`:
///
/// ```text
/// frame=57
/// fps=0.00
/// total_size=158476
/// out_time_us=2200000
/// speed=4.27x
/// progress=continue
/// ```
///
/// `out_time_us` is microseconds of *output timeline written*, which against a known expected duration
/// is a real fraction. `speed` is ffmpeg's own throughput as a multiple of real time, which is where
/// the estimate comes from — measured by the program doing the work, rather than extrapolated from how
/// long we have been waiting.
///
/// The unit is deliberately **raw**: this crate runs one process and has no idea what the process is
/// for. It reports what it was told; dividing by an expectation is the caller's business, and
/// `expected_seconds` is carried along only because the caller already knew it and passing it back
/// saves the consumer from looking it up again.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressTicks {
    /// Seconds of output written so far, from ffmpeg's `out_time_us`.
    pub out_seconds: f64,
    /// Frames muxed so far, when ffmpeg reported a number.
    pub frame: Option<u64>,
    /// Throughput as a multiple of real time: `4.27` is `4.27x`. `None` when ffmpeg said `N/A`.
    pub speed: Option<f64>,
    /// Bytes written so far.
    pub bytes: Option<u64>,
    /// What the step was expected to produce, when the plan knew. `None` for a step of unknown length.
    pub expected_seconds: Option<f64>,
}

/// One parsed block of ffmpeg's progress stream, before the expectation is attached.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Tick {
    out_seconds: Option<f64>,
    frame: Option<u64>,
    speed: Option<f64>,
    bytes: Option<u64>,
}

/// Turn ffmpeg's `-progress` stream into ticks, one per block.
///
/// Incremental by necessity: the stream arrives in whatever chunks the pipe delivers, so a `key=value`
/// pair can be split across two reads and a naive `lines()` over each chunk would drop it. Bytes are
/// buffered until a newline arrives, and a block is emitted when `progress=` ends it.
///
/// A pure function of the text, so every awkward case — `N/A`, a half-delivered line, a block with no
/// `out_time_us`, an unknown key, the `progress=end` terminator — is tested without a process.
#[derive(Debug, Default)]
struct TickReader {
    /// The tail of the last chunk, which has not seen its newline yet.
    partial: String,
    /// The block being assembled.
    block: Tick,
}

impl TickReader {
    /// Feed a chunk of the child's standard output, and take whatever complete ticks it produced.
    fn feed(&mut self, chunk: &[u8]) -> Vec<Tick> {
        self.partial.push_str(&String::from_utf8_lossy(chunk));
        let mut ticks = Vec::new();

        // `split_inclusive` keeps the newline, so what is left after the loop is the unterminated tail.
        let complete = self.partial.rfind('\n').map_or(0, |index| index + 1);
        let text = self.partial[..complete].to_owned();
        self.partial.drain(..complete);

        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "out_time_us" | "out_time_ms" => {
                    // ffmpeg's `out_time_ms` is microseconds too — the name is a long-standing bug in
                    // ffmpeg, not a misreading here. Both are the same number, and both are `N/A` until
                    // the first frame is written.
                    if let Ok(micros) = value.parse::<f64>() {
                        self.block.out_seconds = Some(micros / 1_000_000.0);
                    }
                }
                "frame" => self.block.frame = value.parse::<u64>().ok(),
                "speed" => {
                    self.block.speed = value
                        .trim_end_matches('x')
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite() && *value >= 0.0);
                }
                "total_size" => self.block.bytes = value.parse::<u64>().ok(),
                "progress" => {
                    // A block is only worth reporting if it says **where** the step has got to. ffmpeg's
                    // opening block carries `out_time_us=N/A` and `frame=0`, and a block at the end of a
                    // step that produced no timestamp carries none at all — reporting either would put a
                    // zero on the bar, which reads as the job having restarted. Dropping them means
                    // `out_seconds` is always a real position, and never the previous block's number
                    // carried over.
                    if self.block.out_seconds.is_some() {
                        ticks.push(self.block);
                    }
                    self.block = Tick::default();
                }
                _ => {}
            }
        }

        ticks
    }
}

/// Receives progress. Implemented by the CLI's log, the GUI's event channel and the daemon's
/// server-sent-events stream.
pub trait ProgressSink: Send + Sync {
    /// Record one event.
    fn report(&self, progress: Progress);
}

/// A sink that discards everything, for tests and for a caller with nothing to show.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl ProgressSink for NullSink {
    fn report(&self, _progress: Progress) {}
}

/// A sink that collects events, for assertions in tests.
#[derive(Debug, Default)]
pub struct CollectingSink {
    events: std::sync::Mutex<Vec<Progress>>,
}

impl CollectingSink {
    /// A new, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The events received so far.
    #[must_use]
    pub fn events(&self) -> Vec<Progress> {
        self.events
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl ProgressSink for CollectingSink {
    fn report(&self, progress: Progress) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(progress);
        }
    }
}

/// Ask a run to report where it has got to.
///
/// Opt-in, and deliberately **not** the default. `-progress pipe:1` takes standard output, and one
/// caller already owns it: the frame-hash and SSIM passes read `framemd5` output from stdout and would
/// be corrupted by a progress stream mixed into it. A run that wants ticks asks for them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Watch {
    /// How long the step is expected to produce, when the plan knows. `None` still reports counters,
    /// without a fraction — which is the honest answer for a step whose length is not known in advance.
    pub expected_seconds: Option<f64>,
    /// How often ffmpeg should report, in seconds. Half a second is often enough to look live and rare
    /// enough to be free: a ten-minute copy costs twelve hundred events.
    pub period_seconds: f64,
}

impl Watch {
    /// Watch a step expected to produce `expected_seconds` of output.
    #[must_use]
    pub fn of(expected_seconds: f64) -> Self {
        Self {
            expected_seconds: Some(expected_seconds),
            period_seconds: 0.5,
        }
    }

    /// The flags that make ffmpeg report, exactly as they appear in the recorded command line.
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        ["-nostats", "-progress", "pipe:1", "-stats_period"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .chain(std::iter::once(format!("{:.2}", self.period_seconds)))
            .collect()
    }
}

/// Everything a caller may configure about one process run.
#[derive(Clone)]
pub struct RunOptions {
    /// How to wait.
    pub policy: PollPolicy,
    /// Ask for cancellation.
    pub cancel: CancelFlag,
    /// Where to report progress.
    pub sink: Arc<dyn ProgressSink>,
    /// A label for the step, used in progress events.
    pub label: String,
    /// Ask the run to report where it has got to. `None` runs it silently, as before.
    pub watch: Option<Watch>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            policy: PollPolicy::default(),
            cancel: CancelFlag::new(),
            sink: Arc::new(NullSink),
            label: String::new(),
            watch: None,
        }
    }
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("policy", &self.policy)
            .field("cancel", &self.cancel.is_cancelled())
            .field("label", &self.label)
            .field("watch", &self.watch)
            .finish_non_exhaustive()
    }
}

/// What a finished process produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Standard output, as text with invalid bytes replaced.
    pub stdout: String,
    /// Standard error, as text with invalid bytes replaced. This is where ffmpeg says
    /// everything useful.
    pub stderr: String,
    /// The exit code, or `None` when the process was killed by a signal.
    pub code: Option<i32>,
}

impl Output {
    /// True when the process exited zero.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The last few lines of standard error, for an error message.
    #[must_use]
    pub fn stderr_tail(&self, lines: usize) -> String {
        let all: Vec<&str> = self
            .stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let start = all.len().saturating_sub(lines);
        all[start..].join("\n")
    }
}

/// Read whatever a pipe has right now, without blocking for more.
///
/// Returns `(bytes_read, reached_end)`. The distinction is the whole reason this is a function:
/// "nothing to read *yet*" and "nothing more will ever come" are identical to a single `read`,
/// and treating the first as the second silently discards everything the child still had to
/// write — which is how a successful ffmpeg run reports empty output. A timeout means the
/// first; end-of-file and a read error mean the second.
async fn pump<R>(pipe: &mut R, into: &mut Vec<u8>, chunk: &mut [u8]) -> (usize, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut total = 0usize;
    loop {
        match tokio::time::timeout(Duration::from_millis(1), pipe.read(chunk)).await {
            Ok(Ok(0) | Err(_)) => return (total, true),
            Ok(Ok(read)) => {
                into.extend_from_slice(&chunk[..read]);
                total += read;
            }
            Err(_nothing_yet) => return (total, false),
        }
    }
}

/// Starts processes. Cheap to clone; holds no per-run state.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

impl ProcessRunner {
    /// A runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Run a command and collect its output.
    ///
    /// Standard output is read to the end; standard error is also read to the end, because
    /// ffmpeg's diagnostics are the only thing that explains a failure and they arrive on
    /// stderr. A child that writes more than the pipe buffer holds would deadlock if only one
    /// were drained, so both are drained concurrently.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::Spawn`] when the program cannot start,
    /// [`MediaError::Cancelled`] when cancellation is requested while it runs, and
    /// [`MediaError::ProcessFailed`] when it exits non-zero.
    pub async fn run(
        &self,
        program: &Path,
        args: &[OsString],
        options: &RunOptions,
    ) -> Result<Output, MediaError> {
        options.cancel.check()?;

        // The watch flags go in front of everything else. They are ffmpeg's own global options, so their
        // position does not matter to ffmpeg — and putting them first means the recorded command line
        // starts with the fact that this run is being watched, which is the useful thing to read back.
        let watched: Vec<OsString> = match &options.watch {
            Some(watch) => watch
                .args()
                .iter()
                .map(std::ffi::OsString::from)
                .chain(args.iter().cloned())
                .collect(),
            None => args.to_vec(),
        };

        let display = display_command(program, &watched);
        options.sink.report(Progress::Step {
            label: options.label.clone(),
        });
        options.sink.report(Progress::Command {
            text: display.clone(),
            args: watched
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        });

        let started = Instant::now();
        let mut command = Command::new(program);
        command
            .args(&watched)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if cfg!(windows) {
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn().map_err(|source| MediaError::Spawn {
            tool: program.display().to_string(),
            source,
        })?;

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut stdout_chunk = [0u8; 16 * 1024];
        let mut stderr_chunk = [0u8; 16 * 1024];
        let mut next_heartbeat = options.policy.heartbeat;

        // The progress stream, assembled across reads. Only ever fed when the run asked to be watched.
        let mut reader = TickReader::default();
        let expected = options
            .watch
            .as_ref()
            .and_then(|watch| watch.expected_seconds);

        // Standard error, forwarded as it arrives rather than only in a tail once the step has ended.
        // `-v error` means a successful step usually says nothing, so this costs nothing on the common
        // path and turns a failure into something the operator can read *while* it is happening instead
        // of after the window has decided the step is over. Bounded, because a pathological run must not
        // be able to fill the log with its own noise.
        let mut forwarded = 0usize;
        let mut messages = 0usize;
        const MAX_LIVE_MESSAGES: usize = 200;

        // Drain both pipes and reap the child in one loop. `try_read` on a pipe we own, plus a
        // timed `join`, means the loop wakes on the poll interval whether or not the child has
        // written anything — which is what makes cancellation and the heartbeat possible on a
        // step that prints nothing for minutes.
        let outcome = loop {
            let mut progressed = false;

            if let Some(pipe) = stdout_pipe.as_mut() {
                let (read, eof) = pump(pipe, &mut stdout, &mut stdout_chunk).await;
                progressed |= read > 0;
                if read > 0 && options.watch.is_some() {
                    // Only the newly read bytes, so a tick is reported once rather than once per read.
                    let fresh = &stdout[stdout.len() - read..];
                    for tick in reader.feed(fresh) {
                        options.sink.report(Progress::Ticks {
                            ticks: ProgressTicks {
                                out_seconds: tick.out_seconds.unwrap_or(0.0),
                                frame: tick.frame,
                                speed: tick.speed,
                                bytes: tick.bytes,
                                expected_seconds: expected,
                            },
                        });
                    }
                }
                if eof {
                    stdout_pipe = None;
                }
            }

            if let Some(pipe) = stderr_pipe.as_mut() {
                let (read, eof) = pump(pipe, &mut stderr, &mut stderr_chunk).await;
                progressed |= read > 0;
                if read > 0 && messages < MAX_LIVE_MESSAGES {
                    // Only whole lines: a half-delivered line is not a message yet, and reporting it
                    // would put a truncated sentence in the log a moment before the whole one.
                    let fresh = String::from_utf8_lossy(&stderr[forwarded..]).into_owned();
                    if let Some(last_newline) = fresh.rfind('\n') {
                        for line in fresh[..last_newline].lines() {
                            let line = line.trim();
                            if line.is_empty() || messages >= MAX_LIVE_MESSAGES {
                                continue;
                            }
                            messages += 1;
                            options.sink.report(Progress::Message {
                                text: line.to_owned(),
                            });
                        }
                        forwarded += last_newline + 1;
                    }
                }
                if eof {
                    stderr_pipe = None;
                }
            }

            if let Some(code) = child.try_wait().map_err(|source| MediaError::Spawn {
                tool: program.display().to_string(),
                source,
            })? {
                // Drain whatever is still buffered before reporting.
                if let Some(pipe) = stderr_pipe.as_mut() {
                    let _ = pipe.read_to_end(&mut stderr).await;
                }
                if let Some(pipe) = stdout_pipe.as_mut() {
                    let _ = pipe.read_to_end(&mut stdout).await;
                }
                break code.code();
            }

            if options.cancel.is_cancelled() {
                let _ = child.kill().await;
                let _ = child.wait().await;
                options.sink.report(Progress::Finished {
                    label: options.label.clone(),
                    seconds: started.elapsed().as_secs_f64(),
                    ok: false,
                });
                return Err(MediaError::Cancelled);
            }

            if let Some(timeout) = options.policy.timeout {
                if started.elapsed() > timeout {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(MediaError::ProcessFailed {
                        tool: program.display().to_string(),
                        status: format!("timed out after {:.0}s", timeout.as_secs_f64()),
                        command: display,
                        tail: String::from_utf8_lossy(&stderr).into_owned(),
                    });
                }
            }

            if let Some(period) = next_heartbeat {
                if started.elapsed() >= period {
                    options.sink.report(Progress::Elapsed {
                        seconds: started.elapsed().as_secs_f64(),
                    });
                    next_heartbeat = Some(period + options.policy.heartbeat.unwrap_or(period));
                }
            }

            // If a pipe is still open there is more to read, so poll hard; otherwise sleep the
            // interval. This keeps a fast probe fast without spinning.
            let wait = if progressed {
                Duration::from_millis(1)
            } else {
                options.policy.interval
            };
            tokio::time::sleep(wait).await;
        };

        let elapsed = started.elapsed().as_secs_f64();
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        let output = Output {
            stdout,
            stderr,
            code: outcome,
        };

        options.sink.report(Progress::Finished {
            label: options.label.clone(),
            seconds: elapsed,
            ok: output.ok(),
        });
        if !output.ok() && messages == 0 {
            // The tail is where ffmpeg says why. Showing the whole of it buries the reason — and when
            // the lines have already been forwarded live, showing them again would put the same
            // sentence in the log twice.
            options.sink.report(Progress::Message {
                text: output.stderr_tail(12),
            });
        }

        if output.ok() {
            Ok(output)
        } else {
            Err(MediaError::ProcessFailed {
                tool: program.display().to_string(),
                status: match output.code {
                    Some(code) => format!("exit {code}"),
                    None => "killed by a signal".to_owned(),
                },
                command: display,
                tail: output.stderr_tail(12),
            })
        }
    }
}

/// Render a command for display. Never used to execute anything.
#[must_use]
pub fn display_command(program: &Path, args: &[OsString]) -> String {
    let mut parts = vec![quote_for_display(&program.to_string_lossy())];
    parts.extend(
        args.iter()
            .map(|arg| quote_for_display(&arg.to_string_lossy())),
    );
    parts.join(" ")
}

/// Quote an argument for display when it contains a space, so the log line can be pasted into
/// a terminal and behave the same way.
fn quote_for_display(text: &str) -> String {
    if text.is_empty() {
        return "\"\"".to_owned();
    }
    if text.contains(' ') || text.contains('"') {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        text.to_owned()
    }
}

/// Build an argument vector from strings, for call sites that hold `&str`.
#[must_use]
pub fn argv(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(name: &str) -> PathBuf {
        // The tests need a real executable. `cmd` exists on every supported Windows machine,
        // and `true`/`sh` on the others.
        if cfg!(windows) {
            Path::new("C:\\Windows\\System32\\cmd.exe").to_path_buf()
        } else {
            Path::new(name).to_path_buf()
        }
    }

    use std::path::PathBuf;

    #[tokio::test]
    async fn a_successful_process_returns_its_stdout_and_zero() {
        let runner = ProcessRunner::new();
        let args = if cfg!(windows) {
            argv(&["/C", "echo hello"])
        } else {
            argv(&["-c", "echo hello"])
        };
        let output = runner
            .run(&program("sh"), &args, &RunOptions::default())
            .await
            .expect("runs");
        assert!(output.ok());
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.code, Some(0));
    }

    #[tokio::test]
    async fn a_failing_process_reports_its_exit_code_and_error_tail() {
        let runner = ProcessRunner::new();
        let args = if cfg!(windows) {
            argv(&["/C", ">&2 echo something went wrong & exit 3"])
        } else {
            argv(&["-c", "echo something went wrong >&2; exit 3"])
        };
        let error = runner
            .run(&program("sh"), &args, &RunOptions::default())
            .await
            .expect_err("must fail");
        match error {
            MediaError::ProcessFailed { status, tail, .. } => {
                assert!(status.contains('3'), "{status}");
                assert!(tail.contains("something went wrong"), "{tail}");
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[tokio::test]
    async fn cancellation_stops_a_long_step_promptly() {
        let runner = ProcessRunner::new();
        let cancel = CancelFlag::new();
        let args = if cfg!(windows) {
            // A ping to a non-resolving address blocks for the full count; `timeout` is not
            // available in every container, so ping is the portable-enough sleep here.
            argv(&["/C", "ping -n 30 127.0.0.1 >NUL"])
        } else {
            argv(&["-c", "sleep 30"])
        };
        let options = RunOptions {
            policy: PollPolicy {
                interval: Duration::from_millis(20),
                heartbeat: None,
                timeout: None,
            },
            cancel: cancel.clone(),
            sink: Arc::new(NullSink),
            label: "long".to_owned(),
            watch: None,
        };
        let handle = tokio::spawn(async move { runner.run(&program("sh"), &args, &options).await });
        tokio::time::sleep(Duration::from_millis(120)).await;
        let started = Instant::now();
        cancel.cancel();
        let result = handle.await.expect("the task did not panic");
        assert!(matches!(result, Err(MediaError::Cancelled)), "{result:?}");
        // The poll interval is 20 ms, so this must not be anywhere near the child's full run.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancel was slow"
        );
    }

    #[tokio::test]
    async fn a_run_started_with_the_flag_already_set_never_spawns() {
        let runner = ProcessRunner::new();
        let cancel = CancelFlag::new();
        cancel.cancel();
        let options = RunOptions {
            cancel,
            ..RunOptions::default()
        };
        let result = runner
            .run(&program("sh"), &argv(&["/C", "echo nope"]), &options)
            .await;
        assert!(matches!(result, Err(MediaError::Cancelled)));
    }

    #[tokio::test]
    async fn a_timeout_kills_a_process_that_will_not_finish() {
        let runner = ProcessRunner::new();
        let args = if cfg!(windows) {
            argv(&["/C", "ping -n 30 127.0.0.1 >NUL"])
        } else {
            argv(&["-c", "sleep 30"])
        };
        let options = RunOptions {
            policy: PollPolicy {
                interval: Duration::from_millis(20),
                heartbeat: None,
                timeout: Some(Duration::from_millis(150)),
            },
            ..RunOptions::default()
        };
        let error = runner
            .run(&program("sh"), &args, &options)
            .await
            .expect_err("must time out");
        match error {
            MediaError::ProcessFailed { status, .. } => assert!(status.contains("timed out")),
            other => panic!("wrong error: {other}"),
        }
    }

    #[tokio::test]
    async fn progress_reports_the_step_the_command_the_elapsed_time_and_the_outcome() {
        let runner = ProcessRunner::new();
        let sink = Arc::new(CollectingSink::new());
        let args = if cfg!(windows) {
            argv(&["/C", "ping -n 2 127.0.0.1 >NUL"])
        } else {
            argv(&["-c", "sleep 1"])
        };
        let options = RunOptions {
            policy: PollPolicy {
                interval: Duration::from_millis(20),
                heartbeat: Some(Duration::from_millis(30)),
                timeout: None,
            },
            cancel: CancelFlag::new(),
            sink: sink.clone(),
            label: "copy body".to_owned(),
            watch: None,
        };
        runner
            .run(&program("sh"), &args, &options)
            .await
            .expect("runs");
        let events = sink.events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Progress::Step { label } if label == "copy body")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Progress::Command { .. })),
            "the exact command line must be reported so the log can show it"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Progress::Elapsed { .. })),
            "a slow step must say it is still running: {events:?}"
        );
        assert!(events.iter().any(
            |event| matches!(event, Progress::Finished { ok: true, label, .. } if label == "copy body")
        ));
    }

    #[tokio::test]
    async fn a_large_output_does_not_deadlock() {
        // A child that writes far more than a pipe buffer holds. If only one pipe were drained,
        // this would hang until the timeout rather than fail.
        let runner = ProcessRunner::new();
        let args = if cfg!(windows) {
            argv(&["/C", "for /L %i in (1,1,4000) do @echo line %i"])
        } else {
            argv(&["-c", "for i in $(seq 1 4000); do echo line $i; done"])
        };
        let options = RunOptions {
            policy: PollPolicy {
                interval: Duration::from_millis(5),
                heartbeat: None,
                timeout: Some(Duration::from_secs(30)),
            },
            ..RunOptions::default()
        };
        let output = runner
            .run(&program("sh"), &args, &options)
            .await
            .expect("runs");
        assert!(
            output.stdout.lines().count() > 1_000,
            "output was truncated"
        );
    }

    #[test]
    fn display_quoting_makes_a_command_line_pasteable() {
        let args = argv(&["-i", r"H:\master takes\Andy Ross.mp4", "-c", "copy"]);
        let text = display_command(Path::new("ffmpeg"), &args);
        assert!(text.starts_with("ffmpeg -i "));
        assert!(
            text.contains("\"H:\\master takes\\Andy Ross.mp4\""),
            "{text}"
        );
        assert!(text.ends_with("-c copy"));
    }

    #[test]
    fn an_empty_argument_is_visible_in_a_log_line() {
        let text = display_command(Path::new("ffmpeg"), &argv(&["-metadata", ""]));
        assert!(text.ends_with("\"\""), "{text}");
    }

    #[test]
    fn the_error_tail_keeps_the_last_lines_and_drops_blank_ones() {
        let output = Output {
            stdout: String::new(),
            stderr: "one\n\ntwo\nthree\nfour\n".to_owned(),
            code: Some(1),
        };
        assert_eq!(output.stderr_tail(2), "three\nfour");
        assert_eq!(output.stderr_tail(99), "one\ntwo\nthree\nfour");
    }

    #[test]
    fn the_cancel_flag_is_shared_between_clones() {
        let flag = CancelFlag::new();
        let other = flag.clone();
        assert!(!other.is_cancelled());
        flag.cancel();
        assert!(other.is_cancelled());
        assert!(other.check().is_err());
    }

    #[test]
    fn poll_policies_are_shaped_for_their_jobs() {
        // A probe must not hang a user interface.
        assert!(PollPolicy::quick().timeout.is_some());
        assert!(PollPolicy::quick().heartbeat.is_none());
        // A long copy must not time out, and must say it is alive.
        assert!(PollPolicy::long().timeout.is_none());
        assert!(PollPolicy::long().heartbeat.is_some());
        assert!(PollPolicy::silent().heartbeat.is_none());
    }

    /// One block of ffmpeg's progress output, as it appears on the wire.
    const A_BLOCK: &str = "frame=57\nfps=0.00\nstream_0_0_q=-1.0\nbitrate= 310.7kbits/s\n\
                           total_size=158476\nout_time_us=2200000\nout_time_ms=2200000\n\
                           out_time=00:00:02.200000\ndup_frames=0\ndrop_frames=0\nspeed=4.27x\n\
                           progress=continue\n";

    #[test]
    fn a_progress_block_becomes_a_tick() {
        let mut reader = TickReader::default();
        let ticks = reader.feed(A_BLOCK.as_bytes());
        assert_eq!(ticks.len(), 1, "one block is one tick");
        let tick = ticks[0];
        assert_eq!(tick.out_seconds, Some(2.2), "out_time_us is microseconds");
        assert_eq!(tick.frame, Some(57));
        assert_eq!(
            tick.speed,
            Some(4.27),
            "the x is a suffix, not part of the number"
        );
        assert_eq!(tick.bytes, Some(158_476));
    }

    #[test]
    fn a_block_split_across_reads_is_still_one_tick() {
        // The pipe delivers whatever it delivers. A `key=value` pair landing across two reads is the
        // ordinary case on a slow step, and a `lines()` per chunk would drop half of it.
        let mut reader = TickReader::default();
        let (first, second) = A_BLOCK.split_at(40);
        assert!(
            reader.feed(first.as_bytes()).is_empty(),
            "no newline yet, so no block"
        );
        let ticks = reader.feed(second.as_bytes());
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].out_seconds, Some(2.2));
        assert_eq!(ticks[0].frame, Some(57));
    }

    #[test]
    fn several_blocks_in_one_read_become_several_ticks() {
        let mut reader = TickReader::default();
        let text = format!("{A_BLOCK}{}", A_BLOCK.replace("2200000", "4080000"));
        let ticks = reader.feed(text.as_bytes());
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].out_seconds, Some(2.2));
        assert_eq!(ticks[1].out_seconds, Some(4.08), "each block starts clean");
    }

    #[test]
    fn a_block_with_no_position_is_dropped_rather_than_read_as_zero() {
        // ffmpeg's first block carries `out_time_us=N/A`, and a step that produced no timestamp carries
        // none at all. Reporting either as `0.0` would put the bar back to the start mid-job, so a block
        // with no position is dropped — and the block after it still reports the *new* position rather
        // than inheriting the old one.
        let mut reader = TickReader::default();
        let ticks = reader.feed(
            b"frame=0\nfps=0.00\nout_time_us=N/A\nout_time_ms=N/A\nspeed=N/A\nprogress=continue\n",
        );
        assert!(ticks.is_empty(), "{ticks:?}");

        let ticks = reader.feed(A_BLOCK.as_bytes());
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].out_seconds, Some(2.2));

        // A later block with no timestamp at all must not repeat 2.2.
        let ticks = reader.feed(b"frame=80\nspeed=1.0x\nprogress=continue\n");
        assert!(ticks.is_empty(), "{ticks:?}");
    }

    #[test]
    fn an_end_block_is_reported_like_any_other() {
        let mut reader = TickReader::default();
        let ticks = reader.feed(A_BLOCK.replace("continue", "end").as_bytes());
        assert_eq!(ticks.len(), 1, "the final block carries the real total");
    }

    #[test]
    fn nonsense_in_the_stream_is_ignored_rather_than_fatal() {
        let mut reader = TickReader::default();
        // A line with no `=`, an unknown key, a value that is not a number, and a negative speed: none
        // of these is a reason to lose the tick that follows.
        let ticks = reader.feed(
            b"not a pair\nframe_number=9\nframe=abc\nspeed=-3x\nout_time_us=1500000\nprogress=continue\n",
        );
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].out_seconds, Some(1.5));
        assert_eq!(ticks[0].frame, None, "`abc` is not a frame count");
        assert_eq!(ticks[0].speed, None, "a negative rate is not a rate");
    }

    #[test]
    fn the_watch_flags_name_the_stream_and_the_period() {
        let args = Watch::of(12.5).args();
        assert_eq!(
            args[0], "-nostats",
            "otherwise ffmpeg also writes its own status line"
        );
        assert_eq!(args[1], "-progress");
        assert_eq!(args[2], "pipe:1", "stdout, so stderr keeps the diagnostics");
        assert_eq!(args[3], "-stats_period");
        assert_eq!(args[4], "0.50");
        assert_eq!(Watch::of(1.0).expected_seconds, Some(1.0));
    }
}
