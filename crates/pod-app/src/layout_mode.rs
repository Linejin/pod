//! Layout modes: the structural axis of a theme (docs/10 §D5.4).
//!
//! A preset (`pod_ui::themes::PRESETS`) names a style theme and one of these
//! modes. Each mode is a [`LayoutModule`] that renders the window body between
//! the title bar and the status bar; the workspace keeps the data, actions,
//! dock and dialogs. Adding a mode = one module file + one `register` line in
//! [`LayoutRegistry::new`]; nothing in the workbench module has to change.
//! A mode without a module falls back to the workbench (the settings page says so).
use crate::workspace::Workspace;
use gpui_kit::*;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutMode {
    /// 작업대: tree sidebar + dock (01, 04, 05, 08–10, 14–18).
    Workbench,
    /// 집중 레일: 44px rail + task switcher (02).
    FocusRail,
    /// 응답 대기열/관제: response queue on the left (03, 06).
    Queue,
    /// 칸반 (12).
    Kanban,
    /// 블록 터미널 (11).
    BlockTerminal,
    /// 리뷰 중심 (13).
    Review,
    /// 고밀도 + 세부 열 (07).
    DenseDetails,
}
impl LayoutMode {
    pub const ALL: [Self; 7] = [
        Self::Workbench,
        Self::FocusRail,
        Self::Queue,
        Self::Kanban,
        Self::BlockTerminal,
        Self::Review,
        Self::DenseDetails,
    ];
    /// Persisted id (settings.json `layout`, `pod_ui::themes::Preset::layout`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Workbench => "workbench",
            Self::FocusRail => "focus-rail",
            Self::Queue => "queue",
            Self::Kanban => "kanban",
            Self::BlockTerminal => "block-terminal",
            Self::Review => "review",
            Self::DenseDetails => "dense-details",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Workbench => "작업대",
            Self::FocusRail => "집중 레일",
            Self::Queue => "응답 대기열",
            Self::Kanban => "칸반",
            Self::BlockTerminal => "블록 터미널",
            Self::Review => "리뷰 중심",
            Self::DenseDetails => "고밀도 + 세부 열",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }
}

/// One layout mode's body renderer.
pub trait LayoutModule {
    fn mode(&self) -> LayoutMode;
    /// Everything between the title bar and the status bar.
    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement;
    /// Extra title-bar content after the context line (e.g. status pills); none by default.
    fn title_bar_extra(&self, _ws: &Workspace, _cx: &mut Context<Workspace>) -> Option<AnyElement> {
        None
    }
    /// Whether the status bar shows 작업 중/응답/완료 (false where the title bar already does).
    fn status_counts(&self, _cx: &App) -> bool {
        true
    }
}

/// The registered modules (a GPUI global, set once at startup).
pub struct LayoutRegistry {
    modules: Vec<Rc<dyn LayoutModule>>,
}
impl Global for LayoutRegistry {}
impl Default for LayoutRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl LayoutRegistry {
    pub fn new() -> Self {
        let mut r = Self {
            modules: Vec::new(),
        };
        r.register(Rc::new(Workbench));
        r.register(Rc::new(crate::layouts::kanban::Kanban));
        r.register(Rc::new(crate::layouts::rail::FocusRail));
        r.register(Rc::new(crate::layouts::queue::QueueLayout));
        r.register(Rc::new(crate::layouts::dense::DenseLayout));
        r.register(Rc::new(crate::layouts::blocks::Blocks));
        r.register(Rc::new(crate::layouts::review::ReviewLayout));
        r
    }
    /// Replaces an earlier module for the same mode.
    pub fn register(&mut self, module: Rc<dyn LayoutModule>) {
        self.modules.retain(|m| m.mode() != module.mode());
        self.modules.push(module);
    }
    pub fn available(&self, mode: LayoutMode) -> bool {
        self.modules.iter().any(|m| m.mode() == mode)
    }
    /// The module for `id`, or the workbench when that mode is unknown or not built yet.
    pub fn resolve(&self, id: &str) -> Rc<dyn LayoutModule> {
        LayoutMode::from_id(id)
            .and_then(|mode| self.modules.iter().find(|m| m.mode() == mode))
            .or_else(|| {
                self.modules
                    .iter()
                    .find(|m| m.mode() == LayoutMode::Workbench)
            })
            .cloned()
            .expect("the workbench module is always registered")
    }
}

/// The module for the layout in settings.
fn current(cx: &App) -> Rc<dyn LayoutModule> {
    let layout = cx
        .try_global::<crate::theme::Settings>()
        .map(|s| s.layout.clone())
        .unwrap_or_default();
    match cx.try_global::<LayoutRegistry>() {
        Some(r) => r.resolve(&layout),
        None => LayoutRegistry::new().resolve(&layout),
    }
}

/// Body of the window for the layout in settings.
pub fn render_body(
    ws: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    current(cx).render_body(ws, window, cx)
}

/// Title-bar extra of the layout in settings.
pub fn title_bar_extra(ws: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    current(cx).title_bar_extra(ws, cx)
}

/// Status-bar counts for the layout in settings (06 관제형 shows them in the title bar).
pub fn status_counts(cx: &App) -> bool {
    current(cx).status_counts(cx)
}

/// 작업대: the tree sidebar + dock (the base for every preset in this stage).
struct Workbench;
impl LayoutModule for Workbench {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Workbench
    }
    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        ws.render_workbench_body(window, cx)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would shadow the std one.
    use super::{LayoutMode, LayoutModule, LayoutRegistry, Workspace};
    use gpui_kit::{AnyElement, Context, IntoElement, Window, div};
    use std::rc::Rc;

    struct Fake(LayoutMode);
    impl LayoutModule for Fake {
        fn mode(&self) -> LayoutMode {
            self.0
        }
        fn render_body(
            &self,
            _: &mut Workspace,
            _: &mut Window,
            _: &mut Context<Workspace>,
        ) -> AnyElement {
            div().into_any_element()
        }
    }

    #[test]
    fn unknown_or_unbuilt_modes_fall_back_to_the_workbench() {
        let mut r = LayoutRegistry::new();
        assert!(r.available(LayoutMode::Workbench));
        assert!(
            r.available(LayoutMode::Queue),
            "응답 대기열 (03, 06) is built"
        );
        for m in LayoutMode::ALL {
            assert_eq!(LayoutMode::from_id(m.id()), Some(m), "ids round-trip");
            // Registered modes resolve to themselves, the rest to the workbench.
            let expect = if r.available(m) {
                m
            } else {
                LayoutMode::Workbench
            };
            assert_eq!(r.resolve(m.id()).mode(), expect);
        }
        assert_eq!(r.resolve("nonsense").mode(), LayoutMode::Workbench);
        // A later module plugs in with one call and wins over the fallback.
        r.register(Rc::new(Fake(LayoutMode::Kanban)));
        assert_eq!(r.resolve("kanban").mode(), LayoutMode::Kanban);
        let n = r.modules.len();
        r.register(Rc::new(Fake(LayoutMode::Kanban)));
        assert_eq!(r.modules.len(), n, "re-registering replaces");
    }

    #[test]
    fn every_preset_names_a_known_layout() {
        for p in pod_ui::themes::PRESETS {
            assert!(
                LayoutMode::from_id(p.layout).is_some(),
                "preset {} layout {}",
                p.number,
                p.layout
            );
        }
    }
}
