pub mod mcp;
pub mod hook;
pub mod ipc;
pub mod iroh;
pub mod multicast;
pub mod proto;

pub use multicast::{setup, TodoCommand, TodoItem, TodoList, TodoState};
