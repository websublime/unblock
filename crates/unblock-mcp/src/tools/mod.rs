//! MCP tool handlers.
//!
//! Each tool is a function that validates input, executes the operation,
//! and returns an MCP result. Write tools invalidate the cache and rebuild
//! the graph after mutation.
//!
//! ## Core Workflow (6)
//! `ready`, `claim`, `create`, `update`, `close`, `reopen`
//!
//! ## Dependencies (4)
//! `depends`, `dep_remove`, `dep_cycles`, `comment`
//!
//! ## Query (4)
//! `show`, `list`, `search`, `stats`
//!
//! ## Setup & Diagnostics (3)
//! `setup`, `doctor`, `prime`
