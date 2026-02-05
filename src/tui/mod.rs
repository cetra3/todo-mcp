mod colors;
mod input;
mod state;
mod ui;

use std::io;

use anyhow::Result;
use crossterm::{
    event::{Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc::Receiver as TokioReceiver;

use crate::backends::{
    multicast::{TodoCommand, TodoEvent},
    setup,
};

use self::state::TuiState;

pub async fn run_tui() -> Result<()> {
    let site_id: u32 = rand::random();
    let (command_tx, mut event_rx) = setup(site_id);

    // Terminal setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, command_tx.clone(), &mut event_rx).await;

    // Terminal teardown (always runs)
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Send shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    command_tx
        .send(TodoCommand::Shutdown { sender: shutdown_tx })
        .await
        .ok();
    shutdown_rx.await.ok();

    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    command_tx: tokio::sync::mpsc::Sender<TodoCommand>,
    event_rx: &mut TokioReceiver<TodoEvent>,
) -> Result<()> {
    let mut state = TuiState::new(command_tx);
    let mut reader = EventStream::new();

    // Initial draw
    terminal.draw(|f| ui::draw(f, &mut state))?;

    loop {
        tokio::select! {
            // Terminal events (keyboard, resize)
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        input::handle_key(&mut state, key);
                        if state.should_quit {
                            return Ok(());
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        // Will redraw below
                    }
                    Some(Err(e)) => {
                        return Err(e.into());
                    }
                    None => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
            // Backend state events
            maybe_update = event_rx.recv() => {
                match maybe_update {
                    Some(event) => {
                        state.handle_event(event);
                    }
                    None => {
                        // Backend channel closed
                        return Ok(());
                    }
                }
            }
        }

        terminal.draw(|f| ui::draw(f, &mut state))?;
    }
}
