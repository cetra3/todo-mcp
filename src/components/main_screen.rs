use std::collections::HashMap;

use dioxus::prelude::*;

use crate::{
    backends::TodoCommand,
    components::{CONNECTION_STATE, TODOS, TodoList, TodoState, TodoStateStoreImplExt},
};

use super::TodoTab;

#[component]
pub fn MainScreen() -> Element {
    let state = use_store(|| TodoState::new());

    let mut todos = TODOS.resolve();

    let connection_state = CONNECTION_STATE.read();

    rsx! {
        div { class: "flex flex-col min-h-screen",
        div { class: "p-2",
            div {
                for (idx , todo) in TODOS.resolve().iter().enumerate() {
                    TodoTab {
                        idx,
                        todo,
                        state,
                        on_remove: move |idx| {
                            todos.write().remove(idx);
                            state
                                .send_update(TodoCommand::RemoveList {
                                    list_index: idx,
                                });
                        },
                    }
                }
            }
        }

        div { class: "px-2",
            div { class: "relative rounded-2xl bg-black/5 hover:bg-black/10 transition-colors duration-200",
                button {
                    class: "cursor-pointer w-full p-4 bg-white/30 text-gray-700 flex items-center justify-center gap-2",
                    onclick: move |_evt| {
                        todos
                            .write()
                            .push(TodoList {
                                title: "New Todo List".into(),
                                items: vec![],
                                expanded: true,
                            });
                        state
                            .send_update(TodoCommand::AddList {
                                title: "New Todo List".into(),
                                metadata: HashMap::new(),
                            });
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

                    "Add New Todo List"
                }
            }

        }

        div {
            class: "mt-auto p-4 text-gray-500 flex items-center justify-center text-sm",
            em {
                "{connection_state}"
            }
        }
        }
    }
}
