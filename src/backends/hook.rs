use std::collections::HashMap;

use futures::StreamExt;
use tokio::io;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{debug, warn};

use crate::backends::{
    multicast::{self, TodoEvent, TodoItem},
    TodoCommand, TodoList, TodoState,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "tool_name", content = "tool_input")]
pub enum ToolPayload {
    #[serde(rename = "TaskCreate")]
    Create { subject: String },

    #[serde(rename = "TaskUpdate")]
    Update {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        subject: Option<String>,
    },

    #[serde(untagged)]
    Unknown {
        tool_name: String,
        tool_input: Value,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaudeHook {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tool_response: Option<Value>,

    #[serde(flatten)]
    pub payload: ToolPayload,
}

/// Derive the list name from the hook's cwd field
fn list_name_from_hook(hook: &ClaudeHook) -> String {
    hook.cwd
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .map(|s| format!("Claude: {s}"))
        .unwrap_or_else(|| "Claude Tasks".to_string())
}

/// Extract the taskId from tool_response JSON (returned by TaskCreate)
fn task_id_from_response(response: &Option<Value>) -> Option<String> {
    response
        .as_ref()
        .and_then(|v| v.get("taskId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Read the Claude Code todo file for a session and get the subject at a given task_id (1-based).
/// Files live at ~/.claude/todos/{session_id}-agent-{session_id}.json
fn read_claude_todo_subject(session_id: &str, task_id: &str) -> Option<String> {
    let task_idx: usize = task_id.parse::<usize>().ok()?.checked_sub(1)?;

    let todos_dir = shellexpand::tilde("~/.claude/todos").to_string();
    let filename = format!("{session_id}-agent-{session_id}.json");
    let path = std::path::Path::new(&todos_dir).join(&filename);

    let content = std::fs::read_to_string(&path).ok()?;
    let tasks: Vec<Value> = serde_json::from_str(&content).ok()?;

    tasks
        .get(task_idx)
        .and_then(|t| t.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}

/// Guess the task_id for a newly created item by counting existing items
/// in the list that belong to the same session. Claude Code assigns
/// sequential 1-based IDs per session.
fn guess_task_id(list: &TodoList, session_id: &str) -> String {
    let count = list
        .items
        .iter()
        .filter(|i| {
            i.metadata
                .get("session_id")
                .map_or(false, |s| s == session_id)
        })
        .count();
    (count + 1).to_string()
}

/// Find a list by session_id in metadata, falling back to name match.
/// Returns the index if found.
fn find_list(todo_state: &TodoState, session_id: &str, list_name: &str) -> Option<usize> {
    todo_state
        .lists
        .iter()
        .position(|l| {
            l.metadata
                .get("session_id")
                .map_or(false, |s| s == session_id)
        })
        .or_else(|| todo_state.lists.iter().position(|l| l.title == list_name))
}

/// Ensure a list exists for this session, returning its index.
/// Looks up by session_id metadata first, then by name. Creates if missing.
async fn ensure_list(
    list_name: &str,
    session_id: &str,
    todo_state: &mut TodoState,
    tx: &tokio::sync::mpsc::Sender<TodoCommand>,
) -> anyhow::Result<usize> {
    if let Some(idx) = find_list(todo_state, session_id, list_name) {
        return Ok(idx);
    }

    let mut metadata = HashMap::new();
    metadata.insert("session_id".into(), session_id.into());

    tx.send(TodoCommand::AddList {
        title: list_name.into(),
        metadata: metadata.clone(),
    })
    .await?;

    todo_state.lists.push(TodoList {
        title: list_name.into(),
        items: vec![],
        metadata,
    });

    Ok(todo_state.lists.len() - 1)
}

/// Send shutdown and wait for save to complete
async fn shutdown(tx: &tokio::sync::mpsc::Sender<TodoCommand>) -> anyhow::Result<()> {
    let (sender, recv) = tokio::sync::oneshot::channel();
    tx.send(TodoCommand::Shutdown { sender }).await?;
    recv.await?;
    Ok(())
}

pub async fn run_hook() -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = FramedRead::new(stdin, LinesCodec::new());
    let line = reader.next().await.transpose()?.unwrap_or_default();

    let Ok(hook) = serde_json::from_str::<ClaudeHook>(&line) else {
        warn!("could not parse claude hook: {line}");
        return Ok(());
    };

    debug!("hook received: {hook:?}");

    if let ToolPayload::Unknown { .. } = &hook.payload {
        debug!("unhandled tool, skipping");
        return Ok(());
    }

    let site_id = rand::random();

    let (tx, mut rx) = multicast::setup(site_id);

    // receive at least one state change
    let mut todo_state = loop {
        if let Some(change_message) = rx.recv().await {
            if let TodoEvent::StateUpdate(state) = change_message {
                break state;
            }
        } else {
            return Ok(());
        }
    };

    let list_name = list_name_from_hook(&hook);

    match hook.payload {
        ToolPayload::Create { subject } => {
            let list_idx = ensure_list(&list_name, &hook.session_id, &mut todo_state, &tx).await?;

            let mut metadata = HashMap::new();
            metadata.insert("session_id".into(), hook.session_id.clone());
            let task_id = task_id_from_response(&hook.tool_response)
                .unwrap_or_else(|| guess_task_id(&todo_state.lists[list_idx], &hook.session_id));
            metadata.insert("task_id".into(), task_id);

            tx.send(TodoCommand::AddTodo {
                list_index: list_idx,
                text: subject.clone(),
                metadata: metadata.clone(),
            })
            .await?;

            todo_state.lists[list_idx].items.push(TodoItem {
                text: subject,
                completed: false,
                metadata,
            });

            shutdown(&tx).await?;
        }
        ToolPayload::Update {
            task_id,
            status,
            subject: _,
        } => {
            // Find the list by session_id metadata or name
            let list_idx = find_list(&todo_state, &hook.session_id, &list_name);

            let Some(list_idx) = list_idx else {
                warn!(
                    "could not find list for session_id={} or name='{list_name}'",
                    hook.session_id
                );
                shutdown(&tx).await?;
                return Ok(());
            };

            // Find the item by task_id in metadata
            let mut item_idx = todo_state.lists[list_idx]
                .items
                .iter()
                .position(|i| i.metadata.get("task_id").map_or(false, |t| t == &task_id));

            // Fallback: read the Claude Code todos file and match by subject
            if item_idx.is_none() {
                if let Some(subject) = read_claude_todo_subject(&hook.session_id, &task_id) {
                    debug!("task_id={task_id} not in metadata, falling back to subject match: {subject}");
                    if let Some(idx) = todo_state.lists[list_idx]
                        .items
                        .iter()
                        .position(|i| i.text == subject)
                    {
                        // Backfill the task_id metadata for future lookups
                        todo_state.lists[list_idx].items[idx]
                            .metadata
                            .insert("task_id".into(), task_id.clone());
                        item_idx = Some(idx);
                    }
                }
            }

            let Some(item_idx) = item_idx else {
                warn!("could not find item with task_id={task_id} in list '{list_name}'");
                shutdown(&tx).await?;
                return Ok(());
            };

            match status.as_deref() {
                Some("completed") => {
                    if !todo_state.lists[list_idx].items[item_idx].completed {
                        tx.send(TodoCommand::ToggleTodo {
                            list_index: list_idx,
                            item_index: item_idx,
                        })
                        .await?;
                    }
                }
                Some("pending") => {
                    if todo_state.lists[list_idx].items[item_idx].completed {
                        tx.send(TodoCommand::ToggleTodo {
                            list_index: list_idx,
                            item_index: item_idx,
                        })
                        .await?;
                    }
                }
                _ => {
                    debug!("no actionable status change for status={status:?}");
                }
            }

            shutdown(&tx).await?;
        }
        ToolPayload::Unknown { .. } => unreachable!(),
    }

    Ok(())
}
