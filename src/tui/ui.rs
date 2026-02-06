use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use super::colors::{todo_color, todo_fg};
use super::state::{EditState, Focus, TuiState};

const COLLAPSED_HEIGHT: u16 = 4; // blank + title + counts + blank
const ITEM_HEIGHT: u16 = 1;
const LIST_HEADER_HEIGHT: u16 = 2; // blank + title
const ADD_ITEM_ROW_HEIGHT: u16 = 1;
const ADD_LIST_ROW_HEIGHT: u16 = 2;

fn list_block_height(item_count: u16, expanded: bool) -> u16 {
    if expanded {
        LIST_HEADER_HEIGHT + item_count * ITEM_HEIGHT + ADD_ITEM_ROW_HEIGHT + 1
    } else {
        COLLAPSED_HEIGHT
    }
}

/// Calculate total content height needed for all lists.
fn content_height(state: &TuiState) -> u16 {
    let mut h: u16 = 0;
    for (i, list) in state.todo_state.lists.iter().enumerate() {
        let expanded = state.list_ui.get(i).is_some_and(|u| u.expanded);
        h += list_block_height(list.items.len() as u16, expanded);
    }
    h += ADD_LIST_ROW_HEIGHT;
    h
}

pub fn draw(f: &mut Frame, state: &mut TuiState) {
    let size = f.area();

    let outer = Layout::vertical([
        Constraint::Length(1), // title bar
        Constraint::Min(1),   // main content
        Constraint::Length(1), // status bar
    ])
    .split(size);

    draw_title_bar(f, outer[0]);
    draw_content(f, outer[1], state);
    draw_status_bar(f, outer[2], state);
}

fn draw_title_bar(f: &mut Frame, area: Rect) {
    let bar = Paragraph::new(Line::from(vec![Span::styled(
        " Todo MCP",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )]))
    .style(Style::default().bg(Color::Rgb(60, 60, 80)));
    f.render_widget(bar, area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, state: &TuiState) {
    let mode_hint = match state.focus {
        Focus::ListSelector => "Enter:expand  a:add  d:del  r:rename  q:quit",
        Focus::ItemList => "Space:toggle  a:add  d:del  e:edit  r:rename list  q:quit  Esc:back",
        Focus::Editing => "Enter:confirm  Esc:cancel",
    };

    let status = if state.connection_status.is_empty() {
        mode_hint.to_string()
    } else {
        format!("{} | {}", state.connection_status, mode_hint)
    };

    let bar = Paragraph::new(Line::from(vec![Span::styled(
        format!(" {status}"),
        Style::default().fg(Color::White),
    )]))
    .style(Style::default().bg(Color::Rgb(50, 50, 65)));
    f.render_widget(bar, area);
}

fn draw_content(f: &mut Frame, area: Rect, state: &mut TuiState) {
    let total = content_height(state);

    // Adjust scroll to keep selected visible
    clamp_scroll(state, area.height, total);

    let mut y: i16 = -(state.scroll_offset as i16);

    for (i, list) in state.todo_state.lists.iter().enumerate() {
        let expanded = state.list_ui.get(i).is_some_and(|u| u.expanded);
        let is_selected = i == state.selected_list;

        let block_h = list_block_height(list.items.len() as u16, expanded);
        let r = Rect::new(area.x, area.y.saturating_add_signed(y), area.width, block_h);
        if expanded {
            draw_expanded_list(f, area, r, state, i, is_selected);
        } else {
            draw_collapsed_list(f, area, r, state, i, is_selected);
        }
        y += block_h as i16;
    }

    // "Add New Todo List" row
    let add_y = area.y.saturating_add_signed(y);
    if add_y < area.y + area.height {
        let r = Rect::new(area.x, add_y, area.width, ADD_LIST_ROW_HEIGHT.min(area.y + area.height - add_y));
        draw_add_list_row(f, area, r, state);
    }

    // Scrollbar
    if total > area.height {
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(area.height) as usize)
                .position(state.scroll_offset as usize);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

fn clip(area: Rect, widget_rect: Rect) -> Option<Rect> {
    let top = widget_rect.y.max(area.y);
    let bot = (widget_rect.y + widget_rect.height).min(area.y + area.height);
    if top >= bot {
        return None;
    }
    Some(Rect::new(widget_rect.x, top, widget_rect.width, bot - top))
}

/// Render a single-line edit field (prefix spans + underlined buffer) and position the cursor.
fn render_edit_line(
    f: &mut Frame,
    vis: Rect,
    clip_area: Rect,
    prefix: Vec<Span<'_>>,
    edit: &EditState,
    buf_style: Style,
    line_style: Style,
) {
    let prefix_width: u16 = prefix.iter().map(|s| s.content.len() as u16).sum();
    let mut spans = prefix;
    spans.push(Span::styled(
        &edit.buffer,
        buf_style.add_modifier(Modifier::UNDERLINED),
    ));
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(line_style),
        vis,
    );
    let cx = vis.x + prefix_width + edit.cursor as u16;
    if cx < vis.x + vis.width && vis.y >= clip_area.y && vis.y < clip_area.y + clip_area.height {
        f.set_cursor_position(Position::new(cx, vis.y));
    }
}

fn draw_collapsed_list(
    f: &mut Frame,
    clip_area: Rect,
    rect: Rect,
    state: &TuiState,
    list_idx: usize,
    is_selected: bool,
) {
    let Some(visible) = clip(clip_area, rect) else {
        return;
    };

    let list = &state.todo_state.lists[list_idx];
    let completed = list.items.iter().filter(|i| i.completed).count();
    let total = list.items.len();
    let bg = todo_color(&list.title, list_idx, 93);
    let fg = todo_fg(&list.title, list_idx);
    let active = is_selected && state.focus == Focus::ListSelector;

    // Background fill
    let bg_style = if active {
        Style::default().bg(bg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(bg)
    };
    f.render_widget(Block::default().style(bg_style), visible);

    // Title at y+1
    if let Some(vis) = clip(clip_area, Rect::new(rect.x, rect.y + 1, rect.width, 1)) {
        if let Some(edit) = state.edit.as_ref().filter(|e| e.is_rename_list(list_idx)) {
            render_edit_line(
                f, vis, clip_area,
                vec![Span::styled("   ", Style::default().fg(fg).bg(bg))],
                edit,
                Style::default().fg(fg).bg(bg),
                Style::default().bg(bg),
            );
        } else {
            let marker = if active { " > " } else { "   " };
            let line = Line::from(vec![
                Span::styled(marker, Style::default().fg(fg).add_modifier(Modifier::BOLD)),
                Span::styled(&list.title, Style::default().fg(fg).add_modifier(Modifier::BOLD)),
            ]);
            f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), vis);
        }
    }

    // Counts at y+2
    if let Some(vis) = clip(clip_area, Rect::new(rect.x, rect.y + 2, rect.width, 1)) {
        let line = Line::from(Span::styled(
            format!("     {completed}/{total} completed"),
            Style::default().fg(Color::Rgb(80, 80, 80)).bg(bg),
        ));
        f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), vis);
    }
}

fn draw_expanded_list(
    f: &mut Frame,
    clip_area: Rect,
    rect: Rect,
    state: &TuiState,
    list_idx: usize,
    is_selected: bool,
) {
    let list = &state.todo_state.lists[list_idx];
    let bg = todo_color(&list.title, list_idx, 93);
    let fg = todo_fg(&list.title, list_idx);
    let selected_item = state.list_ui.get(list_idx).map(|u| u.selected_item).unwrap_or(0);

    // Render background fill
    if let Some(visible) = clip(clip_area, rect) {
        let block = Block::default()
            .style(Style::default().bg(bg));
        f.render_widget(block, visible);
    }

    // Title line (offset by 1 for spacing above)
    if let Some(vis) = clip(clip_area, Rect::new(rect.x, rect.y + 1, rect.width, 1)) {
        if let Some(edit) = state.edit.as_ref().filter(|e| e.is_rename_list(list_idx)) {
            render_edit_line(
                f, vis, clip_area,
                vec![Span::styled("   ", Style::default().fg(fg).bg(bg))],
                edit,
                Style::default().fg(fg).bg(bg),
                Style::default().bg(bg),
            );
        } else {
            let marker = if is_selected { " < " } else { "   " };
            let line = Line::from(vec![
                Span::styled(marker, Style::default().fg(fg).bg(bg)),
                Span::styled(
                    &list.title,
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
                ),
            ]);
            f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), vis);
        }
    }

    // Items
    for (item_idx, item) in list.items.iter().enumerate() {
        let item_y = rect.y + LIST_HEADER_HEIGHT + item_idx as u16;
        let Some(vis) = clip(clip_area, Rect::new(rect.x, item_y, rect.width, 1)) else {
            continue;
        };

        let is_item_selected = is_selected
            && state.focus == Focus::ItemList
            && selected_item == item_idx;

        let checkbox = if item.completed { "[x] " } else { "[ ] " };
        let sel_marker = if is_item_selected { " > " } else { "   " };

        if let Some(edit) = state.edit.as_ref().filter(|e| e.is_edit_item(list_idx, item_idx)) {
            render_edit_line(
                f, vis, clip_area,
                vec![
                    Span::styled(sel_marker, Style::default().fg(fg).bg(bg)),
                    Span::styled(checkbox, Style::default().fg(fg).bg(bg)),
                ],
                edit,
                Style::default().fg(fg).bg(bg),
                Style::default().bg(bg),
            );
        } else {
            let text_style = if item.completed {
                Style::default()
                    .fg(Color::Rgb(120, 120, 120))
                    .bg(bg)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default().fg(fg).bg(bg)
            };

            let mut spans = vec![
                Span::styled(sel_marker, Style::default().fg(fg).bg(bg)),
                Span::styled(checkbox, Style::default().fg(fg).bg(bg)),
                Span::styled(&item.text, text_style),
            ];

            if is_item_selected {
                // Highlight the whole line
                for span in &mut spans {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                }
            }

            let line = Line::from(spans);
            f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), vis);
        }
    }

    // "Add new item" row
    let add_y = rect.y + LIST_HEADER_HEIGHT + list.items.len() as u16;
    if let Some(vis) = clip(clip_area, Rect::new(rect.x, add_y, rect.width, 1)) {
        let is_add_selected = is_selected
            && state.focus == Focus::ItemList
            && selected_item == list.items.len();

        if let Some(edit) = state.edit.as_ref().filter(|e| e.is_new_item(list_idx)) {
            render_edit_line(
                f, vis, clip_area,
                vec![Span::styled("    +  ", Style::default().fg(fg).bg(bg))],
                edit,
                Style::default().fg(fg).bg(bg),
                Style::default().bg(bg),
            );
        } else {
            let style = if is_add_selected {
                Style::default()
                    .fg(fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(100, 100, 100)).bg(bg)
            };
            let marker = if is_add_selected { " >  " } else { "    " };
            let line = Line::from(vec![
                Span::styled(marker, style),
                Span::styled("+  Add New Item", style),
            ]);
            f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), vis);
        }
    }
}

fn draw_add_list_row(f: &mut Frame, clip_area: Rect, rect: Rect, state: &TuiState) {
    let Some(visible) = clip(clip_area, rect) else {
        return;
    };

    if let Some(edit) = state.edit.as_ref().filter(|e| e.is_new_list()) {
        render_edit_line(
            f, visible, clip_area,
            vec![Span::styled("  + ", Style::default().fg(Color::Green))],
            edit,
            Style::default(),
            Style::default(),
        );
    } else {
        let is_selected = state.on_add_list_row() && state.focus == Focus::ListSelector;
        let marker = if is_selected { "> " } else { "  " };
        let style = if is_selected {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        let line = Line::from(vec![Span::styled(format!("{marker}+ Add New Todo List"), style)]);
        let para = Paragraph::new(vec![Line::from(""), line]);
        f.render_widget(para, visible);
    }
}

fn clamp_scroll(state: &mut TuiState, viewport_h: u16, total_h: u16) {
    if total_h <= viewport_h {
        state.scroll_offset = 0;
        return;
    }

    // Find the Y range of the currently selected element
    let (sel_top, sel_bot) = selected_y_range(state);

    // Scroll up if needed
    if sel_top < state.scroll_offset {
        state.scroll_offset = sel_top;
    }
    // Scroll down if needed
    if sel_bot > state.scroll_offset + viewport_h {
        state.scroll_offset = sel_bot.saturating_sub(viewport_h);
    }

    let max_scroll = total_h.saturating_sub(viewport_h);
    state.scroll_offset = state.scroll_offset.min(max_scroll);
}

/// Return (top, bottom) Y offset of the currently focused/selected element.
fn selected_y_range(state: &TuiState) -> (u16, u16) {
    let mut y: u16 = 0;

    for (i, list) in state.todo_state.lists.iter().enumerate() {
        let expanded = state.list_ui.get(i).is_some_and(|u| u.expanded);
        let block_h = list_block_height(list.items.len() as u16, expanded);

        if i == state.selected_list {
            if expanded && state.focus == Focus::ItemList {
                let sel = state.list_ui.get(i).map(|u| u.selected_item).unwrap_or(0) as u16;
                let item_y = y + LIST_HEADER_HEIGHT + sel;
                return (item_y, item_y + 1);
            }
            return (y, y + block_h);
        }

        y += block_h;
    }

    (y, y + ADD_LIST_ROW_HEIGHT)
}
