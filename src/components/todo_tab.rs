use std::hash::{DefaultHasher, Hash, Hasher};

use dioxus::prelude::*;

use crate::components::{TodoList, TodoListStoreExt, TodoState};

use super::{collapsed_todo_tab::CollapsedTodoTab, expanded_todo_tab::ExpandedTodoTab};

#[derive(Props, Clone, PartialEq)]
pub struct TodoTabProps {
    idx: usize,
    todo: Store<TodoList>,
    state: Store<TodoState>,
    on_remove: Callback<usize>,
}

pub fn todo_color(text: &str, idx: usize, lightness_pct: usize) -> String {
    let mut default_hasher = DefaultHasher::new();
    text.hash(&mut default_hasher);
    idx.hash(&mut default_hasher);
    let hash = default_hasher.finish();

    let hue = hash % 360;

    format!("oklch({lightness_pct}% 0.09 {hue})")
}

#[component]
pub fn TodoTab(
    TodoTabProps {
        idx,
        todo,
        state,
        on_remove,
    }: TodoTabProps,
) -> Element {
    let mut expanded = todo.expanded();

    if expanded() {
        rsx! {
            ExpandedTodoTab {
                idx: idx,
                todo: todo,
                state: state,
                on_collapse: move |_| expanded.set(false),
                on_remove: on_remove,
            }
        }
    } else {
        rsx! {
            CollapsedTodoTab {
                idx: idx,
                todo: todo,
                on_expand: move |_| expanded.set(true),
            }
        }
    }
}
