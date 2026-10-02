//! Bounded JSONL over a capability-protected Unix socket.
//!
//! A bridge belongs to one shell incarnation. Requests name a captured connection
//! and session, are never replayed, and have at most one unacknowledged operation.
//! Filesystem permissions and a random token protect against accidental cross-pane
//! connections, not hostile code running as the same OS user.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Maximum UTF-8 bytes per JSON record, excluding LF.
pub const MAX_FRAME: usize = 65_536;
/// Maximum inserted or submitted text size, leaving room for JSON escaping.
pub const MAX_TEXT: usize = 8_192;
/// Environment variable naming the pane's socket.
pub const SOCKET_ENV: &str = "SKELLY_VOICE_SOCKET";
/// Environment variable carrying the pane's capability.
pub const TOKEN_ENV: &str = "SKELLY_VOICE_TOKEN";
const TICK: Duration = Duration::from_millis(20);
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_BRIDGE: AtomicU64 = AtomicU64::new(1);

/// A captured routing target. Invalidated by disconnect, reload or session change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    bridge: u64,
    connection: u64,
    session: String,
    pid: u32,
}

impl Target {
    /// Whether this Pi process is the pane\'s current foreground process.
    /// A background or wrapped/remote agent is not silently selected.
    #[must_use]
    pub fn is_foreground(&self, pid: Option<u32>) -> bool {
        pid == Some(self.pid)
    }

    /// The connected Pi session's identifier.
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }
}

/// How a submitted utterance should behave if Pi is busy.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Refuse if busy; never silently queue.
    Idle,
    /// Deliver at Pi's next steering boundary (not immediate tool cancellation).
    Steer,
    /// Queue behind the current run.
    FollowUp,
}

/// Operations on the connected Pi session, never on its PTY.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Paste into Pi's editor without submitting or replacing its contents.
    Insert {
        /// Literal transcribed text.
        text: String,
    },
    /// Submit literal user text using Pi's normal agent pipeline.
    Prompt {
        /// Literal transcribed text.
        text: String,
        /// Explicit busy-agent policy.
        delivery: Delivery,
    },
    /// Request cancellation of agent work, independently of audio playback.
    Abort,
}

/// Local request classification for honest acknowledgment feedback (not sent on the wire).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Draft insertion, not submission.
    Insert,
    /// User turn submitted to Pi.
    Prompt,
    /// Agent cancellation requested.
    Abort,
}

impl Action {
    fn operation(&self) -> Operation {
        match self {
            Self::Insert { .. } => Operation::Insert,
            Self::Prompt { .. } => Operation::Prompt,
            Self::Abort => Operation::Abort,
        }
    }
}

struct Pending {
    id: u64,
    started: Instant,
    operation: Operation,
}

/// An acknowledgment of dispatch, not proof that model/tool work finished.
#[derive(Clone, Debug)]
pub struct Reply {
    /// The original request ID.
    pub id: u64,
    /// Which operation was acknowledged, even when its outcome is unknown.
    pub operation: Operation,
    /// Whether the operation was dispatched by the extension.
    pub accepted: bool,
    /// A safe diagnostic, when rejected or the connection failed.
    pub error: Option<String>,
}

/// Current connection state. Does not contain reasoning or tool output.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Capture this before beginning asynchronous transcription.
    pub target: Option<Target>,
    /// Pi has ongoing work.
    pub busy: bool,
    /// Pi is waiting for a modal interaction; never accept it via dictation.
    pub blocked: bool,
    /// Last completed assistant text, published only at final settlement.
    pub answer: Option<String>,
    /// Monotonic settlement revision within this connection; identical replies remain distinct.
    pub answer_revision: u64,
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    pending: Option<Pending>,
    replies: VecDeque<Reply>,
}

#[derive(Serialize)]
struct Request {
    id: u64,
    session: String,
    #[serde(flatten)]
    action: Action,
}

struct Outgoing {
    target: Target,
    request: Request,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Incoming {
    Hello {
        version: u8,
        token: String,
        session: String,
        pid: u32,
    },
    State {
        session: String,
        busy: bool,
        blocked: bool,
    },
    Reply {
        session: String,
        id: u64,
        accepted: bool,
        error: Option<String>,
    },
    Settled {
        session: String,
        text: String,
    },
}

/// One private endpoint owned by a pane, with all socket I/O on a worker.
pub struct Bridge {
    path: PathBuf,
    token: String,
    state: Arc<Mutex<State>>,
    alive: Arc<AtomicBool>,
    sender: mpsc::SyncSender<Outgoing>,
    worker: thread::Thread,
    next_request: AtomicU64,
}

impl Bridge {
    /// Start a private bridge. Does not start Pi or capture audio.
    ///
    /// # Errors
    /// Returns filesystem, entropy or thread-start failures.
    pub fn new(wakeup: impl Fn() + Send + 'static) -> io::Result<Self> {
        // /tmp keeps the path below macOS's small sockaddr_un limit. The random
        // directory is created atomically with 0700, regardless of the umask.
        let dir = tempfile::Builder::new()
            .prefix("skelly-voice-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?;
        let path = dir.path().join("pi.sock");
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let mut entropy = [0_u8; 32];
        getrandom::fill(&mut entropy).map_err(io::Error::other)?;
        let token = entropy.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        });
        let state = Arc::new(Mutex::new(State::default()));
        let alive = Arc::new(AtomicBool::new(true));
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_state = Arc::clone(&state);
        let worker_alive = Arc::clone(&alive);
        let worker_token = token.clone();
        let id = NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed);
        let worker = thread::Builder::new().name("skelly-pi-bridge".into()).spawn(move || {
            let _dir = dir;
            let mut connection = 0;
            while worker_alive.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        connection += 1;
                        let result = serve(
                            stream, &worker_token, id, connection, &worker_state,
                            &worker_alive, &receiver, &wakeup,
                        );
                        let diagnostic = result.err().map_or_else(
                            || "Pi disconnected; pending delivery is unknown and was not retried".into(),
                            |e| format!("Pi bridge: {e}; pending delivery was not retried"),
                        );
                        if let Ok(mut state) = lock(&worker_state) {
                            state.snapshot = Snapshot::default();
                            if let Some(pending) = state.pending.take() {
                                state.replies.push_back(Reply {
                                    id: pending.id, operation: pending.operation, accepted: false, error: Some(diagnostic),
                                });
                            }
                        }
                        wakeup();
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::park_timeout(TICK),
                    Err(_) => break,
                }
            }
        })?;
        Ok(Self {
            path,
            token,
            state,
            alive,
            sender,
            worker: worker.thread().clone(),
            next_request: AtomicU64::new(1),
        })
    }

    /// Socket path inherited by the pane's shell.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Secret capability inherited by the pane's shell. Never log it.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Read a bounded snapshot without socket I/O.
    ///
    /// # Errors
    /// Returns an error if internal state was poisoned.
    pub fn snapshot(&self) -> io::Result<Snapshot> {
        Ok(lock(&self.state)?.snapshot.clone())
    }

    /// Remove one acknowledgment for UI feedback.
    ///
    /// # Errors
    /// Returns an error if internal state was poisoned.
    pub fn take_reply(&self) -> io::Result<Option<Reply>> {
        Ok(lock(&self.state)?.replies.pop_front())
    }

    /// Queue an operation for a previously captured target, without blocking.
    /// No retries occur; a disconnect after dispatch has an unknown outcome.
    ///
    /// # Errors
    /// Rejects stale targets, malformed text, blocked UI, busy-idle prompts,
    /// backpressure or a disconnected worker.
    pub fn send(&self, target: &Target, action: Action) -> io::Result<u64> {
        validate_action(&action)?;
        let mut state = lock(&self.state)?;
        if state.snapshot.target.as_ref() != Some(target) {
            return Err(invalid("Pi session changed or disconnected"));
        }
        if state.snapshot.blocked && !matches!(action, Action::Abort) {
            return Err(invalid("Finish the Pi dialog before sending voice input"));
        }
        if state.snapshot.busy
            && matches!(
                action,
                Action::Prompt {
                    delivery: Delivery::Idle,
                    ..
                }
            )
        {
            return Err(invalid(
                "Pi is busy; choose steering or follow-up explicitly",
            ));
        }
        if state.pending.is_some() || state.replies.len() >= 16 {
            return Err(invalid("Pi bridge is waiting for an acknowledgment"));
        }
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let operation = action.operation();
        let outgoing = Outgoing {
            target: target.clone(),
            request: Request {
                id,
                session: target.session.clone(),
                action,
            },
        };
        self.sender.try_send(outgoing).map_err(io::Error::other)?;
        state.pending = Some(Pending {
            id,
            started: Instant::now(),
            operation,
        });
        self.worker.unpark();
        Ok(id)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.worker.unpark();
        // Revoke the endpoint immediately. The worker owns directory cleanup and
        // exits asynchronously, so closing a pane never joins a socket thread.
        let _ = fs::remove_file(&self.path);
    }
}

fn lock(state: &Mutex<State>) -> io::Result<MutexGuard<'_, State>> {
    state
        .lock()
        .map_err(|_| io::Error::other("Pi bridge state poisoned"))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn validate_action(action: &Action) -> io::Result<()> {
    if let Action::Insert { text } | Action::Prompt { text, .. } = action {
        if text.trim().is_empty()
            || text.len() > MAX_TEXT
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(invalid(
                "Voice input is empty, too long or contains control characters",
            ));
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "worker owns one connection's transport and lifecycle"
)]
fn serve(
    mut stream: UnixStream,
    token: &str,
    bridge: u64,
    connection: u64,
    shared: &Mutex<State>,
    alive: &AtomicBool,
    receiver: &mpsc::Receiver<Outgoing>,
    wakeup: &impl Fn(),
) -> io::Result<()> {
    stream.set_nonblocking(true)?;
    let started = Instant::now();
    let mut target: Option<Target> = None;
    let mut input = Vec::new();
    let mut output = Vec::new();
    let mut written = 0;
    let mut chunk = [0_u8; 4096];
    while alive.load(Ordering::Acquire) {
        if target.is_none() && started.elapsed() > HELLO_TIMEOUT {
            return Err(invalid("Pi handshake timed out"));
        }
        if lock(shared)?
            .pending
            .as_ref()
            .is_some_and(|pending| pending.started.elapsed() > REQUEST_TIMEOUT)
        {
            return Err(invalid("Pi acknowledgment timed out (delivery unknown)"));
        }
        if output.is_empty() {
            if let Ok(outgoing) = receiver.try_recv() {
                // Never route a queued request into a replacement connection.
                if target.as_ref() == Some(&outgoing.target) {
                    output = serde_json::to_vec(&outgoing.request)?;
                    output.push(b'\n');
                    written = 0;
                }
            }
        }
        if written < output.len() {
            match stream.write(&output[written..]) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                Ok(n) => {
                    written += n;
                    if written == output.len() {
                        output.clear();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e),
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                input.extend_from_slice(&chunk[..n]);
                while let Some(end) = input.iter().position(|b| *b == b'\n') {
                    if end > MAX_FRAME {
                        return Err(invalid("Pi frame too large"));
                    }
                    let event = serde_json::from_slice::<Incoming>(&input[..end])?;
                    input.drain(..=end);
                    if let Incoming::Hello {
                        version,
                        token: supplied,
                        session,
                        pid,
                    } = event
                    {
                        if target.is_some()
                            || version != 1
                            || supplied != token
                            || pid == 0
                            || session.is_empty()
                            || session.len() > 128
                        {
                            return Err(invalid("Invalid Pi handshake"));
                        }
                        let connected = Target {
                            bridge,
                            connection,
                            session,
                            pid,
                        };
                        output = serde_json::to_vec(&serde_json::json!({
                            "type": "welcome", "version": 1, "session": connected.session,
                        }))?;
                        output.push(b'\n');
                        written = 0;
                        lock(shared)?.snapshot.target = Some(connected.clone());
                        target = Some(connected);
                    } else {
                        let current = target
                            .as_ref()
                            .ok_or_else(|| invalid("Pi handshake required"))?;
                        apply(event, current, shared)?;
                    }
                    wakeup();
                }
                if input.len() > MAX_FRAME {
                    return Err(invalid("Pi frame too large"));
                }
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        thread::park_timeout(TICK);
    }
    Ok(())
}

fn apply(event: Incoming, target: &Target, shared: &Mutex<State>) -> io::Result<()> {
    let session = match &event {
        Incoming::State { session, .. }
        | Incoming::Reply { session, .. }
        | Incoming::Settled { session, .. } => session,
        Incoming::Hello { .. } => return Err(invalid("Duplicate Pi handshake")),
    };
    if session != &target.session {
        return Err(invalid("Pi session mismatch"));
    }
    let mut state = lock(shared)?;
    match event {
        Incoming::State { busy, blocked, .. } => {
            state.snapshot.busy = busy;
            state.snapshot.blocked = blocked;
            if busy {
                state.snapshot.answer = None;
            }
        }
        Incoming::Reply {
            id,
            accepted,
            error,
            ..
        } => {
            let pending = state
                .pending
                .as_ref()
                .ok_or_else(|| invalid("Unexpected Pi acknowledgment"))?;
            if pending.id != id {
                return Err(invalid("Unexpected Pi acknowledgment"));
            }
            if error.as_ref().is_some_and(|e| e.len() > 512) {
                return Err(invalid("Pi diagnostic too long"));
            }
            let operation = pending.operation;
            state.pending = None;
            state.replies.push_back(Reply {
                id,
                operation,
                accepted,
                error,
            });
        }
        Incoming::Settled { text, .. } => {
            if text.len() > MAX_TEXT {
                return Err(invalid("Pi answer too long"));
            }
            state.snapshot.busy = false;
            state.snapshot.answer = Some(text);
            state.snapshot.answer_revision = state.snapshot.answer_revision.saturating_add(1);
        }
        Incoming::Hello { .. } => unreachable!(),
    }
    Ok(())
}
