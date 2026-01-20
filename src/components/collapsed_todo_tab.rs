use dioxus::prelude::*;

use crate::components::{TodoList, TodoListStoreExt};

use super::todo_tab::todo_color;

#[derive(Props, Clone, PartialEq)]
pub struct CollapsedTodoTabProps {
    idx: usize,
    todo: Store<TodoList>,
    on_expand: Callback<()>,
}

#[component]
pub fn CollapsedTodoTab(
    CollapsedTodoTabProps {
        idx,
        todo,
        on_expand,
    }: CollapsedTodoTabProps,
) -> Element {
    let title = todo.title();

    let completed = todo
        .read()
        .items
        .iter()
        .filter(|item| item.completed)
        .count();

    let total = todo.read().items.len();

    rsx! {
        div {
            class: "relative rounded-t-3xl last:rounded-b-3xl -mt-8 first:mt-0 px-4 pt-4 pb-10 last:pb-8 hover:shadow-[0_0_15px_0_rgba(0,0,0,0.2)] transition-all duration-300 ease-out cursor-pointer",
            style: format!("background-color: {}", todo_color(&todo.read().title, idx, 93)),
            onclick: move |_| on_expand(()),

            div { class: "flex justify-between items-start gap-2",
                h2 { class: "text-2xl font-bold text-gray-900", "{title}" }

                span {
                    class: "px-3 py-1 rounded-full text-sm font-medium whitespace-nowrap truncate min-w-16",
                    style: format!("background-color: {}", todo_color(&todo.read().title, idx, 98)),
                    "{completed} Completed"
                }
            }

            div { class: "flex justify-between items-center pt-2",
                p { class: "text-gray-700", "{total} Items" }

                svg {
                    class: "w-5 h-5 text-gray-700",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    view_box: "0 0 24 24",
                    path {
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        d: "M7 17L17 7M17 7H7M17 7V17",
                    }
                }
            }
        }
    }
}
