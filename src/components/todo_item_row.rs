use dioxus::prelude::*;

use crate::{
    backends::TodoCommand,
    components::{TodoItem, TodoItemStoreExt, TodoState, TodoStateStoreImplExt, },
};

fn resize_textarea(id: &str) {
    document::eval(&format!(
        r#"
        let el = document.getElementById('{id}');
        if (el) {{
            el.style.height = 'auto';
            el.style.height = el.scrollHeight + 'px';
        }}
        "#
    ));
}

#[derive(Props, Clone, PartialEq)]
pub struct TodoItemRowProps {
    list_idx: usize,
    item_idx: usize,
    todo: Store<TodoItem>,
    state: Store<TodoState>,
    #[props(default)]
    autofocus: bool,
    on_remove: Callback<usize>,
    #[props(default)]
    on_focused: Callback<()>,
}

#[component]
pub fn TodoItemRow(
    TodoItemRowProps {
        list_idx,
        item_idx,
        todo,
        state,
        autofocus,
        on_remove,
        on_focused,
    }: TodoItemRowProps,
) -> Element {
    let mut todo = todo;

    rsx! {
        div {
            class: "flex items-center gap-2 p-2 bg-white/40 rounded-2xl cursor-pointer hover:bg-white/50 transition-colors duration-200",


            // Checkbox
            button {
                class: "p-1 cursor-pointer rounded-full transition-all duration-300",
                class: if todo.read().completed {
                    "bg-gray-700"
                } else {
                    "bg-white"
                },
                onclick: move |evt| {
                    evt.stop_propagation();
                    let is_completed = todo.read().completed;
                    todo.write().completed = !is_completed;
                    state.send_update(TodoCommand::ToggleTodo {
                        list_index: list_idx,
                        item_index: item_idx,
                    });
                },
                svg {
                    class: "w-5 h-5 text-white",
                    style: if todo.read().completed { "opacity: 1" } else { "opacity: 0" },
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    view_box: "0 0 24 24",
                    path {
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        d: "M5 13l4 4L19 7",
                    }
                }
            }

            textarea {
                id: "todo-textarea-{list_idx}-{item_idx}",
                class: if todo.read().completed {
                    "w-full text-gray-500 line-through resize-none overflow-hidden bg-transparent"
                } else {
                    "w-full text-gray-900 resize-none overflow-hidden bg-transparent"
                },
                rows: "1",
                value: todo.text(),
                onclick: move |evt| evt.stop_propagation(),
                oninput: move |evt| {
                    evt.stop_propagation();
                    todo.text().set(evt.value());
                    state.send_update(TodoCommand::RenameTodo {
                        list_index: list_idx,
                        text: evt.value(),
                        item_index: item_idx,
                    });
                    resize_textarea(&format!("todo-textarea-{list_idx}-{item_idx}"));
                },
                onmounted: move |evt| {
                    if autofocus {
                        spawn(async move {
                            let _ = evt.set_focus(true).await;
                        });
                        on_focused(());
                    }
                    resize_textarea(&format!("todo-textarea-{list_idx}-{item_idx}"));
                },
            }

            button {
                class: "p-2 cursor-pointer rounded-full bg-white/80 transition-colors duration-200",
                onclick: move |evt| {
                    evt.stop_propagation();
                    on_remove(item_idx);
                },
                svg {
                    class: "w-4 h-4 text-gray-900",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    view_box: "0 0 24 24",
                    path {
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        d: "M18 6L6 18M6 6l12 12",
                    }
                }
            }
        }
    }
}
