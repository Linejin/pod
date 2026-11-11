//! Layout mode modules (docs/10 §D5.4): one file per mode, registered in
//! [`crate::layout_mode::LayoutRegistry::new`].
pub mod blocks;
pub mod blocks_model;
#[cfg(test)]
mod blocks_qa;
pub mod dense;
pub mod kanban;
#[cfg(test)]
mod kanban_qa;
// The narrow layout contract every mode renders through (docs/20-refactor/app.md).
pub(crate) mod kit;
pub mod queue;
pub mod rail;
pub mod review;
#[cfg(test)]
mod review_qa;
pub mod shell_integration;
pub mod workbench;
