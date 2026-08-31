//! `vault-cli` — operator command-line interface for the Memory Vault
//! (T0.1.6 Phase C1b + C2).
//!
//! ## Scope (V0.1, founder-only alpha)
//!
//! Dead-letter triage + on-demand divergence check:
//!
//! - `dead-letter list` — show unresolved dead-letter rows
//! - `dead-letter inspect <id>` — show full detail for one row
//! - `dead-letter retry <id>` — re-run the cascade for one row, mark
//!   resolved as `retried_succeeded` or `retried_failed`
//! - `dead-letter acknowledge <id> --reason <text>` — operator accepts the
//!   loss; row stays for audit but no further retries
//! - `divergence-check` — two-tier consistency check (count + sampled
//!   existence) between SQLite and the vector store; non-zero exit on
//!   findings so scripts notice. See ADR-018.
//!
//! Any richer admin surface lives in its own task with its own scope
//! review (per Phase C plan Q4 "tightness constraint").
//!
//! ## Authentication
//!
//! Authentication is implicit via OS-user keychain access. vault-cli reads
//! the master_key from Windows Credential Manager (the SAME entry vault-
//! tauri manages) via [`vault_app::keychain::read_or_init_master_key`],
//! derives the SqlCipher passphrase + at-rest key per ADR-040 amendment v2
//! option β derivation tree, then opens the storage backend via the sealed
//! companion [`vault_storage::StorageBackend::open_with_at_rest_key`]. The
//! V0.1 passphrase prompt (`rpassword`) was removed at T0.2.0 Phase 3 sub-
//! task (a) (2026-05-11) — `windows-native-keyring-store` reads Credential
//! Manager entries transparently for the running OS user with no separate
//! unlock event to prompt for. Any keychain failure or backend-open failure
//! is reported generically as "authentication failed" with no information
//! leak about which check triggered the failure (BRD §11.7.2 / §11.4.4);
//! diagnostic-side detail goes to the local `tracing` subscriber for dev
//! debugging only.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use uuid::Uuid;
use vault_mcp::DaemonServer;

use vault_app::install_paths;
use vault_app::maintenance_state::{self, RunOutcome, RunSummary};
use vault_app::model_fetch;

use vault_app::keychain::{
    derive_at_rest_key, derive_sqlcipher_passphrase, read_or_init_master_key, PRODUCTION_NAMESPACE,
    VAULT_ID,
};
use vault_app::{AppConfig, Application, ConsolidatorLock, VAULT_LOCKFILE_NAME};
use vault_consolidator::ConsolidationReport;
use vault_core::{Boundary, MemoryId};
use vault_storage::{
    hash_capability_token, AgentToken, CascadeOperation, CheckpointId, CheckpointStatus,
    DeadLetterEntry, DivergenceDetector, DivergenceReport, Resolution, StorageBackend,
};

#[derive(Parser, Debug)]
#[command(name = "zaaheen", about = "Zaaheen operator CLI.", version)]
struct Cli {
    /// Path to the SQLCipher metadata DB. Defaults to the installed vault
    /// (ADR-101) when omitted.
    #[arg(long, value_name = "PATH")]
    vault_db: Option<PathBuf>,

    /// Path to the LanceDB data directory. Defaults to the installed vault
    /// (ADR-101) when omitted.
    #[arg(long, value_name = "PATH")]
    vector_dir: Option<PathBuf>,

    /// Path to the DuckDB graph database file. Defaults to the installed vault
    /// (ADR-101) when omitted.
    #[arg(long, value_name = "PATH")]
    graph_db: Option<PathBuf>,

    /// Embedding dimension expected by the vector store. Must match the
    /// dimension the vault was created with — passing a mismatched value
    /// will be reported as authentication failure (no info leak).
    #[arg(long, default_value_t = 384)]
    dimension: usize,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Dead-letter queue triage — list / inspect / retry / acknowledge.
    DeadLetter {
        #[command(subcommand)]
        action: DeadLetterAction,
    },
    /// On-demand consistency check between SQLite and the LanceDB
    /// vector store. Reports tier-1 count comparison + tier-2 sampled
    /// existence findings. Per Phase A Q3 / ADR-018.
    DivergenceCheck,
    /// Checkpoint & rollback (T0.2.5) — list retained consolidation
    /// checkpoints or undo a run by id. Storage-only: needs no models, so
    /// none of the `--bge-*` / `--phi4-model` flags are required.
    Checkpoint {
        #[command(subcommand)]
        action: CheckpointAction,
    },
    /// Sleep-cycle consolidation per BRD §5.6 — merge near-duplicates,
    /// surface contradictions, emit a run summary. Locked-next-arc Step 4
    /// (T0.3.x Batch A, 2026-05-26): Phi-4-mini drives the merge
    /// classifier; Qwen-7B is NOT used in this path. Requires the BGE
    /// embedder model + tokenizer + ONNX Runtime library + Phi-4-mini
    /// GGUF on disk; arguments accept `VAULT_*` env-var fallbacks for
    /// shell-profile convenience.
    Consolidate {
        /// Path to the BGE-small-en-v1.5 ONNX model file.
        #[arg(long, env = "VAULT_BGE_MODEL_PATH", value_name = "PATH")]
        bge_model: PathBuf,
        /// Path to the BGE-small-en-v1.5 tokenizer.json file.
        #[arg(long, env = "VAULT_BGE_TOKENIZER_PATH", value_name = "PATH")]
        bge_tokenizer: PathBuf,
        /// Path to the ONNX Runtime dynamic library
        /// (libonnxruntime.{dll,dylib,so}).
        #[arg(long, env = "VAULT_ORT_LIB_PATH", value_name = "PATH")]
        ort_lib: PathBuf,
        /// Path to the Phi-4-mini-instruct Q4_K_M GGUF file.
        #[arg(long, env = "VAULT_PHI4_MODEL_PATH", value_name = "PATH")]
        phi4_model: PathBuf,

        #[command(subcommand)]
        action: ConsolidateAction,
    },
    /// MCP (Model Context Protocol) server — exposes the vault to an MCP
    /// client (Claude Desktop, Cursor, etc.) over a stdio JSON-RPC
    /// transport. Locked-next-arc Commit 8 (T0.3.x Batch B, 2026-05-27):
    /// the entrypoint binary that ADR-034 forward-pointed to as part of
    /// "V0.2 alpha-distribution / subcommand-split design". Constructs a
    /// full Application (BGE embedder + read pipeline + cascading worker)
    /// and binds rmcp's stdio transport via Application::start_with_mcp.
    /// Phi-4 model OPTIONAL — only needed if this same process should
    /// also be runnable as a consolidator host; the typical alpha
    /// deployment runs consolidation via `vault-cli consolidate run` in
    /// a separate process.
    Mcp {
        /// Path to the BGE-small-en-v1.5 ONNX model file. Defaults to the copy
        /// installed beside this binary (ADR-101).
        #[arg(long, env = "VAULT_BGE_MODEL_PATH", value_name = "PATH")]
        bge_model: Option<PathBuf>,
        /// Path to the BGE-small-en-v1.5 tokenizer.json file. Defaults to the
        /// copy installed beside this binary (ADR-101).
        #[arg(long, env = "VAULT_BGE_TOKENIZER_PATH", value_name = "PATH")]
        bge_tokenizer: Option<PathBuf>,
        /// Path to the ONNX Runtime dynamic library
        /// (libonnxruntime.{dll,dylib,so}). Defaults to the copy installed
        /// beside this binary (ADR-101).
        #[arg(long, env = "VAULT_ORT_LIB_PATH", value_name = "PATH")]
        ort_lib: Option<PathBuf>,
        /// Path to the Phi-4-mini-instruct Q4_K_M GGUF file. Optional for
        /// this subcommand — the MCP server itself does not require Phi-4
        /// (the read path is fully deterministic per ADR-052). Supply only
        /// if you want this process to also be able to run consolidation
        /// jobs (uncommon for the MCP-server role).
        #[arg(long, env = "VAULT_PHI4_MODEL_PATH", value_name = "PATH")]
        phi4_model: Option<PathBuf>,
        /// Path to the Qwen3-Reranker-0.6B seq-cls ONNX model. When supplied
        /// (with `--rerank-tokenizer`), the read pipeline uses the cross-encoder
        /// reranker as its relevance gate (ADR-057 amendment). Omit both to fall
        /// back to the cosine relevance gate (no-signal abstention only).
        #[arg(long, env = "VAULT_RERANK_MODEL_PATH", value_name = "PATH")]
        rerank_model: Option<PathBuf>,
        /// Path to the Qwen3-Reranker tokenizer.json. Required iff
        /// `--rerank-model` is supplied; reuses `--ort-lib` for the dylib.
        #[arg(long, env = "VAULT_RERANK_TOKENIZER_PATH", value_name = "PATH")]
        rerank_tokenizer: Option<PathBuf>,
        /// Directory the CLI may download its own models into (ADR-100).
        ///
        /// Supplying this makes the CLI self-sufficient: when
        /// `--rerank-model`/`--rerank-tokenizer` are omitted, the reranker is
        /// resolved *inside* this directory and, if the files are not there
        /// yet, downloaded in the background while the server is already
        /// answering. Serving never blocks on the 1.15 GB transfer — reads
        /// use the cosine gate until the download lands, then the reranker
        /// warms up and takes over (the ADR-089 retry contract).
        ///
        /// Required for the `.mcpb` bundle path, where an MCP client launches
        /// this binary directly and there is no desktop app to acquire models
        /// first. Ignored when explicit reranker paths are supplied.
        #[arg(long, env = "VAULT_MODELS_DIR", value_name = "DIR")]
        models_dir: Option<PathBuf>,
        /// Authorized boundary to expose to the MCP client. Repeatable;
        /// defaults to ["personal"] when not supplied. Each boundary the
        /// client can read/write/update/delete must be listed here at
        /// launch time — the server refuses tool calls that touch other
        /// boundaries (BRD §11.4.3).
        #[arg(long, value_name = "NAME", default_values_t = vec!["personal".to_string()])]
        boundary: Vec<String>,
        /// Local time of day (24-hour `HH:MM`) at which the in-process
        /// nightly consolidator should run. Defaults to `03:00` (BRD §5.6)
        /// when omitted. Only meaningful when `--phi4-model` is also
        /// supplied (otherwise no consolidator is wired). Primarily an
        /// ops/testing convenience — point it a minute ahead to watch the
        /// scheduler fire the full pipeline end-to-end.
        #[arg(long, value_name = "HH:MM")]
        run_at: Option<String>,

        #[command(subcommand)]
        action: McpAction,
    },
    /// Manage per-agent capability tokens for the multi-agent daemon
    /// (ADR-SEC-001). Storage-only — loads no models. Each agent connecting to
    /// the daemon presents a bearer token that scopes it to a set of
    /// boundaries; only the token's BLAKE3 hash is stored (the plaintext is
    /// shown ONCE at `add` time). Agents authorized for the same boundary
    /// SHARE its memories (D5 — boundaries are the walls, not agents).
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Multi-agent vault daemon (ADR-SEC-001). Serves the vault to MULTIPLE
    /// agents AT ONCE over rmcp streamable-HTTP on loopback — one process owns
    /// the stores, so concurrent agents are safe (vs `mcp serve`'s 1:1 stdio).
    /// Each agent authenticates with a capability token (`vault-cli agent add`)
    /// that scopes it to its boundaries; agents sharing a boundary share its
    /// memories. Blocks until Ctrl-C. Run nightly consolidation separately via
    /// `vault-cli consolidate run`.
    Daemon {
        /// Path to the BGE-small-en-v1.5 ONNX model file.
        #[arg(long, env = "VAULT_BGE_MODEL_PATH", value_name = "PATH")]
        bge_model: PathBuf,
        /// Path to the BGE-small-en-v1.5 tokenizer.json file.
        #[arg(long, env = "VAULT_BGE_TOKENIZER_PATH", value_name = "PATH")]
        bge_tokenizer: PathBuf,
        /// Path to the ONNX Runtime dynamic library
        /// (libonnxruntime.{dll,dylib,so}).
        #[arg(long, env = "VAULT_ORT_LIB_PATH", value_name = "PATH")]
        ort_lib: PathBuf,
        /// Path to the Qwen3-Reranker-0.6B seq-cls ONNX model. When supplied
        /// (with `--rerank-tokenizer`), the read pipeline uses the cross-encoder
        /// reranker as its relevance gate; omit both to fall back to the cosine
        /// relevance gate.
        #[arg(long, env = "VAULT_RERANK_MODEL_PATH", value_name = "PATH")]
        rerank_model: Option<PathBuf>,
        /// Path to the Qwen3-Reranker tokenizer.json. Required iff
        /// `--rerank-model` is supplied; reuses `--ort-lib` for the dylib.
        #[arg(long, env = "VAULT_RERANK_TOKENIZER_PATH", value_name = "PATH")]
        rerank_tokenizer: Option<PathBuf>,
        /// Loopback TCP port to listen on. The daemon binds 127.0.0.1 only
        /// (never a routable address); agents connect to
        /// `http://127.0.0.1:<port>/mcp`.
        #[arg(long, default_value_t = 8765)]
        port: u16,
    },
}

#[derive(Subcommand, Debug)]
enum ConsolidateAction {
    /// Run one consolidation cycle immediately, print the summary, then exit.
    /// Refuses with `consolidator busy` if another run is in progress
    /// (cross-process lockfile at `<vault_root>/.consolidator.lock`). Hard
    /// timeout 30 min — past this, the run is cancelled and the previous
    /// nightly summary remains the latest artifact on disk.
    Run {
        /// Record the run's outcome into this `maintenance.json` so the
        /// desktop app can show it (ADR-SEC-016).
        ///
        /// Only the run's **counters** are recorded — never the summary
        /// markdown or a contradiction's reasoning, both of which are derived
        /// from memory content and would land in a plaintext file. See
        /// `vault_app::maintenance_state`.
        ///
        /// Written by this process because this is the process holding the
        /// `ConsolidationReport`. A run that never gets this far (a held lock,
        /// a crash) is recorded by whoever spawned us.
        #[arg(long, value_name = "PATH")]
        record_status: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum CheckpointAction {
    /// List retained consolidation checkpoints, newest first.
    List,
    /// Undo a consolidation run: restore every memory it changed to its
    /// pre-run state and delete every memory it created. Idempotent guard —
    /// a checkpoint can only be rolled back once.
    Rollback {
        /// Checkpoint id (UUID) to roll back. Find it via
        /// `vault-cli checkpoint list` or a run summary's footer.
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum McpAction {
    /// Start the MCP server. Blocks on stdio until the client disconnects
    /// (stdio EOF) or the process receives SIGINT (Ctrl-C); on first
    /// SIGINT the cascading retry worker is asked to drain gracefully, on
    /// second SIGINT the process exits with code 130 per the locked
    /// `handle_signals` semantics in `vault-app`.
    Serve,
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Register a new agent and mint its capability token. Prints the token
    /// ONCE — copy it into that agent's MCP config; only its hash is stored,
    /// so it cannot be recovered afterwards.
    Add {
        /// Stable agent identity (lowercase kebab-case, e.g. `claude`,
        /// `work-coder`).
        name: String,
        /// A boundary this agent may reach. Repeatable; at least one required.
        /// Agents authorized for the same boundary share its memories (D5).
        #[arg(long, value_name = "NAME", required = true)]
        boundary: Vec<String>,
    },
    /// List every registered agent (active + revoked), name-ordered.
    List,
    /// Re-scope an active agent's authorized boundaries (replaces the old set;
    /// effective on the agent's next request — no restart, ADR-SEC-001 D4).
    SetBoundaries {
        /// Agent name to re-scope.
        name: String,
        /// The new full boundary set. Repeatable; at least one required.
        #[arg(long, value_name = "NAME", required = true)]
        boundary: Vec<String>,
    },
    /// Revoke an agent's token — its next request is rejected (401).
    Revoke {
        /// Agent name to revoke.
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum DeadLetterAction {
    /// List unresolved dead-letter rows, oldest first.
    List {
        /// Maximum number of rows to display.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Show full detail for one dead-letter row by id.
    Inspect {
        /// Dead-letter row id (UUID).
        id: String,
    },
    /// Re-run the cascade for one dead-letter row. On success the row is
    /// marked `retried_succeeded`; on failure it stays for audit and is
    /// marked `retried_failed`.
    Retry {
        /// Dead-letter row id (UUID).
        id: String,
    },
    /// Operator explicitly accepts the loss. The row stays for audit but
    /// is marked `acknowledged` — no further retries.
    Acknowledge {
        /// Dead-letter row id (UUID).
        id: String,
        /// Operator-supplied reason. Recorded for posterity.
        #[arg(long)]
        reason: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match real_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    // ADR-SEC-015: when we are the child of the windowless maintenance
    // launcher, there is no console and therefore no stderr for logs to reach.
    // `VAULT_LOG_DIR` redirects them into the shared application log file
    // instead, so an overnight failure leaves evidence. Falls through to the
    // stderr subscriber below if the directory cannot be opened -- a log we
    // could not create must never stop a maintenance run.
    if let Some(dir) = std::env::var_os(vault_app::logging::LOG_DIR_ENV) {
        match vault_app::logging::init(Path::new(&dir)) {
            Ok(path) => {
                tracing::info!(log_file = %path.display(), "file logging started");
                return;
            }
            Err(e) => {
                eprintln!("zaaheen: could not start file logging: {e}");
            }
        }
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,vault_cli=info,vault_mcp=info"));
    // Write to STDERR, not STDOUT. The `mcp serve` subcommand reserves
    // STDOUT for the MCP JSON-RPC protocol stream — any byte written
    // there that isn't a valid JSON-RPC message corrupts the channel
    // and the MCP client (Claude Desktop / Cursor / Codex) disconnects.
    // Other subcommands (consolidate / dead-letter / divergence-check)
    // emit their human-facing output via `println!` to STDOUT;
    // diagnostic + lifecycle logs belong on STDERR by convention regardless.
    // ANSI colour codes auto-disable when the writer isn't a terminal
    // (subscriber default) — leave colouring on for interactive runs.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

/// Fill in any storage path the caller omitted, from the installed vault's
/// layout (ADR-101).
///
/// **Why this exists.** The MCP config snippet the desktop app hands users
/// promises `zaaheen mcp serve` with no arguments. Before ADR-101 that command
/// exited with "the following required arguments were not provided", so every
/// tester who followed our own instructions got a dead connection. The snippet
/// was right about the shape; the binary was wrong to demand to be told where
/// its own vault lives.
///
/// **A fully explicit invocation never consults the environment.** That branch
/// returns before `data_dir()` is called, so scripts, CI and dev-tree runs
/// behave exactly as they did — this can only add a path where there was none.
///
/// **Failure is an error, never a guess.** If the environment does not name a
/// data directory we say so and ask for explicit paths. Inventing a plausible
/// location would silently create a second, empty vault, and the user would
/// conclude their memories were gone.
fn resolve_storage_paths(
    vault_db: Option<PathBuf>,
    vector_dir: Option<PathBuf>,
    graph_db: Option<PathBuf>,
) -> Result<(PathBuf, PathBuf, PathBuf)> {
    match (vault_db, vector_dir, graph_db) {
        (Some(db), Some(vectors), Some(graph)) => Ok((db, vectors, graph)),
        (db, vectors, graph) => {
            let data_dir = install_paths::data_dir().ok_or_else(|| {
                anyhow!(
                    "could not determine where the vault lives on this system, \
                     and not every storage path was supplied. Pass --vault-db, \
                     --vector-dir and --graph-db explicitly."
                )
            })?;
            Ok((
                db.unwrap_or_else(|| install_paths::vault_db_in(&data_dir)),
                vectors.unwrap_or_else(|| install_paths::vector_dir_in(&data_dir)),
                graph.unwrap_or_else(|| install_paths::graph_db_in(&data_dir)),
            ))
        }
    }
}

/// Fill in the embedder paths from the files installed beside this binary
/// (ADR-101).
///
/// Mirrors `vault-tauri`'s `resolve_model_path` / `resolve_ort_lib_path`, which
/// ask Tauri for `BaseDirectory::Resource`. That directory is the one holding
/// the executable, which `install_paths::resource_dir` reaches without Tauri —
/// the same sibling-of-current-exe reasoning `vault-maintenance` already uses,
/// and immune to `PATH` shadowing for the same reason.
///
/// As with the storage paths, a fully explicit call returns before the
/// environment is touched, so dev-tree and scripted runs are unaffected.
fn resolve_embedder_paths(
    bge_model: Option<PathBuf>,
    bge_tokenizer: Option<PathBuf>,
    ort_lib: Option<PathBuf>,
) -> Result<(PathBuf, PathBuf, PathBuf)> {
    match (bge_model, bge_tokenizer, ort_lib) {
        (Some(model), Some(tokenizer), Some(lib)) => Ok((model, tokenizer, lib)),
        (model, tokenizer, lib) => {
            let resource_dir = install_paths::resource_dir().ok_or_else(|| {
                anyhow!(
                    "could not locate the directory holding this executable, \
                     and not every embedder path was supplied. Pass \
                     --bge-model, --bge-tokenizer and --ort-lib explicitly."
                )
            })?;
            let lib = match lib {
                Some(explicit) => explicit,
                // `None` here means an OS we do not ship a runtime for. Saying
                // so beats handing the loader a path that cannot exist.
                None => install_paths::ort_lib_in(&resource_dir).ok_or_else(|| {
                    anyhow!(
                        "no bundled ONNX Runtime for this platform ({}); \
                         pass --ort-lib explicitly.",
                        std::env::consts::OS
                    )
                })?,
            };
            Ok((
                model.unwrap_or_else(|| install_paths::bge_model_in(&resource_dir)),
                tokenizer.unwrap_or_else(|| install_paths::bge_tokenizer_in(&resource_dir)),
                lib,
            ))
        }
    }
}

async fn real_main() -> Result<()> {
    let cli = Cli::parse();

    // Destructure command out of cli so the match arms can move action /
    // ConsolidateAction by value while we still borrow the storage-path
    // fields by reference for dispatch helpers. Pre-T0.3.x this function
    // opened the backend upfront and matched on `cli.command` directly;
    // the Consolidate arm needs to NOT open the redundant backend (it
    // builds its own Application internally), so we split the open into
    // a per-arm helper that takes individual fields.
    let Cli {
        vault_db,
        vector_dir,
        graph_db,
        dimension,
        command,
    } = cli;

    let (vault_db, vector_dir, graph_db) = resolve_storage_paths(vault_db, vector_dir, graph_db)?;

    match command {
        Command::DeadLetter { action } => {
            let backend = open_and_warn(&vault_db, &vector_dir, &graph_db, dimension).await?;
            dispatch_dead_letter(&backend, action).await
        }
        Command::DivergenceCheck => {
            let backend = open_and_warn(&vault_db, &vector_dir, &graph_db, dimension).await?;
            run_divergence_check(&backend).await
        }
        Command::Checkpoint { action } => {
            let backend = open_and_warn(&vault_db, &vector_dir, &graph_db, dimension).await?;
            dispatch_checkpoint(&backend, action).await
        }
        Command::Consolidate {
            bge_model,
            bge_tokenizer,
            ort_lib,
            phi4_model,
            action,
        } => {
            // `dimension` is intentionally not threaded into
            // `dispatch_consolidate` — `Application::new` uses the locked
            // `EMBEDDING_DIM` constant from `vault_embedding` internally,
            // not a caller-supplied value. The CLI flag stays for backward
            // compatibility with the dead-letter / divergence-check arms.
            let _ = dimension;
            dispatch_consolidate(
                &vault_db,
                &vector_dir,
                &graph_db,
                bge_model,
                bge_tokenizer,
                ort_lib,
                phi4_model,
                action,
            )
            .await
        }
        Command::Mcp {
            bge_model,
            bge_tokenizer,
            ort_lib,
            phi4_model,
            rerank_model,
            rerank_tokenizer,
            models_dir,
            boundary,
            run_at,
            action,
        } => {
            // Same rationale as Consolidate above — `Application::new`
            // owns the embedding dimension.
            let _ = dimension;
            dispatch_mcp(
                &vault_db,
                &vector_dir,
                &graph_db,
                bge_model,
                bge_tokenizer,
                ort_lib,
                phi4_model,
                rerank_model,
                rerank_tokenizer,
                models_dir,
                boundary,
                run_at,
                action,
            )
            .await
        }
        Command::Agent { action } => {
            let backend = open_and_warn(&vault_db, &vector_dir, &graph_db, dimension).await?;
            dispatch_agent(&backend, action).await
        }
        Command::Daemon {
            bge_model,
            bge_tokenizer,
            ort_lib,
            rerank_model,
            rerank_tokenizer,
            port,
        } => {
            // `Application::new` owns the embedding dimension (same as mcp/consolidate).
            let _ = dimension;
            dispatch_daemon(
                &vault_db,
                &vector_dir,
                &graph_db,
                bge_model,
                bge_tokenizer,
                ort_lib,
                rerank_model,
                rerank_tokenizer,
                port,
            )
            .await
        }
    }
}

/// Open the storage backend via [`open_backend_fields`] and emit a degraded-mode
/// warning to stderr if any downstream store is unreadable. Takes
/// individual storage-path + dimension fields rather than `&Cli` so it
/// can be called inside `real_main`'s match arms (cli.command has been
/// moved by the destructuring — `&cli` no longer borrows cleanly).
async fn open_and_warn(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    dimension: usize,
) -> Result<StorageBackend> {
    let backend = open_backend_fields(vault_db, vector_dir, graph_db, dimension).await?;
    if backend.degraded().is_degraded() {
        eprintln!(
            "warning: vault opened in degraded mode ({:?}) — some downstream stores are unreadable",
            backend.degraded()
        );
    }
    Ok(backend)
}

async fn run_divergence_check(backend: &StorageBackend) -> Result<()> {
    let detector = DivergenceDetector::new(backend.clone());
    let report = detector.run().await?;
    print_divergence_report(&report);
    if report.has_findings() {
        // Non-zero exit when there's anything to triage. Scripts can
        // pipe to a notification channel or fail a CI job.
        anyhow::bail!("divergence findings present — see report above");
    }
    Ok(())
}

fn print_divergence_report(r: &DivergenceReport) {
    println!("divergence check at {}", r.run_at.to_rfc3339());
    println!("  sqlite memories  : {}", r.sqlite_memory_count);
    println!("  vector rows      : {}", r.vector_count);
    if r.count_mismatch() {
        let delta = r.sqlite_memory_count as i64 - r.vector_count as i64;
        println!("  count mismatch   : sqlite - vector = {delta} (tier-1 finding)");
    } else {
        println!("  count match      : ok");
    }
    println!("  samples checked  : {}", r.samples_checked);
    if r.missing_in_vector.is_empty() {
        println!("  missing in vector: (none)");
    } else {
        println!(
            "  missing in vector: {} id(s) — tier-2 sampled-existence finding",
            r.missing_in_vector.len()
        );
        for id in &r.missing_in_vector {
            println!("    - {id}");
        }
    }
    println!(
        "  pending_sync resync count: {} (V0.1 stub — see ADR-018 / HANDOFF tech debt)",
        r.pending_sync_resync_count
    );
    if r.has_findings() {
        println!("\nfindings present — investigate via vault-cli dead-letter list / inspect");
    } else {
        println!("\nno findings.");
    }
}

/// Dispatch the `checkpoint` subcommand (T0.2.5). Storage-only — operates on
/// the already-open [`StorageBackend`]; no models are loaded.
async fn dispatch_checkpoint(backend: &StorageBackend, action: CheckpointAction) -> Result<()> {
    match action {
        CheckpointAction::List => list_checkpoints(backend).await,
        CheckpointAction::Rollback { id } => rollback_checkpoint(backend, &id).await,
    }
}

/// Print all retained consolidation checkpoints, newest first.
async fn list_checkpoints(backend: &StorageBackend) -> Result<()> {
    let checkpoints = backend.list_checkpoints().await?;
    if checkpoints.is_empty() {
        println!("No consolidation checkpoints retained.");
        return Ok(());
    }
    println!("{} checkpoint(s), newest first:", checkpoints.len());
    for cp in &checkpoints {
        let status = match cp.status {
            CheckpointStatus::Active => "active",
            CheckpointStatus::RolledBack => "rolled back",
        };
        println!(
            "  {}  {}  {} change(s)  [{status}]",
            cp.id,
            cp.created_at.to_rfc3339(),
            cp.entry_count,
        );
    }
    Ok(())
}

/// Undo one consolidation run by checkpoint id.
async fn rollback_checkpoint(backend: &StorageBackend, id: &str) -> Result<()> {
    let checkpoint_id: CheckpointId = id
        .parse()
        .map_err(|e| anyhow!("invalid checkpoint id {id:?}: {e}"))?;
    let report = backend
        .rollback_checkpoint(checkpoint_id)
        .await
        .with_context(|| format!("rollback of checkpoint {id} failed"))?;
    println!(
        "Rolled back checkpoint {}: restored {} memory(ies), deleted {} created memory(ies).",
        report.checkpoint_id, report.restored, report.deleted,
    );
    Ok(())
}

// =====================================================================
// `agent` subcommand (ADR-SEC-001) — per-agent capability tokens for the
// multi-agent daemon. Storage-only: operates on the open StorageBackend,
// loads no models.
// =====================================================================

/// Dispatch the `agent` subcommand to its action handler.
async fn dispatch_agent(backend: &StorageBackend, action: AgentAction) -> Result<()> {
    match action {
        AgentAction::Add { name, boundary } => agent_add(backend, &name, boundary).await,
        AgentAction::List => agent_list(backend).await,
        AgentAction::SetBoundaries { name, boundary } => {
            agent_set_boundaries(backend, &name, boundary).await
        }
        AgentAction::Revoke { name } => agent_revoke(backend, &name).await,
    }
}

/// Parse the repeatable `--boundary` flags into validated [`Boundary`] values.
fn parse_agent_boundaries(raw: Vec<String>) -> Result<Vec<Boundary>> {
    raw.iter()
        .map(|b| Boundary::new(b).map_err(|e| anyhow!("invalid boundary {b:?}: {e}")))
        .collect()
}

/// Mint a fresh capability token. Two v4 UUIDs (uuid draws from a CSPRNG via
/// getrandom) concatenated as 64 lowercase-hex chars ≈ 244 bits of entropy —
/// ample for a local bearer token, and adds no new dependency.
fn mint_capability_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Render an agent's boundary set as a comma-separated display string.
fn boundary_display(boundaries: &[Boundary]) -> String {
    boundaries
        .iter()
        .map(Boundary::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Register a new agent and print its freshly-minted token ONCE.
async fn agent_add(backend: &StorageBackend, name: &str, boundary: Vec<String>) -> Result<()> {
    let boundaries = parse_agent_boundaries(boundary)?;
    let token = mint_capability_token();
    let token_hash = hash_capability_token(&token);
    backend
        .register_agent_token(name, &token_hash, &boundaries)
        .await
        .with_context(|| format!("register agent {name:?} (does it already exist?)"))?;

    println!(
        "Registered agent {name:?} with boundaries: {}",
        boundary_display(&boundaries)
    );
    println!();
    println!("Capability token — shown ONCE, copy it into {name}'s MCP config now");
    println!("(only its hash is stored; it cannot be recovered later):");
    println!();
    println!("    {token}");
    println!();
    println!("The agent presents it to the vault daemon as an HTTP header:");
    println!("    Authorization: Bearer {token}");
    Ok(())
}

/// List every registered agent (active + revoked).
async fn agent_list(backend: &StorageBackend) -> Result<()> {
    let agents: Vec<AgentToken> = backend.list_agent_tokens().await?;
    if agents.is_empty() {
        println!("No agents registered. Add one with:");
        println!("    vault-cli agent add <name> --boundary <boundary>");
        return Ok(());
    }
    println!("{} agent(s):", agents.len());
    for a in &agents {
        let status = match a.revoked_at {
            None => "active".to_string(),
            Some(ts) => format!("revoked {}", ts.to_rfc3339()),
        };
        println!(
            "  {}  [{status}]  boundaries: {}",
            a.agent_name,
            boundary_display(&a.boundaries)
        );
    }
    Ok(())
}

/// Re-scope an active agent's authorized boundaries (live — effective on the
/// agent's next request per ADR-SEC-001 D4).
async fn agent_set_boundaries(
    backend: &StorageBackend,
    name: &str,
    boundary: Vec<String>,
) -> Result<()> {
    let boundaries = parse_agent_boundaries(boundary)?;
    let found = backend.set_agent_boundaries(name, &boundaries).await?;
    if !found {
        anyhow::bail!("no active agent named {name:?} (list via `vault-cli agent list`)");
    }
    println!(
        "Re-scoped agent {name:?} to boundaries: {} (effective on its next request).",
        boundary_display(&boundaries)
    );
    Ok(())
}

/// Revoke an active agent's token.
async fn agent_revoke(backend: &StorageBackend, name: &str) -> Result<()> {
    let found = backend.revoke_agent_token(name).await?;
    if !found {
        anyhow::bail!("no active agent named {name:?} to revoke");
    }
    println!("Revoked agent {name:?}. Its capability token is now rejected.");
    Ok(())
}

/// Dispatch the `consolidate` subcommand. Constructs a full [`Application`]
/// (which loads BGE embedder + Phi-4-mini at startup) and routes to the
/// chosen [`ConsolidateAction`]. T0.3.x Batch A.
///
/// Takes the storage-path + model-path fields by value/ref rather than
/// `&Cli` so it can be called from `real_main`'s match arm after
/// `cli.command` has been moved by the destructuring. `dimension` is
/// intentionally omitted — `Application::new` reads the embedding
/// dimension from the locked `vault_embedding::EMBEDDING_DIM` constant.
#[allow(clippy::too_many_arguments)]
async fn dispatch_consolidate(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    bge_model: PathBuf,
    bge_tokenizer: PathBuf,
    ort_lib: PathBuf,
    phi4_model: PathBuf,
    action: ConsolidateAction,
) -> Result<()> {
    // ADR-SEC-002: hold the vault-owner lock for the command's lifetime so a
    // manual consolidation can't race a running daemon (or another writer) —
    // restores the single-owner guard the in-memory graph removed.
    let _vault_lock = ConsolidatorLock::try_acquire_named(
        vault_db
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
        VAULT_LOCKFILE_NAME,
    )
    .context("vault is already in use by another vault-cli process (daemon/serve/consolidate)")?;

    let app = build_application(
        vault_db,
        vector_dir,
        graph_db,
        bge_model,
        bge_tokenizer,
        ort_lib,
        Some(phi4_model),
        // Consolidate has no read pipeline → no reranker.
        None,
        None,
    )
    .await?;
    match action {
        ConsolidateAction::Run { record_status } => {
            run_one_consolidation(&app, record_status.as_deref()).await
        }
    }
}

/// Dispatch the `mcp` subcommand. Constructs a full [`Application`] and
/// hands it to [`Application::start_with_mcp`] which binds rmcp's stdio
/// transport. Blocks until the client (e.g. Claude Desktop) disconnects
/// (stdio EOF) or the user Ctrl-Cs. Locked-next-arc Commit 8.
///
/// `phi4_model` is `Option<PathBuf>` here because the MCP server's read
/// path is fully deterministic per ADR-052 (no LLM in the read path) —
/// Phi-4 is only required if this same process should also be able to
/// host consolidation runs. The typical alpha deployment runs the MCP
/// server and the consolidator in separate processes, so omitting
/// `--phi4-model` is the common case.
#[allow(clippy::too_many_arguments)]
async fn dispatch_mcp(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    bge_model: Option<PathBuf>,
    bge_tokenizer: Option<PathBuf>,
    ort_lib: Option<PathBuf>,
    phi4_model: Option<PathBuf>,
    rerank_model: Option<PathBuf>,
    rerank_tokenizer: Option<PathBuf>,
    models_dir: Option<PathBuf>,
    boundary: Vec<String>,
    run_at: Option<String>,
    action: McpAction,
) -> Result<()> {
    // ADR-101 — the embedder and its runtime ship *beside* this binary, so an
    // installed `zaaheen mcp serve` needs no paths. Resolved before the
    // keychain and the backend so a missing install surfaces immediately
    // rather than after the expensive setup work.
    let (bge_model, bge_tokenizer, ort_lib) =
        resolve_embedder_paths(bge_model, bge_tokenizer, ort_lib)?;

    // ADR-101 + ADR-100 — default the acquisition directory to the vault's own
    // `models/`, derived from the resolved `--vault-db` rather than re-deriving
    // the data directory, so a caller who pointed at a non-default vault gets
    // that vault's models rather than the installed one's. Without this the
    // bare snippet would run on the cosine gate forever: `--models-dir` is what
    // turns the reranker acquisition on, and nobody pasting two lines of JSON
    // is going to pass it.
    let models_dir = models_dir.or_else(|| vault_db.parent().map(install_paths::models_dir_in));
    // Map raw boundary strings to typed Boundary values up front so any
    // parse failure surfaces before we touch the keychain / open the
    // backend / load models (all expensive).
    let authorized_boundaries: Vec<Boundary> = boundary
        .into_iter()
        .map(|raw| {
            // `as_str()` so `raw` stays usable for the error message;
            // `Boundary::new` takes `impl Into<String>` and `&str` is the
            // simplest type that satisfies it without an early move.
            Boundary::new(raw.as_str())
                .map_err(|e| anyhow!("invalid --boundary value {raw:?}: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;

    // Parse the optional --run-at override up front (cheap, fail-fast)
    // before the expensive keychain/backend/model work. `None` leaves the
    // BRD §5.6 default (03:00) in force inside `start_with_mcp`.
    let consolidation_run_at = run_at
        .as_deref()
        .map(|s| {
            chrono::NaiveTime::parse_from_str(s, "%H:%M")
                .map_err(|e| anyhow!("invalid --run-at value {s:?} (expected 24-hour HH:MM): {e}"))
        })
        .transpose()?;

    // ADR-SEC-002: hold the vault-owner lock for the serve session's lifetime
    // so a stdio MCP server can't race a running daemon (or another writer) —
    // restores the single-owner guard the in-memory graph removed.
    let _vault_lock = ConsolidatorLock::try_acquire_named(
        vault_db
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
        VAULT_LOCKFILE_NAME,
    )
    .context("vault is already in use by another vault-cli process (daemon/serve/consolidate)")?;

    // ADR-100 — a `--models-dir` with no explicit reranker paths means "resolve
    // the reranker inside that directory, and fetch it if it isn't there yet".
    // The files legitimately may not exist at this point: `Application::new`
    // wires the reranker from the paths alone, and ADR-089 makes the first load
    // fail soft and leave the cell cold so a later warm-up retries. That is what
    // lets serving start immediately and the model arrive afterwards.
    let (rerank_model, rerank_tokenizer) = match (rerank_model, rerank_tokenizer, &models_dir) {
        (None, None, Some(dir)) => {
            let paths = model_fetch::reranker_paths_in(dir);
            (Some(paths.model), Some(paths.tokenizer))
        }
        (explicit_model, explicit_tokenizer, _) => (explicit_model, explicit_tokenizer),
    };

    let app = build_application(
        vault_db,
        vector_dir,
        graph_db,
        bge_model,
        bge_tokenizer,
        ort_lib,
        phi4_model,
        rerank_model,
        rerank_tokenizer,
    )
    .await?;

    match action {
        McpAction::Serve => {
            run_mcp_serve(app, authorized_boundaries, consolidation_run_at, models_dir).await
        }
    }
}

async fn run_mcp_serve(
    app: Application,
    authorized_boundaries: Vec<Boundary>,
    consolidation_run_at: Option<chrono::NaiveTime>,
    models_dir: Option<PathBuf>,
) -> Result<()> {
    eprintln!(
        "zaaheen mcp serve: ready ({} authorized boundary{})",
        authorized_boundaries.len(),
        if authorized_boundaries.len() == 1 {
            ""
        } else {
            "ies"
        },
    );
    let handle = app
        .start_with_mcp(authorized_boundaries, consolidation_run_at)
        .await
        .context("MCP transport bind failed")?;

    // ADR-100 — first-run model acquisition for the CLI.
    //
    // Deliberately runs CONCURRENTLY with serving rather than before it. An
    // MCP client gives the server a handshake budget measured in seconds; a
    // 1.15 GB download in front of `start_with_mcp` would blow through it and
    // the client would report the server as failed. So the vault answers from
    // the moment it is ready, on the cosine gate, and the reranker takes over
    // when it lands.
    //
    // `join!` rather than `tokio::spawn`: both futures borrow `app`, and a
    // spawned task would need `'static`. Nothing here outlives the serve.
    //
    // A failed download is a WARN, never fatal — an offline user still has a
    // working vault with cosine-gated reads, which is precisely the degraded
    // mode ADR-089 designed for.
    let acquire = async {
        let Some(dir) = models_dir.as_deref() else {
            return;
        };
        match model_fetch::ensure_reranker(dir).await {
            Ok(_) => {
                tracing::info!("reranker acquired; warming up");
                app.spawn_reranker_warmup();
            }
            Err(e) => tracing::warn!(
                error = %e,
                "reranker acquisition failed; reads continue on the cosine gate"
            ),
        }
    };

    let (_, serve_result) = tokio::join!(acquire, handle.wait());
    serve_result.context("MCP serve task exited with an error")?;
    eprintln!("zaaheen mcp serve: clean shutdown");
    Ok(())
}

/// Dispatch the `daemon` subcommand (ADR-SEC-001). Builds the real
/// [`Application`], wraps its adapter in the per-agent-auth [`DaemonServer`],
/// and serves it to MULTIPLE agents over rmcp streamable-HTTP on loopback until
/// Ctrl-C. The single-instance guard is the explicit `.vault.lock` acquired at
/// the top of this function (ADR-SEC-002): the graph now runs in-memory + sealed
/// so the DuckDB exclusive file lock no longer guards the vault, and the port
/// bind only guards a single port — the lockfile makes a second daemon on ANY
/// port fail to start.
#[allow(clippy::too_many_arguments)]
async fn dispatch_daemon(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    bge_model: PathBuf,
    bge_tokenizer: PathBuf,
    ort_lib: PathBuf,
    rerank_model: Option<PathBuf>,
    rerank_tokenizer: Option<PathBuf>,
    port: u16,
) -> Result<()> {
    // ADR-SEC-002: acquire the vault-owner lock BEFORE opening the vault. The
    // graph now runs in-memory + sealed (no plaintext DuckDB file), so the
    // DuckDB exclusive file lock that used to make a second daemon fail to open
    // is gone — this explicit `.vault.lock` restores that single-owner guard.
    // The guard is held for the daemon's whole lifetime (RAII) and released on
    // shutdown. A second daemon on ANY port now fails here with a clear error.
    let vault_root = vault_db
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let _vault_lock = ConsolidatorLock::try_acquire_named(vault_root, VAULT_LOCKFILE_NAME)
        .context("vault is already in use by another vault-cli process (daemon/serve)")?;

    // No Phi-4: the daemon serves reads/writes; nightly consolidation runs
    // out-of-band via `vault-cli consolidate run` (an in-daemon scheduler that
    // mirrors `start_with_mcp` is a follow-up — see ADR-SEC-001).
    let app = build_application(
        vault_db,
        vector_dir,
        graph_db,
        bge_model,
        bge_tokenizer,
        ort_lib,
        None,
        rerank_model,
        rerank_tokenizer,
    )
    .await?;

    // Start the cascading retry worker (drains SQLite → vector store). The
    // returned sender is held for the daemon's lifetime; dropping it on
    // shutdown asks the worker to exit.
    let _worker_shutdown = app.spawn_retry_worker();

    // Wrap the REAL adapter in the auth-gating multi-agent handler.
    let adapter: Arc<dyn vault_mcp::Adapter> = app.adapter().clone();
    let daemon = DaemonServer::new(adapter);

    // rmcp streamable-HTTP service over the daemon handler. The per-session
    // factory hands each connection a clone (all share the one inner adapter →
    // every agent funnels through the single storage gate). The default config
    // restricts `allowed_hosts` to loopback (DNS-rebinding guard).
    let service = StreamableHttpService::new(
        move || Ok::<_, std::io::Error>(daemon.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await.with_context(|| {
        format!("bind {addr} failed (is another vault daemon already running on this port?)")
    })?;
    eprintln!(
        "vault-cli daemon: listening on http://{addr}/mcp — connect agents with their \
         capability tokens (Authorization: Bearer <token>). Ctrl-C to stop."
    );

    // Accept loop with graceful Ctrl-C shutdown.
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _peer) = accepted.context("accept connection failed")?;
                let io = TokioIo::new(stream);
                let hyper_service = TowerToHyperService::new(service.clone());
                tokio::spawn(async move {
                    // A per-connection error (an agent dropping mid-stream) is
                    // normal teardown, not a daemon fault.
                    let _ = auto::Builder::new(TokioExecutor::new())
                        .serve_connection(io, hyper_service)
                        .await;
                });
            }
            _ = &mut shutdown => {
                eprintln!("vault-cli daemon: shutdown signal received — stopping.");
                break;
            }
        }
    }

    // `_worker_shutdown` drops here → the cascading worker exits cleanly.
    eprintln!("vault-cli daemon: clean shutdown.");
    Ok(())
}

/// Build an [`Application`] for the consolidate path. Reads master_key
/// from the OS keychain (same pattern as [`open_backend_inner`]),
/// derives the SqlCipher passphrase + at-rest key, then calls
/// [`Application::new`] which constructs the full V0.2 dependency graph
/// including Phi-4-mini + a [`vault_consolidator::Consolidator`].
///
/// Generic `authentication failed` on keychain / open failure — no info
/// leak per BRD §11.7.2 / §11.4.4. Phi-4 load failures surface a more
/// specific "model load failed" message because the user is the one who
/// just supplied a `--phi4-model` path; obscuring that error would only
/// confuse them, and the path is not a secret.
#[allow(clippy::too_many_arguments)]
async fn build_application(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    bge_model: PathBuf,
    bge_tokenizer: PathBuf,
    ort_lib: PathBuf,
    phi4_model: Option<PathBuf>,
    rerank_model: Option<PathBuf>,
    rerank_tokenizer: Option<PathBuf>,
) -> Result<Application> {
    let master_key = read_or_init_master_key(PRODUCTION_NAMESPACE, VAULT_ID).map_err(|e| {
        tracing::warn!(error = %e, "keychain read failed");
        anyhow!("authentication failed")
    })?;
    let sqlcipher_passphrase = derive_sqlcipher_passphrase(&master_key);
    let at_rest_key = derive_at_rest_key(&master_key);

    // `phi4_model` is required by the consolidate path (the consolidator
    // calls Phi-4-mini for merge classification) and optional for the
    // mcp-serve path (read pipeline is fully deterministic per ADR-052).
    // When `None`, `Application::new` logs a WARN and leaves the
    // Consolidator unwired (`run_consolidation_with_safety` will return
    // `ConsolidatorUnconfigured` if invoked) — graceful degradation.
    let config = AppConfig {
        metadata_path: vault_db.to_path_buf(),
        vector_dir: vector_dir.to_path_buf(),
        graph_path: graph_db.to_path_buf(),
        key: sqlcipher_passphrase,
        model_path: bge_model,
        tokenizer_path: bge_tokenizer,
        ort_lib_path: ort_lib,
        at_rest_key,
        // Both subcommands skip the V0.2-era Qwen read pipeline — ADR-052
        // retired Qwen-7B from the read path entirely.
        qwen_model_path: None,
        phi4_model_path: phi4_model,
        // Cross-encoder reranker (ADR-057 amendment). Supplied for the
        // mcp-serve path; None for consolidate (no read pipeline there).
        // Both must be Some for Application::new to wire the reranker.
        rerank_model_path: rerank_model,
        rerank_tokenizer_path: rerank_tokenizer,
    };

    Application::new(&config).await.map_err(|e| {
        // Surface the underlying VaultError class so the user can act on
        // it: a `Llm(...)` error names "Phi-4-mini load failed at startup"
        // which is the most common case for first-time setup (wrong path,
        // wrong file, missing GGUF). Any other class falls through to
        // "authentication failed" to avoid leaking storage-layer detail.
        tracing::warn!(error = %e, "Application::new failed in consolidate path");
        match e {
            vault_core::VaultError::Llm(msg) => anyhow!("model load failed: {msg}"),
            vault_core::VaultError::Config(msg) => anyhow!("configuration error: {msg}"),
            _ => anyhow!("authentication failed"),
        }
    })
}

async fn run_one_consolidation(app: &Application, record_status: Option<&Path>) -> Result<()> {
    println!("starting consolidation run...");
    let report = app
        .run_consolidation_with_safety()
        .await
        .context("consolidation run failed")?;
    print_consolidation_report(&report);

    if let Some(status_path) = record_status {
        let outcome = RunOutcome::Completed(run_summary_of(&report));
        // Best-effort: a status file we could not write is a stale badge in
        // the UI, not a reason to fail a consolidation that already succeeded
        // and already committed its work.
        if let Err(e) =
            maintenance_state::record_run(status_path, &outcome, chrono::Utc::now().to_rfc3339())
        {
            tracing::warn!(error = %e, "could not record the maintenance outcome");
        }
    }
    Ok(())
}

/// Project a [`ConsolidationReport`] onto the counters that are safe to write
/// to `maintenance.json`.
///
/// This is the privacy boundary for ADR-SEC-016: the report also carries
/// `summary_markdown` and per-conflict `reasoning`, both derived from memory
/// content, and neither crosses into the plaintext status file. The mapping is
/// explicit field-by-field rather than a serde passthrough so that adding a
/// text field to `ConsolidationReport` cannot silently start leaking it.
fn run_summary_of(r: &ConsolidationReport) -> RunSummary {
    RunSummary {
        memories_processed: r.memories_processed as u64,
        memories_merged: r.memories_merged as u64,
        memories_deduped: r.memories_deduped as u64,
        memories_archived: r.memories_archived as u64,
        contradictions_resolved: r.contradictions_resolved as u64,
        duration_secs: r.duration.as_secs(),
    }
}

fn print_consolidation_report(r: &ConsolidationReport) {
    println!();
    println!("consolidation run complete.");
    println!("  memories processed   : {}", r.memories_processed);
    println!("  merges applied       : {}", r.memories_merged);
    println!("  clusters deduped     : {}", r.clusters_deduped);
    println!("  memories deduped     : {}", r.memories_deduped);
    println!("  clusters skipped     : {}", r.clusters_skipped);
    println!("  contradictions queued: {}", r.contradictions_resolved);
    println!(
        "  facts retired (contra): {}",
        r.contradictions_auto_resolved
    );
    println!("  memories archived    : {}", r.memories_archived);
    println!("  duration             : {:.2}s", r.duration.as_secs_f64());
    if r.clusters_skipped > 0 {
        println!(
            "  note: {} cluster(s) were skipped (a merge failed and was logged); \
             they remain unmerged and the next run retries.",
            r.clusters_skipped
        );
    }
    if !r.conflicts_for_user_review.is_empty() {
        println!();
        println!("contradictions surfaced for user review:");
        for c in &r.conflicts_for_user_review {
            println!("  - conflict {} (boundary: {})", c.conflict_id, c.boundary);
            println!("    {}", c.reasoning);
        }
    }
    if !r.summary_markdown.is_empty() {
        println!();
        println!("--- summary markdown ---");
        println!("{}", r.summary_markdown);
    }
}

/// Open the storage backend from individual path + dimension fields,
/// reading master_key from the OS keychain and deriving the SqlCipher
/// passphrase + at-rest key per ADR-040 amendment v2 option β derivation
/// tree. Generic `authentication failed` on any error — no info leak per
/// BRD §11.7.2 / §11.4.4. Detailed diagnostics go to the local `tracing`
/// subscriber for dev debugging only.
///
/// Production callers (`real_main`'s match arms) invoke this directly;
/// [`open_backend_inner`] is the per-test sibling that accepts a custom
/// `namespace` + `vault_id` for keychain isolation. T0.3.x Batch A
/// replaced the prior `&Cli`-based entrypoint with this fields-based
/// shape so the match arms can borrow individual fields after
/// `cli.command` has been moved by destructuring.
async fn open_backend_fields(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    dimension: usize,
) -> Result<StorageBackend> {
    open_backend_inner(
        vault_db,
        vector_dir,
        graph_db,
        dimension,
        PRODUCTION_NAMESPACE,
        VAULT_ID,
    )
    .await
}

/// Inner helper taking individual storage fields + keychain `namespace` +
/// `vault_id` as parameters so tests can inject unique-per-test ids via
/// [`vault_app::keychain::test_helpers::unique_test_namespace`] and avoid
/// colliding with the production keychain entry.
///
/// Production callers use [`open_backend_fields`] which passes
/// [`PRODUCTION_NAMESPACE`] + [`VAULT_ID`]. T0.3.x Batch A migrated this
/// fn from a `&Cli` parameter to individual fields so it composes
/// cleanly with the cli-destructured `real_main` dispatch.
pub(crate) async fn open_backend_inner(
    vault_db: &Path,
    vector_dir: &Path,
    graph_db: &Path,
    dimension: usize,
    namespace: &str,
    vault_id: &str,
) -> Result<StorageBackend> {
    let master_key = read_or_init_master_key(namespace, vault_id).map_err(|e| {
        tracing::warn!(error = %e, "keychain read failed");
        anyhow!("authentication failed")
    })?;
    let sqlcipher_passphrase = derive_sqlcipher_passphrase(&master_key);
    let at_rest_key = derive_at_rest_key(&master_key);
    StorageBackend::open_with_at_rest_key(
        vault_db,
        vector_dir,
        graph_db,
        sqlcipher_passphrase,
        dimension,
        &at_rest_key,
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "open backend failed");
        anyhow!("authentication failed")
    })
}

async fn dispatch_dead_letter(backend: &StorageBackend, action: DeadLetterAction) -> Result<()> {
    match action {
        DeadLetterAction::List { limit } => list_dead_letters(backend, limit).await,
        DeadLetterAction::Inspect { id } => inspect_dead_letter(backend, &id).await,
        DeadLetterAction::Retry { id } => retry_dead_letter(backend, &id).await,
        DeadLetterAction::Acknowledge { id, reason } => {
            acknowledge_dead_letter(backend, &id, &reason).await
        }
    }
}

async fn list_dead_letters(backend: &StorageBackend, limit: usize) -> Result<()> {
    let entries = backend.dead_letter().list_unresolved(limit).await?;
    if entries.is_empty() {
        println!("(no unresolved dead-letter rows)");
        return Ok(());
    }
    println!(
        "{:<36}  {:<8}  {:<6}  {:<25}  reason",
        "id", "op", "tries", "first_failed_at"
    );
    println!("{}", "-".repeat(110));
    for e in &entries {
        let reason_summary = summarize(&e.failure_reason, 60);
        println!(
            "{:<36}  {:<8}  {:<6}  {:<25}  {}",
            e.id,
            e.failed_operation.as_str(),
            e.attempts_made,
            e.first_failed_at.format("%Y-%m-%dT%H:%M:%SZ"),
            reason_summary,
        );
    }
    println!("\n{} unresolved row(s).", entries.len());
    Ok(())
}

async fn inspect_dead_letter(backend: &StorageBackend, id_str: &str) -> Result<()> {
    let id = Uuid::parse_str(id_str).context("invalid uuid for dead-letter id")?;
    let entry = backend
        .dead_letter()
        .get(id)
        .await?
        .ok_or_else(|| anyhow!("dead-letter row {id} not found"))?;
    print_full_entry(&entry);
    Ok(())
}

async fn retry_dead_letter(backend: &StorageBackend, id_str: &str) -> Result<()> {
    let id = Uuid::parse_str(id_str).context("invalid uuid for dead-letter id")?;
    let entry = backend
        .dead_letter()
        .get(id)
        .await?
        .ok_or_else(|| anyhow!("dead-letter row {id} not found"))?;

    if let Some(existing) = entry.resolution {
        println!(
            "row {id} is already resolved as {} (no action taken)",
            resolution_str(existing)
        );
        return Ok(());
    }

    println!(
        "retrying {} for memory {} (attempts so far: {})...",
        entry.failed_operation.as_str(),
        entry.memory_id,
        entry.attempts_made,
    );

    let cascade_result = run_cascade_synchronous(backend, &entry).await;

    match cascade_result {
        Ok(()) => {
            backend
                .dead_letter()
                .resolve(id, Resolution::RetriedSucceeded)
                .await?;
            println!("retry succeeded; row marked retried_succeeded");
            Ok(())
        }
        Err(e) => {
            backend
                .dead_letter()
                .resolve(id, Resolution::RetriedFailed)
                .await?;
            // Print the error so the operator can see what's still broken,
            // but exit non-zero so scripts notice.
            Err(anyhow!("retry failed; row marked retried_failed: {e}"))
        }
    }
}

async fn acknowledge_dead_letter(
    backend: &StorageBackend,
    id_str: &str,
    reason: &str,
) -> Result<()> {
    if reason.trim().is_empty() {
        anyhow::bail!("--reason must not be empty");
    }
    let id = Uuid::parse_str(id_str).context("invalid uuid for dead-letter id")?;
    let entry = backend
        .dead_letter()
        .get(id)
        .await?
        .ok_or_else(|| anyhow!("dead-letter row {id} not found"))?;
    if let Some(existing) = entry.resolution {
        println!(
            "row {id} is already resolved as {} (no action taken)",
            resolution_str(existing)
        );
        return Ok(());
    }
    backend
        .dead_letter()
        .resolve(id, Resolution::Acknowledged)
        .await?;
    println!("row {id} acknowledged. reason recorded: {reason}");
    Ok(())
}

/// Run the cascade for one dead-letter entry synchronously — vector op
/// only, since V0.1 graph cascade is a no-op for memory writes. Mirrors
/// `RetryWorker::run_cascade` but without fault hooks (operator retry is
/// production-mode by definition).
async fn run_cascade_synchronous(
    backend: &StorageBackend,
    entry: &DeadLetterEntry,
) -> anyhow::Result<()> {
    // Decode the payload. Format-version dispatch lets us evolve the
    // shape later without breaking older rows.
    let payload: vault_storage::cascading::CascadePayloadV1 =
        serde_json::from_value(entry.payload.clone()).context("decode cascade payload")?;

    match entry.failed_operation {
        CascadeOperation::Write | CascadeOperation::Update => {
            let boundary = Boundary::new(payload.boundary).context("payload boundary invalid")?;
            backend
                .vector_store()
                .upsert(&entry.memory_id, &payload.embedding, &boundary)
                .await?;
        }
        CascadeOperation::Delete => {
            backend.vector_store().delete(&entry.memory_id).await?;
        }
    }
    Ok(())
}

fn print_full_entry(e: &DeadLetterEntry) {
    println!("dead-letter row {}", e.id);
    println!("  memory id          : {}", e.memory_id);
    println!("  failed operation   : {}", e.failed_operation.as_str());
    println!("  attempts made      : {}", e.attempts_made);
    println!("  first failed at    : {}", e.first_failed_at.to_rfc3339());
    println!(
        "  last attempted at  : {}",
        e.last_attempted_at.to_rfc3339()
    );
    println!("  payload version    : {}", e.payload_format_version);
    println!(
        "  resolution         : {}",
        e.resolution.map(resolution_str).unwrap_or("pending"),
    );
    if let Some(at) = e.resolved_at {
        println!("  resolved at        : {}", at.to_rfc3339());
    }
    println!("  failure reason:");
    for line in e.failure_reason.lines() {
        println!("    {line}");
    }
    if !e.payload.is_object() && !e.payload.is_array() {
        println!("  payload            : {}", e.payload);
    } else if let Ok(pretty) = serde_json::to_string_pretty(&e.payload) {
        println!("  payload:");
        for line in pretty.lines() {
            println!("    {line}");
        }
    }
}

fn resolution_str(r: Resolution) -> &'static str {
    match r {
        Resolution::RetriedSucceeded => "retried_succeeded",
        Resolution::RetriedFailed => "retried_failed",
        Resolution::Acknowledged => "acknowledged",
        Resolution::AutoRecovered => "auto_recovered",
    }
}

fn summarize(s: &str, max_chars: usize) -> String {
    let first_line = s.lines().next().unwrap_or("");
    if first_line.chars().count() <= max_chars {
        first_line.to_string()
    } else {
        let mut out: String = first_line.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    }
}

// silence unused-import lint when the cascading::CascadePayloadV1 path is
// not present at integration-test time.
#[allow(dead_code)]
fn _force_link(_id: MemoryId) {}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    //! These are smoke tests against a real SQLCipher tempdir per Phase C
    //! plan Q4 ("vault-cli smoke tests against a real SQLCipher tempdir").
    //! Authentication is exercised end-to-end — no mocking the backend.

    use super::*;

    use std::path::Path;

    use tempfile::TempDir;

    use vault_storage::{NewDeadLetter, SqlCipherKey, PAYLOAD_FORMAT_VERSION};

    // Keychain test helpers (unique_test_namespace, cleanup_keychain_entry,
    // keychain_test_guard, plant_malformed_keychain_entry) are imported from
    // vault_app::keychain::test_helpers via the `test-helpers` feature
    // (vault-cli's [dev-dependencies] enables it). Module is itself
    // `#[cfg(windows)]`-gated upstream; this `use` mirrors that gating.
    #[cfg(windows)]
    use vault_app::keychain::test_helpers::*;

    const DIM: usize = 4;

    /// Test-fixture at-rest key consumed by `make_backend` + the 2 new
    /// sub-task (a) tests. Distinct from any production-derived at_rest_key
    /// (which is BLAKE3-derived from the master_key per ADR-040 amendment
    /// v2). 32 bytes of `0x42` is intentionally non-random + non-zero so
    /// debug output is recognisable as test-data.
    const TEST_AT_REST_KEY: [u8; 32] = [0x42u8; 32];

    async fn make_backend(tmp: &Path) -> StorageBackend {
        let metadata_path = tmp.join("vault.db");
        let vector_dir = tmp.join("lance");
        let graph_path = tmp.join("graph.duckdb");
        let key = SqlCipherKey::new("vault-cli-test-key");
        StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap()
    }

    fn embedding(fill: f32) -> Vec<f32> {
        vec![fill; DIM]
    }

    /// Plant a dead-letter row directly via the `DeadLetter::insert` API
    /// so the test doesn't need to spin up a worker.
    async fn plant_dead_letter(
        backend: &StorageBackend,
        memory_id: MemoryId,
        op: CascadeOperation,
        reason: &str,
        payload: serde_json::Value,
    ) -> Uuid {
        let now = chrono::Utc::now();
        let new = NewDeadLetter {
            memory_id,
            failed_operation: op,
            failure_reason: reason.to_string(),
            attempts_made: 8,
            first_failed_at: now - chrono::Duration::seconds(60),
            last_attempted_at: now,
            payload_format_version: PAYLOAD_FORMAT_VERSION,
            payload,
        };
        backend.dead_letter().insert(new).await.unwrap()
    }

    // --------------------------------------------------------------
    // CLI argument parsing
    // --------------------------------------------------------------

    #[test]
    fn cli_parses_dead_letter_list() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "--dimension",
            "16",
            "dead-letter",
            "list",
            "--limit",
            "10",
        ])
        .unwrap();
        assert_eq!(cli.dimension, 16);
        match cli.command {
            Command::DeadLetter {
                action: DeadLetterAction::List { limit },
            } => assert_eq!(limit, 10),
            _ => panic!("expected DeadLetter::List"),
        }
    }

    #[test]
    fn cli_parses_dead_letter_acknowledge() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "dead-letter",
            "acknowledge",
            "deadbeef-dead-beef-dead-beefdeadbeef",
            "--reason",
            "operator accepts loss",
        ])
        .unwrap();
        match cli.command {
            Command::DeadLetter {
                action: DeadLetterAction::Acknowledge { id, reason },
            } => {
                assert_eq!(id, "deadbeef-dead-beef-dead-beefdeadbeef");
                assert_eq!(reason, "operator accepts loss");
            }
            _ => panic!("expected DeadLetter::Acknowledge"),
        }
    }

    #[test]
    fn cli_parses_divergence_check() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "divergence-check",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::DivergenceCheck));
    }

    #[test]
    fn cli_parses_checkpoint_list_with_no_model_flags() {
        // `checkpoint` is storage-only — it must parse WITHOUT any --bge-* /
        // --phi4-model flags (the whole point of making it a top-level command).
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "checkpoint",
            "list",
        ])
        .expect("checkpoint-list should parse with no model flags");
        match cli.command {
            Command::Checkpoint { action } => {
                assert!(
                    matches!(action, CheckpointAction::List),
                    "expected CheckpointAction::List; got {action:?}"
                );
            }
            other => panic!("expected Command::Checkpoint, got: {other:?}"),
        }
    }

    #[test]
    fn cli_parses_checkpoint_rollback_with_id() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "checkpoint",
            "rollback",
            "0190a0c1-2b3c-7d4e-8f90-1a2b3c4d5e6f",
        ])
        .expect("checkpoint-rollback should parse with an id");
        match cli.command {
            Command::Checkpoint {
                action: CheckpointAction::Rollback { id },
            } => assert_eq!(id, "0190a0c1-2b3c-7d4e-8f90-1a2b3c4d5e6f"),
            other => panic!("expected Command::Checkpoint rollback, got: {other:?}"),
        }
    }

    #[test]
    fn cli_parses_consolidate_run_with_all_paths_supplied() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "consolidate",
            "--bge-model",
            "/tmp/bge.onnx",
            "--bge-tokenizer",
            "/tmp/tokenizer.json",
            "--ort-lib",
            "/tmp/libonnxruntime.so",
            "--phi4-model",
            "/tmp/phi-4-mini.gguf",
            "run",
        ])
        .expect("consolidate-run flat path should parse");
        match cli.command {
            Command::Consolidate {
                bge_model,
                bge_tokenizer,
                ort_lib,
                phi4_model,
                action,
            } => {
                // NOT Option here: ADR-101 gave defaults to `mcp` only, which
                // is the subcommand an MCP client launches from a pasted
                // snippet. `consolidate` is invoked by the maintenance runner,
                // which always passes explicit paths, so it keeps its required
                // arguments and this arm keeps its plain `PathBuf` assertions.
                assert_eq!(bge_model, PathBuf::from("/tmp/bge.onnx"));
                assert_eq!(bge_tokenizer, PathBuf::from("/tmp/tokenizer.json"));
                assert_eq!(ort_lib, PathBuf::from("/tmp/libonnxruntime.so"));
                assert_eq!(phi4_model, PathBuf::from("/tmp/phi-4-mini.gguf"));
                assert!(
                    matches!(
                        action,
                        ConsolidateAction::Run {
                            record_status: None
                        }
                    ),
                    "expected ConsolidateAction::Run; got {action:?}"
                );
            }
            other => panic!("expected Command::Consolidate, got: {other:?}"),
        }
    }

    /// ADR-SEC-016: the scheduled path passes `--record-status` so the run
    /// that does the work is the one that reports it. A parse regression here
    /// would silently return the Maintenance tab to showing a stale outcome
    /// forever, which is the exact bug the flag exists to fix.
    #[test]
    fn cli_parses_consolidate_run_with_a_status_file() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/vault.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/graph.duckdb",
            "consolidate",
            "--bge-model",
            "/tmp/bge.onnx",
            "--bge-tokenizer",
            "/tmp/tokenizer.json",
            "--ort-lib",
            "/tmp/libonnxruntime.so",
            "--phi4-model",
            "/tmp/phi-4-mini.gguf",
            "run",
            "--record-status",
            "/tmp/maintenance.json",
        ])
        .expect("consolidate-run with --record-status should parse");
        match cli.command {
            Command::Consolidate { action, .. } => match action {
                ConsolidateAction::Run { record_status } => {
                    assert_eq!(record_status, Some(PathBuf::from("/tmp/maintenance.json")));
                }
            },
            other => panic!("expected Command::Consolidate, got: {other:?}"),
        }
    }

    /// The projection that keeps memory-derived text out of a plaintext file.
    #[test]
    fn the_recorded_summary_carries_counters_not_report_prose() {
        let report = ConsolidationReport {
            memories_processed: 41,
            memories_merged: 3,
            contradictions_resolved: 4,
            contradictions_auto_resolved: 0,
            memories_archived: 1,
            memories_decayed: 0,
            clusters_deduped: 0,
            memories_deduped: 2,
            clusters_skipped: 0,
            duration: std::time::Duration::from_secs(82),
            conflicts_for_user_review: Vec::new(),
            checkpoint_id: None,
            summary_markdown: "the user moved to Porto and stopped cycling".into(),
        };

        let summary = run_summary_of(&report);
        assert_eq!(summary.memories_processed, 41);
        assert_eq!(summary.memories_merged, 3);
        assert_eq!(summary.memories_deduped, 2);
        assert_eq!(summary.memories_archived, 1);
        assert_eq!(summary.contradictions_resolved, 4);
        assert_eq!(summary.duration_secs, 82);
        assert!(
            summary.to_line().contains("in 82s"),
            "the duration must reach the summary line a human reads: {}",
            summary.to_line()
        );

        // The report's prose must not survive the projection.
        let line = summary.to_line();
        assert!(
            !line.contains("Porto") && !line.contains("cycling"),
            "memory-derived text reached the plaintext status line: {line}"
        );
    }

    #[test]
    fn cli_rejects_consolidate_run_with_missing_phi4_model_path() {
        let result = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "consolidate",
            "--bge-model",
            "/tmp/bge.onnx",
            "--bge-tokenizer",
            "/tmp/tokenizer.json",
            "--ort-lib",
            "/tmp/libonnxruntime.so",
            // --phi4-model deliberately omitted; env-var also unset for this test
            "run",
        ]);
        // clap will reject missing required-or-env arg unless env var supplies it.
        // The CI matrix does not set VAULT_PHI4_MODEL_PATH; under `cargo test` the
        // env var is also unset. clap surfaces a parse error before our code runs.
        // If a future CI layer DOES set the env var, this test will start passing
        // unexpectedly — that's a signal to harden test isolation rather than
        // silently weaken the contract.
        assert!(
            result.is_err() || std::env::var("VAULT_PHI4_MODEL_PATH").is_ok(),
            "consolidate-run MUST refuse missing --phi4-model unless env var supplies it"
        );
    }

    #[test]
    fn cli_accepts_a_partial_storage_path_set_and_leaves_the_rest_unset() {
        // INVERTED BY ADR-101. This test previously asserted the CLI REJECTED a
        // missing --vector-dir. That contract is deliberately gone: an
        // installed `zaaheen` knows where its own vault lives, and demanding to
        // be told was why the connect snippet the app hands users could not
        // run. What must still hold is that parsing leaves the omitted field
        // `None` — `resolve_storage_paths` fills it, so a default arriving at
        // parse time would silently defeat the "explicit wins" guarantee.
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            // --vector-dir omitted on purpose
            "--graph-db",
            "/tmp/g.duckdb",
            "dead-letter",
            "list",
        ])
        .expect("omitting a storage path MUST parse; the installed layout supplies it");

        assert_eq!(cli.vault_db, Some(PathBuf::from("/tmp/v.db")));
        assert_eq!(
            cli.vector_dir, None,
            "an omitted path must stay None at parse time so resolve_storage_paths owns the default"
        );
        assert_eq!(cli.graph_db, Some(PathBuf::from("/tmp/g.duckdb")));
    }

    // --------------------------------------------------------------
    // T0.3.x Batch B Commit 8 — `mcp serve` subcommand parsing
    // --------------------------------------------------------------

    #[test]
    fn cli_parses_mcp_serve_with_default_boundary() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "mcp",
            "--bge-model",
            "/tmp/bge.onnx",
            "--bge-tokenizer",
            "/tmp/tokenizer.json",
            "--ort-lib",
            "/tmp/libonnxruntime.so",
            // --phi4-model deliberately omitted (Option<PathBuf>; mcp does not require it)
            // --boundary deliberately omitted (defaults to ["personal"])
            "serve",
        ])
        .expect("mcp-serve flat path should parse without phi4 / boundary");
        match cli.command {
            Command::Mcp {
                bge_model,
                bge_tokenizer,
                ort_lib,
                phi4_model,
                rerank_model,
                rerank_tokenizer,
                models_dir,
                boundary,
                run_at,
                action,
            } => {
                // ADR-101 made these `Option`. `Some` is the assertion that
                // matters: an explicitly-passed path must survive parsing
                // untouched, never be replaced by the installed default.
                assert_eq!(bge_model, Some(PathBuf::from("/tmp/bge.onnx")));
                assert_eq!(bge_tokenizer, Some(PathBuf::from("/tmp/tokenizer.json")));
                assert_eq!(ort_lib, Some(PathBuf::from("/tmp/libonnxruntime.so")));
                assert!(
                    run_at.is_none(),
                    "mcp-serve without --run-at MUST yield None (defaults to 03:00 at start_with_mcp)"
                );
                assert!(
                    phi4_model.is_none() || std::env::var("VAULT_PHI4_MODEL_PATH").is_ok(),
                    "mcp-serve without --phi4-model MUST yield None unless env var supplies it"
                );
                assert!(
                    rerank_model.is_none() || std::env::var("VAULT_RERANK_MODEL_PATH").is_ok(),
                    "mcp-serve without --rerank-model MUST yield None unless env var supplies it"
                );
                assert!(
                    rerank_tokenizer.is_none()
                        || std::env::var("VAULT_RERANK_TOKENIZER_PATH").is_ok(),
                    "mcp-serve without --rerank-tokenizer MUST yield None unless env var supplies it"
                );
                // ADR-100. `None` here is the load-bearing default: it is what
                // keeps every pre-existing invocation (explicit model paths, no
                // models dir) on exactly its old behaviour, with no acquisition
                // attempted. Self-sufficiency is opt-in, never assumed.
                assert!(
                    models_dir.is_none() || std::env::var("VAULT_MODELS_DIR").is_ok(),
                    "mcp-serve without --models-dir MUST yield None unless env var supplies it"
                );
                assert_eq!(
                    boundary,
                    vec!["personal".to_string()],
                    "default --boundary list MUST be exactly ['personal']"
                );
                assert!(
                    matches!(action, McpAction::Serve),
                    "expected McpAction::Serve; got {action:?}"
                );
            }
            other => panic!("expected Command::Mcp, got: {other:?}"),
        }
    }

    #[test]
    fn cli_parses_mcp_serve_with_multiple_boundaries_and_phi4() {
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "mcp",
            "--bge-model",
            "/tmp/bge.onnx",
            "--bge-tokenizer",
            "/tmp/tokenizer.json",
            "--ort-lib",
            "/tmp/libonnxruntime.so",
            "--phi4-model",
            "/tmp/phi-4-mini.gguf",
            "--boundary",
            "personal",
            "--boundary",
            "work",
            "--boundary",
            "family",
            "serve",
        ])
        .expect("mcp-serve flat path should parse with all opts supplied");
        match cli.command {
            Command::Mcp {
                phi4_model,
                boundary,
                action,
                ..
            } => {
                assert_eq!(phi4_model, Some(PathBuf::from("/tmp/phi-4-mini.gguf")));
                assert_eq!(
                    boundary,
                    vec![
                        "personal".to_string(),
                        "work".to_string(),
                        "family".to_string(),
                    ],
                    "--boundary MUST be repeatable preserving caller order"
                );
                assert!(matches!(action, McpAction::Serve));
            }
            other => panic!("expected Command::Mcp, got: {other:?}"),
        }
    }

    #[test]
    fn cli_parses_the_bare_connect_snippet_invocation() {
        // THE REGRESSION GUARD FOR THE WHOLE ADR-101 BUG, and the inverse of
        // the `cli_rejects_mcp_serve_with_missing_bge_model` test it replaces.
        //
        // This argument vector is EXACTLY what the connect snippet in
        // dist/app.js tells every user to run: `zaaheen mcp serve`, nothing
        // else. Before ADR-101 it exited with "the following required
        // arguments were not provided", so anyone following the app's own
        // instructions got a dead connection on the one step the product
        // exists for. If this test ever fails again, the snippet is broken
        // again.
        let cli = Cli::try_parse_from(["zaaheen", "mcp", "serve"])
            .expect("the bare connect-snippet invocation MUST parse with no arguments at all");

        assert!(
            cli.vault_db.is_none() && cli.vector_dir.is_none() && cli.graph_db.is_none(),
            "storage paths must be None at parse time; resolve_storage_paths owns the defaults"
        );

        match cli.command {
            Command::Mcp {
                bge_model,
                bge_tokenizer,
                ort_lib,
                action,
                ..
            } => {
                // `|| env::var(..).is_ok()` because these carry clap `env`
                // fallbacks: a developer with VAULT_* exported in their shell
                // would otherwise see a spurious failure here.
                assert!(
                    bge_model.is_none() || std::env::var("VAULT_BGE_MODEL_PATH").is_ok(),
                    "embedder path must stay None at parse time unless the env var supplies it"
                );
                assert!(
                    bge_tokenizer.is_none() || std::env::var("VAULT_BGE_TOKENIZER_PATH").is_ok(),
                    "tokenizer path must stay None at parse time unless the env var supplies it"
                );
                assert!(
                    ort_lib.is_none() || std::env::var("VAULT_ORT_LIB_PATH").is_ok(),
                    "runtime path must stay None at parse time unless the env var supplies it"
                );
                assert!(matches!(action, McpAction::Serve));
            }
            other => panic!("expected Command::Mcp, got: {other:?}"),
        }
    }

    // --------------------------------------------------------------
    // Subcommand behaviour against a real backend
    // --------------------------------------------------------------

    #[tokio::test]
    async fn list_returns_empty_message_on_clean_vault() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        // Should not error.
        list_dead_letters(&backend, 100).await.unwrap();
    }

    #[tokio::test]
    async fn list_shows_planted_row() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": [], "boundary": "work"});
        let _id = plant_dead_letter(
            &backend,
            mem,
            CascadeOperation::Write,
            "simulated lance io",
            payload,
        )
        .await;
        // Just verify the call doesn't error — output goes to stdout.
        list_dead_letters(&backend, 100).await.unwrap();
    }

    #[tokio::test]
    async fn inspect_returns_not_found_for_unknown_id() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let unknown = Uuid::now_v7().to_string();
        let err = inspect_dead_letter(&backend, &unknown).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn inspect_returns_invalid_uuid() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let err = inspect_dead_letter(&backend, "not-a-uuid")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid uuid"));
    }

    #[tokio::test]
    async fn acknowledge_marks_row_acknowledged() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": [], "boundary": "work"});
        let id = plant_dead_letter(&backend, mem, CascadeOperation::Write, "stuck", payload).await;

        acknowledge_dead_letter(&backend, &id.to_string(), "operator accepts loss")
            .await
            .unwrap();

        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::Acknowledged));
    }

    #[tokio::test]
    async fn acknowledge_rejects_empty_reason() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": [], "boundary": "work"});
        let id = plant_dead_letter(&backend, mem, CascadeOperation::Write, "stuck", payload).await;
        let err = acknowledge_dead_letter(&backend, &id.to_string(), "   ")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--reason must not be empty"));
    }

    #[tokio::test]
    async fn retry_succeeds_on_clean_lance_and_marks_retried_succeeded() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // Plant a Write dead-letter with a real payload that the vector
        // store can ingest cleanly.
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": embedding(0.1), "boundary": "work"});
        let id = plant_dead_letter(
            &backend,
            mem,
            CascadeOperation::Write,
            "transient io",
            payload,
        )
        .await;

        retry_dead_letter(&backend, &id.to_string()).await.unwrap();
        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::RetriedSucceeded));

        // Vector store now has the embedding.
        let b = Boundary::new("work").unwrap();
        let hits = backend
            .vector_store()
            .search(&embedding(0.1), 10, &[b])
            .await
            .unwrap();
        assert!(hits.iter().any(|(hid, _)| hid == &mem));
    }

    #[tokio::test]
    async fn retry_dimension_mismatch_marks_retried_failed() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        // Plant a Write dead-letter with the wrong embedding dimension —
        // retrying will fail the vector_store.upsert.
        let mem = MemoryId::new();
        let payload = serde_json::json!({
            "embedding": vec![0.1f32, 0.2, 0.3],  // dim 3, not DIM=4
            "boundary": "work",
        });
        let id =
            plant_dead_letter(&backend, mem, CascadeOperation::Write, "wrong dim", payload).await;

        let err = retry_dead_letter(&backend, &id.to_string())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("retry failed"));
        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::RetriedFailed));
    }

    #[tokio::test]
    async fn retry_on_already_resolved_row_is_noop() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": embedding(0.1), "boundary": "work"});
        let id = plant_dead_letter(&backend, mem, CascadeOperation::Write, "stuck", payload).await;
        backend
            .dead_letter()
            .resolve(id, Resolution::Acknowledged)
            .await
            .unwrap();

        // Now a "retry" call should print a no-op message and NOT error.
        retry_dead_letter(&backend, &id.to_string()).await.unwrap();
        // Resolution unchanged.
        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::Acknowledged));
    }

    // --------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------

    #[test]
    fn summarize_truncates_long_lines() {
        let long = "a".repeat(200);
        let s = summarize(&long, 60);
        assert_eq!(s.chars().count(), 60);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summarize_keeps_short_lines() {
        let s = summarize("short", 60);
        assert_eq!(s, "short");
    }

    #[test]
    fn summarize_takes_first_line_only() {
        let multi = "first line\nsecond line";
        assert_eq!(summarize(multi, 60), "first line");
    }

    // --------------------------------------------------------------
    // divergence-check end-to-end
    // --------------------------------------------------------------

    #[tokio::test]
    async fn divergence_check_on_clean_vault_succeeds() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        // No memories → no findings → Ok return.
        run_divergence_check(&backend).await.unwrap();
    }

    #[tokio::test]
    async fn divergence_check_returns_err_when_findings_present() {
        use vault_storage::StepResult;
        use vault_storage::{FixedJitter, RetryWorker};

        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // Plant a memory + drain the cascade so SQLite + LanceDB are in sync.
        let m = vault_core::Memory::try_new(vault_core::NewMemory {
            content: "doomed".into(),
            memory_type: vault_core::MemoryType::Semantic,
            boundary: Boundary::new("work").unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .unwrap();
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let mut w = RetryWorker::with_jitter(backend.clone(), Box::new(FixedJitter(0.0)));
        let far_future = chrono::Utc::now() + chrono::Duration::seconds(60 * 60);
        loop {
            let r = w.step_at(far_future).await.unwrap();
            if r == StepResult::Idle {
                break;
            }
        }

        // Silently drop the vector row → divergence finding.
        backend.vector_store().delete(&m.id).await.unwrap();

        // run_divergence_check should return Err so scripts notice.
        let err = run_divergence_check(&backend).await.unwrap_err();
        assert!(
            err.to_string().contains("divergence findings present"),
            "expected findings error, got: {err}"
        );
    }

    // --------------------------------------------------------------
    // Sub-task (a) sealed-open coverage (keychain-aware open_backend)
    //
    // Per HANDOFF.md "T0.2.0 close-out plan iteration 4" §3 + §9 (a)
    // floor pre-declaration: +2 firm (sealed-open success + keychain-
    // missing fail-closed). Optional wrong-at-rest-key test subsumed
    // by sub-task (f) BRD §6 T0.2.0 acceptance suite criterion (c).
    //
    // Both tests `#[cfg(windows)]` — mirrors vault_app::keychain's
    // Windows-only V0.2 Phase 1 scope (cross-platform per-platform
    // keychain crates land at T0.2.0.x sub-task per ADR-040 OQ #1).
    // --------------------------------------------------------------

    // `clippy::await_holding_lock` fires because `keychain_test_guard()`
    // returns a `std::sync::MutexGuard` held across `.await` points
    // (`open_with_at_rest_key`, `open_backend_inner`). The mutex serializes
    // process-global `keyring_core::set_default_store` /
    // `unset_default_store` state across tests — releasing before awaits
    // would defeat the serialization invariant the mutex exists to enforce.
    // Safe here because: (a) `std::sync::Mutex` is sync (no runtime yield);
    // (b) no other tokio task contends for KEYCHAIN_TEST_MUTEX; (c) async
    // calls inside don't try to reacquire it. Production has no contention
    // (vault-tauri + vault-cli each call keychain helpers once at startup).
    /// The three storage paths out of a `Cli` that was parsed with all of them
    /// supplied.
    ///
    /// ADR-101 made them `Option` so an installed `zaaheen` can fill them in.
    /// The callers below always pass them on the command line, so `expect`
    /// documents that precondition rather than papering over a real `None`.
    ///
    /// `#[cfg(windows)]` because both callers are — without it, the other CI
    /// legs see an unused function and `-D warnings` turns that into a failed
    /// build on Linux and macOS.
    #[cfg(windows)]
    fn explicit_storage_paths(cli: &Cli) -> (&Path, &Path, &Path) {
        (
            cli.vault_db
                .as_deref()
                .expect("this test passes --vault-db explicitly"),
            cli.vector_dir
                .as_deref()
                .expect("this test passes --vector-dir explicitly"),
            cli.graph_db
                .as_deref()
                .expect("this test passes --graph-db explicitly"),
        )
    }

    #[tokio::test]
    #[cfg(windows)]
    #[allow(clippy::await_holding_lock)]
    async fn open_backend_succeeds_with_keychain_initialized_vault() {
        let _guard = keychain_test_guard();
        let namespace = unique_test_namespace("open_backend_success");
        let vault_id = "test-open-backend-success";
        cleanup_keychain_entry(&namespace, vault_id);

        // Bootstrap a real keychain entry via first-run path; derive the same
        // subkeys that open_backend_inner will derive when it re-reads the
        // entry (deterministic per ADR-040 amendment v2 derivation tree).
        let master_key = read_or_init_master_key(&namespace, vault_id)
            .expect("first-run should generate + persist master_key");
        let sqlcipher_passphrase = derive_sqlcipher_passphrase(&master_key);
        let at_rest_key = derive_at_rest_key(&master_key);

        let tmp = TempDir::new().unwrap();
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            tmp.path().join("vault.db").to_str().unwrap(),
            "--vector-dir",
            tmp.path().join("lance").to_str().unwrap(),
            "--graph-db",
            tmp.path().join("graph.duckdb").to_str().unwrap(),
            "--dimension",
            &DIM.to_string(),
            "divergence-check",
        ])
        .expect("Cli::try_parse_from for success test");
        let (vault_db, vector_dir, graph_db) = explicit_storage_paths(&cli);

        // Create the sealed vault first so open_backend_inner has a vault to
        // re-open. Drop it explicitly so file handles close before the
        // open_backend_inner call re-opens via its own internal opens.
        {
            let _initial = StorageBackend::open_with_at_rest_key(
                vault_db,
                vector_dir,
                graph_db,
                sqlcipher_passphrase.clone(),
                DIM,
                &at_rest_key,
            )
            .await
            .expect("initial open_with_at_rest_key should succeed");
        }

        let result = open_backend_inner(
            vault_db,
            vector_dir,
            graph_db,
            cli.dimension,
            &namespace,
            vault_id,
        )
        .await;
        assert!(
            result.is_ok(),
            "open_backend_inner should succeed with valid keychain entry + sealed vault; got: {:?}",
            result.err()
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }

    // Same `clippy::await_holding_lock` allow as the success test above —
    // see that test's preceding comment block for the full justification.
    #[tokio::test]
    #[cfg(windows)]
    #[allow(clippy::await_holding_lock)]
    async fn open_backend_fails_closed_with_generic_message_on_keychain_error() {
        let _guard = keychain_test_guard();
        let namespace = unique_test_namespace("open_backend_fail_closed");
        let vault_id = "test-open-backend-fail";
        cleanup_keychain_entry(&namespace, vault_id);

        // Plant a 31-byte (malformed) keychain entry. read_or_init_master_key
        // detects the wrong-length secret and returns
        // VaultError::KeychainProvenance("...exists but secret is 31 bytes...").
        // open_backend_inner's map_err converts this to a generic
        // "authentication failed" with no info leak per BRD §11.7.2.
        plant_malformed_keychain_entry(&namespace, vault_id);

        let tmp = TempDir::new().unwrap();
        let cli = Cli::try_parse_from([
            "zaaheen",
            "--vault-db",
            tmp.path().join("vault.db").to_str().unwrap(),
            "--vector-dir",
            tmp.path().join("lance").to_str().unwrap(),
            "--graph-db",
            tmp.path().join("graph.duckdb").to_str().unwrap(),
            "--dimension",
            &DIM.to_string(),
            "divergence-check",
        ])
        .expect("Cli::try_parse_from for fail-closed test");
        let (vault_db, vector_dir, graph_db) = explicit_storage_paths(&cli);

        let result = open_backend_inner(
            vault_db,
            vector_dir,
            graph_db,
            cli.dimension,
            &namespace,
            vault_id,
        )
        .await;
        // Cannot use `Result::expect_err` here because `StorageBackend` does
        // not implement `Debug` by design (BRD §11 secrets-in-logs / ADR-007
        // redaction posture). Explicit match yields the `anyhow::Error` for
        // message inspection without requiring Debug on the Ok variant.
        let err = match result {
            Ok(_) => panic!("expected fail-closed on malformed keychain entry; got Ok"),
            Err(e) => e,
        };
        let msg = format!("{}", err);
        assert_eq!(
            msg, "authentication failed",
            "BRD §11.7.2 demands a generic error message with no info leak; \
             leaking the underlying KeychainProvenance details would tell an \
             attacker which check failed. Got: {msg}"
        );

        cleanup_keychain_entry(&namespace, vault_id);
    }
}
