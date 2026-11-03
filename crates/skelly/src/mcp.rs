//! `skelly mcp`: Skelly's side of working with an agent, as an MCP server on stdin/stdout.
//!
//! An agent's harness (Claude Code, Codex, Pi's MCP adapter, …) starts `skelly mcp` and
//! finds the tools by asking for them, so the agent needs to know nothing about where Skelly
//! keeps anything. Today that is the diff review: the agent reads the comments the user left
//! on lines of its changes, answers them, and resolves them, through the same helpers as
//! `skelly review` ([`crate::review`]), into the one review file the window watches.
//!
//! The official SDK owns the protocol (ADR-0012): it serves 2026-07-28 clients statelessly
//! (`server/discover`, the version and capabilities on every request) and earlier ones
//! through the `initialize` handshake. Nothing but protocol messages may reach stdout, so
//! this runs before Skelly sets up any logging.

use std::path::PathBuf;

use anyhow::Context as _;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::service::ServerInitializeError;
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt as _};
use serde::Deserialize;

use crate::review;

/// What the server tells the agent when it connects: what Skelly is, and when to reach for
/// which tool.
const INSTRUCTIONS: &str = "Skelly is the terminal the user is running you in. In its git \
    dock the user reviews the changes in a repository and leaves comments on specific lines. \
    These tools are your side of that review: the user sees your replies and resolutions \
    under their comments as they land.\n\
    \n\
    When the user asks you to read or address their review comments, call \
    `list_review_comments`, change the code for each open comment, then call \
    `resolve_review_comment` for each one with a short note saying what you changed. To ask a \
    question or explain why you did not change something, call `reply_to_review_comment` \
    instead and leave the comment open for the user.\n\
    \n\
    Each tool works on the git repository that contains `directory`: pass your working \
    directory. Without it, a tool uses the directory this server was started in.";

/// What every tool's `directory` argument is, for the agent.
const DIRECTORY: &str = "A directory in the git repository, such as your working directory. \
    Defaults to where the server started.";

/// Serve MCP on stdin/stdout until the client closes the connection, which may be before it
/// asks for anything.
///
/// # Errors
/// When the runtime cannot start, or the connection fails before it is closed.
pub(crate) fn serve() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the MCP runtime")?;
    runtime.block_on(async move {
        let service = match (Skelly { cwd }).serve(rmcp::transport::stdio()).await {
            Ok(service) => service,
            Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
            Err(err) => return Err(err).context("starting the MCP server"),
        };
        service.waiting().await.context("serving MCP")?;
        Ok(())
    })
}

/// The server: the directory it was started in is where a tool looks for the repository
/// when the agent does not say.
#[derive(Clone)]
struct Skelly {
    cwd: PathBuf,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Include resolved comments too.
    #[serde(default)]
    all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", description = DIRECTORY)]
    directory: Option<PathBuf>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplyArgs {
    /// The comment's number, from `list_review_comments`.
    #[schemars(range(min = 1))]
    id: u64,
    /// What to say to the user.
    #[schemars(length(min = 1))]
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", description = DIRECTORY)]
    directory: Option<PathBuf>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResolveArgs {
    /// The comment's number, from `list_review_comments`.
    #[schemars(range(min = 1))]
    id: u64,
    /// What you changed to address it.
    #[schemars(length(min = 1))]
    note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", description = DIRECTORY)]
    directory: Option<PathBuf>,
}

#[tool_router]
impl Skelly {
    #[tool(
        title = "Read review comments",
        description = "Read the comments the user left on specific lines of your changes in \
            Skelly's diff review. Each comment has its number, the file and line it is on now \
            (a comment on a removed line is on the file before the change), the quoted line \
            the user saw, what they asked, and any replies. An outdated comment is on a line \
            that has changed since; use the quoted line to find the place. Returns the open \
            comments, or every one with `all`.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_review_comments(&self, Parameters(args): Parameters<ListArgs>) -> CallToolResult {
        let dir = self.dir(args.directory);
        answer(move || {
            let (repo, store) = review::open(&dir)?;
            review::instructions(&repo, &store, args.all)
        })
        .await
    }

    #[tool(
        title = "Reply to a review comment",
        description = "Answer one of the user's Skelly review comments without resolving it: \
            to ask a question, or to explain why you did not change something. The user sees \
            the reply under their comment.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn reply_to_review_comment(
        &self,
        Parameters(args): Parameters<ReplyArgs>,
    ) -> CallToolResult {
        let dir = self.dir(args.directory);
        answer(move || {
            let (_, store) = review::open(&dir)?;
            review::reply(&store, args.id, &args.message)?;
            Ok(format!("Replied to #{}.", args.id))
        })
        .await
    }

    #[tool(
        title = "Resolve a review comment",
        description = "Resolve one of the user's Skelly review comments once you have \
            addressed it. The note is shown to the user under their comment, so say what you \
            changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn resolve_review_comment(
        &self,
        Parameters(args): Parameters<ResolveArgs>,
    ) -> CallToolResult {
        let dir = self.dir(args.directory);
        answer(move || {
            let (_, store) = review::open(&dir)?;
            review::resolve(&store, args.id, Some(&args.note))?;
            Ok(format!("Resolved #{}.", args.id))
        })
        .await
    }
}

#[tool_handler]
impl ServerHandler for Skelly {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("skelly", env!("CARGO_PKG_VERSION")).with_title("Skelly"),
            )
            .with_instructions(INSTRUCTIONS)
    }
}

impl Skelly {
    /// Where a tool looks for the repository: `directory`, relative to where the server was
    /// started, or there.
    fn dir(&self, directory: Option<PathBuf>) -> PathBuf {
        directory.map_or_else(|| self.cwd.clone(), |dir| self.cwd.join(dir))
    }
}

/// Run `work` (git and the review file, which block) off the connection's task, as the
/// tool's result: its text, or why it failed. A failure is the agent's to read and act on,
/// so it is a tool error, not a protocol one.
async fn answer(work: impl FnOnce() -> anyhow::Result<String> + Send + 'static) -> CallToolResult {
    let done = tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|err| Err(anyhow::anyhow!("the tool stopped: {err}")));
    match done {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(err) => CallToolResult::error(vec![ContentBlock::text(format!("{err:#}"))]),
    }
}
