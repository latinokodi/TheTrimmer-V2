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
        self.events.lock().map(|guard| guard.clone()).unwrap_or_default()
    }
}

impl ProgressSink for CollectingSink {
    fn report(&self, progress: Progress) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(progress);
        }
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
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            policy: PollPolicy::default(),
            cancel: CancelFlag::new(),
            sink: Arc::new(NullSink),
            label: String::new(),
        }
    }
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("policy", &self.policy)
            .field("cancel", &self.cancel.is_cancelled())
            .field("label", &self.label)
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
        let all: Vec<&str> = self.stderr.lines().filter(|line| !line.trim().is_empty()).collect();
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
        let display = display_command(program, args);
        options.sink.report(Progress::Step {
            label: options.label.clone(),
        });
        options.sink.report(Progress::Command {
            text: display.clone(),
            args: args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect(),
        });

        let started = Instant::now();
        let mut command = Command::new(program);
        command
            .args(args)
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

        // Drain both pipes and reap the child in one loop. `try_read` on a pipe we own, plus a
        // timed `join`, means the loop wakes on the poll interval whether or not the child has
        // written anything — which is what makes cancellation and the heartbeat possible on a
        // step that prints nothing for minutes.
        let outcome = loop {
            let mut progressed = false;

            if let Some(pipe) = stdout_pipe.as_mut() {
                let (read, eof) = pump(pipe, &mut stdout, &mut stdout_chunk).await;
                progressed |= read > 0;
                if eof {
                    stdout_pipe = None;
                }
            }

            if let Some(pipe) = stderr_pipe.as_mut() {
                let (read, eof) = pump(pipe, &mut stderr, &mut stderr_chunk).await;
                progressed |= read > 0;
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
        if !output.ok() {
            // The tail is where ffmpeg says why. Showing the whole of it buries the reason.
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
    parts.extend(args.iter().map(|arg| quote_for_display(&arg.to_string_lossy())));
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
        };
        let handle = tokio::spawn(async move { runner.run(&program("sh"), &args, &options).await });
        tokio::time::sleep(Duration::from_millis(120)).await;
        let started = Instant::now();
        cancel.cancel();
        let result = handle.await.expect("the task did not panic");
        assert!(matches!(result, Err(MediaError::Cancelled)), "{result:?}");
        // The poll interval is 20 ms, so this must not be anywhere near the child's full run.
        assert!(started.elapsed() < Duration::from_secs(5), "cancel was slow");
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
        };
        runner.run(&program("sh"), &args, &options).await.expect("runs");
        let events = sink.events();
        assert!(
            events.iter().any(|event| matches!(event, Progress::Step { label } if label == "copy body")),
            "{events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(event, Progress::Command { .. })),
            "the exact command line must be reported so the log can show it"
        );
        assert!(
            events.iter().any(|event| matches!(event, Progress::Elapsed { .. })),
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
        let output = runner.run(&program("sh"), &args, &options).await.expect("runs");
        assert!(output.stdout.lines().count() > 1_000, "output was truncated");
    }

    #[test]
    fn display_quoting_makes_a_command_line_pasteable() {
        let args = argv(&["-i", r"H:\master takes\Andy Ross.mp4", "-c", "copy"]);
        let text = display_command(Path::new("ffmpeg"), &args);
        assert!(text.starts_with("ffmpeg -i "));
        assert!(text.contains("\"H:\\master takes\\Andy Ross.mp4\""), "{text}");
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
}
