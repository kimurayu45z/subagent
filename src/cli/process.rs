//! Managed child process execution with raw stream forwarding and bounded
//! stdout capture for the durable pair exchange.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

const IO_CHUNK_BYTES: usize = 16 * 1024;
const MAX_FORWARDING_ERRORS: usize = 8;
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildExit {
    Exited(i32),
    #[cfg(unix)]
    Signaled(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildOutcome {
    pub exit: ChildExit,
    pub stdout_capture: Vec<u8>,
    pub stdout_truncated: bool,
    pub timed_out: bool,
    pub forwarding_errors: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum ChildProcessError {
    Spawn(std::io::Error),
    Wait(std::io::Error),
}

impl std::fmt::Display for ChildProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChildProcessError::Spawn(error) => write!(f, "failed to spawn child: {error}"),
            ChildProcessError::Wait(error) => write!(f, "failed while waiting for child: {error}"),
        }
    }
}

impl std::error::Error for ChildProcessError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Stdout,
    Stderr,
}

enum StreamEvent {
    Bytes(StreamKind, Vec<u8>),
    ReadError(StreamKind, String),
    Closed(StreamKind),
}

pub(crate) struct ChildRunRequest<'a> {
    pub program: &'a OsStr,
    pub args: &'a [OsString],
    pub cwd: &'a Path,
    pub stdin_bytes: Vec<u8>,
    pub env_overrides: Vec<(OsString, OsString)>,
    pub max_capture_bytes: usize,
    pub forward_signals: bool,
    pub timeout: Option<Duration>,
}

/// Spawns one child, writes the prepared bootstrap/caller stdin, tees both
/// output streams without text decoding, and captures a bounded prefix of
/// stdout for persistence. Stream write/read failures are reported in the
/// outcome but never replace a successfully observed child exit status.
pub(crate) fn run_child(
    request: ChildRunRequest<'_>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<ChildOutcome, ChildProcessError> {
    let mut command: Command = Command::new(request.program);
    command
        .args(request.args)
        .current_dir(request.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &request.env_overrides {
        command.env(name, value);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child: Child = command.spawn().map_err(ChildProcessError::Spawn)?;
    let child_pid: u32 = child.id();
    let signal_guard: SignalGuard = SignalGuard::install(request.forward_signals);

    let mut child_stdin: std::process::ChildStdin = child
        .stdin
        .take()
        .expect("piped child stdin must be available");
    let stdin_bytes: Vec<u8> = request.stdin_bytes;
    let stdin_thread: thread::JoinHandle<Option<String>> = thread::spawn(move || {
        let result: Result<(), std::io::Error> = child_stdin.write_all(&stdin_bytes);
        drop(child_stdin);
        match result {
            Ok(()) => None,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => None,
            Err(error) => Some(format!("child stdin write failed: {error}")),
        }
    });

    let child_stdout: std::process::ChildStdout = child
        .stdout
        .take()
        .expect("piped child stdout must be available");
    let child_stderr: std::process::ChildStderr = child
        .stderr
        .take()
        .expect("piped child stderr must be available");
    let (sender, receiver): (Sender<StreamEvent>, Receiver<StreamEvent>) = mpsc::channel();
    let stdout_thread: thread::JoinHandle<()> =
        spawn_reader(StreamKind::Stdout, child_stdout, sender.clone());
    let stderr_thread: thread::JoinHandle<()> =
        spawn_reader(StreamKind::Stderr, child_stderr, sender);

    let mut stdout_capture: Vec<u8> = Vec::with_capacity(request.max_capture_bytes.min(64 * 1024));
    let mut stdout_truncated: bool = false;
    let mut forwarding_errors: Vec<String> = Vec::new();
    let mut stdout_closed: bool = false;
    let mut stderr_closed: bool = false;
    let mut exit_status: Option<ExitStatus> = None;
    let started: Instant = Instant::now();
    let mut timed_out: bool = false;
    #[cfg(unix)]
    let mut forwarded_signal: i32 = 0;

    while exit_status.is_none() || !stdout_closed || !stderr_closed {
        match receiver.recv_timeout(WAIT_POLL_INTERVAL) {
            Ok(event) => handle_stream_event(
                event,
                out,
                err,
                &mut stdout_capture,
                request.max_capture_bytes,
                &mut stdout_truncated,
                &mut forwarding_errors,
                &mut stdout_closed,
                &mut stderr_closed,
            ),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                stdout_closed = true;
                stderr_closed = true;
            }
        }

        #[cfg(unix)]
        if request.forward_signals {
            let observed: i32 = FORWARDED_SIGNAL.load(Ordering::SeqCst);
            if observed != 0 && observed != forwarded_signal {
                let child_group: i32 = -(child_pid as i32);
                let result: i32 = unsafe { libc::kill(child_group, observed) };
                if result != 0 {
                    push_forwarding_error(
                        &mut forwarding_errors,
                        format!("failed to forward signal {observed} to child process group"),
                    );
                }
                forwarded_signal = observed;
            }
        }

        if exit_status.is_none() {
            exit_status = child.try_wait().map_err(ChildProcessError::Wait)?;
        }
        if exit_status.is_none()
            && !timed_out
            && request
                .timeout
                .is_some_and(|timeout: Duration| started.elapsed() >= timeout)
        {
            timed_out = true;
            terminate_child(&mut child, child_pid, &mut forwarding_errors);
        }
    }

    let status: ExitStatus = match exit_status {
        Some(status) => status,
        None => child.wait().map_err(ChildProcessError::Wait)?,
    };
    if let Ok(Some(stdin_error)) = stdin_thread.join() {
        push_forwarding_error(&mut forwarding_errors, stdin_error);
    }
    if stdout_thread.join().is_err() {
        push_forwarding_error(
            &mut forwarding_errors,
            "child stdout reader thread panicked".to_string(),
        );
    }
    if stderr_thread.join().is_err() {
        push_forwarding_error(
            &mut forwarding_errors,
            "child stderr reader thread panicked".to_string(),
        );
    }
    drop(signal_guard);

    let exit: ChildExit = child_exit(status);
    Ok(ChildOutcome {
        exit,
        stdout_capture,
        stdout_truncated,
        timed_out,
        forwarding_errors,
    })
}

fn terminate_child(child: &mut Child, child_pid: u32, errors: &mut Vec<String>) {
    #[cfg(unix)]
    {
        let process_group: i32 = -(child_pid as i32);
        let result: i32 = unsafe { libc::kill(process_group, libc::SIGKILL) };
        if result != 0
            && let Err(error) = child.kill()
        {
            push_forwarding_error(
                errors,
                format!("failed to terminate timed-out child: {error}"),
            );
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = child.kill() {
        push_forwarding_error(
            errors,
            format!("failed to terminate timed-out child: {error}"),
        );
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    kind: StreamKind,
    mut reader: R,
    sender: Sender<StreamEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer: Vec<u8> = vec![0_u8; IO_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if sender
                        .send(StreamEvent::Bytes(kind, buffer[..count].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = sender.send(StreamEvent::ReadError(kind, error.to_string()));
                    break;
                }
            }
        }
        let _ = sender.send(StreamEvent::Closed(kind));
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_stream_event(
    event: StreamEvent,
    out: &mut dyn Write,
    err: &mut dyn Write,
    stdout_capture: &mut Vec<u8>,
    max_capture_bytes: usize,
    stdout_truncated: &mut bool,
    forwarding_errors: &mut Vec<String>,
    stdout_closed: &mut bool,
    stderr_closed: &mut bool,
) {
    match event {
        StreamEvent::Bytes(StreamKind::Stdout, bytes) => {
            if let Err(error) = out.write_all(&bytes).and_then(|()| out.flush()) {
                push_forwarding_error(
                    forwarding_errors,
                    format!("child stdout forwarding failed: {error}"),
                );
            }
            let remaining: usize = max_capture_bytes.saturating_sub(stdout_capture.len());
            let kept: usize = remaining.min(bytes.len());
            stdout_capture.extend_from_slice(&bytes[..kept]);
            *stdout_truncated |= kept < bytes.len();
        }
        StreamEvent::Bytes(StreamKind::Stderr, bytes) => {
            if let Err(error) = err.write_all(&bytes).and_then(|()| err.flush()) {
                push_forwarding_error(
                    forwarding_errors,
                    format!("child stderr forwarding failed: {error}"),
                );
            }
        }
        StreamEvent::ReadError(kind, error) => {
            push_forwarding_error(
                forwarding_errors,
                format!("child {kind:?} read failed: {error}"),
            );
        }
        StreamEvent::Closed(StreamKind::Stdout) => *stdout_closed = true,
        StreamEvent::Closed(StreamKind::Stderr) => *stderr_closed = true,
    }
}

fn push_forwarding_error(errors: &mut Vec<String>, message: String) {
    if errors.len() < MAX_FORWARDING_ERRORS && !errors.iter().any(|existing| existing == &message) {
        errors.push(message);
    }
}

#[cfg(unix)]
fn child_exit(status: ExitStatus) -> ChildExit {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), None) => ChildExit::Exited(code),
        (None, Some(signal)) => ChildExit::Signaled(signal),
        _ => ChildExit::Exited(125),
    }
}

#[cfg(not(unix))]
fn child_exit(status: ExitStatus) -> ChildExit {
    ChildExit::Exited(status.code().unwrap_or(125))
}

#[cfg(unix)]
static FORWARDED_SIGNAL: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn signal_handler(signal: libc::c_int) {
    FORWARDED_SIGNAL.store(signal, Ordering::SeqCst);
}

struct SignalGuard {
    #[cfg(unix)]
    previous_int: Option<libc::sigaction>,
    #[cfg(unix)]
    previous_term: Option<libc::sigaction>,
}

impl SignalGuard {
    #[cfg(unix)]
    fn install(enabled: bool) -> SignalGuard {
        if !enabled {
            return SignalGuard {
                previous_int: None,
                previous_term: None,
            };
        }
        FORWARDED_SIGNAL.store(0, Ordering::SeqCst);
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = signal_handler as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;

            let mut previous_int: libc::sigaction = std::mem::zeroed();
            let mut previous_term: libc::sigaction = std::mem::zeroed();
            let int_result: i32 = libc::sigaction(libc::SIGINT, &action, &mut previous_int);
            let term_result: i32 = libc::sigaction(libc::SIGTERM, &action, &mut previous_term);
            SignalGuard {
                previous_int: (int_result == 0).then_some(previous_int),
                previous_term: (term_result == 0).then_some(previous_term),
            }
        }
    }

    #[cfg(not(unix))]
    fn install(_enabled: bool) -> SignalGuard {
        SignalGuard {}
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if let Some(previous) = &self.previous_int {
                libc::sigaction(libc::SIGINT, previous, std::ptr::null_mut());
            }
            if let Some(previous) = &self.previous_term {
                libc::sigaction(libc::SIGTERM, previous, std::ptr::null_mut());
            }
            FORWARDED_SIGNAL.store(0, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn shell_request<'a>(script: &'a str, stdin_bytes: Vec<u8>) -> ChildRunRequest<'a> {
        let args: Vec<OsString> = vec![OsString::from("-c"), OsString::from(script)];
        // The leaked slice is bounded to this short-lived test process and
        // avoids hiding the lifetime relationship in a more complex fixture.
        let args: &'a [OsString] = Box::leak(args.into_boxed_slice());
        ChildRunRequest {
            program: OsStr::new("/bin/sh"),
            args,
            cwd: Path::new("/"),
            stdin_bytes,
            env_overrides: Vec::new(),
            max_capture_bytes: 1024,
            forward_signals: false,
            timeout: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn forwards_streams_captures_stdout_and_preserves_exit_code() {
        let request: ChildRunRequest<'_> =
            shell_request("printf 'out'; printf 'err' >&2; exit 42", Vec::new());
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let outcome: ChildOutcome = run_child(request, &mut out, &mut err).unwrap();
        assert_eq!(out, b"out");
        assert_eq!(err, b"err");
        assert_eq!(outcome.stdout_capture, b"out");
        assert_eq!(outcome.exit, ChildExit::Exited(42));
        assert!(outcome.forwarding_errors.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn writes_prepared_stdin_and_bounds_only_the_capture() {
        let mut request: ChildRunRequest<'_> = shell_request("cat", b"abcdef".to_vec());
        request.max_capture_bytes = 3;
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let outcome: ChildOutcome = run_child(request, &mut out, &mut err).unwrap();
        assert_eq!(out, b"abcdef");
        assert_eq!(outcome.stdout_capture, b"abc");
        assert!(outcome.stdout_truncated);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_the_child_process_group() {
        let mut request: ChildRunRequest<'_> = shell_request("sleep 5", Vec::new());
        request.timeout = Some(Duration::from_millis(50));
        let started: Instant = Instant::now();
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let outcome: ChildOutcome = run_child(request, &mut out, &mut err).unwrap();
        assert!(outcome.timed_out);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn reports_spawn_failure_without_panicking() {
        let args: Vec<OsString> = Vec::new();
        let request = ChildRunRequest {
            program: OsStr::new("/definitely/missing/subagent-child"),
            args: &args,
            cwd: Path::new("/"),
            stdin_bytes: Vec::new(),
            env_overrides: Vec::new(),
            max_capture_bytes: 1024,
            forward_signals: false,
            timeout: None,
        };
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        assert!(matches!(
            run_child(request, &mut out, &mut err),
            Err(ChildProcessError::Spawn(_))
        ));
    }

    #[test]
    fn forwarding_errors_are_deduplicated_and_bounded() {
        let mut errors: Vec<String> = Vec::new();
        for index in 0..32 {
            push_forwarding_error(&mut errors, format!("error-{}", index % 16));
        }
        assert_eq!(errors.len(), MAX_FORWARDING_ERRORS);
        assert_eq!(errors[0], "error-0");
        assert_eq!(errors[7], "error-7");
    }
}
