use std::collections::HashMap;

use tokio::sync::mpsc::Sender as TokioSender;

use crate::backends::multicast::{TodoCommand, TodoEvent, TodoItem, TodoList, TodoState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Navigating the list of todo lists
    ListSelector,
    /// Navigating items within an expanded list
    ItemList,
    /// Editing text inline (new list, rename list, new item, edit item)
    Editing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    NewList,
    RenameList { list_index: usize },
    NewItem { list_index: usize },
    EditItem { list_index: usize, item_index: usize },
}

#[derive(Debug, Clone)]
pub struct EditState {
    pub buffer: String,
    pub cursor: usize,
    pub target: EditTarget,
}

impl EditState {
    pub fn new(target: EditTarget, initial: &str) -> Self {
        Self {
            cursor: initial.len(),
            buffer: initial.to_string(),
            target,
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn delete_back(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor -= prev;
            self.buffer.remove(self.cursor);
        }
    }

    pub fn delete_forward(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor -= prev;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            let next = self.buffer[self.cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor += next;
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListUiState {
    pub expanded: bool,
    pub selected_item: usize,
}

pub struct TuiState {
    pub todo_state: TodoState,
    pub selected_list: usize,
    pub list_ui: Vec<ListUiState>,
    pub focus: Focus,
    pub edit: Option<EditState>,
    pub connection_status: String,
    pub command_tx: TokioSender<TodoCommand>,
    pub scroll_offset: u16,
    pub should_quit: bool,
}

impl TuiState {
    pub fn new(command_tx: TokioSender<TodoCommand>) -> Self {
        Self {
            todo_state: TodoState::default(),
            selected_list: 0,
            list_ui: Vec::new(),
            focus: Focus::ListSelector,
            edit: None,
            connection_status: String::new(),
            command_tx,
            scroll_offset: 0,
            should_quit: false,
        }
    }

    pub fn handle_event(&mut self, event: TodoEvent) {
        match event {
            TodoEvent::StateUpdate(state) => {
                // Preserve UI state (expanded, selected_item) across syncs,
                // matching the pattern in components/mod.rs:83-99
                let prev_ui: Vec<ListUiState> = std::mem::take(&mut self.list_ui);
                self.list_ui = state
                    .lists
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        prev_ui.get(i).cloned().unwrap_or(ListUiState {
                            expanded: false,
                            selected_item: 0,
                        })
                    })
                    .collect();
                self.todo_state = state;

                // Clamp selected_list
                if !self.todo_state.lists.is_empty() {
                    self.selected_list = self.selected_list.min(self.todo_state.lists.len() - 1);
                } else {
                    self.selected_list = 0;
                }
            }
            TodoEvent::ConnectionStatus(status) => {
                self.connection_status = status;
            }
        }
    }

    pub fn send_command(&self, cmd: TodoCommand) {
        let tx = self.command_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = tx.send(cmd).await {
                tracing::error!("Failed to send command: {e}");
            }
        });
    }

    pub fn list_count(&self) -> usize {
        self.todo_state.lists.len()
    }

    pub fn selected_list_expanded(&self) -> bool {
        self.list_ui
            .get(self.selected_list)
            .is_some_and(|ui| ui.expanded)
    }

    pub fn selected_item_index(&self) -> usize {
        self.list_ui
            .get(self.selected_list)
            .map(|ui| ui.selected_item)
            .unwrap_or(0)
    }

    pub fn items_in_selected_list(&self) -> usize {
        self.todo_state
            .lists
            .get(self.selected_list)
            .map(|l| l.items.len())
            .unwrap_or(0)
    }

    /// Number of selectable rows inside expanded list: items + "Add new item" row
    pub fn selectable_rows_in_list(&self) -> usize {
        self.items_in_selected_list() + 1
    }

    pub fn expand_selected(&mut self) {
        if let Some(ui) = self.list_ui.get_mut(self.selected_list) {
            ui.expanded = true;
            self.focus = Focus::ItemList;
        }
    }

    pub fn collapse_selected(&mut self) {
        if let Some(ui) = self.list_ui.get_mut(self.selected_list) {
            ui.expanded = false;
            ui.selected_item = 0;
            self.focus = Focus::ListSelector;
        }
    }

    pub fn move_list_up(&mut self) {
        if self.selected_list > 0 {
            self.selected_list -= 1;
        }
    }

    pub fn move_list_down(&mut self) {
        // +1 to allow navigating to the "Add New Todo List" row
        if self.selected_list + 1 <= self.list_count() {
            self.selected_list += 1;
        }
    }

    /// True when the cursor is on the "Add New Todo List" row (past all lists).
    pub fn on_add_list_row(&self) -> bool {
        self.selected_list == self.list_count()
    }

    pub fn move_item_up(&mut self) {
        if let Some(ui) = self.list_ui.get_mut(self.selected_list) {
            if ui.selected_item > 0 {
                ui.selected_item -= 1;
            }
        }
    }

    pub fn move_item_down(&mut self) {
        let max = self.selectable_rows_in_list();
        if let Some(ui) = self.list_ui.get_mut(self.selected_list) {
            if ui.selected_item + 1 < max {
                ui.selected_item += 1;
            }
        }
    }

    pub fn toggle_item(&mut self, list_index: usize, item_index: usize) {
        if let Some(item) = self
            .todo_state
            .lists
            .get_mut(list_index)
            .and_then(|l| l.items.get_mut(item_index))
        {
            item.completed = !item.completed;
        }
        self.send_command(TodoCommand::ToggleTodo {
            list_index,
            item_index,
        });
    }

    pub fn remove_item(&mut self, list_index: usize, item_index: usize) {
        if let Some(list) = self.todo_state.lists.get_mut(list_index) {
            if item_index < list.items.len() {
                list.items.remove(item_index);
            }
        }
        self.send_command(TodoCommand::RemoveTodo {
            list_index,
            item_index,
        });
        // Clamp selection
        let new_count = self.items_in_selected_list();
        if let Some(ui) = self.list_ui.get_mut(list_index) {
            if new_count == 0 {
                ui.selected_item = 0;
            } else if ui.selected_item >= new_count {
                ui.selected_item = new_count - 1;
            }
        }
    }

    pub fn remove_list(&mut self, list_index: usize) {
        if list_index < self.todo_state.lists.len() {
            self.todo_state.lists.remove(list_index);
            self.list_ui.remove(list_index);
        }
        self.send_command(TodoCommand::RemoveList { list_index });
        // Clamp selected_list
        if !self.todo_state.lists.is_empty() {
            self.selected_list = self.selected_list.min(self.todo_state.lists.len() - 1);
        } else {
            self.selected_list = 0;
        }
    }

    pub fn start_edit(&mut self, target: EditTarget, initial: &str) {
        self.edit = Some(EditState::new(target, initial));
        self.focus = Focus::Editing;
    }

    pub fn cancel_edit(&mut self) {
        self.edit.take();
        self.focus = if self.selected_list_expanded() {
            Focus::ItemList
        } else {
            Focus::ListSelector
        };
    }

    pub fn confirm_edit(&mut self) {
        if let Some(edit) = self.edit.take() {
            let text = edit.buffer.trim().to_string();
            if text.is_empty() {
                self.focus = if self.selected_list_expanded() {
                    Focus::ItemList
                } else {
                    Focus::ListSelector
                };
                return;
            }

            match edit.target {
                EditTarget::NewList => {
                    // Optimistic local update
                    self.todo_state.lists.push(TodoList::new(text.clone()));
                    self.list_ui.push(ListUiState {
                        expanded: false,
                        selected_item: 0,
                    });
                    self.send_command(TodoCommand::AddList {
                        title: text,
                        metadata: HashMap::new(),
                    });
                    self.focus = Focus::ListSelector;
                }
                EditTarget::RenameList { list_index } => {
                    // Optimistic local update
                    if let Some(list) = self.todo_state.lists.get_mut(list_index) {
                        list.title = text.clone();
                    }
                    self.send_command(TodoCommand::RenameList {
                        list_index,
                        title: text,
                    });
                    self.focus = if self.selected_list_expanded() {
                        Focus::ItemList
                    } else {
                        Focus::ListSelector
                    };
                }
                EditTarget::NewItem { list_index } => {
                    // Optimistic local update
                    if let Some(list) = self.todo_state.lists.get_mut(list_index) {
                        list.items.push(TodoItem {
                            text: text.clone(),
                            completed: false,
                            metadata: HashMap::new(),
                        });
                    }
                    self.send_command(TodoCommand::AddTodo {
                        list_index,
                        text,
                        metadata: HashMap::new(),
                    });
                    self.focus = Focus::ItemList;
                }
                EditTarget::EditItem {
                    list_index,
                    item_index,
                } => {
                    // Optimistic local update
                    if let Some(item) = self
                        .todo_state
                        .lists
                        .get_mut(list_index)
                        .and_then(|l| l.items.get_mut(item_index))
                    {
                        item.text = text.clone();
                    }
                    self.send_command(TodoCommand::RenameTodo {
                        list_index,
                        item_index,
                        text,
                    });
                    self.focus = Focus::ItemList;
                }
            }
        }
    }
}
