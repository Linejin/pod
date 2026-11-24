//! External A2A peers of a project (docs/09 §8.1): the project graph's
//! "외부 A2A 피어" section, a standard Agent2Agent 1.0 client surface.
//!
//! The user picks one peer configured for the selected project
//! ([`config`], the app's own `a2a-peers.json`), sees its Agent Card, writes
//! a message, reviews exactly what will be sent (peer, interface, skill,
//! text, ids, credential use) and sends it with the real `pod-a2a` client
//! (SendMessage, `returnImmediately`). Remote tasks are then shown with their
//! task/context ids, A2A state and results, refreshed with GetTask.
//!
//! Boundaries:
//! - Pod-internal orchestration (orch mail, the agent graph's terminal
//!   relations) is untouched; remote tasks are listed apart and labelled as
//!   remote A2A states, never drawn as graph edges.
//! - Nothing is sent without the review step: no project file, terminal
//!   output or credential is added. A token configured for a peer is sent
//!   only when its card declares HTTP Bearer, and the review says so.
//! - A send whose outcome is unknown is never repeated by Pod; the user may
//!   deliberately resend with the same message id after a new review.
//! - Work runs only for a viewer: the card is fetched when a peer is chosen
//!   or the section is shown, and active tasks are refreshed with bounded
//!   backoff only while the section is shown for their project.
//! - Peer content is data: escaped, never executed, URLs never opened.

pub mod config;
pub mod copy;
pub mod model;
pub mod net;
#[cfg(test)]
mod qa;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::orch::{mode, tc, tokens};
use crate::qa_marks::QaId;
use crate::workspace::Workspace;
use model::{CardState, PartView, ProjectA2a, Review, Submit};
use pod_a2a::model::{Message, StateClass};
use pod_ui::{ControlState, Tone, semantic_button};

/// Who is looking: the graph panel showing the section, and for which project.
struct Viewer {
    panel: WeakEntity<crate::project_graph::ProjectGraphPanel>,
    repo: String,
}

pub struct A2aSection {
    workspace: WeakEntity<Workspace>,
    config_dir: PathBuf,
    viewer: Option<Viewer>,
    /// Per project (registered repository name).
    projects: HashMap<String, ProjectA2a>,
    drafts: HashMap<String, Entity<TextareaState>>,
    /// Last "cannot review yet" reason, per project.
    hints: HashMap<String, String>,
    poller: Option<Task<()>>,
    polling: bool,
}

impl A2aSection {
    pub fn new(workspace: WeakEntity<Workspace>, config_dir: PathBuf) -> Self {
        Self {
            workspace,
            config_dir,
            viewer: None,
            projects: HashMap::new(),
            drafts: HashMap::new(),
            hints: HashMap::new(),
            poller: None,
            polling: false,
        }
    }

    /// The project shown, if a live panel shows the section.
    fn viewer_repo(&self) -> Option<&str> {
        self.viewer
            .as_ref()
            .filter(|v| v.panel.upgrade().is_some())
            .map(|v| v.repo.as_str())
    }

    /// The graph panel shows (Some) or hides (None) the section. Safe to call
    /// while the panel or the workspace is being updated: the reload runs
    /// deferred.
    pub fn set_viewer(
        &mut self,
        viewer: Option<(WeakEntity<crate::project_graph::ProjectGraphPanel>, String)>,
        cx: &mut Context<Self>,
    ) {
        let changed = self.viewer.as_ref().map(|v| &v.repo) != viewer.as_ref().map(|v| &v.1);
        self.viewer = viewer.map(|(panel, repo)| Viewer { panel, repo });
        if self.viewer.is_some() && changed {
            let me = cx.entity();
            cx.defer(move |cx| me.update(cx, |s, cx| s.on_view(cx)));
        }
        cx.notify();
    }

    /// The section became visible for its project: read the peers, fetch the
    /// chosen peer's card if needed, refresh active tasks once.
    fn on_view(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.viewer_repo().map(str::to_string) else {
            return;
        };
        self.reload_peers(&repo, cx);
        let p = self.projects.entry(repo.clone()).or_default();
        p.reset_polls();
        if let Some(id) = p.selected.clone()
            && !p.cards.contains_key(&id)
        {
            self.fetch_card(&repo, &id, cx);
        }
        self.refresh_now(&repo, None, cx);
        cx.notify();
    }

    fn reload_peers(&mut self, repo: &str, cx: &mut Context<Self>) {
        let path = self.workspace.upgrade().and_then(|w| {
            w.read(cx)
                .repos
                .iter()
                .find(|r| r.name == repo)
                .map(|r| r.path.clone())
        });
        let peers = config::load(&self.config_dir, repo, path.as_deref());
        let p = self.projects.entry(repo.to_string()).or_default();
        p.set_peers(peers);
        if let Some(id) = p.selected.clone()
            && !p.cards.contains_key(&id)
        {
            self.fetch_card(repo, &id, cx);
        }
    }

    pub(crate) fn select_peer(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.viewer_repo().map(str::to_string) else {
            return;
        };
        let p = self.projects.entry(repo.clone()).or_default();
        if p.select(id) {
            self.fetch_card(&repo, id, cx);
        }
        self.hints.remove(&repo);
        cx.notify();
    }

    fn fetch_card(&mut self, repo: &str, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self.projects.get_mut(repo) else {
            return;
        };
        let Some(def) = p.peers.usable(id).cloned() else {
            return;
        };
        p.cards.insert(id.to_string(), CardState::Loading);
        let (repo, id) = (repo.to_string(), id.to_string());
        let task = cx.spawn(async move |this, cx| {
            let asked = def.clone();
            let result = cx.background_spawn(async move { net::resolve(&def) }).await;
            let _ = this.update(cx, |s, cx| {
                let Some(p) = s.projects.get_mut(&repo) else {
                    return;
                };
                // A reloaded, changed definition makes this answer stale.
                if p.peers.usable(&id) != Some(&asked) {
                    return;
                }
                let state = match result {
                    Ok(r) => CardState::Ready(Box::new(r)),
                    Err(e) => CardState::Failed {
                        code: e.code(),
                        error: e.to_string(),
                    },
                };
                p.cards.insert(id, state);
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// One-shot calls run to completion (the section lives as long as the
    /// workspace); their answers go to the project they were made for.
    fn keep(&mut self, task: Task<()>) {
        task.detach();
    }

    pub(crate) fn choose_skill(&mut self, skill: Option<String>, cx: &mut Context<Self>) {
        let Some(repo) = self.viewer_repo().map(str::to_string) else {
            return;
        };
        let p = self.projects.entry(repo).or_default();
        let Some(peer) = p.selected.clone() else {
            return;
        };
        if let Submit::Reviewing(_) = p.submit {
            p.submit = Submit::Compose;
        }
        match skill {
            Some(s) => p.skill.insert(peer, s),
            None => p.skill.remove(&peer),
        };
        cx.notify();
    }

    fn draft_text(&self, repo: &str, cx: &App) -> String {
        self.drafts
            .get(repo)
            .map(|d| d.read(cx).value().to_string())
            .unwrap_or_default()
    }

    /// Freezes the draft for review (nothing is sent).
    pub(crate) fn request_review(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.viewer_repo().map(str::to_string) else {
            return;
        };
        let text = self.draft_text(&repo, cx);
        let p = self.projects.entry(repo.clone()).or_default();
        match p.review(&repo, &text, Message::new_id()) {
            Ok(r) => {
                p.submit = Submit::Reviewing(Box::new(r));
                self.hints.remove(&repo);
            }
            Err(n) => {
                self.hints.insert(repo, copy::not_ready(&n));
            }
        }
        cx.notify();
    }

    pub(crate) fn back_to_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.current_mut() {
            p.leave_review();
        }
        cx.notify();
    }

    pub(crate) fn review_resend(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.current_mut()
            && let Some(r) = p.resend_review()
        {
            p.submit = Submit::Reviewing(Box::new(r));
        }
        cx.notify();
    }

    fn current_mut(&mut self) -> Option<&mut ProjectA2a> {
        let repo = self.viewer_repo()?.to_string();
        self.projects.get_mut(&repo)
    }

    /// Sends the reviewed message, exactly as reviewed, once.
    pub(crate) fn confirm_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.viewer_repo().map(str::to_string) else {
            return;
        };
        let Some(p) = self.projects.get_mut(&repo) else {
            return;
        };
        let Submit::Reviewing(review) = std::mem::take(&mut p.submit) else {
            return;
        };
        let Some(def) = p.peers.usable(&review.peer_id).cloned() else {
            p.refuse_send(*review, copy::not_ready(&model::NotReady::NoPeer));
            cx.notify();
            return;
        };
        p.submit = Submit::Sending(review.clone());
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let sent: Review = (*review).clone();
            let outcome = cx
                .background_spawn(async move { net::send(&def, &review) })
                .await;
            let _ = this.update_in(cx, |s, window, cx| {
                let Some(p) = s.projects.get_mut(&sent.repo) else {
                    return;
                };
                let delivered = match outcome {
                    net::SendOutcome::Done(result) => p.apply_send(sent.clone(), *result),
                    net::SendOutcome::Changed(why) => {
                        p.refuse_send(sent.clone(), why);
                        false
                    }
                };
                if delivered && let Some(d) = s.drafts.get(&sent.repo) {
                    d.update(cx, |d, cx| d.set_value("", window, cx));
                }
                s.ensure_poller(cx);
                cx.notify();
            });
        });
        self.keep(task);
    }

    pub(crate) fn new_message(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.current_mut() {
            p.forget_unconfirmed();
        }
        cx.notify();
    }

    pub(crate) fn continue_task(&mut self, peer: &str, id: &str, cx: &mut Context<Self>) {
        if let Some(p) = self.current_mut() {
            p.continue_task(peer, id);
        }
        cx.notify();
    }

    pub(crate) fn continue_context(&mut self, n: usize, cx: &mut Context<Self>) {
        if let Some(p) = self.current_mut() {
            p.continue_context(n);
        }
        cx.notify();
    }

    pub(crate) fn stop_continue(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.current_mut() {
            p.continuing = None;
            if let Submit::Reviewing(_) = p.submit {
                p.submit = Submit::Compose;
            }
        }
        cx.notify();
    }

    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(repo) = self.viewer_repo().map(str::to_string) {
            self.reload_peers(&repo, cx);
        }
        cx.notify();
    }

    pub(crate) fn refetch_card(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.viewer_repo().map(str::to_string) else {
            return;
        };
        let selected = self.projects.get(&repo).and_then(|p| p.selected.clone());
        if let Some(id) = selected {
            self.fetch_card(&repo, &id, cx);
        }
        cx.notify();
    }

    /// GetTask now: one task (`manual`) or every task that wants it.
    pub(crate) fn refresh_now(
        &mut self,
        repo: &str,
        manual: Option<(&str, &str)>,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.projects.get_mut(repo) else {
            return;
        };
        let jobs = p.take_refreshes(manual);
        for (peer, id) in jobs {
            self.spawn_get(repo, peer, id, cx);
        }
        self.ensure_poller(cx);
    }

    fn spawn_get(&mut self, repo: &str, peer: String, id: String, cx: &mut Context<Self>) {
        let Some(p) = self.projects.get_mut(repo) else {
            return;
        };
        let Some(def) = p.peers.usable(&peer).cloned() else {
            p.apply_task(
                &peer,
                &id,
                Err(pod_a2a::A2aError::Invalid(
                    "이 피어가 더 이상 설정에 없습니다".into(),
                )),
            );
            return;
        };
        let repo = repo.to_string();
        let task = cx.spawn(async move |this, cx| {
            let asked = id.clone();
            let result = cx
                .background_spawn(async move { net::get_task(&def, &asked) })
                .await;
            let _ = this.update(cx, |s, cx| {
                if let Some(p) = s.projects.get_mut(&repo) {
                    p.apply_task(&peer, &id, result);
                }
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// Bounded auto refresh for the viewed project's active tasks. Stops when
    /// nothing is active, the auto rounds run out, or nobody views.
    fn ensure_poller(&mut self, cx: &mut Context<Self>) {
        if self.polling || self.viewer_repo().is_none() {
            return;
        }
        self.polling = true;
        self.poller = Some(cx.spawn(async move |this, cx| {
            loop {
                let delay = this.update(cx, |s, _| {
                    let repo = s.viewer_repo()?;
                    s.projects.get(repo)?.next_poll()
                });
                let Ok(Some(delay)) = delay else { break };
                cx.background_executor().timer(delay).await;
                let go = this.update(cx, |s, cx| {
                    let Some(repo) = s.viewer_repo().map(str::to_string) else {
                        return false;
                    };
                    let Some(p) = s.projects.get_mut(&repo) else {
                        return false;
                    };
                    for (peer, id) in p.take_refreshes(None) {
                        s.spawn_get(&repo, peer, id, cx);
                    }
                    cx.notify();
                    true
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |s, _| s.polling = false);
        }));
    }

    /// The viewed project's state (tests).
    #[cfg(test)]
    pub(crate) fn project(&self, repo: &str) -> Option<&ProjectA2a> {
        self.projects.get(repo)
    }

    #[cfg(test)]
    pub(crate) fn draft(&self, repo: &str) -> Option<Entity<TextareaState>> {
        self.drafts.get(repo).cloned()
    }

    #[cfg(test)]
    pub(crate) fn is_polling(&self) -> bool {
        self.polling
    }
}

// ---------------------------------------------------------------- render

fn small(text: impl Into<SharedString>, color: u32) -> Div {
    div().text_xs().text_color(tc(color)).child(text.into())
}

/// `label: value` line (value selectable as text, wraps).
fn field(label: &str, value: impl Into<SharedString>) -> Div {
    let t = tokens();
    div()
        .flex()
        .gap_2()
        .min_w_0()
        .text_xs()
        .child(
            div()
                .flex_shrink_0()
                .w(px(92.))
                .text_color(tc(t.text_muted))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(tc(t.text_primary))
                .child(value.into()),
        )
}

fn note_box(id: &'static str, tone: Tone, title: String, detail: Option<String>) -> AnyElement {
    let label = match &detail {
        Some(d) => format!("{title} {d}"),
        None => title.clone(),
    };
    div()
        .id(id)
        .qa_id(move || id.to_string())
        .role(Role::Status)
        .aria_label(label)
        .child(pod_ui::banner(
            tone,
            mode(),
            title,
            detail.map(SharedString::from),
        ))
        .into_any_element()
}

fn parts_view(prefix: String, parts: &[pod_a2a::model::Part]) -> Vec<AnyElement> {
    let t = tokens();
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let body: AnyElement = match model::part_view(p) {
                PartView::Text(s) => div()
                    .text_xs()
                    .text_color(tc(t.text_primary))
                    .child(s)
                    .into_any_element(),
                PartView::Data(s) => div()
                    .text_xs()
                    .font_family("Consolas")
                    .text_color(tc(t.text_secondary))
                    .child(s)
                    .into_any_element(),
                PartView::Url { url, name } => div()
                    .flex()
                    .gap_1()
                    .text_xs()
                    .text_color(tc(t.text_secondary))
                    .child(pod_ui::icon_sized(pod_ui::Icon::Link, 12., t.text_muted))
                    .child(format!(
                        "{} · {}{}",
                        copy::URL_NOT_OPENED,
                        if name.is_empty() {
                            String::new()
                        } else {
                            format!("{name} · ")
                        },
                        url
                    ))
                    .into_any_element(),
                PartView::Raw { bytes, name } => {
                    small(copy::raw(bytes, &name), t.text_secondary).into_any_element()
                }
            };
            let qa = format!("{prefix}-part-{i}");
            div()
                .id(SharedString::from(qa.clone()))
                .qa_id(move || qa)
                .pl_2()
                .border_l_2()
                .border_color(tc(t.border_subtle))
                .child(body)
                .into_any_element()
        })
        .collect()
}

impl A2aSection {
    fn render_peers(&self, repo: &str, p: &ProjectA2a, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens();
        let m = mode();
        let path = config::path_in(&self.config_dir).display().to_string();
        match &p.peers {
            config::ProjectPeers::NoFile | config::ProjectPeers::NotConfigured => {
                let title = if matches!(p.peers, config::ProjectPeers::NoFile) {
                    copy::NO_FILE_TITLE
                } else {
                    copy::NOT_CONFIGURED_TITLE
                };
                return note_box(
                    "a2a-no-peers",
                    Tone::Muted,
                    title.into(),
                    Some(copy::config_hint(&path, repo)),
                );
            }
            config::ProjectPeers::Invalid(e) => {
                return note_box(
                    "a2a-config-error",
                    Tone::Warning,
                    copy::INVALID_TITLE.into(),
                    Some(e.clone()),
                );
            }
            config::ProjectPeers::OtherPath { configured } => {
                return note_box(
                    "a2a-config-other-path",
                    Tone::Warning,
                    copy::INVALID_TITLE.into(),
                    Some(copy::other_path(configured)),
                );
            }
            config::ProjectPeers::Ready(_) => {}
        }
        let busy = p.submit.busy();
        let mut group = pod_ui::radio_group("a2a-peers", copy::PEERS, m);
        for slot in p.peers.slots() {
            let id = slot.def.id.clone();
            let on = p.selected.as_deref() == Some(id.as_str());
            let usable = slot.problem.is_none() && !busy;
            let name = match &slot.problem {
                Some(why) => format!("{} — 사용할 수 없음: {why}", slot.def.display()),
                None => format!("{} · {}", slot.def.display(), slot.def.source_url()),
            };
            let qa = id.clone();
            let me = cx.entity().downgrade();
            group = group.child(div().qa_id(move || format!("a2a-peer:{qa}")).child(
                pod_ui::choice(
                    SharedString::from(format!("a2a-peer-{id}")),
                    slot.def.display(),
                    name,
                    Some(pod_ui::Icon::Server),
                    on,
                    usable,
                    m,
                    move |_, cx| {
                        let _ = me.update(cx, |s, cx| s.select_peer(&id, cx));
                    },
                ),
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(small(copy::PEERS, t.text_muted).font_weight(FontWeight::SEMIBOLD))
            .child(group)
            .when(p.selected.is_none(), |d| {
                d.child(small(copy::CHOOSE_PEER, t.text_muted))
            })
            .into_any_element()
    }

    fn render_card(&self, p: &ProjectA2a, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = tokens();
        let m = mode();
        let peer = p.selected.as_deref()?;
        let def = p.peers.usable(peer)?;
        let me = cx.entity().downgrade();
        let reload = pod_ui::IconButton::new(
            "a2a-card-reload",
            pod_ui::Icon::RefreshCw,
            copy::CARD_RELOAD,
        )
        .render(m, move |_, cx| {
            let _ = me.update(cx, |s, cx| s.refetch_card(cx));
        })
        .qa_id(|| "a2a-card-reload".into());
        let body: AnyElement = match p.cards.get(peer) {
            None | Some(CardState::Loading) => {
                small(copy::CARD_LOADING, t.text_muted).into_any_element()
            }
            Some(CardState::Failed { code, error }) => note_box(
                "a2a-card-error",
                Tone::Warning,
                format!("에이전트 카드 오류 ({code})"),
                Some(format!("{} · {}", def.source_url(), error)),
            ),
            Some(CardState::Ready(r)) => {
                let c = &r.card;
                let schemes: Vec<String> = c
                    .security_schemes
                    .iter()
                    .map(|(k, v)| format!("{} {}", pod_a2a::sanitize_line(k, 40), v.describe()))
                    .collect();
                let me = cx.entity().downgrade();
                let chosen = p.chosen_skill().map(str::to_string);
                let mut skills = pod_ui::radio_group("a2a-skills", copy::SKILLS, m).child({
                    let me = me.clone();
                    pod_ui::choice(
                        "a2a-skill-none",
                        copy::NO_SKILL,
                        copy::NO_SKILL,
                        None,
                        chosen.is_none(),
                        !p.submit.busy(),
                        m,
                        move |_, cx| {
                            let _ = me.update(cx, |s, cx| s.choose_skill(None, cx));
                        },
                    )
                });
                for (i, s) in c.skills.iter().enumerate() {
                    let label =
                        pod_a2a::sanitize_line(if s.name.is_empty() { &s.id } else { &s.name }, 60);
                    let spoken = format!(
                        "{label} ({}) {}",
                        pod_a2a::sanitize_line(&s.id, 60),
                        pod_a2a::sanitize_line(&s.description, 200)
                    );
                    let id = s.id.clone();
                    let me = me.clone();
                    skills = skills.child(pod_ui::choice(
                        ("a2a-skill", i),
                        label,
                        spoken,
                        None,
                        chosen.as_deref() == Some(s.id.as_str()),
                        !p.submit.busy(),
                        m,
                        move |_, cx| {
                            let _ = me.update(cx, |s, cx| s.choose_skill(Some(id.clone()), cx));
                        },
                    ));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(field(
                        "에이전트",
                        format!(
                            "{} · 버전 {}",
                            pod_a2a::sanitize_line(&c.name, 80),
                            pod_a2a::sanitize_line(&c.version, 30)
                        ),
                    ))
                    .when(!c.description.is_empty(), |d| {
                        d.child(field("설명", pod_a2a::sanitize_text(&c.description, 400)))
                    })
                    .child(field("카드", r.card_url.clone()))
                    .child(field(
                        "전송 주소",
                        format!(
                            "{} {} · {}",
                            r.interface.protocol_binding,
                            r.interface.protocol_version,
                            r.interface.url
                        ),
                    ))
                    .child(field(
                        "기능",
                        copy::capabilities(
                            c.capabilities.streaming,
                            c.capabilities.push_notifications,
                        ),
                    ))
                    .child(field(
                        "인증",
                        if schemes.is_empty() {
                            "선언 없음".to_string()
                        } else {
                            format!(
                                "{}{}",
                                schemes.join(", "),
                                if def.has_token() {
                                    " · 토큰 설정됨"
                                } else {
                                    " · 토큰 설정 없음"
                                }
                            )
                        },
                    ))
                    .when(!c.signatures.is_empty(), |d| {
                        d.child(field("서명", "있음 (Pod는 검증하지 않음)"))
                    })
                    .child(small(copy::SKILLS, t.text_muted).font_weight(FontWeight::SEMIBOLD))
                    .child(skills)
                    .child(small(copy::SKILL_NOTE, t.text_muted))
                    .into_any_element()
            }
        };
        Some(
            div()
                .id("a2a-card")
                .qa_id(|| "a2a-card".into())
                .role(Role::Group)
                .aria_label(format!("에이전트 카드 · {}", def.display()))
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(tc(t.border_subtle))
                .bg(tc(t.bg_raised))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(pod_ui::icon_sized(pod_ui::Icon::Server, 14., t.text_muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(def.display()),
                        )
                        .child(reload),
                )
                .child(body)
                .into_any_element(),
        )
    }

    /// The project's draft editor (created once, kept for the session).
    fn ensure_draft(
        &mut self,
        repo: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        self.drafts
            .entry(repo.to_string())
            .or_insert_with(|| {
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(3, 10)
                        .placeholder(copy::DRAFT_PLACEHOLDER)
                })
            })
            .clone()
    }

    fn render_compose(
        &self,
        repo: &str,
        p: &ProjectA2a,
        draft: Entity<TextareaState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens();
        let m = mode();
        let me = cx.entity().downgrade();
        let mut col = div().flex().flex_col().gap_1();

        // Failure / unknown outcome first: the draft below is still there.
        if let Submit::Failed { review, error } = &p.submit {
            col = col.child(note_box(
                "a2a-failed",
                Tone::Danger,
                copy::FAILED_TITLE.into(),
                Some(format!("피어 {} · {error}", review.peer_display)),
            ));
        }
        let reviewing = matches!(p.submit, Submit::Reviewing(_) | Submit::Sending(_));
        if let Some(model::Unconfirmed { review, error }) = &p.unconfirmed
            && !reviewing
        {
            let (a, b) = (me.clone(), me.clone());
            col = col
                .child(note_box(
                    "a2a-unknown",
                    Tone::Warning,
                    copy::UNKNOWN_TITLE.into(),
                    Some(format!(
                        "메시지 ID {} · 피어 {} · {error}",
                        review.message_id, review.peer_display
                    )),
                ))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            semantic_button(
                                "a2a-resend",
                                copy::RESEND,
                                Tone::Warning,
                                m,
                                ControlState::Ready,
                                move |_, cx| {
                                    let _ = a.update(cx, |s, cx| s.review_resend(cx));
                                },
                            )
                            .qa_id(|| "a2a-resend".into()),
                        )
                        .child(
                            semantic_button(
                                "a2a-new-message",
                                copy::NEW_MESSAGE,
                                Tone::Secondary,
                                m,
                                ControlState::Ready,
                                move |_, cx| {
                                    let _ = b.update(cx, |s, cx| s.new_message(cx));
                                },
                            )
                            .qa_id(|| "a2a-new-message".into()),
                        ),
                );
        }

        if let Some(c) = &p.continuing {
            let me = me.clone();
            col = col.child(
                div()
                    .id("a2a-continuing")
                    .qa_id(|| "a2a-continuing".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        small(copy::continuing(&c.task_id, &c.context_id), t.info_fg)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        pod_ui::IconButton::new(
                            "a2a-stop-continue",
                            pod_ui::Icon::Minus,
                            copy::STOP_CONTINUE,
                        )
                        .render(m, move |_, cx| {
                            let _ = me.update(cx, |s, cx| s.stop_continue(cx));
                        }),
                    ),
            );
        }

        let unknown = p.unconfirmed.is_some();
        col = col
            .child(small(copy::DRAFT, t.text_muted).font_weight(FontWeight::SEMIBOLD))
            .child(
                div()
                    .id("a2a-draft")
                    .qa_id(|| "a2a-draft".into())
                    .aria_label(copy::DRAFT)
                    .child(Textarea::new(&draft)),
            );
        if !reviewing && !unknown {
            let me = me.clone();
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        semantic_button(
                            "a2a-review",
                            copy::REVIEW,
                            Tone::Info,
                            m,
                            ControlState::Ready,
                            move |_, cx| {
                                let _ = me.update(cx, |s, cx| s.request_review(cx));
                            },
                        )
                        .qa_id(|| "a2a-review-button".into()),
                    )
                    .when_some(self.hints.get(repo).cloned(), |d, h| {
                        d.child(
                            div()
                                .id("a2a-hint")
                                .qa_id(|| "a2a-hint".into())
                                .role(Role::Status)
                                .aria_label(h.clone())
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(tc(t.warning_fg))
                                .child(h),
                        )
                    }),
            );
        }
        if let Submit::Reviewing(r) | Submit::Sending(r) = &p.submit {
            col = col.child(self.render_review(r, p.submit.busy(), cx));
        }
        col.into_any_element()
    }

    fn render_review(&self, r: &Review, sending: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens();
        let m = mode();
        let me = cx.entity().downgrade();
        let back = me.clone();
        let cont = if r.task_id.is_empty() && r.context_id.is_empty() {
            "새 대화 (피어가 작업·컨텍스트 ID를 정함)".to_string()
        } else {
            format!(
                "작업 {} · 컨텍스트 {}",
                if r.task_id.is_empty() {
                    "-"
                } else {
                    &r.task_id
                },
                if r.context_id.is_empty() {
                    "-"
                } else {
                    &r.context_id
                }
            )
        };
        div()
            .id("a2a-review")
            .qa_id(|| "a2a-review".into())
            .role(Role::Group)
            .aria_label(copy::REVIEW_TITLE)
            .p_2()
            .rounded_md()
            .border_2()
            .border_color(tc(t.accent_fg))
            .bg(tc(t.bg_raised))
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(copy::REVIEW_TITLE))
            .when(r.resend, |d| {
                d.child(small(
                    "같은 메시지 ID로 다시 보냅니다: 피어가 이미 받았다면 중복으로 알아볼 수 있습니다.",
                    t.warning_fg,
                ))
            })
            .child(field("프로젝트", r.repo.clone()))
            .child(field("피어", r.peer_display.clone()))
            .child(field("카드", r.card_url.clone()))
            .child(field("전송 주소", r.interface_url.clone()))
            .child(field(
                "기술",
                match &r.skill {
                    Some((id, name)) => format!("{name} ({id})"),
                    None => copy::NO_SKILL.into(),
                },
            ))
            .child(field(
                "받을 형식",
                if r.accepted_output_modes.is_empty() {
                    "지정 안 함".to_string()
                } else {
                    r.accepted_output_modes
                        .iter()
                        .map(|s| pod_a2a::sanitize_line(s, 60))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            ))
            .child(field("메시지 ID", r.message_id.clone()))
            .child(field("이어 보내기", cont))
            .child(field("인증", copy::bearer(r.bearer)))
            .child(field("응답 방식", copy::RESPONSE_MODE))
            .child(small("보낼 글 (그대로 전송, text/plain 한 부분):", t.text_muted))
            .child(
                div()
                    .id("a2a-review-text")
                    .qa_id(|| "a2a-review-text".into())
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_sm()
                    .bg(tc(t.bg_surface))
                    .text_xs()
                    .child(pod_a2a::sanitize_text(&r.text, model::MAX_TEXT_BYTES)),
            )
            .child(small(copy::NOTHING_ELSE, t.text_secondary))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        semantic_button(
                            "a2a-send",
                            copy::SEND,
                            Tone::Info,
                            m,
                            if sending {
                                ControlState::Loading
                            } else {
                                ControlState::Ready
                            },
                            move |window, cx| {
                                let _ = me.update(cx, |s, cx| s.confirm_send(window, cx));
                            },
                        )
                        .qa_id(|| "a2a-send".into()),
                    )
                    .when(!sending, |d| {
                        d.child(
                            semantic_button(
                                "a2a-edit",
                                copy::EDIT,
                                Tone::Secondary,
                                m,
                                ControlState::Ready,
                                move |_, cx| {
                                    let _ = back.update(cx, |s, cx| s.back_to_edit(cx));
                                },
                            )
                            .qa_id(|| "a2a-edit".into()),
                        )
                    })
                    .when(sending, |d| d.child(small(copy::SENDING, t.text_muted))),
            )
            .into_any_element()
    }

    fn render_tasks(&self, p: &ProjectA2a, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens();
        let m = mode();
        let mut list = div()
            .id("a2a-tasks")
            .qa_id(|| "a2a-tasks".into())
            .role(Role::List)
            .aria_label(copy::TASKS)
            .flex()
            .flex_col()
            .gap_2()
            .child(small(copy::TASKS, t.text_muted).font_weight(FontWeight::SEMIBOLD));
        if p.tasks.is_empty() {
            list = list.child(small(copy::NO_TASKS, t.text_muted));
        }
        for (i, rt) in p.tasks.iter().enumerate() {
            let state = rt.state();
            let class = state.class();
            let (fg, bg) = match class {
                StateClass::Active => (t.info_fg, t.info_bg),
                StateClass::Interrupted => (t.warning_fg, t.warning_bg),
                StateClass::Terminal => match state {
                    pod_a2a::model::TaskState::Completed => (t.success_fg, t.success_bg),
                    _ => (t.danger_fg, t.danger_bg),
                },
                StateClass::Unknown => (t.text_secondary, t.bg_chrome),
            };
            let label = copy::state_text(state);
            let spoken = format!(
                "{} · {} · 작업 {} · {label}",
                copy::TASKS,
                rt.peer_display,
                pod_a2a::sanitize_line(&rt.task_id, 100)
            );
            let me = cx.entity().downgrade();
            let (peer, id) = (rt.peer_id.clone(), rt.task_id.clone());
            let refresh = pod_ui::IconButton::new(
                ("a2a-task-refresh", i),
                pod_ui::Icon::RefreshCw,
                copy::REFRESH,
            );
            let refresh = if rt.refreshing {
                pod_ui::IconButton {
                    state: ControlState::Loading,
                    ..refresh
                }
            } else {
                refresh
            };
            let qa_task = rt.task_id.clone();
            let qa_refresh = rt.task_id.clone();
            let mut card = div()
                .id(("a2a-task", i))
                .qa_id(move || format!("a2a-task:{qa_task}"))
                .role(Role::ListItem)
                .aria_label(spoken)
                .p_2()
                .rounded_md()
                .border_1()
                .border_dashed()
                .border_color(tc(t.border_strong))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(pod_ui::icon_sized(pod_ui::Icon::Server, 14., t.text_muted))
                        .child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .text_xs()
                                .bg(tc(bg))
                                .text_color(tc(fg))
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(tc(t.text_secondary))
                                .child(rt.peer_display.clone()),
                        )
                        .child({
                            let (peer, id) = (peer.clone(), id.clone());
                            refresh
                                .render(m, move |_, cx| {
                                    let _ = me.update(cx, |s, cx| {
                                        if let Some(repo) = s.viewer_repo().map(str::to_string) {
                                            s.refresh_now(&repo, Some((&peer, &id)), cx);
                                        }
                                        cx.notify();
                                    });
                                })
                                .qa_id(move || format!("a2a-task-refresh:{qa_refresh}"))
                        }),
                )
                .child(field("작업 ID", pod_a2a::sanitize_line(&rt.task_id, 200)))
                .child(field(
                    "컨텍스트 ID",
                    pod_a2a::sanitize_line(&rt.context_id, 200),
                ))
                .child(field("보낸 메시지", rt.message_id.clone()))
                .child(field(
                    "확인",
                    format!(
                        "{}{}",
                        copy::fetched(rt.fetched),
                        rt.task
                            .status
                            .timestamp
                            .as_deref()
                            .map(|s| format!(" · 피어 시각 {}", pod_a2a::sanitize_line(s, 40)))
                            .unwrap_or_default()
                    ),
                ));
            if let Some(msg) = &rt.task.status.message {
                card = card
                    .child(small(
                        format!("{} · 상태 메시지", copy::PEER_DATA),
                        t.text_muted,
                    ))
                    .children(parts_view(format!("a2a-task-{i}-status"), &msg.parts));
            }
            for (k, a) in rt.task.artifacts.iter().flatten().enumerate() {
                let name = if a.name.is_empty() {
                    pod_a2a::sanitize_line(&a.artifact_id, 80)
                } else {
                    pod_a2a::sanitize_line(&a.name, 80)
                };
                card = card
                    .child(small(
                        format!("{} · 결과물 {name}", copy::PEER_DATA),
                        t.text_muted,
                    ))
                    .children(parts_view(format!("a2a-task-{i}-art-{k}"), &a.parts));
            }
            if *state == pod_a2a::model::TaskState::AuthRequired {
                card = card.child(small(copy::AUTH_REQUIRED_NOTE, t.warning_fg));
            }
            if let Some(e) = &rt.error {
                let qa = rt.task_id.clone();
                card = card.child(
                    div()
                        .id(("a2a-task-error", i))
                        .qa_id(move || format!("a2a-task-error:{qa}"))
                        .role(Role::Status)
                        .aria_label(format!("마지막 확인 실패 (이전 결과 유지): {e}"))
                        .text_xs()
                        .text_color(tc(t.warning_fg))
                        .child(format!("마지막 확인 실패 (이전 결과 유지): {e}")),
                );
            }
            if let Some(kind) = &rt.no_refresh {
                card = card.child(small(
                    format!("피어가 GetTask에 {kind}로 답해 자동 확인하지 않습니다"),
                    t.text_muted,
                ));
            } else if rt.active() && rt.polls >= model::MAX_AUTO_POLLS {
                card = card.child(small(copy::AUTO_STOPPED, t.text_muted));
            }
            if class == StateClass::Interrupted && *state != pod_a2a::model::TaskState::AuthRequired
            {
                let me = cx.entity().downgrade();
                let (peer, id) = (peer.clone(), id.clone());
                let qa = rt.task_id.clone();
                card = card.child(
                    semantic_button(
                        ("a2a-task-continue", i),
                        copy::CONTINUE_TASK,
                        Tone::Info,
                        m,
                        if p.submit.busy() {
                            ControlState::Disabled
                        } else {
                            ControlState::Ready
                        },
                        move |_, cx| {
                            let _ = me.update(cx, |s, cx| s.continue_task(&peer, &id, cx));
                        },
                    )
                    .qa_id(move || format!("a2a-task-continue:{qa}")),
                );
            }
            list = list.child(card);
        }
        if !p.replies.is_empty() {
            list = list.child(small(copy::REPLIES, t.text_muted).font_weight(FontWeight::SEMIBOLD));
            for (n, r) in p.replies.iter().enumerate() {
                let me = cx.entity().downgrade();
                list = list.child(
                    div()
                        .id(("a2a-reply", n))
                        .qa_id(move || format!("a2a-reply:{n}"))
                        .role(Role::ListItem)
                        .aria_label(format!("{} · {}", copy::REPLIES, r.peer_display))
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_dashed()
                        .border_color(tc(t.border_strong))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(field("피어", r.peer_display.clone()))
                        .child(field("보낸 메시지", r.sent_message_id.clone()))
                        .child(field(
                            "컨텍스트 ID",
                            pod_a2a::sanitize_line(&r.reply.context_id, 200),
                        ))
                        .child(small(copy::PEER_DATA, t.text_muted))
                        .children(parts_view(format!("a2a-reply-{n}"), &r.reply.parts))
                        .when(!r.reply.context_id.is_empty(), |d| {
                            d.child(semantic_button(
                                ("a2a-reply-continue", n),
                                copy::CONTINUE_CONTEXT,
                                Tone::Secondary,
                                m,
                                if p.submit.busy() {
                                    ControlState::Disabled
                                } else {
                                    ControlState::Ready
                                },
                                move |_, cx| {
                                    let _ = me.update(cx, |s, cx| s.continue_context(n, cx));
                                },
                            ))
                        }),
                );
            }
        }
        list.into_any_element()
    }
}

impl Render for A2aSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let m = mode();
        let Some(repo) = self.viewer.as_ref().map(|v| v.repo.clone()) else {
            return div().into_any_element();
        };
        self.projects.entry(repo.clone()).or_default();
        let me = cx.entity().downgrade();
        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(pod_ui::icon_sized(pod_ui::Icon::Plug, 14., t.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(copy::TITLE),
                    )
                    .child(
                        pod_ui::IconButton::new(
                            "a2a-reload",
                            pod_ui::Icon::RotateCcw,
                            copy::RELOAD,
                        )
                        .render(m, move |_, cx| {
                            let _ = me.update(cx, |s, cx| s.reload(cx));
                        })
                        .qa_id(|| "a2a-reload".into()),
                    ),
            )
            .child(small(copy::SCOPE, t.text_secondary));
        let draft = self.ensure_draft(&repo, window, cx);
        let p = &self.projects[&repo];
        let peers = self.render_peers(&repo, p, cx);
        let card = self.render_card(p, cx);
        let compose = p
            .selected
            .is_some()
            .then(|| self.render_compose(&repo, p, draft, cx));
        let tasks = self.render_tasks(p, cx);
        div()
            .id("a2a-section")
            .qa_id(|| "a2a-section".into())
            .role(Role::Group)
            .aria_label(copy::section_label(&repo))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .border_t_2()
            .border_color(tc(t.border_strong))
            .bg(tc(t.bg_surface))
            .child(header)
            .child(peers)
            .children(card)
            .children(compose)
            .child(tasks)
            .into_any_element()
    }
}

impl Workspace {
    /// The A2A section (one per workspace, created on first use; it keeps
    /// every project's drafts and remote tasks for the session).
    pub(crate) fn a2a_section(&mut self, cx: &mut Context<Self>) -> Entity<A2aSection> {
        if let Some(s) = &self.orch.a2a {
            return s.clone();
        }
        let ws = cx.entity().downgrade();
        let s = cx.new(|_| A2aSection::new(ws, crate::layout::config_dir()));
        self.orch.a2a = Some(s.clone());
        s
    }
}

/// The graph header's entry: a labelled toggle button (pressed while the
/// section is shown), reachable with Tab and operated with Enter/Space.
/// `focus` is the graph panel's handle for it.
pub fn toggle(
    shown: bool,
    focus: &FocusHandle,
    panel: WeakEntity<crate::project_graph::ProjectGraphPanel>,
) -> AnyElement {
    let t = tokens();
    let m = mode();
    let fg = if shown {
        t.text_primary
    } else {
        t.text_secondary
    };
    div()
        .id("a2a-toggle")
        .qa_id(|| "a2a-toggle".into())
        .role(Role::Button)
        .aria_label(copy::TOGGLE)
        .aria_toggled(if shown { Toggled::True } else { Toggled::False })
        .track_focus(focus)
        .tab_index(0)
        .flex_shrink_0()
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(pod_ui::themes::metrics().radius.control))
        .border_1()
        .border_color(tc(if shown { t.accent_fg } else { t.border_subtle }))
        .when(shown, |d| d.bg(tc(t.bg_selected)))
        .text_xs()
        .text_color(tc(fg))
        .cursor_pointer()
        .hover(|s| s.bg(tc(t.bg_hover)))
        .focus_visible(|s| pod_ui::focus_style(s, m))
        .child(pod_ui::icon_sized(pod_ui::Icon::Plug, 12., fg))
        .child(copy::TOGGLE)
        .tooltip(move |w, cx| {
            gpui_kit::component::tooltip::Tooltip::new(copy::TOGGLE_TIP)
                .max_w(px(420.))
                .build(w, cx)
        })
        // Pointer and keyboard: GPUI turns Enter/Space on the focused
        // element into one click (on key-up), so there is no key handler.
        .on_click(move |_, _, cx| {
            let _ = panel.update(cx, |p, cx| p.toggle_a2a(cx));
        })
        .into_any_element()
}
