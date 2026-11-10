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
pub mod queue;
pub mod rail;
pub mod review;
#[cfg(test)]
mod review_qa;
pub mod shell_integration;
