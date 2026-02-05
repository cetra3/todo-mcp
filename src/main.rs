// Dioxus UI components (only needed when a dioxus feature is active)
#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
use dioxus::prelude::*;

#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
use components::MainScreen;

#[cfg(feature = "desktop")]
use {
    crate::backends::{hook, mcp},
    clap::Parser,
    cli::{Cli, Commands},
    std::fs::OpenOptions,
    tracing_error::ErrorLayer,
    tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter},
};

// TUI-only build (no desktop feature): use clap directly
#[cfg(all(feature = "tui", not(feature = "desktop")))]
use {
    clap::Parser,
    cli::{Cli, Commands},
    std::fs::OpenOptions,
    tracing_error::ErrorLayer,
    tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter},
};

/// Define a components module that contains all shared components for our app.
#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
mod components;

/// Backend modules for MCP server and multicast state sync
pub mod backends;

/// TUI module for terminal user interface
#[cfg(feature = "tui")]
mod tui;

#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
const FAVICON: Asset = asset!("/assets/favicon.ico");
#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
const MAIN_CSS: &str = include_str!("../assets/styling/main.css");
#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
const TAILWIND_CSS: &str = include_str!("../assets/tailwind.css");

#[cfg(feature = "desktop")]
mod cli {
    use clap::{Parser, Subcommand};

    #[derive(Parser)]
    #[command(version, about, long_about = None)]
    pub struct Cli {
        #[command(subcommand)]
        pub command: Option<Commands>,
    }

    #[derive(Subcommand)]
    pub enum Commands {
        /// Run as MCP stdio server
        Mcp,
        /// Run once off claude tool change
        Hook,
        /// Run the terminal user interface
        #[cfg(feature = "tui")]
        Tui,
    }
}

// CLI for TUI-only builds (no desktop, no dioxus GUI)
#[cfg(all(feature = "tui", not(feature = "desktop")))]
mod cli {
    use clap::{Parser, Subcommand};

    #[derive(Parser)]
    #[command(version, about, long_about = None)]
    pub struct Cli {
        #[command(subcommand)]
        pub command: Option<Commands>,
    }

    #[derive(Subcommand)]
    pub enum Commands {
        /// Run the terminal user interface
        Tui,
    }
}

fn main() {
    #[cfg(feature = "desktop")]
    desktop_main();

    #[cfg(all(feature = "tui", not(feature = "desktop")))]
    tui_main();

    #[cfg(not(any(feature = "desktop", feature = "tui")))]
    {
        #[cfg(any(feature = "web", feature = "mobile"))]
        dioxus::launch(App);
    }
}

#[cfg(feature = "desktop")]
fn desktop_main() {
    let cli = Cli::parse();

    const FALLBACK_RUST_LOG: &str = concat!(env!("CARGO_CRATE_NAME"), "=DEBUG");

    let debug_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open("/tmp/todo-mcp.log")
        .expect("Failed to open log file");

    // TUI mode: only log to file (no stdout layer that would corrupt terminal)
    #[cfg(feature = "tui")]
    let is_tui = matches!(cli.command, Some(Commands::Tui));
    #[cfg(not(feature = "tui"))]
    let is_tui = false;

    if is_tui {
        tracing_subscriber::registry()
            .with(
                EnvFilter::builder()
                    .try_from_env()
                    .unwrap_or_else(|_| EnvFilter::new(FALLBACK_RUST_LOG)),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(debug_file)
                    .with_file(true)
                    .with_line_number(true),
            )
            .with(ErrorLayer::default())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(
                EnvFilter::builder()
                    .try_from_env()
                    .unwrap_or_else(|_| EnvFilter::new(FALLBACK_RUST_LOG)),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_file(true)
                    .with_line_number(true),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(debug_file)
                    .with_file(true)
                    .with_line_number(true),
            )
            .with(ErrorLayer::default())
            .init();
    }

    match cli.command {
        None => {
            dioxus::LaunchBuilder::desktop()
                .with_cfg(
                    dioxus::desktop::Config::new().with_menu(None).with_window(
                        dioxus::desktop::WindowBuilder::new()
                            .with_inner_size(dioxus::desktop::LogicalSize::new(500.0, 800.0))
                            .with_title("Todo MCP"),
                    ),
                )
                .launch(App);
        }
        Some(command) => {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(async {
                match command {
                    Commands::Mcp => mcp::run_mcp().await.expect("MCP server failed"),
                    Commands::Hook => hook::run_hook().await.expect("Hook failed"),
                    #[cfg(feature = "tui")]
                    Commands::Tui => tui::run_tui().await.expect("TUI failed"),
                }
            });
        }
    }
}

/// Entry point for TUI-only builds (no desktop/dioxus).
#[cfg(all(feature = "tui", not(feature = "desktop")))]
fn tui_main() {
    let cli = Cli::parse();

    const FALLBACK_RUST_LOG: &str = concat!(env!("CARGO_CRATE_NAME"), "=DEBUG");

    let debug_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open("/tmp/todo-mcp.log")
        .expect("Failed to open log file");

    // TUI: file-only logging
    tracing_subscriber::registry()
        .with(
            EnvFilter::builder()
                .try_from_env()
                .unwrap_or_else(|_| EnvFilter::new(FALLBACK_RUST_LOG)),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(debug_file)
                .with_file(true)
                .with_line_number(true),
        )
        .with(ErrorLayer::default())
        .init();

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    rt.block_on(async {
        match cli.command {
            Some(Commands::Tui) | None => {
                tui::run_tui().await.expect("TUI failed");
            }
        }
    });
}

#[cfg(any(feature = "desktop", feature = "web", feature = "mobile"))]
#[component]
fn App() -> Element {
    rsx! {
        Title {
            "Todo MCP"
        }
        document::Link { rel: "icon", href: FAVICON }
        style { {MAIN_CSS} }
        style { {TAILWIND_CSS} }

        MainScreen {}

    }
}
