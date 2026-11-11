//! The core commands: terminals, repositories, tasks and the daemon itself.
//! The specialised ones (orch, status, session, pr, …) keep their modules at
//! the crate root; `main` routes to both.

pub(crate) mod repo;
pub(crate) mod server;
pub(crate) mod task;
pub(crate) mod terminal;
