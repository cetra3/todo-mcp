//! The components module contains all shared components for our app. Components are the building blocks of dioxus apps.
//! They can be used to defined common UI elements like buttons, forms, and modals. In this template, we define a Hero
//! component  to be used in our app.

mod main_screen;

use dioxus::prelude::*;
pub use main_screen::MainScreen;

mod collapsed_todo_tab;
mod expanded_todo_tab;
mod todo_item_row;
mod todo_tab;

pub use todo_tab::TodoTab;
use tokio::sync::mpsc::Sender as TokioSender;

use crate::backends::{
    multicast::TodoEvent, setup, TodoCommand, TodoItem as McTodoItem, TodoList as McTodoList,
};

#[derive(Store, Clone)]
pub struct TodoItem {
    pub text: String,
    pub completed: bool,
}

#[derive(Store, Clone)]
pub struct TodoList {
    pub title: String,
    pub items: Vec<TodoItem>,
    pub expanded: bool,
}

impl From<McTodoItem> for TodoItem {
    fn from(item: McTodoItem) -> Self {
        Self {
            text: item.text,
            completed: item.completed,
        }
    }
}

impl From<McTodoList> for TodoList {
    fn from(item: McTodoList) -> Self {
        Self {
            title: item.title,
            items: item.items.into_iter().map(Into::into).collect(),
            expanded: false,
        }
    }
}

#[derive(Store)]
pub struct TodoState {
    pub sender: TokioSender<TodoCommand>,
}

#[store]
impl<Lens> Store<TodoState, Lens> {
    fn send_update(&self, update: TodoCommand) {
        let sender = self.sender().read().clone();
        spawn(async move {
            if let Err(err) = sender.send(update).await {
                error!("Failed to send update: {}", err);
            }
        });
    }
}

pub static TODOS: GlobalStore<Vec<TodoList>> = Global::new(|| Vec::new());
pub static CONNECTION_STATE: GlobalStore<String> = Global::new(|| String::new());

impl TodoState {
    pub fn new() -> Self {
        let site_id = rand::random();

        let (sender, mut recv) = setup(site_id);

        spawn(async move {
            while let Some(update) = recv.recv().await {
                match update {
                    TodoEvent::StateUpdate(update) => {
                        let mut todos = TODOS.write();
                        let prev = std::mem::take(&mut *todos);

                        *todos = update
                            .lists
                            .into_iter()
                            .zip(
                                prev.into_iter()
                                    .map(|t| t.expanded)
                                    .chain(std::iter::repeat(false)),
                            )
                            .map(|(list, expanded)| TodoList {
                                expanded,
                                ..list.into()
                            })
                            .collect();
                    }
                    TodoEvent::ConnectionStatus(count) => {
                        let mut alive_connections = CONNECTION_STATE.write();
                        *alive_connections = count;
                    }
                }
            }
        });

        Self { sender }
    }
}

impl Drop for TodoState {
    fn drop(&mut self) {
        let (sender, recv) = tokio::sync::oneshot::channel();

        self.sender
            .try_send(TodoCommand::Shutdown { sender })
            .expect("Could not send shutdown message");

        std::thread::spawn(move || {
            recv.blocking_recv()
                .expect("Could not receive shutdown response");

            debug!("shutdown completed");
        })
        .join()
        .expect("could not join shutdown thread");
    }
}
