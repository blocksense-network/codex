//! Persist Codex session rollouts (.jsonl) so sessions can be replayed or inspected later.

use std::fs::File;
use std::fs::{self};
use std::io::BufWriter;
use std::io::Error as IoError;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;

use codex_protocol::mcp_protocol::ConversationId;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;
use tracing::error;
use tracing::info;
use tracing::warn;

use super::SESSIONS_SUBDIR;
use super::list::ConversationsPage;
use super::list::Cursor;
use super::list::get_conversations;
use super::policy::is_persisted_response_item;
use crate::config::Config;
use crate::default_client::ORIGINATOR;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::ResumedHistory;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SessionStateSnapshot {}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SavedSession {
    pub session: SessionMeta,
    #[serde(default)]
    pub items: Vec<ResponseItem>,
    #[serde(default)]
    pub state: SessionStateSnapshot,
    pub session_id: ConversationId,
}

/// Records all [`ResponseItem`]s for a session and flushes them to disk after
/// every update.
///
/// Rollouts are recorded as JSONL and can be inspected with tools such as:
///
/// ```ignore
/// $ jq -C . ~/.codex/sessions/rollout-2025-05-07T17-24-21-5973b6c0-94b8-487b-a530-2aeb6098ae0e.jsonl
/// $ fx ~/.codex/sessions/rollout-2025-05-07T17-24-21-5973b6c0-94b8-487b-a530-2aeb6098ae0e.jsonl
/// ```
#[derive(Clone)]
pub struct RolloutRecorder {
    writer: Arc<Mutex<BufWriter<File>>>,
    pub(crate) rollout_path: PathBuf,
    hook_command: Option<Vec<String>>,
}

#[derive(Clone)]
pub enum RolloutRecorderParams {
    Create {
        conversation_id: ConversationId,
        instructions: Option<String>,
    },
    Resume {
        path: PathBuf,
    },
}

impl RolloutRecorderParams {
    pub fn new(conversation_id: ConversationId, instructions: Option<String>) -> Self {
        Self::Create {
            conversation_id,
            instructions,
        }
    }

    pub fn resume(path: PathBuf) -> Self {
        Self::Resume { path }
    }
}

impl RolloutRecorder {
    /// List conversations (rollout files) under the provided Codex home directory.
    pub async fn list_conversations(
        codex_home: &Path,
        page_size: usize,
        cursor: Option<&Cursor>,
    ) -> std::io::Result<ConversationsPage> {
        get_conversations(codex_home, page_size, cursor).await
    }

    /// Attempt to create a new [`RolloutRecorder`]. If the sessions directory
    /// cannot be created or the rollout file cannot be opened we return the
    /// error so the caller can decide whether to disable persistence.
    pub fn new(config: &Config, params: RolloutRecorderParams) -> std::io::Result<Self> {
        let (file, rollout_path, meta) = match params {
            RolloutRecorderParams::Create {
                conversation_id,
                instructions,
            } => {
                let LogFileInfo {
                    file,
                    path,
                    conversation_id: session_id,
                    timestamp,
                } = create_log_file(config, conversation_id)?;

                let timestamp_format: &[FormatItem] = format_description!(
                    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
                );
                let timestamp = timestamp
                    .to_offset(time::UtcOffset::UTC)
                    .format(timestamp_format)
                    .map_err(|e| IoError::other(format!("failed to format timestamp: {e}")))?;

                (
                    file,
                    path,
                    Some(SessionMeta {
                        id: session_id,
                        timestamp,
                        cwd: config.cwd.clone(),
                        originator: ORIGINATOR.value.clone(),
                        cli_version: env!("CARGO_PKG_VERSION").to_string(),
                        instructions,
                    }),
                )
            }
            RolloutRecorderParams::Resume { path } => (
                std::fs::OpenOptions::new().append(true).open(&path)?,
                path,
                None,
            ),
        };

        let writer = Arc::new(Mutex::new(BufWriter::new(file)));

        // If we have a meta, write it synchronously
        if let Some(session_meta) = meta {
            let git_info = std::thread::spawn(move || {
                // We need to call collect_git_info synchronously since we're not in an async context
                // For now, we'll skip git info collection in the synchronous version
                // TODO: Make git info collection synchronous or handle it differently
                None
            })
            .join()
            .unwrap_or(None);

            let session_meta_line = SessionMetaLine {
                meta: session_meta,
                git: git_info,
            };

            let mut guard = writer.lock().unwrap();
            Self::write_rollout_item_sync(
                &mut *guard,
                RolloutItem::SessionMeta(session_meta_line),
            )?;
            guard.flush()?;
        }

        Ok(Self {
            writer,
            rollout_path,
            hook_command: config.rollout_entry_hook.clone(),
        })
    }

    pub fn record_items(&self, items: &[RolloutItem]) -> std::io::Result<()> {
        let mut filtered = Vec::new();
        for item in items {
            // Note that function calls may look a bit strange if they are
            // "fully qualified MCP tool calls," so we could consider
            // reformatting them in that case.
            if is_persisted_response_item(item) {
                filtered.push(item.clone());
            }
        }
        if filtered.is_empty() {
            return Ok(());
        }

        let mut guard = self.writer.lock().unwrap();
        for item in filtered {
            Self::write_rollout_item_sync(&mut *guard, item.clone())?;
            guard.flush()?;

            // Execute hook after each item is written
            if let Some(hook_cmd) = &self.hook_command {
                Self::execute_hook(hook_cmd, &item)?;
            }
        }
        Ok(())
    }

    /// Flush all buffered writes to disk.
    pub fn flush(&self) -> std::io::Result<()> {
        let mut guard = self.writer.lock().unwrap();
        guard.flush()
    }

    fn write_rollout_item_sync<W: Write>(
        writer: &mut W,
        rollout_item: RolloutItem,
    ) -> std::io::Result<()> {
        let timestamp_format: &[FormatItem] = format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        );
        let timestamp = OffsetDateTime::now_utc()
            .format(timestamp_format)
            .map_err(|e| IoError::other(format!("failed to format timestamp: {e}")))?;

        let line = RolloutLine {
            timestamp,
            item: rollout_item,
        };
        Self::write_line_sync(writer, &line)
    }

    fn write_line_sync<W: Write>(
        writer: &mut W,
        item: &impl serde::Serialize,
    ) -> std::io::Result<()> {
        let mut json = serde_json::to_string(item)?;
        json.push('\n');
        writer.write_all(json.as_bytes())?;
        Ok(())
    }

    fn execute_hook(hook_cmd: &[String], item: &RolloutItem) -> std::io::Result<()> {
        if hook_cmd.is_empty() {
            return Ok(());
        }

        let json = serde_json::to_string(item)
            .map_err(|e| IoError::other(format!("failed to serialize item for hook: {e}")))?;

        let mut cmd = Command::new(&hook_cmd[0]);
        cmd.args(&hook_cmd[1..]);
        cmd.arg(json);

        match cmd.status() {
            Ok(status) => {
                if status.success() {
                    Ok(())
                } else {
                    error!("Hook command failed with exit code: {:?}", status.code());
                    // Don't fail the rollout recording if hook fails
                    Ok(())
                }
            }
            Err(e) => {
                error!("Failed to execute hook command: {e}");
                // Don't fail the rollout recording if hook execution fails
                Ok(())
            }
        }
    }

    pub async fn get_rollout_history(path: &Path) -> std::io::Result<InitialHistory> {
        info!("Resuming rollout from {path:?}");
        let text = tokio::fs::read_to_string(path).await?;
        if text.trim().is_empty() {
            return Err(IoError::other("empty session file"));
        }

        let mut items: Vec<RolloutItem> = Vec::new();
        let mut conversation_id: Option<ConversationId> = None;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    warn!("failed to parse line as JSON: {line:?}, error: {e}");
                    continue;
                }
            };

            // Parse the rollout line structure
            match serde_json::from_value::<RolloutLine>(v.clone()) {
                Ok(rollout_line) => match rollout_line.item {
                    RolloutItem::SessionMeta(session_meta_line) => {
                        // Use the FIRST SessionMeta encountered in the file as the canonical
                        // conversation id and main session information. Keep all items intact.
                        if conversation_id.is_none() {
                            conversation_id = Some(session_meta_line.meta.id);
                        }
                        items.push(RolloutItem::SessionMeta(session_meta_line));
                    }
                    RolloutItem::ResponseItem(item) => {
                        items.push(RolloutItem::ResponseItem(item));
                    }
                    RolloutItem::Compacted(item) => {
                        items.push(RolloutItem::Compacted(item));
                    }
                    RolloutItem::TurnContext(item) => {
                        items.push(RolloutItem::TurnContext(item));
                    }
                    RolloutItem::EventMsg(_ev) => {
                        items.push(RolloutItem::EventMsg(_ev));
                    }
                },
                Err(e) => {
                    warn!("failed to parse rollout line: {v:?}, error: {e}");
                }
            }
        }

        info!(
            "Resumed rollout with {} items, conversation ID: {:?}",
            items.len(),
            conversation_id
        );
        let conversation_id = conversation_id
            .ok_or_else(|| IoError::other("failed to parse conversation ID from rollout file"))?;

        if items.is_empty() {
            return Ok(InitialHistory::New);
        }

        info!("Resumed rollout successfully from {path:?}");
        Ok(InitialHistory::Resumed(ResumedHistory {
            conversation_id,
            history: items,
            rollout_path: path.to_path_buf(),
        }))
    }

    pub(crate) fn get_rollout_path(&self) -> PathBuf {
        self.rollout_path.clone()
    }

    pub fn shutdown(&self) -> std::io::Result<()> {
        // Flush any remaining buffered data
        self.flush()
    }
}

struct LogFileInfo {
    /// Opened file handle to the rollout file.
    file: File,

    /// Full path to the rollout file.
    path: PathBuf,

    /// Session ID (also embedded in filename).
    conversation_id: ConversationId,

    /// Timestamp for the start of the session.
    timestamp: OffsetDateTime,
}

fn create_log_file(
    config: &Config,
    conversation_id: ConversationId,
) -> std::io::Result<LogFileInfo> {
    // Resolve ~/.codex/sessions/YYYY/MM/DD and create it if missing.
    let timestamp = OffsetDateTime::now_local()
        .map_err(|e| IoError::other(format!("failed to get local time: {e}")))?;
    let mut dir = config.codex_home.clone();
    dir.push(SESSIONS_SUBDIR);
    dir.push(timestamp.year().to_string());
    dir.push(format!("{:02}", u8::from(timestamp.month())));
    dir.push(format!("{:02}", timestamp.day()));
    fs::create_dir_all(&dir)?;

    // Custom format for YYYY-MM-DDThh-mm-ss. Use `-` instead of `:` for
    // compatibility with filesystems that do not allow colons in filenames.
    let format: &[FormatItem] =
        format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]");
    let date_str = timestamp
        .format(format)
        .map_err(|e| IoError::other(format!("failed to format timestamp: {e}")))?;

    let filename = format!("rollout-{date_str}-{conversation_id}.jsonl");

    let path = dir.join(filename);
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;

    Ok(LogFileInfo {
        file,
        path,
        conversation_id,
        timestamp,
    })
}
