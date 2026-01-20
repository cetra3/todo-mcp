# Todo MCP

A collaborative todo list app built with Rust and [Dioxus](https://dioxuslabs.com/) that synchronizes state across devices using [Automerge](https://automerge.org/) CRDTs and UDP multicast. It also exposes an [MCP](https://modelcontextprotocol.io/) server interface, making your todo lists accessible to AI tools like Claude Code.

![](./todo-mcp.png)

## Features

- **Multi-list management** -- Create, rename, and delete multiple todo lists, each with a unique color
- **Real-time sync** -- Instances on the same LAN discover each other via UDP multicast and stay in sync using Automerge CRDTs, so concurrent edits merge without conflicts
- **MCP server** -- Exposes todo operations (`get_todos`, `add_todo`, `toggle_todo`, etc.) over stdio so AI assistants can read and manage your lists
- **Claude Code hook** -- Bridges Claude Code's `TaskCreate`/`TaskUpdate` events into your todo lists, letting you track AI-generated tasks in the same UI
- **Persistent storage** -- State is saved to disk as an Automerge document and restored on restart
- **Cross-platform** -- Builds for desktop (default), web, and mobile via Dioxus feature flags
## Installing

You can install it from this git repo or using cargo:

```bash
cargo install todo-mcp
```

## Building and Running

Requires [Rust](https://rustup.rs/) and the [Dioxus CLI](https://dioxuslabs.com/learn/0.6/getting_started):

```bash
cargo install dioxus-cli
```

### Desktop app (default)

```bash
dx serve --platform desktop
```

### Web app

```bash
dx serve --platform web
```

### MCP server

Run as a stdio-based MCP server for use with Claude Code or other MCP clients:

```bash
todo-mcp mcp
```

### Claude Code hook

Process a Claude Code tool event from stdin and sync it into your todo lists:

```bash
todo-mcp hook
```

## MCP Tools

When running in MCP server mode, the following tools are available:

| Tool | Description |
|---|---|
| `get_todos` | Retrieve all lists, or a specific list by index |
| `add_list` | Create a new todo list |
| `remove_list` | Delete a list by index |
| `rename_list` | Rename an existing list |
| `add_todo` | Add an item to a list |
| `remove_todo` | Remove an item from a list |
| `toggle_todo` | Toggle an item's completion status |
| `clear_completed` | Remove all completed items from a list |
| `name_session` | Name a Claude Code session for hook integration |

## Claude Code Integration

To use todo-mcp as a Claude Code hook, add the following to your Claude Code settings (`.claude/settings.json`):

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "TaskCreate|TaskUpdate",
        "hooks": [
          {
            "type": "command",
            "command": "/path/to/todo-mcp hook"
          }
        ]
      }
    ]
  }
}
```

This routes task lifecycle events into todo-mcp so Claude Code's task lists appear in your todo app in real time.

## Sync Details

Instances sync over UDP multicast on `239.1.1.1:1111`. Messages larger than 1400 bytes are automatically fragmented and reassembled. Peers are considered connected if they've sent a message within the last 5 seconds.

State is persisted to `~/.local/share/todo_mcp/automerge.save` by default (override with the `MPAD_AUTOSAVE_PATH` environment variable).

## Configuration

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `todo_mcp=DEBUG` | Tracing log filter |
| `TODOMCP_AUTOSAVE_PATH` | `~/.local/share/todo_mcp/automerge.save` | Automerge save file location |

Logs are written to both stdout and `/tmp/todo-mcp.log`.

## License

See [Cargo.toml](Cargo.toml) for authorship details.
