//! Layout modes: the structural axis of a theme (docs/10 §D5.4).
//!
//! A preset (`pod_ui::themes::PRESETS`) names a style theme and one of these
//! modes. Each mode is a [`LayoutModule`] that renders the window body between
//! the title bar and the status bar; the workspace keeps the data, actions,
//! dock and dialogs. Adding a mode = one module file + one `register` line in
//! [`LayoutRegistry::new`]; nothing in the workbench module has to change.
//! A mode without a module falls back to the workbench (the settings page says so).
//!
//! Refactor in progress (docs/20-refactor/app.md, card A-F0): a module may also
//! offer a narrow [`LayoutRenderer`] over the layout kit (`layouts::kit`). The
//! body is still drawn by [`LayoutModule::render_body`]; once the workspace
//! hosts the kit (A-F1), a mode without a renderer keeps that path.
use crate::layouts::kit::LayoutRenderer;
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
    /// The mode's narrow renderer over the layout kit. `None` (the default)
    /// keeps [`Self::render_body`] and [`Self::title_bar_extra`]; with one,
    /// the host draws the body and the title-bar extra through it.
    fn renderer(&self) -> Option<Rc<dyn LayoutRenderer>> {
        None
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

/// Narrow renderer of the layout in settings, if its mode has one. `None`:
/// the caller keeps [`render_body`] and [`title_bar_extra`].
#[allow(
    dead_code,
    reason = "A-F1 calls this from the workspace shell; A-T2 removes this allow"
)]
pub(crate) fn renderer(cx: &App) -> Option<Rc<dyn LayoutRenderer>> {
    current(cx).renderer()
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
    use super::{LayoutMode, LayoutModule, LayoutRegistry, Workbench, Workspace};
    use crate::layouts::kit::{LayoutFrame, LayoutHost, LayoutRenderer};
    use gpui_kit::{AnyElement, App, Context, IntoElement, Window, div};
    use std::rc::Rc;

    /// A mode moved onto the layout kit.
    struct Migrated(LayoutMode);
    impl LayoutModule for Migrated {
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
        fn renderer(&self) -> Option<Rc<dyn LayoutRenderer>> {
            Some(Rc::new(Narrow))
        }
    }
    struct Narrow;
    impl LayoutRenderer for Narrow {
        fn render_body(
            &self,
            _: &LayoutFrame<'_>,
            _: &mut dyn LayoutHost,
            _: &mut Window,
            _: &mut App,
        ) -> AnyElement {
            div().into_any_element()
        }
    }

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
    fn modes_without_a_module_fall_back_to_the_workbench() {
        let r = LayoutRegistry {
            modules: vec![Rc::new(Workbench)],
        };
        for m in LayoutMode::ALL {
            assert_eq!(r.available(m), m == LayoutMode::Workbench, "{m:?}");
            assert_eq!(r.resolve(m.id()).mode(), LayoutMode::Workbench, "{m:?}");
        }
        for id in ["", "nonsense", "Kanban", " kanban"] {
            assert_eq!(r.resolve(id).mode(), LayoutMode::Workbench, "{id:?}");
        }
    }

    #[test]
    fn re_registering_replaces_the_module_and_its_renderer() {
        let mut r = LayoutRegistry::new();
        let n = r.modules.len();
        assert_eq!(n, LayoutMode::ALL.len(), "one module per mode");
        assert!(
            Fake(LayoutMode::Kanban).renderer().is_none(),
            "no narrow renderer unless a mode offers one"
        );
        r.register(Rc::new(Migrated(LayoutMode::Kanban)));
        assert_eq!(r.modules.len(), n);
        assert_eq!(r.resolve("kanban").mode(), LayoutMode::Kanban);
        assert!(
            r.resolve("kanban").renderer().is_some(),
            "the later module wins"
        );
        r.register(Rc::new(Fake(LayoutMode::Kanban)));
        assert_eq!(r.modules.len(), n);
        assert!(
            r.resolve("kanban").renderer().is_none(),
            "replacing the module drops its renderer too"
        );
        // The fallback is the workbench module as registered, renderer included.
        r.register(Rc::new(Migrated(LayoutMode::Workbench)));
        assert_eq!(r.resolve("nonsense").mode(), LayoutMode::Workbench);
        assert!(r.resolve("nonsense").renderer().is_some());
        // Other modes are untouched by either replacement.
        for m in LayoutMode::ALL {
            assert_eq!(r.resolve(m.id()).mode(), m, "{m:?}");
        }
    }

    #[test]
    fn the_settings_layout_picks_the_module_and_its_renderer() {
        let cx = ::gpui::TestAppContext::single();
        cx.update(|cx| {
            // No settings, no registry global: the built-in registry, workbench.
            assert_eq!(super::current(cx).mode(), LayoutMode::Workbench);
            let layout = |id: &str| crate::theme::Settings {
                layout: id.into(),
                ..Default::default()
            };
            cx.set_global(layout("kanban"));
            assert_eq!(super::current(cx).mode(), LayoutMode::Kanban);
            let mut r = LayoutRegistry::new();
            r.register(Rc::new(Migrated(LayoutMode::Kanban)));
            cx.set_global(r);
            assert_eq!(super::current(cx).mode(), LayoutMode::Kanban);
            assert!(super::renderer(cx).is_some(), "the registry global is used");
            cx.set_global(layout("queue"));
            assert_eq!(super::current(cx).mode(), LayoutMode::Queue);
            assert!(super::renderer(cx).is_none(), "queue still renders itself");
            cx.set_global(layout("nonsense"));
            assert_eq!(super::current(cx).mode(), LayoutMode::Workbench);
            assert!(super::renderer(cx).is_none());
        });
        cx.update(|cx| cx.quit());
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
