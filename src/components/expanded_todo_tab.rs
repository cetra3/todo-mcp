use std::collections::HashMap;

use dioxus::prelude::*;

use crate::{
    backends::TodoCommand,
    components::{
        TodoItem, TodoList, TodoListStoreExt, TodoState, TodoStateStoreImplExt,
    },
};

use super::{todo_item_row::TodoItemRow, todo_tab::todo_color};

#[derive(Props, Clone, PartialEq)]
pub struct ExpandedTodoTabProps {
    idx: usize,
    todo: Store<TodoList>,
    state: Store<TodoState>,
    on_collapse: Callback<()>,
    on_remove: Callback<usize>,
}

#[component]
pub fn ExpandedTodoTab(
    ExpandedTodoTabProps {
        idx,
        todo,
        state,
        on_collapse,
        on_remove,
    }: ExpandedTodoTabProps,
) -> Element {
    let mut title = todo.title();
    let mut items = todo.items();
    let mut focus_new_item = use_signal(|| false);

    rsx! {
        div {
            class: "relative rounded-t-3xl last:rounded-b-3xl -mt-8 first:mt-0 px-2 pt-4 pb-10 last:pb-2 hover:shadow-[0_0_15px_0_rgba(0,0,0,0.2)] transition-all duration-300 ease-out cursor-pointer",
            style: format!("background-color: {}", todo_color(&todo.read().title, idx, 93)),

            div { class: "flex justify-between items-center",
                // Back button
                button {
                    class: "p-2 mr-2 cursor-pointer rounded-full bg-white/50 transition-colors duration-200",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_collapse(());
                    },

                    svg {
                        class: "w-6 h-6 text-gray-900",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        view_box: "0 0 24 24",
                        path {
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            d: "M15 19l-7-7 7-7",
                        }
                    }
                }

                input {
                    class: "text-xl w-full font-bold text-gray-900",
                    value: "{title}",
                    oninput: move |evt| {
                        title.set(evt.value());
                        state.send_update(TodoCommand::RenameList {
                            list_index: idx,
                            title: evt.value(),
                        });
                    },
                }

                // Delete button
                button {
                    class: "p-2 cursor-pointer rounded-full bg-white/50 transition-colors duration-200",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_remove(idx);
                    },

                    svg {
                        class: "w-6 h-6 text-gray-900",
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

            div { class: "mt-4 space-y-2 animate-fade-in",
                for (item_idx, todo_item) in items.iter().enumerate() {
                    {
                        let is_last = item_idx == items.len() - 1;
                        let should_focus = is_last && *focus_new_item.read();
                        rsx! {
                            TodoItemRow {
                                list_idx: idx,
                                item_idx: item_idx,
                                todo: todo_item,
                                state: state,
                                autofocus: should_focus,
                                on_remove: move |item_idx| {
                                    items.remove(item_idx);
                                    state.send_update(TodoCommand::RemoveTodo {
                                        list_index: idx,
                                        item_index: item_idx,
                                    });
                                },
                                on_focused: move |_| {
                                    focus_new_item.set(false);
                                },
                            }
                        }
                    }
                }

                // Add new item button
                button {
                    class: "cursor-pointer mt-2 w-full p-2 bg-white/30 rounded-2xl text-gray-700 hover:bg-white/50 transition-colors duration-200 flex items-center justify-center gap-2",
                    onclick: move |_evt| {
                        items.write().push(TodoItem {
                            text: "".into(),
                            completed: false,
                        });
                        state.send_update(TodoCommand::AddTodo {
                            list_index: idx,
                            text: "".into(),
                            metadata: HashMap::new(),
                        });
                        focus_new_item.set(true);
                    },

                    svg {
                        class: "w-5 h-5",
                        fill: "none",
                        stroke: "currentColor",
                        stroke_width: "2",
                        view_box: "0 0 24 24",
                        path {
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            d: "M12 4v16m8-8H4",
                        }
                    }

                    span { "Add New Todo Item" }
                }
            }
        }
    }
}
