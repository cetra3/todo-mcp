use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{EditTarget, Focus, TuiState};

pub fn handle_key(state: &mut TuiState, key: KeyEvent) {
    // Global quit
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        state.should_quit = true;
        return;
    }

    match state.focus {
        Focus::ListSelector => handle_list_selector(state, key),
        Focus::ItemList => handle_item_list(state, key),
        Focus::Editing => handle_editing(state, key),
    }
}

fn handle_list_selector(state: &mut TuiState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => {
            state.should_quit = true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.move_list_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.move_list_up();
        }
        KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('l') | KeyCode::Right => {
            if state.on_add_list_row() {
                state.start_edit(EditTarget::NewList, "");
            } else if state.list_count() > 0 {
                state.expand_selected();
            }
        }
        KeyCode::Char('a') => {
            state.start_edit(EditTarget::NewList, "");
        }
        KeyCode::Char('d') => {
            if !state.on_add_list_row() && state.list_count() > 0 {
                let idx = state.selected_list;
                state.remove_list(idx);
            }
        }
        KeyCode::Char('r') => {
            if !state.on_add_list_row() && state.list_count() > 0 {
                let idx = state.selected_list;
                let title = state.todo_state.lists[idx].title.clone();
                state.start_edit(EditTarget::RenameList { list_index: idx }, &title);
            }
        }
        _ => {}
    }
}

fn handle_item_list(state: &mut TuiState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => {
            state.collapse_selected();
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.move_item_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.move_item_up();
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            let list_idx = state.selected_list;
            let item_idx = state.selected_item_index();

            // If on the "Add new item" row
            if item_idx == state.items_in_selected_list() {
                state.start_edit(EditTarget::NewItem { list_index: list_idx }, "");
            } else {
                state.toggle_item(list_idx, item_idx);
            }
        }
        KeyCode::Char('a') => {
            let list_idx = state.selected_list;
            state.start_edit(EditTarget::NewItem { list_index: list_idx }, "");
        }
        KeyCode::Char('d') => {
            let list_idx = state.selected_list;
            let item_idx = state.selected_item_index();
            if item_idx < state.items_in_selected_list() {
                state.remove_item(list_idx, item_idx);
            }
        }
        KeyCode::Char('e') => {
            let list_idx = state.selected_list;
            let item_idx = state.selected_item_index();
            if item_idx < state.items_in_selected_list() {
                let text = state.todo_state.lists[list_idx].items[item_idx].text.clone();
                state.start_edit(
                    EditTarget::EditItem {
                        list_index: list_idx,
                        item_index: item_idx,
                    },
                    &text,
                );
            }
        }
        KeyCode::Char('r') => {
            let list_idx = state.selected_list;
            let title = state.todo_state.lists[list_idx].title.clone();
            state.start_edit(EditTarget::RenameList { list_index: list_idx }, &title);
        }
        KeyCode::Char('q') => {
            state.should_quit = true;
        }
        _ => {}
    }
}

fn handle_editing(state: &mut TuiState, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            state.cancel_edit();
        }
        KeyCode::Enter => {
            state.confirm_edit();
        }
        KeyCode::Left => {
            if let Some(edit) = &mut state.edit {
                edit.move_left();
            }
        }
        KeyCode::Right => {
            if let Some(edit) = &mut state.edit {
                edit.move_right();
            }
        }
        KeyCode::Backspace => {
            if let Some(edit) = &mut state.edit {
                edit.delete_back();
            }
        }
        KeyCode::Delete => {
            if let Some(edit) = &mut state.edit {
                edit.delete_forward();
            }
        }
        KeyCode::Char(ch) => {
            if let Some(edit) = &mut state.edit {
                edit.insert_char(ch);
            }
        }
        _ => {}
    }
}
