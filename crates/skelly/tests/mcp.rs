//! Integration test: an agent's harness drives the real `skelly mcp` over stdin/stdout.
//!
//! A human comments on lines of a change through `skelly review add`; a client of each
//! protocol era then reads the comments, answers one and resolves one through the server's
//! tools - a 2025 client after the `initialize` handshake, a 2026-07-28 client with the
//! version and capabilities on every request and no handshake. The review file lives under a
//! temp `XDG_STATE_HOME`, never in the repository.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

/// How long a response may take before the test fails instead of hanging.
const PATIENCE: Duration = Duration::from_secs(30);

/// What a 2026-07-28 client puts in every request's `_meta`.
fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
    })
}

/// A running `skelly mcp`, its responses read line by line on a thread of their own.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Server {
    /// Start `skelly mcp` in `cwd`, keeping its state under `home`.
    fn start(cwd: &Path, home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_skelly"))
            .arg("mcp")
            .current_dir(cwd)
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("starting skelly mcp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").expect("writing to skelly mcp");
        stdin.flush().expect("flushing to skelly mcp");
    }

    /// Send request `id` and wait for its response. Every line on stdout must be JSON-RPC.
    fn request(&mut self, id: u64, method: &str, params: &Value) -> Value {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self
                .lines
                .recv_timeout(PATIENCE)
                .unwrap_or_else(|err| panic!("no response to {method}: {err}"));
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|err| panic!("stdout is not JSON-RPC ({err}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] == id {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method }));
    }

    /// Close stdin, as a client does when it is done, and wait for the server to exit.
    fn finish(mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        self.child.wait().expect("waiting for skelly mcp")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The text of a `tools/call` response, and whether the tool reported an error.
fn tool_text(response: &Value) -> (bool, String) {
    let result = &response["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text in {response}"))
        .to_owned();
    (result["isError"] == true, text)
}

/// Run the built `skelly review` in `cwd` with `args`, keeping its state under `home`.
fn review(cwd: &Path, home: &Path, args: &[&str]) -> String {
    let out: Output = Command::new(env!("CARGO_BIN_EXE_skelly"))
        .arg("review")
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .output()
        .expect("running skelly review");
    assert!(
        out.status.success(),
        "skelly review failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run `git -C <root> <args>` in a config-isolated environment, asserting success.
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository under `home` with an uncommitted change and two comments on it, #1 on the
/// renamed line and #2 on the line above.
fn reviewed_repo(home: &Path) -> std::path::PathBuf {
    let root = home.join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\nfn old() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\nfn total() {}\n").unwrap();
    review(&root, home, &["add", "src/lib.rs:2", "call", "it", "sum"]);
    review(&root, home, &["add", "src/lib.rs:1", "why", "a?"]);
    root
}

#[test]
fn a_legacy_client_initializes_then_reads_answers_and_resolves() {
    let home = tempfile::tempdir().expect("temp home");
    let root = reviewed_repo(home.path());
    let status = git(&root, &["status", "--porcelain"]);
    let mut server = Server::start(&root, home.path());

    let init = server.request(
        1,
        "initialize",
        &json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1" },
        }),
    );
    let result = &init["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18", "{init}");
    assert_eq!(result["serverInfo"]["name"], "skelly");
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(result["capabilities"]["tools"].is_object(), "{init}");
    let instructions = result["instructions"].as_str().expect("instructions");
    assert!(
        instructions.contains("list_review_comments")
            && instructions.contains("resolve_review_comment"),
        "the agent is told which tool to reach for: {instructions}"
    );
    server.notify("notifications/initialized");

    let listed = server.request(2, "tools/list", &json!({}));
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names,
        [
            "list_review_comments",
            "reply_to_review_comment",
            "resolve_review_comment"
        ]
    );
    for tool in tools {
        let directory = &tool["inputSchema"]["properties"]["directory"];
        assert_eq!(
            directory["type"], "string",
            "an optional string, not a nullable one, which single-type schema dialects reject: {tool}"
        );
        assert!(
            directory.get("default").is_none(),
            "no null default: {tool}"
        );
    }
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
    assert_eq!(tools[2]["annotations"]["readOnlyHint"], false);
    assert_eq!(
        tools[2]["inputSchema"]["required"],
        json!(["id", "note"]),
        "a resolution always says what was done"
    );

    let open = server.request(
        3,
        "tools/call",
        &json!({ "name": "list_review_comments", "arguments": {} }),
    );
    let (failed, text) = tool_text(&open);
    assert!(!failed, "{text}");
    assert!(text.contains("left 2 open comments"), "{text}");
    assert!(
        text.contains("#1 src/lib.rs:2\n  > fn total() {}\n  call it sum\n"),
        "{text}"
    );

    let replied = server.request(
        4,
        "tools/call",
        &json!({
            "name": "reply_to_review_comment",
            "arguments": { "id": 2, "message": "a is the entry point" },
        }),
    );
    assert_eq!(tool_text(&replied), (false, "Replied to #2.".to_owned()));
    let resolved = server.request(
        5,
        "tools/call",
        &json!({
            "name": "resolve_review_comment",
            "arguments": { "id": 1, "note": "renamed total to sum" },
        }),
    );
    assert_eq!(tool_text(&resolved), (false, "Resolved #1.".to_owned()));
    assert!(server.finish().success(), "closing stdin ends the server");

    let all = review(&root, home.path(), &["--all", "--json"]);
    let json: Value = serde_json::from_str(&all).expect("json");
    let threads = &json["threads"];
    assert_eq!(threads[0]["status"], "resolved");
    assert_eq!(threads[0]["messages"][1]["author"], "agent");
    assert_eq!(threads[0]["messages"][1]["body"], "renamed total to sum");
    assert_eq!(threads[1]["status"], "open", "a reply leaves it open");
    assert_eq!(threads[1]["messages"][1]["body"], "a is the entry point");
    assert_eq!(
        git(&root, &["status", "--porcelain"]),
        status,
        "the review never writes into the repository"
    );
}

#[test]
fn a_2026_client_discovers_and_calls_with_no_handshake() {
    let home = tempfile::tempdir().expect("temp home");
    let root = reviewed_repo(home.path());
    let mut server = Server::start(home.path(), home.path());

    let discovered = server.request(1, "server/discover", &json!({ "_meta": meta() }));
    let result = &discovered["result"];
    assert_eq!(result["resultType"], "complete", "{discovered}");
    let versions = result["supportedVersions"].as_array().expect("versions");
    assert!(
        versions.contains(&json!("2026-07-28")) && versions.contains(&json!("2025-11-25")),
        "{discovered}"
    );
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "skelly"
    );
    assert!(result["instructions"].is_string(), "{discovered}");

    let resolved = server.request(
        2,
        "tools/call",
        &json!({
            "name": "resolve_review_comment",
            "arguments": {
                "id": 2,
                "note": "kept it",
                "directory": root.join("src"),
            },
            "_meta": meta(),
        }),
    );
    assert_eq!(resolved["result"]["resultType"], "complete", "{resolved}");
    assert_eq!(tool_text(&resolved), (false, "Resolved #2.".to_owned()));

    let open = server.request(
        3,
        "tools/call",
        &json!({
            "name": "list_review_comments",
            "arguments": { "directory": "app" },
            "_meta": meta(),
        }),
    );
    let (failed, text) = tool_text(&open);
    assert!(!failed, "{text}");
    assert!(text.contains("left an open comment"), "{text}");
    assert!(
        !text.contains("#2 "),
        "a resolved comment is not asked for again: {text}"
    );

    let bare = server.request(4, "tools/list", &json!({}));
    assert_eq!(
        bare["error"]["code"], -32602,
        "a 2026-07-28 request names its version and capabilities: {bare}"
    );
    assert!(server.finish().success());
}

#[test]
fn mistakes_come_back_as_tool_errors_the_agent_can_read() {
    let home = tempfile::tempdir().expect("temp home");
    let root = reviewed_repo(home.path());
    let mut server = Server::start(&root, home.path());
    let mut call = |id: u64, name: &str, arguments: Value| {
        let response = server.request(
            id,
            "tools/call",
            &json!({ "name": name, "arguments": arguments, "_meta": meta() }),
        );
        let (failed, text) = tool_text(&response);
        assert!(failed, "{name} {arguments} should fail: {text}");
        text
    };

    let missing = call(1, "resolve_review_comment", json!({ "id": 9, "note": "x" }));
    assert!(missing.contains("there is no comment #9"), "{missing}");
    let empty = call(
        2,
        "reply_to_review_comment",
        json!({ "id": 1, "message": "" }),
    );
    assert!(empty.contains("between 1 and 32000 bytes"), "{empty}");
    let outside = call(
        3,
        "list_review_comments",
        json!({ "directory": home.path() }),
    );
    assert!(
        outside.contains("is not inside a git repository"),
        "{outside}"
    );
    let unknown = call(4, "list_review_comments", json!({ "everything": true }));
    assert!(unknown.contains("unknown field `everything`"), "{unknown}");
    assert!(server.finish().success());
}

#[test]
fn a_client_that_closes_before_asking_anything_is_not_an_error() {
    let home = tempfile::tempdir().expect("temp home");
    let server = Server::start(home.path(), home.path());
    assert!(server.finish().success());
}
