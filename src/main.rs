use dioxus::prelude::*;

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

/// Define a components module that contains all shared components for our app.
mod components;

/// Backend modules for MCP server and multicast state sync
pub mod backends;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: &str = include_str!("../assets/styling/main.css");
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
    }
}

fn main() {
    #[cfg(feature = "desktop")]
    desktop_main();

    #[cfg(not(feature = "desktop"))]
    dioxus::launch(App);
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
                }
            });
        }
    }
}

/// App is the main component of our app. Components are the building blocks of dioxus apps. Each component is a function
/// that takes some props and returns an Element. In this case, App takes no props because it is the root of our app.
///
/// Components should be annotated with `#[component]` to support props, better error messages, and autocomplete
#[component]
fn App() -> Element {
    // The `rsx!` macro lets us define HTML inside of rust. It expands to an Element with all of our HTML inside.
    rsx! {
        Title {
            "Todo MCP"
        }
        // In addition to element and text (which we will see later), rsx can contain other components. In this case,
        // we are using the `document::Link` component to add a link to our favicon and main CSS file into the head of our app.
        document::Link { rel: "icon", href: FAVICON }
        style { {MAIN_CSS} }
        style { {TAILWIND_CSS} }

        MainScreen {}

    }
}
