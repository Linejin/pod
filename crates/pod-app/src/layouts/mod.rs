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
// The narrow layout contract (docs/20-refactor/app.md, card A-F0). The modes
// still render through `LayoutModule::render_body`, so most of it has no
// caller outside its tests yet.
#[allow(
    dead_code,
    unused_imports,
    reason = "A-F0 contract; callers arrive with A-F1 (workspace/layout_host.rs) and A-P10..A-P14; A-T2 removes this allow"
)]
pub(crate) mod kit;
pub mod queue;
pub mod rail;
pub mod review;
#[cfg(test)]
mod review_qa;
pub mod shell_integration;
