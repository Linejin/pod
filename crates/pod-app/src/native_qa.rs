//! Native-QA interaction tests (docs/10 §12, D3 checklist): the former MANUAL
//! rows, proven inside the app on GPUI's headless test platform.
//!
//! What this proves: the real `Workspace` view, the real key bindings
//! ([`crate::install`]), the real toolkit dialogs, inputs and focus handling,
//! and the real DirectWrite text system (layout bounds use real Malgun Gothic
//! metrics). Keystrokes go through GPUI's keystroke dispatch
//! (`Window::dispatch_keystroke`: key bindings, key-down handlers, then text
//! input). IME composition is driven through the focused control's own
//! `EntityInputHandler` — the calls GPUI's Windows backend makes for
//! `WM_IME_COMPOSITION` (GCS_COMPSTR → `replace_and_mark_text_in_range`,
//! GCS_RESULTSTR → `replace_text_in_range`). No mouse event is ever simulated.
//!
//! What it does not prove: the Win32 message pump (`WM_KEYDOWN` translation),
//! the Microsoft IME candidate window, Narrator speech or the compositor. The
//! data source is [`TestBackend`](crate::source::TestBackend), an in-process
//! podd double served on the test thread; nothing is spawned or contacted.
//!
//! Every test prints `[native-qa]` step lines (`--nocapture`); the saved run
//! is the evidence in `crates/pod-app/qa/native/`.
//!
//! Layout: [`harness`] is the shared driver (window, keys, IME, focus names,
//! bounds and dock observation), [`fixtures`] the shared backend seeds; the
//! scenarios live in one file per topic. Other QA modules use only the two
//! re-exports below. Test names are unchanged; each test moved from
//! `native_qa::<name>` to `native_qa::<file>::<name>`, so the filter
//! `native_qa` still selects all 26 (27 with `layout_contract`, card T2):
//!
//! | file | tests |
//! |---|---|
//! | `keyboard` | `status_branch_tracks_task_and_hides_before_totals`, `a_keyboard_only_repo_task_terminal_blocked_work_done`, `a_shortcuts_work_at_launch_and_after_the_focused_panel_closes`, `a_enter_in_dialogs_goes_to_the_focused_control`, `f_palette_sends_ctrl_p_and_ctrl_k_to_the_current_terminal` |
//! | `focus` | `dock_activation_does_not_open_inspector`, `b_escape_returns_focus_to_the_opener`, `c_tab_order_cycles_and_reverses_on_main_screens`, `c_shift_tab_from_the_agents_tree_reverses_tab`, `c_review_controls_are_reachable_by_keyboard` |
//! | `ime` | `d_ime_terminal_sends_bytes_only_on_commit`, `d_ime_palette_sidebar_filter_new_task_name`, `d_ime_review_comment_editor` |
//! | `dpi` | `e_dpi_scales_keep_chrome_rows_dialogs_inside_the_window` |
//! | `layout_contract` | `seven_modes_cycle_keeps_panels_dock_focus_requests_and_state` |
//! | `links` | `f_terminal_links_keyboard_picker_and_ctrl_click` |
//! | `extensions` | `f_hook_prompt_installs_only_after_the_user_confirms`, `f_hook_prompt_later_is_remembered_per_build`, `h_layout_preset_from_the_palette_rearranges_and_survives_restart`, `h_missing_and_invalid_presets_explain_themselves_in_korean` |
//! | `editor` | `f_status_bar_language_server_menu_off_restart_log`, `g_saving_in_the_editor_touches_the_task`, `g_task_removing_saves_unsaved_edits_or_closes_a_clean_editor`, `g_snippet_completion_tab_stops_and_auto_import`, `h_editor_tabs_keyboard_preview_mru_close_reopen`, `h_tree_delete_prompt_esc_cancels_enter_deletes`, `h_editor_tab_menu_and_move_keyboard` |

mod dpi;
mod editor;
mod extensions;
mod fixtures;
mod focus;
mod harness;
mod ime;
mod keyboard;
mod layout_contract;
mod links;

pub(crate) use fixtures::seed_two_tasks;
pub(crate) use harness::Harness;
