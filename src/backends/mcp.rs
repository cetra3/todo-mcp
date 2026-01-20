use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, Json, ServiceExt,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc::Sender;
use tracing::debug;

use crate::backends::multicast::{self, TodoEvent, TodoCommand, TodoState};

// Parameter structs for MCP tools
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct AddListParams {
    pub title: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct RemoveListParams {
    pub list_index: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct RenameListParams {
    pub list_index: u32,
    pub title: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct AddTodoParams {
    pub list_index: u32,
    pub text: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct ToggleTodoParams {
    pub list_index: u32,
    pub item_index: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct RemoveTodoParams {
    pub list_index: u32,
    pub item_index: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct ClearCompletedParams {
    pub list_index: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct GetListParams {
    pub list_index: Option<u32>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct NameSessionParams {
    /// The session_id from the todo-mcp hook output
    pub session_id: String,
    /// Short descriptive name (2-5 words). "Claude: " prefix is added automatically.
    pub name: String,
}

// Response types
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct TodoListsResponse {
    pub lists: Vec<TodoListResponse>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct TodoListResponse {
    pub index: u32,
    pub title: String,
    pub items: Vec<TodoItemResponse>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, schemars::JsonSchema)]
pub struct TodoItemResponse {
    pub index: u32,
    pub text: String,
    pub completed: bool,
}

pub struct TodoMcp {
    todo_state: Arc<RwLock<TodoState>>,
    tx: Sender<TodoCommand>,
    tool_router: ToolRouter<Self>,
}

impl From<&TodoState> for TodoListsResponse {
    fn from(value: &TodoState) -> Self {
        TodoListsResponse {
            lists: value
                .lists
                .iter()
                .enumerate()
                .map(|(list_index, list)| TodoListResponse {
                    index: list_index as u32,
                    title: list.title.clone(),
                    items: list
                        .items
                        .iter()
                        .enumerate()
                        .map(|(item_index, item)| TodoItemResponse {
                            index: item_index as u32,
                            text: item.text.clone(),
                            completed: item.completed,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

pub async fn run_mcp() -> anyhow::Result<()> {
    let todo_mcp = TodoMcp::new().serve(stdio()).await?;

    todo_mcp.waiting().await?;

    Ok(())
}

#[tool_router]
impl TodoMcp {
    pub fn new() -> Self {
        let todo_state = Arc::new(RwLock::new(TodoState::default()));

        let bg_state = todo_state.clone();
        let site_id = rand::random();

        let (tx, mut recv) = multicast::setup(site_id);

        tokio::spawn(async move {
            while let Some(change) = recv.recv().await {
                if let TodoEvent::StateUpdate(new_state) = change {
                    debug!("New update received");
                    let mut cur_state = bg_state.write().unwrap();
                    *cur_state = new_state;
                }
            }
        });

        Self {
            todo_state,
            tx,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Get all todo lists, or a specific list by index")]
    async fn get_todos(
        &self,
        Parameters(params): Parameters<GetListParams>,
    ) -> Result<Json<TodoListsResponse>, McpError> {
        let state = self.todo_state.read().unwrap();
        let response: TodoListsResponse = (&*state).into();

        // If a specific list is requested, filter to just that one
        if let Some(list_index) = params.list_index {
            let filtered = TodoListsResponse {
                lists: response
                    .lists
                    .into_iter()
                    .filter(|l| l.index == list_index)
                    .collect(),
            };
            return Ok(Json(filtered));
        }

        Ok(Json(response))
    }

    #[tool(description = "Create a new todo list with the given title")]
    async fn add_list(
        &self,
        Parameters(params): Parameters<AddListParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            state
                .lists
                .push(multicast::TodoList::new(params.title.clone()));
        }

        self.tx
            .send(TodoCommand::AddList {
                title: params.title,
                metadata: HashMap::new(),
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(description = "Remove a todo list by index")]
    async fn remove_list(
        &self,
        Parameters(params): Parameters<RemoveListParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            let list_index = params.list_index as usize;
            if list_index < state.lists.len() {
                state.lists.remove(list_index);
            }
        }

        self.tx
            .send(TodoCommand::RemoveList {
                list_index: params.list_index as usize,
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(description = "Rename a todo list")]
    async fn rename_list(
        &self,
        Parameters(params): Parameters<RenameListParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            let list_index = params.list_index as usize;
            if list_index < state.lists.len() {
                state.lists[list_index].title = params.title.clone();
            }
        }

        self.tx
            .send(TodoCommand::RenameList {
                list_index: params.list_index as usize,
                title: params.title,
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(description = "Add a new todo item to a specific list")]
    async fn add_todo(
        &self,
        Parameters(params): Parameters<AddTodoParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            let list_index = params.list_index as usize;
            if list_index < state.lists.len() {
                state.lists[list_index].items.push(multicast::TodoItem {
                    text: params.text.clone(),
                    completed: false,
                    metadata: HashMap::new(),
                });
            }
        }

        self.tx
            .send(TodoCommand::AddTodo {
                list_index: params.list_index as usize,
                text: params.text,
                metadata: HashMap::new(),
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(description = "Toggle a todo item as either completed or incomplete")]
    async fn toggle_todo(
        &self,
        Parameters(params): Parameters<ToggleTodoParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            let list_index = params.list_index as usize;
            let item_index = params.item_index as usize;
            if list_index < state.lists.len() {
                let list = &mut state.lists[list_index];
                if item_index < list.items.len() {
                    list.items[item_index].completed = !list.items[item_index].completed;
                }
            }
        }

        self.tx
            .send(TodoCommand::ToggleTodo {
                list_index: params.list_index as usize,
                item_index: params.item_index as usize,
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(description = "Remove a specific todo item from a list")]
    async fn remove_todo(
        &self,
        Parameters(params): Parameters<RemoveTodoParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            let list_index = params.list_index as usize;
            let item_index = params.item_index as usize;
            if list_index < state.lists.len() {
                let list = &mut state.lists[list_index];
                if item_index < list.items.len() {
                    list.items.remove(item_index);
                }
            }
        }

        self.tx
            .send(TodoCommand::RemoveTodo {
                list_index: params.list_index as usize,
                item_index: params.item_index as usize,
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(description = "Remove all completed todo items from a specific list")]
    async fn clear_completed(
        &self,
        Parameters(params): Parameters<ClearCompletedParams>,
    ) -> Result<CallToolResult, McpError> {
        {
            let mut state = self.todo_state.write().unwrap();
            let list_index = params.list_index as usize;
            if list_index < state.lists.len() {
                state.lists[list_index].items.retain(|item| !item.completed);
            }
        }

        self.tx
            .send(TodoCommand::ClearCompleted {
                list_index: params.list_index as usize,
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![]))
    }

    #[tool(
        description = "Rename a session's todo list by session_id. Use this after creating tasks to give the list a descriptive name."
    )]
    async fn name_session(
        &self,
        Parameters(params): Parameters<NameSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        let list_index = {
            let state = self.todo_state.read().unwrap();
            state.lists.iter().position(|l| {
                l.metadata
                    .get("session_id")
                    .map_or(false, |s| s == &params.session_id)
            })
        };

        let Some(list_index) = list_index else {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No list found for session_id: {}",
                params.session_id
            ))]));
        };

        let new_title = format!("Claude: {}", params.name);

        {
            let mut state = self.todo_state.write().unwrap();
            if list_index < state.lists.len() {
                state.lists[list_index].title = new_title.clone();
            }
        }

        self.tx
            .send(TodoCommand::RenameList {
                list_index,
                title: new_title.clone(),
            })
            .await
            .expect("always sends");

        Ok(CallToolResult::success(vec![Content::text(format!(
            "List renamed to \"{}\"",
            new_title
        ))]))
    }
}

// Implement the server handler
#[tool_handler]
impl rmcp::ServerHandler for TodoMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Manages multiple todo lists with items. Supports creating lists, adding/toggling/removing items, and syncing state across devices. Use name_session to rename a session's list by session_id after creating tasks.".into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
