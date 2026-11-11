//! Byte pins for the shared copy. Every constant, `tr!` template, label map and
//! toolkit catalog entry is dumped in a fixed order and compared with the pin taken
//! from the single-file strings.rs before the A-T1 split. A changed byte, a dropped
//! entry or a swapped template fails here. Set POD_STRINGS_DUMP=<dir> to write the
//! dumps for a textual diff.

/// FNV-1a 64 over the dump text.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn check(name: &str, rows: &[(&str, String)], count: usize, pin: u64) {
    let dump: String = rows
        .iter()
        .map(|(key, text)| format!("{key}\t{text}\n"))
        .collect();
    if let Some(dir) = std::env::var_os("POD_STRINGS_DUMP") {
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{name}.txt")),
            &dump,
        )
        .expect("write dump");
    }
    let hash = fnv1a(&dump);
    if hash != pin {
        eprintln!("{dump}");
    }
    assert_eq!((rows.len(), hash), (count, pin), "{name} copy changed");
}

#[test]
fn host_catalog_reaches_dock_and_diff() {
    super::init_toolkit_locale();
    assert_eq!(
        gpui_kit::component::_rust_i18n_translate("ko", "Dock.Close"),
        "닫기"
    );
    assert_eq!(
        gpui_kit::component::_rust_i18n_translate("ko", "Diff.SelectLine"),
        "%{side} %{line}줄 선택"
    );
}

#[test]
fn constants_keep_their_bytes() {
    let rows: Vec<(&str, String)> = [
        ("AGENT_SUFFIX", super::AGENT_SUFFIX),
        ("AGENT_SHELL_TERMINAL", super::AGENT_SHELL_TERMINAL),
        ("NEW_TASK", super::NEW_TASK),
        ("OPEN_REPOSITORY", super::OPEN_REPOSITORY),
        (
            "REGISTER_A_LOCAL_GIT_REPOSITORY",
            super::REGISTER_A_LOCAL_GIT_REPOSITORY,
        ),
        ("NEW_TERMINAL", super::NEW_TERMINAL),
        ("SETTINGS", super::SETTINGS),
        ("CURRENT_BRANCH", super::CURRENT_BRANCH),
        (
            "FIND_NEXT_BLOCKED_REQUEST",
            super::FIND_NEXT_BLOCKED_REQUEST,
        ),
        (
            "RETURN_TO_THE_TERMINAL_THAT_NEEDS_YOU",
            super::RETURN_TO_THE_TERMINAL_THAT_NEEDS_YOU,
        ),
        (
            "INSPECT_CURRENT_TERMINAL_STATUS",
            super::INSPECT_CURRENT_TERMINAL_STATUS,
        ),
        ("SOURCE_AND_FRESHNESS", super::SOURCE_AND_FRESHNESS),
        ("NOTIFICATIONS", super::NOTIFICATIONS),
        ("SETTINGS_DAMAGED", super::SETTINGS_DAMAGED),
        ("SETTINGS_DAMAGED_BACKUP", super::SETTINGS_DAMAGED_BACKUP),
        (
            "SETTINGS_DAMAGED_NO_BACKUP",
            super::SETTINGS_DAMAGED_NO_BACKUP,
        ),
        ("NO_NOTIFICATIONS", super::NO_NOTIFICATIONS),
        ("JUST_NOW", super::JUST_NOW),
        (
            "PENDING_REQUESTS_AND_RECENT_EVENTS",
            super::PENDING_REQUESTS_AND_RECENT_EVENTS,
        ),
        (
            "SEND_CTRL_K_TO_CURRENT_TERMINAL",
            super::SEND_CTRL_K_TO_CURRENT_TERMINAL,
        ),
        (
            "TERMINAL_SHORTCUT_PASSTHROUGH",
            super::TERMINAL_SHORTCUT_PASSTHROUGH,
        ),
        ("ARRANGE_PANELS_IN_A_GRID", super::ARRANGE_PANELS_IN_A_GRID),
        ("NEW_COMPARISON_GROUP", super::NEW_COMPARISON_GROUP),
        ("COMPARE_PARALLEL_TASKS", super::COMPARE_PARALLEL_TASKS),
        ("OPEN_GATES_INBOX", super::OPEN_GATES_INBOX),
        ("PENDING_APPROVALS", super::PENDING_APPROVALS),
        ("OPEN_AGENT_TREE", super::OPEN_AGENT_TREE),
        (
            "TASKS_GROUPS_AND_PARENT_RELATIONS",
            super::TASKS_GROUPS_AND_PARENT_RELATIONS,
        ),
        ("TERMINALS", super::TERMINALS),
        ("YOUR_WORKSPACE", super::YOUR_WORKSPACE),
        (
            "SEARCH_COMMANDS_TERMINALS_CTRL_K",
            super::SEARCH_COMMANDS_TERMINALS_CTRL_K,
        ),
        ("SEARCH_PILL_PLACEHOLDER", super::SEARCH_PILL_PLACEHOLDER),
        ("NO_BLOCKED_REQUESTS", super::NO_BLOCKED_REQUESTS),
        (
            "ALL_TERMINALS_ARE_CLEAR_OF_REPORTED_REQUESTS",
            super::ALL_TERMINALS_ARE_CLEAR_OF_REPORTED_REQUESTS,
        ),
        (
            "LOCAL_GIT_REPOSITORY_PATH",
            super::LOCAL_GIT_REPOSITORY_PATH,
        ),
        ("OPENING", super::OPENING),
        ("OPEN", super::OPEN),
        (
            "CONNECTION_CLOSED_RETRY_WHEN_CONNECTED",
            super::CONNECTION_CLOSED_RETRY_WHEN_CONNECTED,
        ),
        ("JUMP_TO_WORK", super::JUMP_TO_WORK),
        (
            "SEARCH_TASKS_TERMINALS_OR_COMMANDS",
            super::SEARCH_TASKS_TERMINALS_OR_COMMANDS,
        ),
        (
            "NO_MATCHING_ITEMS_CLEAR_THE_SEARCH_TO_SHOW_ALL_COMMANDS",
            super::NO_MATCHING_ITEMS_CLEAR_THE_SEARCH_TO_SHOW_ALL_COMMANDS,
        ),
        ("NOT_REPORTED", super::NOT_REPORTED),
        (
            "SETTINGS_COULD_NOT_BE_SAVED",
            super::SETTINGS_COULD_NOT_BE_SAVED,
        ),
        ("PODD_DISCONNECTED", super::PODD_DISCONNECTED),
        ("PODD_RECONNECTED", super::PODD_RECONNECTED),
        ("COULD_NOT_SAVE_LAYOUT", super::COULD_NOT_SAVE_LAYOUT),
        ("KILL_TERMINAL", super::KILL_TERMINAL),
        ("CANCEL", super::CANCEL),
        ("KILL", super::KILL),
        ("RENAME", super::RENAME),
        ("RENAME_FAILED", super::RENAME_FAILED),
        ("WORKING_DIRECTORY", super::WORKING_DIRECTORY),
        ("OPTIONAL_E_G_REVIEW_BOT", super::OPTIONAL_E_G_REVIEW_BOT),
        ("PROGRAM", super::PROGRAM),
        ("COMMAND", super::COMMAND),
        ("NAME", super::NAME),
        ("RUN", super::RUN),
        ("PODD_CLIENT_STOPPED", super::PODD_CLIENT_STOPPED),
        (
            "REVIEW_NEEDS_A_NEWER_PODD",
            super::REVIEW_NEEDS_A_NEWER_PODD,
        ),
        (
            "PODD_PROTOCOL_V5_REVIEW_IS_REQUIRED",
            super::PODD_PROTOCOL_V5_REVIEW_IS_REQUIRED,
        ),
        ("NO_SUCH_TASK", super::NO_SUCH_TASK),
        ("NO_TASK_TO_REVIEW", super::NO_TASK_TO_REVIEW),
        ("CREATE_A_TASK_FIRST_TASK", super::CREATE_A_TASK_FIRST_TASK),
        (
            "TASK_NAME_E_G_FIX_LOGIN_BRANCH_POD_NAME",
            super::TASK_NAME_E_G_FIX_LOGIN_BRANCH_POD_NAME,
        ),
        (
            "BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH",
            super::BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH,
        ),
        (
            "OPTIONAL_FIRST_PROMPT_CLAUDE_CODEX",
            super::OPTIONAL_FIRST_PROMPT_CLAUDE_CODEX,
        ),
        ("ADD_REPOSITORY", super::ADD_REPOSITORY),
        ("SHELL", super::SHELL),
        ("REPOSITORY", super::REPOSITORY),
        ("BASE", super::BASE),
        ("AGENT", super::AGENT),
        ("PROMPT", super::PROMPT),
        ("CREATING", super::CREATING),
        ("CREATE", super::CREATE),
        ("ENTER_A_TASK_NAME", super::ENTER_A_TASK_NAME),
        (
            "CHOOSE_A_REPOSITORY_OR_ENTER_ITS_PATH",
            super::CHOOSE_A_REPOSITORY_OR_ENTER_ITS_PATH,
        ),
        (
            "CONNECTION_CLOSED_WHILE_ADDING_REPOSITORY",
            super::CONNECTION_CLOSED_WHILE_ADDING_REPOSITORY,
        ),
        (
            "CONNECTION_CLOSED_WHILE_CREATING_TASK_CHECK_THE_TASK_LIST_BEFORE_",
            super::CONNECTION_CLOSED_WHILE_CREATING_TASK_CHECK_THE_TASK_LIST_BEFORE_,
        ),
        (
            "THIS_TASK_NO_LONGER_EXISTS",
            super::THIS_TASK_NO_LONGER_EXISTS,
        ),
        ("MISSING", super::MISSING),
        ("LAST_COMMIT", super::LAST_COMMIT),
        ("DELETE_BRANCH", super::DELETE_BRANCH),
        ("DELETE_BRANCH_94", super::DELETE_BRANCH_94),
        ("REVIEW_CHANGES", super::REVIEW_CHANGES),
        ("CLOSE", super::CLOSE),
        ("REMOVE_TASK", super::REMOVE_TASK),
        ("REMOVE_ANYWAY", super::REMOVE_ANYWAY),
        (
            "PODD_REFUSED_TO_REMOVE_THE_TASK",
            super::PODD_REFUSED_TO_REMOVE_THE_TASK,
        ),
        (
            "REMOVING_ANYWAY_DELETES_THE_WORKTREE_WITH_ITS_UNCOMMITTED_CHANGES",
            super::REMOVING_ANYWAY_DELETES_THE_WORKTREE_WITH_ITS_UNCOMMITTED_CHANGES,
        ),
        ("KEEP_TASK", super::KEEP_TASK),
        ("GRID", super::GRID),
        (
            "ARRANGE_OPEN_PANELS_IN_A_GRID_CTRL_SHIFT_G",
            super::ARRANGE_OPEN_PANELS_IN_A_GRID_CTRL_SHIFT_G,
        ),
        ("TASK_107", super::TASK_107),
        (
            "NEW_TASK_GIT_WORKTREE_AGENT_TERMINAL",
            super::NEW_TASK_GIT_WORKTREE_AGENT_TERMINAL,
        ),
        ("NEW", super::NEW),
        (
            "NEW_TERMINAL_CTRL_SHIFT_T",
            super::NEW_TERMINAL_CTRL_SHIFT_T,
        ),
        ("CONNECTION_LOST", super::CONNECTION_LOST),
        ("KILL_TERMINAL_112", super::KILL_TERMINAL_112),
        ("REMOVE_EXITED_TERMINAL", super::REMOVE_EXITED_TERMINAL),
        ("PODD_CONNECTING", super::PODD_CONNECTING),
        ("PODD_CONNECTED", super::PODD_CONNECTED),
        ("STATUS", super::STATUS),
        ("TASK_DETAILS", super::TASK_DETAILS),
        ("ADD_SHELL_TERMINAL", super::ADD_SHELL_TERMINAL),
        (
            "CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED",
            super::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED,
        ),
        (
            "A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK",
            super::A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK,
        ),
        (
            "CONNECTING_TO_YOUR_WORKSPACE",
            super::CONNECTING_TO_YOUR_WORKSPACE,
        ),
        ("EMPTY_NO_TASK_TITLE", super::EMPTY_NO_TASK_TITLE),
        ("EMPTY_NO_TASK", super::EMPTY_NO_TASK),
        ("EMPTY_PICK_TASK_TITLE", super::EMPTY_PICK_TASK_TITLE),
        ("EMPTY_PICK_TASK", super::EMPTY_PICK_TASK),
        (
            "OPEN_A_REPOSITORY_CREATE_A_TASK_OR_RETURN_TO_AN_EXISTING_TERMINAL",
            super::OPEN_A_REPOSITORY_CREATE_A_TASK_OR_RETURN_TO_AN_EXISTING_TERMINAL,
        ),
        (
            "YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_",
            super::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_,
        ),
        ("NOT_DELIVERED", super::NOT_DELIVERED),
        (
            "OPTIONAL_INSTRUCTIONS_FOR_THE_AGENT_SENT_BEFORE_THE_COMMENTS",
            super::OPTIONAL_INSTRUCTIONS_FOR_THE_AGENT_SENT_BEFORE_THE_COMMENTS,
        ),
        ("TASK_HAS_NO_WORKTREE", super::TASK_HAS_NO_WORKTREE),
        (
            "COULD_NOT_READ_REVIEW_COMMENTS",
            super::COULD_NOT_READ_REVIEW_COMMENTS,
        ),
        ("ADD_COMMENT_152", super::ADD_COMMENT_152),
        (
            "SELECT_LINES_CLICK_A_LINE_NUMBER_SHIFT_OR_POINT_AT_A_LINE_FIRST",
            super::SELECT_LINES_CLICK_A_LINE_NUMBER_SHIFT_OR_POINT_AT_A_LINE_FIRST,
        ),
        ("COMMENT_NOT_SAVED", super::COMMENT_NOT_SAVED),
        ("COULD_NOT_REMOVE_COMMENT", super::COULD_NOT_REMOVE_COMMENT),
        ("REVIEW_UPDATE_FAILED", super::REVIEW_UPDATE_FAILED),
        ("CANCEL_FAILED", super::CANCEL_FAILED),
        ("SPLIT", super::SPLIT),
        (
            "SIDE_BY_SIDE_NEEDS_A_WIDE_PANEL_HIDE_FILES_COMMENTS",
            super::SIDE_BY_SIDE_NEEDS_A_WIDE_PANEL_HIDE_FILES_COMMENTS,
        ),
        ("INLINE", super::INLINE),
        ("EXPAND", super::EXPAND),
        ("COLLAPSE", super::COLLAPSE),
        (
            "SHOW_OR_FOLD_UNCHANGED_LINES",
            super::SHOW_OR_FOLD_UNCHANGED_LINES,
        ),
        ("WRAP", super::WRAP),
        ("WRAP_169", super::WRAP_169),
        ("SOFT_WRAP_LONG_LINES", super::SOFT_WRAP_LONG_LINES),
        (
            "PREVIOUS_CHANGE_SHIFT_F7_ALT",
            super::PREVIOUS_CHANGE_SHIFT_F7_ALT,
        ),
        ("NEXT_CHANGE_F7_ALT", super::NEXT_CHANGE_F7_ALT),
        ("PREVIOUS_FILE_ALT_PGUP", super::PREVIOUS_FILE_ALT_PGUP),
        ("NEXT_FILE_ALT_PGDN", super::NEXT_FILE_ALT_PGDN),
        ("FILES_175", super::FILES_175),
        ("FILES_176", super::FILES_176),
        (
            "SHOW_OR_HIDE_THE_FILE_LIST",
            super::SHOW_OR_HIDE_THE_FILE_LIST,
        ),
        ("COMMENTS", super::COMMENTS),
        ("COMMENTS_179", super::COMMENTS_179),
        (
            "SHOW_OR_HIDE_THE_COMMENTS_PANE",
            super::SHOW_OR_HIDE_THE_COMMENTS_PANE,
        ),
        (
            "RE_READ_CHANGES_COMMENTS_AND_DELIVERIES_F5",
            super::RE_READ_CHANGES_COMMENTS_AND_DELIVERIES_F5,
        ),
        ("WILL_QUEUE_UNTIL_IDLE", super::WILL_QUEUE_UNTIL_IDLE),
        ("SEND_WILL_BE_REFUSED", super::SEND_WILL_BE_REFUSED),
        ("DELIVERS_NOW", super::DELIVERS_NOW),
        (
            "NO_COMMENTS_YET_CLICK_A_LINE_OR_DRAG_LINE_NUMBERS_THEN_TO_COMMENT",
            super::NO_COMMENTS_YET_CLICK_A_LINE_OR_DRAG_LINE_NUMBERS_THEN_TO_COMMENT,
        ),
        ("SEND_TO_AGENT_194", super::SEND_TO_AGENT_194),
        ("SENDING", super::SENDING),
        (
            "REVIEW_SEND_CTRL_SHIFT_ENTER_CHECKED_COMMENTS_ELSE_ALL_OPEN_ONES",
            super::REVIEW_SEND_CTRL_SHIFT_ENTER_CHECKED_COMMENTS_ELSE_ALL_OPEN_ONES,
        ),
        ("REOPEN", super::REOPEN),
        ("RESOLVE", super::RESOLVE),
        ("DELETE", super::DELETE),
        ("LOADING_CHANGES", super::LOADING_CHANGES),
        (
            "NO_CHANGES_AGAINST_THE_TASK_BASE_YET",
            super::NO_CHANGES_AGAINST_THE_TASK_BASE_YET,
        ),
        (
            "CHOOSE_A_REPOSITORY_OR_ENTER_THE_PATH_OF_ONE_TO_REGISTER",
            super::CHOOSE_A_REPOSITORY_OR_ENTER_THE_PATH_OF_ONE_TO_REGISTER,
        ),
        ("ENTER_A_GROUP_NAME", super::ENTER_A_GROUP_NAME),
        (
            "NAMES_USE_LETTERS_DIGITS_AND_ONLY",
            super::NAMES_USE_LETTERS_DIGITS_AND_ONLY,
        ),
        (
            "ENTER_THE_PROMPT_EVERY_AGENT_GETS",
            super::ENTER_THE_PROMPT_EVERY_AGENT_GETS,
        ),
        ("ADD_AT_LEAST_ONE_AGENT", super::ADD_AT_LEAST_ONE_AGENT),
        (
            "AT_MOST_16_MEMBERS_PER_GROUP",
            super::AT_MOST_16_MEMBERS_PER_GROUP,
        ),
        (
            "ENTER_THE_CUSTOM_AGENT_S_COMMAND_LINE",
            super::ENTER_THE_CUSTOM_AGENT_S_COMMAND_LINE,
        ),
        ("GROUPS_NEED_A_NEWER_PODD", super::GROUPS_NEED_A_NEWER_PODD),
        (
            "PODD_PROTOCOL_V6_GROUP_IS_REQUIRED",
            super::PODD_PROTOCOL_V6_GROUP_IS_REQUIRED,
        ),
        (
            "GROUP_NAME_E_G_LOGIN_FIX_MEMBERS_NAME_AGENT_N",
            super::GROUP_NAME_E_G_LOGIN_FIX_MEMBERS_NAME_AGENT_N,
        ),
        (
            "THE_PROMPT_EVERY_AGENT_GETS",
            super::THE_PROMPT_EVERY_AGENT_GETS,
        ),
        (
            "CUSTOM_AGENT_COMMAND_LINE_E_G_AIDER_YES",
            super::CUSTOM_AGENT_COMMAND_LINE_E_G_AIDER_YES,
        ),
        (
            "OPTIONAL_CHECK_COMMAND_E_G_CARGO_TEST_RUNS_VIA_CMD_C_IN_EACH_WORK",
            super::OPTIONAL_CHECK_COMMAND_E_G_CARGO_TEST_RUNS_VIA_CMD_C_IN_EACH_WORK,
        ),
        ("PROMPT_AS_CLI_ARGUMENT", super::PROMPT_AS_CLI_ARGUMENT),
        (
            "POWERSHELL_PROMPT_TYPED_AS_A_COMMAND",
            super::POWERSHELL_PROMPT_TYPED_AS_A_COMMAND,
        ),
        ("CUSTOM", super::CUSTOM),
        (
            "ANY_PROGRAM_ARGV_BELOW_PROMPT_TYPED_IN",
            super::ANY_PROGRAM_ARGV_BELOW_PROMPT_TYPED_IN,
        ),
        ("NO_MEMBERS_YET", super::NO_MEMBERS_YET),
        (
            "NEW_GROUP_FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS",
            super::NEW_GROUP_FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS,
        ),
        ("CHECK_OPTIONAL", super::CHECK_OPTIONAL),
        ("CREATE_GROUP", super::CREATE_GROUP),
        ("COULD_NOT_ADD_REPOSITORY", super::COULD_NOT_ADD_REPOSITORY),
        (
            "CHECK_COMMAND_E_G_CARGO_TEST_CMD_C_IN_EACH_WORKTREE",
            super::CHECK_COMMAND_E_G_CARGO_TEST_CMD_C_IN_EACH_WORKTREE,
        ),
        (
            "ENTER_A_CHECK_COMMAND_FIRST",
            super::ENTER_A_CHECK_COMMAND_FIRST,
        ),
        (
            "SAFE_PODD_REFUSES_A_MEMBER_WITH_UNCOMMITTED_CHANGES_OR_UNPUSHED_C",
            super::SAFE_PODD_REFUSES_A_MEMBER_WITH_UNCOMMITTED_CHANGES_OR_UNPUSHED_C,
        ),
        (
            "ALSO_DELETE_THEIR_BRANCHES",
            super::ALSO_DELETE_THEIR_BRANCHES,
        ),
        ("PICK_WINNER", super::PICK_WINNER),
        ("NO_MEMBERS_REMOVED", super::NO_MEMBERS_REMOVED),
        ("PODD_KEPT_SOME_MEMBERS", super::PODD_KEPT_SOME_MEMBERS),
        (
            "REMOVING_ANYWAY_DELETES_THEIR_WORKTREES_WITH_UNCOMMITTED_CHANGES_",
            super::REMOVING_ANYWAY_DELETES_THEIR_WORKTREES_WITH_UNCOMMITTED_CHANGES_,
        ),
        ("KEEP_THEM", super::KEEP_THEM),
        ("NO_TERMINAL", super::NO_TERMINAL),
        ("CHECK", super::CHECK),
        ("CHECKING", super::CHECKING),
        ("RUN_CHECK", super::RUN_CHECK),
        (
            "GROUP_CHECK_RUN_THE_COMMAND_IN_EVERY_MEMBER_S_WORKTREE_AT_ONCE",
            super::GROUP_CHECK_RUN_THE_COMMAND_IN_EVERY_MEMBER_S_WORKTREE_AT_ONCE,
        ),
        ("CANCEL_CHECK", super::CANCEL_CHECK),
        ("COMPARING", super::COMPARING),
        ("COMPARE", super::COMPARE),
        (
            "GROUP_COMPARE_RE_READ_EVERY_MEMBER_S_DIFF_AGAINST_THE_BASE",
            super::GROUP_COMPARE_RE_READ_EVERY_MEMBER_S_DIFF_AGAINST_THE_BASE,
        ),
        ("NO_CHECK_RUN_YET", super::NO_CHECK_RUN_YET),
        ("NO_DIFF_YET", super::NO_DIFF_YET),
        ("CHECK_RUNNING", super::CHECK_RUNNING),
        ("CHECK_NOT_RUN", super::CHECK_NOT_RUN),
        ("HIDE_OUTPUT", super::HIDE_OUTPUT),
        ("SHOW_OUTPUT", super::SHOW_OUTPUT),
        ("WINNER_279", super::WINNER_279),
        ("TASK_REMOVED", super::TASK_REMOVED),
        (
            "OPEN_THE_MEMBER_S_AGENT_TERMINAL",
            super::OPEN_THE_MEMBER_S_AGENT_TERMINAL,
        ),
        ("REVIEW_284", super::REVIEW_284),
        (
            "OPEN_THE_MEMBER_S_REVIEW_PANEL_DIFF_COMMENTS",
            super::OPEN_THE_MEMBER_S_REVIEW_PANEL_DIFF_COMMENTS,
        ),
        ("COULD_NOT_LOAD_THE_GROUP", super::COULD_NOT_LOAD_THE_GROUP),
        ("LOADING_GROUP", super::LOADING_GROUP),
        ("EXPIRING_NOW", super::EXPIRING_NOW),
        (
            "NO_TERMINAL_FOR_THIS_GATE",
            super::NO_TERMINAL_FOR_THIS_GATE,
        ),
        (
            "ITS_TERMINAL_OR_TASK_IS_GONE",
            super::ITS_TERMINAL_OR_TASK_IS_GONE,
        ),
        (
            "OPTIONAL_NOTE_FOR_THE_AGENT",
            super::OPTIONAL_NOTE_FOR_THE_AGENT,
        ),
        ("NO_EXPIRY", super::NO_EXPIRY),
        ("OPEN_TERMINAL", super::OPEN_TERMINAL),
        ("APPROVE", super::APPROVE),
        ("REJECT", super::REJECT),
        ("GATES", super::GATES),
        ("RE_READ_GATES_GATE_LIST", super::RE_READ_GATES_GATE_LIST),
        ("NO_PENDING_GATES", super::NO_PENDING_GATES),
        ("NOT_RUNNING_IN_PODD", super::NOT_RUNNING_IN_PODD),
        (
            "TASKS_AND_GROUPS_STARTED_FROM_INSIDE_A_POD_TERMINAL_APPEAR_UNDER_",
            super::TASKS_AND_GROUPS_STARTED_FROM_INSIDE_A_POD_TERMINAL_APPEAR_UNDER_,
        ),
        ("NO_AGENTS_YET", super::NO_AGENTS_YET),
        (
            "CREATE_A_TASK_OR_A_GROUP_TO_SEE_IT_HERE",
            super::CREATE_A_TASK_OR_A_GROUP_TO_SEE_IT_HERE,
        ),
        ("NO_ORIGIN_RECORDED", super::NO_ORIGIN_RECORDED),
        (
            "OPEN_THE_GATES_INBOX_STATUS_BAR_CTRL_SHIFT_I",
            super::OPEN_THE_GATES_INBOX_STATUS_BAR_CTRL_SHIFT_I,
        ),
        ("GATES_NEED_A_NEWER_PODD", super::GATES_NEED_A_NEWER_PODD),
        (
            "PODD_PROTOCOL_V6_IS_REQUIRED",
            super::PODD_PROTOCOL_V6_IS_REQUIRED,
        ),
        ("GROUP_330", super::GROUP_330),
        (
            "FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS_CTRL_SHIFT_N",
            super::FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS_CTRL_SHIFT_N,
        ),
        (
            "AGENT_TREE_GROUPS_TASKS_AND_TERMINALS_STARTED_FROM_OTHER_TERMINAL",
            super::AGENT_TREE_GROUPS_TASKS_AND_TERMINALS_STARTED_FROM_OTHER_TERMINAL,
        ),
        ("NO_GATES", super::NO_GATES),
        ("OPEN_IN_ANOTHER_PANEL", super::OPEN_IN_ANOTHER_PANEL),
        ("RENAME_336", super::RENAME_336),
        ("CLOSE_PANEL_DETACH", super::CLOSE_PANEL_DETACH),
        (
            "PODD_DISCONNECTED_REATTACHING",
            super::PODD_DISCONNECTED_REATTACHING,
        ),
        ("TERMINAL_REMOVED", super::TERMINAL_REMOVED),
        ("INSPECTOR_EMPTY", super::INSPECTOR_EMPTY),
        ("CTRL_K", super::CTRL_K),
        ("FUZZY_SAMPLE", super::FUZZY_SAMPLE),
        ("FUZZY_QUERY", super::FUZZY_QUERY),
        ("SIDEBAR_SEARCH", super::SIDEBAR_SEARCH),
        ("COUNT_WORKING", super::COUNT_WORKING),
        ("COUNT_BLOCKED", super::COUNT_BLOCKED),
        ("ERROR", super::ERROR),
        ("COUNT_UNKNOWN", super::COUNT_UNKNOWN),
        ("IDLE", super::IDLE),
        ("DONE", super::DONE),
        ("INFERRED", super::INFERRED),
        ("SUMMARY_SAMPLE", super::SUMMARY_SAMPLE),
        ("SUBMIT_PENDING", super::SUBMIT_PENDING),
        ("SUBMIT_CONFIRMED", super::SUBMIT_CONFIRMED),
        ("SUBMIT_UNCONFIRMED", super::SUBMIT_UNCONFIRMED),
        (
            "REVIEW_SUBMIT_UNCONFIRMED",
            super::REVIEW_SUBMIT_UNCONFIRMED,
        ),
        (
            "CHECKED_COMMENTS_ALL_RESOLVED",
            super::CHECKED_COMMENTS_ALL_RESOLVED,
        ),
        ("PRESS_ENTER", super::PRESS_ENTER),
        ("FILE_CHANGED_ON_DISK", super::FILE_CHANGED_ON_DISK),
        ("QUICK_OPEN_FILE", super::QUICK_OPEN_FILE),
        ("QUICK_OPEN_FILE_DETAIL", super::QUICK_OPEN_FILE_DETAIL),
        ("FILES_PANEL", super::FILES_PANEL),
        ("FILES_PANEL_DETAIL", super::FILES_PANEL_DETAIL),
        ("SAVE_ALL_FILES", super::SAVE_ALL_FILES),
        ("SAVE_ALL_FILES_DETAIL", super::SAVE_ALL_FILES_DETAIL),
        ("OPEN_IN_EDITOR", super::OPEN_IN_EDITOR),
        ("OPEN_IN_EDITOR_SHORT", super::OPEN_IN_EDITOR_SHORT),
        ("OPEN_IN_EDITOR_TIP", super::OPEN_IN_EDITOR_TIP),
        ("OPEN_FILES", super::OPEN_FILES),
        ("NO_TASK_FOR_FILES", super::NO_TASK_FOR_FILES),
        ("PICK_A_TASK_FIRST", super::PICK_A_TASK_FIRST),
        ("OPEN_LINK_FROM_TERMINAL", super::OPEN_LINK_FROM_TERMINAL),
        ("FILTER_LINKS", super::FILTER_LINKS),
        (
            "OPEN_LINK_FROM_TERMINAL_DETAIL",
            super::OPEN_LINK_FROM_TERMINAL_DETAIL,
        ),
        ("LINK_KIND_FILE", super::LINK_KIND_FILE),
        ("LINK_KIND_URL", super::LINK_KIND_URL),
        ("NO_TERMINAL_FOR_LINKS", super::NO_TERMINAL_FOR_LINKS),
        ("PICK_A_TERMINAL_FIRST", super::PICK_A_TERMINAL_FIRST),
        ("NO_LINKS_IN_VIEW", super::NO_LINKS_IN_VIEW),
        ("NO_LINKS_IN_VIEW_BODY", super::NO_LINKS_IN_VIEW_BODY),
        ("LINK_FILE_NOT_FOUND", super::LINK_FILE_NOT_FOUND),
        ("LINK_OUTSIDE_TASKS", super::LINK_OUTSIDE_TASKS),
        ("LINK_OPEN_FAILED", super::LINK_OPEN_FAILED),
        (
            "SEND_CTRL_P_TO_CURRENT_TERMINAL",
            super::SEND_CTRL_P_TO_CURRENT_TERMINAL,
        ),
        ("LSP_MENU_RESTART", super::LSP_MENU_RESTART),
        ("LSP_MENU_OFF", super::LSP_MENU_OFF),
        ("LSP_MENU_LOG", super::LSP_MENU_LOG),
        ("LSP_OFF_LABEL", super::LSP_OFF_LABEL),
        ("LSP_TURNED_OFF", super::LSP_TURNED_OFF),
        ("LSP_TURNED_OFF_BODY", super::LSP_TURNED_OFF_BODY),
        ("LSP_LOG_TITLE", super::LSP_LOG_TITLE),
        ("LSP_LOG_EMPTY", super::LSP_LOG_EMPTY),
        ("LSP_STATUS_TOOLTIP", super::LSP_STATUS_TOOLTIP),
        ("HOOKS_MISSING_TITLE", super::HOOKS_MISSING_TITLE),
        ("HOOKS_OUTDATED_TITLE", super::HOOKS_OUTDATED_TITLE),
        ("HOOKS_INSTALL", super::HOOKS_INSTALL),
        ("HOOKS_INSTALL_TOOLTIP", super::HOOKS_INSTALL_TOOLTIP),
        ("HOOKS_LATER", super::HOOKS_LATER),
        ("HOOKS_LATER_TOOLTIP", super::HOOKS_LATER_TOOLTIP),
        ("HOOKS_CHECKING", super::HOOKS_CHECKING),
        ("HOOKS_PREVIEW_TITLE", super::HOOKS_PREVIEW_TITLE),
        ("HOOKS_PREVIEW_INTRO", super::HOOKS_PREVIEW_INTRO),
        ("HOOKS_PREVIEW_BACKUP", super::HOOKS_PREVIEW_BACKUP),
        (
            "HOOKS_PREVIEW_CODEX_TRUST",
            super::HOOKS_PREVIEW_CODEX_TRUST,
        ),
        ("HOOKS_INSTALL_CONFIRM", super::HOOKS_INSTALL_CONFIRM),
        ("HOOKS_INSTALLED", super::HOOKS_INSTALLED),
        (
            "HOOKS_INSTALLED_NO_BACKUP",
            super::HOOKS_INSTALLED_NO_BACKUP,
        ),
        ("HOOKS_INSTALL_FAILED", super::HOOKS_INSTALL_FAILED),
        ("HOOKS_NO_CLI", super::HOOKS_NO_CLI),
        ("HOOKS_NO_CLI_BODY", super::HOOKS_NO_CLI_BODY),
        ("HOOKS_FILE_CREATE", super::HOOKS_FILE_CREATE),
        ("HOOKS_FILE_UPDATE", super::HOOKS_FILE_UPDATE),
        ("HOOKS_FILE_UNCHANGED", super::HOOKS_FILE_UNCHANGED),
        ("TASK_REMOVING_SAVED", super::TASK_REMOVING_SAVED),
        ("TERMINAL_LAGGED", super::TERMINAL_LAGGED),
        ("INSTALL_AGENT_HOOKS", super::INSTALL_AGENT_HOOKS),
        (
            "INSTALL_AGENT_HOOKS_DETAIL",
            super::INSTALL_AGENT_HOOKS_DETAIL,
        ),
        ("UNSAVED_FILES_TITLE", super::UNSAVED_FILES_TITLE),
        ("SAVE_ALL_AND_QUIT", super::SAVE_ALL_AND_QUIT),
        ("QUIT_WITHOUT_SAVING", super::QUIT_WITHOUT_SAVING),
        ("REMOVE_WITHOUT_SAVING", super::REMOVE_WITHOUT_SAVING),
        ("LSP_SUSPENDED", super::LSP_SUSPENDED),
        ("RETRY", super::RETRY),
        ("APPEARANCE", super::APPEARANCE),
        ("SETTINGS_GENERAL", super::SETTINGS_GENERAL),
        ("SETTINGS_FONTS", super::SETTINGS_FONTS),
        ("SETTINGS_CATEGORIES", super::SETTINGS_CATEGORIES),
        ("REDUCE_MOTION", super::REDUCE_MOTION),
        ("HIGH_CONTRAST_PREVIEW", super::HIGH_CONTRAST_PREVIEW),
        ("OS_NOTIFICATIONS", super::OS_NOTIFICATIONS),
        ("THEME_STYLE", super::THEME_STYLE),
        ("THEME_ACCENT", super::THEME_ACCENT),
        ("THEME_LAYOUT", super::THEME_LAYOUT),
        ("THEME_DEFAULT", super::THEME_DEFAULT),
        ("THEME_ACCENT_FIXED", super::THEME_ACCENT_FIXED),
        ("LAYOUT_NOT_READY", super::LAYOUT_NOT_READY),
        ("THEME_FILE_SKIPPED", super::THEME_FILE_SKIPPED),
        ("AGENT_MARK_SKIPPED", super::AGENT_MARK_SKIPPED),
        ("THEME_LOW_CONTRAST", super::THEME_LOW_CONTRAST),
        ("THEME_PRESETS", super::THEME_PRESETS),
        ("THEME_PRESET_DETAIL", super::THEME_PRESET_DETAIL),
        ("THEME_MODE_DETAIL", super::THEME_MODE_DETAIL),
        ("THEME_RELOAD", super::THEME_RELOAD),
        ("THEME_RELOAD_DETAIL", super::THEME_RELOAD_DETAIL),
        ("WORKING", super::WORKING),
        ("BLOCKED", super::BLOCKED),
        ("UNKNOWN", super::UNKNOWN),
        ("LIVE", super::LIVE),
        ("KILLED", super::KILLED),
        ("LOST", super::LOST),
        ("UNREACHABLE", super::UNREACHABLE),
        ("APPROVED", super::APPROVED),
        ("REJECTED", super::REJECTED),
        ("EXPIRED", super::EXPIRED),
        ("RECENT", super::RECENT),
        ("RESULTS", super::RESULTS),
        ("CLEAR_SEARCH", super::CLEAR_SEARCH),
        ("GATE_HELP", super::GATE_HELP),
        ("REPO_PATH", super::REPO_PATH),
        ("COMMAND_EXAMPLE", super::COMMAND_EXAMPLE),
        ("ORIGINAL", super::ORIGINAL),
        ("MODIFIED", super::MODIFIED),
        ("UNRESOLVED", super::UNRESOLVED),
        ("QUEUED", super::QUEUED),
        ("SENT", super::SENT),
        ("RESOLVED", super::RESOLVED),
        ("EXITED", super::EXITED),
        ("REMOVED", super::REMOVED),
        ("REFRESHING", super::REFRESHING),
        ("BINARY", super::BINARY),
        ("EOL_ONLY", super::EOL_ONLY),
        ("DELIVERED", super::DELIVERED),
        ("ATTACHING", super::ATTACHING),
    ]
    .into_iter()
    .map(|(key, text)| (key, text.to_string()))
    .collect();
    check("constants", &rows, 370, CONSTANTS_PIN);
}

#[test]
fn templates_keep_their_bytes() {
    let id = "⟪id⟫";
    let line = "⟪line⟫";
    let passed = "⟪passed⟫";
    let failed = "⟪failed⟫";
    let s = "⟪s⟫";
    let error = "⟪error⟫";
    let pid = "⟪pid⟫";
    let title = "⟪title⟫";
    let why = "⟪why⟫";
    let e = "⟪e⟫";
    let attempt = "⟪attempt⟫";
    let n = "⟪n⟫";
    let short = "⟪short⟫";
    let hint = "⟪hint⟫";
    let resolved = "⟪resolved⟫";
    let label = "⟪label⟫";
    let task = "⟪task⟫";
    let group = "⟪group⟫";
    let base = "⟪base⟫";
    let members = "⟪members⟫";
    let w = "⟪w⟫";
    let m = "⟪m⟫";
    let more = "⟪more⟫";
    let secs = 2.5_f64;
    let t = "⟪t⟫";
    let gr = "⟪gr⟫";
    let expiry = "⟪expiry⟫";
    let c = "⟪c⟫";
    let rows: [(&str, String); 143] = [
        (
            "TASK_REMOVING_SAVED_BODY",
            crate::tr!(TASK_REMOVING_SAVED_BODY; ; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        (
            "HOOKS_VERSION_BODY",
            crate::tr!(HOOKS_VERSION_BODY; ; "⟪0⟫", "⟪1⟫"),
        ),
        (
            "HOOKS_MISSING_BODY",
            crate::tr!(HOOKS_MISSING_BODY; ; "⟪0⟫"),
        ),
        (
            "HOOKS_OUTDATED_BODY",
            crate::tr!(HOOKS_OUTDATED_BODY; ; "⟪0⟫"),
        ),
        (
            "HOOKS_PREVIEW_BINARY",
            crate::tr!(HOOKS_PREVIEW_BINARY; ; "⟪0⟫"),
        ),
        (
            "HOOKS_INSTALLED_BACKUPS",
            crate::tr!(HOOKS_INSTALLED_BACKUPS; ; "⟪0⟫"),
        ),
        ("LSP_LOG_BODY", crate::tr!(LSP_LOG_BODY; ; "⟪0⟫", "⟪1⟫")),
        ("FILES_TAB", crate::tr!(FILES_TAB; ; "⟪0⟫", "⟪1⟫")),
        ("UNSAVED_COUNT", crate::tr!(UNSAVED_COUNT; ; "⟪0⟫")),
        (
            "FILE_CHANGED_ON_DISK_BODY",
            crate::tr!(FILE_CHANGED_ON_DISK_BODY; ; "⟪0⟫", "⟪1⟫"),
        ),
        (
            "UNSAVED_FILES_BODY",
            crate::tr!(UNSAVED_FILES_BODY; ; "⟪0⟫", "⟪1⟫"),
        ),
        (
            "REMOVE_TASK_UNSAVED_BODY",
            crate::tr!(REMOVE_TASK_UNSAVED_BODY; ; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        (
            "LSP_SUSPENDED_BODY",
            crate::tr!(LSP_SUSPENDED_BODY; ; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        ("LSP_MORE", crate::tr!(LSP_MORE; ; "⟪0⟫", "⟪1⟫")),
        (
            "NO_WORKTREE_FOR_FILES",
            crate::tr!(NO_WORKTREE_FOR_FILES; ; "⟪0⟫"),
        ),
        (
            "CHECK_AGENT_INPUT",
            crate::tr!(CHECK_AGENT_INPUT; id; "⟪0⟫"),
        ),
        (
            "CHECK_PROGRESS_REPORT",
            crate::tr!(CHECK_PROGRESS_REPORT; line, passed, failed; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        (
            "CHECK_PASSED_REPORT",
            crate::tr!(CHECK_PASSED_REPORT; line, passed; "⟪0⟫"),
        ),
        (
            "CHECK_FAILED_REPORT",
            crate::tr!(CHECK_FAILED_REPORT; line, failed, passed; "⟪0⟫"),
        ),
        (
            "CHECK_CANCELLED_REPORT",
            crate::tr!(CHECK_CANCELLED_REPORT; line; "⟪0⟫"),
        ),
        (
            "CHECK_INTERRUPTED_REPORT",
            crate::tr!(CHECK_INTERRUPTED_REPORT; line; "⟪0⟫"),
        ),
        ("PICK_RESULT", crate::tr!(PICK_RESULT; ; "⟪0⟫", "⟪1⟫")),
        (
            "DURATION_MINUTES",
            crate::tr!(DURATION_MINUTES; ; "⟪0⟫", "⟪1⟫"),
        ),
        ("DURATION_HOURS", crate::tr!(DURATION_HOURS; ; "⟪0⟫", "⟪1⟫")),
        ("DURATION_DAYS", crate::tr!(DURATION_DAYS; ; "⟪0⟫", "⟪1⟫")),
        ("DURATION_SECONDS", crate::tr!(DURATION_SECONDS; s; )),
        (
            "UP_DOWN_NAVIGATE_ENTER_OPEN_ESC_CLOSE_RESULTS",
            crate::tr!(UP_DOWN_NAVIGATE_ENTER_OPEN_ESC_CLOSE_RESULTS; ; "⟪0⟫"),
        ),
        ("S_AGO", crate::tr!(S_AGO; ; "⟪0⟫")),
        (
            "RECONNECTING_ERROR",
            crate::tr!(RECONNECTING_ERROR; error; ),
        ),
        (
            "DAEMON_PID_PID_PANELS_RE_ATTACH",
            crate::tr!(DAEMON_PID_PID_PANELS_RE_ATTACH; pid; ),
        ),
        ("TITLE_NEEDS_YOU", crate::tr!(TITLE_NEEDS_YOU; title; )),
        (
            "BLOCKED_ON_WHY_TURN_AT",
            crate::tr!(BLOCKED_ON_WHY_TURN_AT; why; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        ("TITLE_IS_DONE", crate::tr!(TITLE_IS_DONE; title; )),
        ("TURN", crate::tr!(TURN; ; "⟪0⟫", "⟪1⟫", "⟪2⟫")),
        ("TITLE_EXITED", crate::tr!(TITLE_EXITED; title; )),
        (
            "KILL_THE_WHOLE_PROCESS_TREE_ENDS_PANELS_STAY_OPEN_SHOWING_THE_FIN",
            crate::tr!(KILL_THE_WHOLE_PROCESS_TREE_ENDS_PANELS_STAY_OPEN_SHOWING_THE_FIN; ; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        ("KILL_ID_FAILED", crate::tr!(KILL_ID_FAILED; id; )),
        ("REMOVE_ID_FAILED", crate::tr!(REMOVE_ID_FAILED; id; )),
        ("RENAME_ID", crate::tr!(RENAME_ID; id; )),
        ("COULD_NOT_START", crate::tr!(COULD_NOT_START; ; "⟪0⟫")),
        ("TASK_CREATED", crate::tr!(TASK_CREATED; ; "⟪0⟫")),
        (
            "COULD_NOT_CREATE_TASK_TITLE",
            crate::tr!(COULD_NOT_CREATE_TASK_TITLE; title; "⟪0⟫"),
        ),
        ("TASK_ID", crate::tr!(TASK_ID; id; )),
        ("TASK", crate::tr!(TASK; ; "⟪0⟫")),
        ("UNAVAILABLE_E", crate::tr!(UNAVAILABLE_E; e; )),
        (
            "AHEAD_BEHIND_DIRTY_UNPUSHED",
            crate::tr!(AHEAD_BEHIND_DIRTY_UNPUSHED; ; "⟪0⟫", "⟪1⟫", "⟪2⟫", "⟪3⟫"),
        ),
        ("TASK_ID_REMOVED", crate::tr!(TASK_ID_REMOVED; id; )),
        ("TERMINALS_100", crate::tr!(TERMINALS_100; ; "⟪0⟫")),
        ("COULD_NOT_REMOVE_ID", crate::tr!(COULD_NOT_REMOVE_ID; id; )),
        (
            "PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT",
            crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT; attempt; ),
        ),
        (
            "TERMINALS_RUNNING_PANELS",
            crate::tr!(TERMINALS_RUNNING_PANELS; ; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        ("WORKING", crate::tr!(WORKING; ; "⟪0⟫")),
        ("BLOCKED", crate::tr!(BLOCKED; ; "⟪0⟫")),
        ("DONE", crate::tr!(DONE; ; "⟪0⟫")),
        ("TASKS", crate::tr!(TASKS; ; "⟪0⟫")),
        ("NOTIFICATIONS_122", crate::tr!(NOTIFICATIONS_122; ; "⟪0⟫")),
        ("TASKS_123", crate::tr!(TASKS_123; ; "⟪0⟫", "⟪1⟫")),
        (
            "PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR",
            crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt, error; ),
        ),
        (
            "N_COMMENT_S_QUEUED_UNTIL_THE_AGENT_IS_IDLE_WAS",
            crate::tr!(N_COMMENT_S_QUEUED_UNTIL_THE_AGENT_IS_IDLE_WAS; n; "⟪0⟫", "⟪1⟫"),
        ),
        (
            "N_COMMENT_S_DELIVERED_TO",
            crate::tr!(N_COMMENT_S_DELIVERED_TO; n; "⟪0⟫", "⟪1⟫", "⟪2⟫"),
        ),
        ("CANCELLED", crate::tr!(CANCELLED; ; "⟪0⟫")),
        ("FAILED", crate::tr!(FAILED; ; "⟪0⟫", "⟪1⟫")),
        ("PREVIEW", crate::tr!(PREVIEW; ; "⟪0⟫")),
        (
            "REFUSED_AGENT_BLOCKED",
            crate::tr!(REFUSED_AGENT_BLOCKED; ; "⟪0⟫"),
        ),
        (
            "REFUSED_AGENT_TERMINAL_EXITED",
            crate::tr!(REFUSED_AGENT_TERMINAL_EXITED; ; "⟪0⟫"),
        ),
        ("NOTHING_TO_SEND", crate::tr!(NOTHING_TO_SEND; ; "⟪0⟫")),
        ("SEND_FAILED", crate::tr!(SEND_FAILED; ; "⟪0⟫")),
        ("REVIEW_FOR", crate::tr!(REVIEW_FOR; ; "⟪0⟫", "⟪1⟫")),
        ("READING_FILES_E", crate::tr!(READING_FILES_E; e; )),
        (
            "WORKTREE_VS_SHORT",
            crate::tr!(WORKTREE_VS_SHORT; short; "⟪0⟫"),
        ),
        (
            "COMMENT_ON_LINE_ENTER_SAVES_SHIFT_ENTER_NEW_LINE",
            crate::tr!(COMMENT_ON_LINE_ENTER_SAVES_SHIFT_ENTER_NEW_LINE; line; "⟪0⟫"),
        ),
        ("REVIEW_SENT_TO", crate::tr!(REVIEW_SENT_TO; ; "⟪0⟫")),
        ("REVIEW_QUEUED_FOR", crate::tr!(REVIEW_QUEUED_FOR; ; "⟪0⟫")),
        (
            "REVIEW_NOT_SENT_TO",
            crate::tr!(REVIEW_NOT_SENT_TO; ; "⟪0⟫"),
        ),
        ("FILES", crate::tr!(FILES; ; "⟪0⟫")),
        ("FILES_182", crate::tr!(FILES_182; ; "⟪0⟫")),
        (
            "SEND_SELECTED_TO_AGENT",
            crate::tr!(SEND_SELECTED_TO_AGENT; ; "⟪0⟫"),
        ),
        (
            "SEND_N_OPEN_TO_AGENT",
            crate::tr!(SEND_N_OPEN_TO_AGENT; n; ),
        ),
        ("AGENT_HINT", crate::tr!(AGENT_HINT; hint; "⟪0⟫", "⟪1⟫")),
        ("AGENT_189", crate::tr!(AGENT_189; ; "⟪0⟫")),
        (
            "COMMENTS_OPEN_TOTAL",
            crate::tr!(COMMENTS_OPEN_TOTAL; ; "⟪0⟫", "⟪1⟫"),
        ),
        (
            "HIDE_RESOLVED_RESOLVED",
            crate::tr!(HIDE_RESOLVED_RESOLVED; resolved; ),
        ),
        (
            "SHOW_RESOLVED_RESOLVED",
            crate::tr!(SHOW_RESOLVED_RESOLVED; resolved; ),
        ),
        (
            "NEW_COMMENT_ON_LABEL",
            crate::tr!(NEW_COMMENT_ON_LABEL; label; "⟪0⟫"),
        ),
        ("NOTE_N", crate::tr!(NOTE_N; n; )),
        ("REVIEW", crate::tr!(REVIEW; ; "⟪0⟫")),
        (
            "COULD_NOT_LOAD_THE_DIFF_E",
            crate::tr!(COULD_NOT_LOAD_THE_DIFF_E; e; ),
        ),
        (
            "MEMBER_TASK_S_ONE_WORKTREE_EACH",
            crate::tr!(MEMBER_TASK_S_ONE_WORKTREE_EACH; ; "⟪0⟫", "⟪1⟫"),
        ),
        ("CREATING_GROUP", crate::tr!(CREATING_GROUP; ; "⟪0⟫")),
        (
            "N_MEMBER_TASK_S_WORKTREE_AGENT_EACH",
            crate::tr!(N_MEMBER_TASK_S_WORKTREE_AGENT_EACH; n; ),
        ),
        ("GROUP_CREATED", crate::tr!(GROUP_CREATED; ; "⟪0⟫")),
        ("MEMBERS_FROM", crate::tr!(MEMBERS_FROM; ; "⟪0⟫", "⟪1⟫")),
        (
            "COULD_NOT_CREATE_GROUP_TITLE",
            crate::tr!(COULD_NOT_CREATE_GROUP_TITLE; title; ),
        ),
        (
            "PICK_TASK_AS_THE_WINNER",
            crate::tr!(PICK_TASK_AS_THE_WINNER; task; ),
        ),
        (
            "RECORDS_TASK_AS_THE_WINNER_OF_GROUP_NOTHING_IS_MERGED_THE_WINNER_",
            crate::tr!(RECORDS_TASK_AS_THE_WINNER_OF_GROUP_NOTHING_IS_MERGED_THE_WINNER_; group, task; ),
        ),
        (
            "REMOVE_THE_OTHER_MEMBERS",
            crate::tr!(REMOVE_THE_OTHER_MEMBERS; ; "⟪0⟫"),
        ),
        ("REMOVED", crate::tr!(REMOVED; ; "⟪0⟫")),
        ("WINNER", crate::tr!(WINNER; ; "⟪0⟫")),
        (
            "COULD_NOT_PICK_TASK",
            crate::tr!(COULD_NOT_PICK_TASK; task; ),
        ),
        (
            "TASK_HAS_NO_TERMINAL_REMOVED",
            crate::tr!(TASK_HAS_NO_TERMINAL_REMOVED; task; ),
        ),
        ("BASE_253", crate::tr!(BASE_253; ; "⟪0⟫", "⟪1⟫")),
        ("GROUP", crate::tr!(GROUP; ; "⟪0⟫")),
        (
            "BASE_MEMBERS_MEMBER_S",
            crate::tr!(BASE_MEMBERS_MEMBER_S; base, members; "⟪0⟫"),
        ),
        ("WINNER_W", crate::tr!(WINNER_W; w; )),
        ("PROMPT_257", crate::tr!(PROMPT_257; ; "⟪0⟫")),
        ("PICK_REFUSED_M", crate::tr!(PICK_REFUSED_M; m; )),
        ("REMOVED_269", crate::tr!(REMOVED_269; ; "⟪0⟫")),
        ("KEPT", crate::tr!(KEPT; ; "⟪0⟫", "⟪1⟫")),
        ("FILE_S", crate::tr!(FILE_S; ; "⟪0⟫")),
        ("DIRTY", crate::tr!(DIRTY; ; "⟪0⟫")),
        ("MORE_MORE", crate::tr!(MORE_MORE; more; )),
        ("CHECK_ERROR_E", crate::tr!(CHECK_ERROR_E; e; )),
        (
            "CHECK_TIMED_OUT_AFTER_SECS_1_S",
            crate::tr!(CHECK_TIMED_OUT_AFTER_SECS_1_S; secs; ),
        ),
        (
            "CHECK_PASSED_EXIT_SECS_1_S",
            crate::tr!(CHECK_PASSED_EXIT_SECS_1_S; secs; "⟪0⟫"),
        ),
        (
            "CHECK_FAILED_EXIT_SECS_1_S",
            crate::tr!(CHECK_FAILED_EXIT_SECS_1_S; secs; "⟪0⟫"),
        ),
        ("GROUP_290", crate::tr!(GROUP_290; ; "⟪0⟫")),
        ("EXPIRES_IN", crate::tr!(EXPIRES_IN; ; "⟪0⟫")),
        (
            "COULD_NOT_RESOLVE_ID",
            crate::tr!(COULD_NOT_RESOLVE_ID; id; ),
        ),
        ("REQUESTED_AGO", crate::tr!(REQUESTED_AGO; ; "⟪0⟫")),
        ("GATES_N", crate::tr!(GATES_N; n; )),
        (
            "PENDING_APPROVALS_306",
            crate::tr!(PENDING_APPROVALS_306; ; "⟪0⟫"),
        ),
        ("RECENT_DECISIONS", crate::tr!(RECENT_DECISIONS; ; "⟪0⟫")),
        ("GROUP_ID", crate::tr!(GROUP_ID; id; )),
        ("TASK_T", crate::tr!(TASK_T; t; )),
        ("GROUP_GR", crate::tr!(GROUP_GR; gr; )),
        ("TERMINAL_T", crate::tr!(TERMINAL_T; t; )),
        (
            "GATE_S_WAITING_FOR_APPROVAL",
            crate::tr!(GATE_S_WAITING_FOR_APPROVAL; ; "⟪0⟫"),
        ),
        ("GATE", crate::tr!(GATE; ; "⟪0⟫", "⟪1⟫")),
        ("CHECK_PASSED", crate::tr!(CHECK_PASSED; ; "⟪0⟫")),
        ("CHECK_FAILED", crate::tr!(CHECK_FAILED; ; "⟪0⟫")),
        ("CHECK_CANCELLED", crate::tr!(CHECK_CANCELLED; ; "⟪0⟫")),
        ("EXPIRES_IN_325", crate::tr!(EXPIRES_IN_325; ; "⟪0⟫")),
        (
            "APPROVAL_REQUESTED",
            crate::tr!(APPROVAL_REQUESTED; ; "⟪0⟫"),
        ),
        (
            "EXPIRY_INBOX",
            crate::tr!(EXPIRY_INBOX; expiry; "⟪0⟫", "⟪1⟫"),
        ),
        ("N_GATE_PENDING", crate::tr!(N_GATE_PENDING; n; "⟪0⟫")),
        ("EXITED_C", crate::tr!(EXITED_C; c; )),
        ("UNREACHABLE_R", crate::tr!(UNREACHABLE_R; ; "⟪0⟫")),
        ("EXITED_C_342", crate::tr!(EXITED_C_342; c; )),
        ("DIRTY_343", crate::tr!(DIRTY_343; ; "⟪0⟫")),
        ("THEME_FOLDER", crate::tr!(THEME_FOLDER; ; "⟪0⟫")),
        (
            "INSPECTOR_TERMINAL",
            crate::tr!(INSPECTOR_TERMINAL; ; "⟪0⟫", "⟪1⟫"),
        ),
        ("INSPECTOR_UPDATED", crate::tr!(INSPECTOR_UPDATED; ; "⟪0⟫")),
        ("ID", crate::tr!(ID; id; "⟪0⟫", "⟪1⟫")),
    ];
    check("templates", &rows, 143, TEMPLATES_PIN);
}

#[test]
fn label_maps_keep_their_bytes() {
    let mut rows: Vec<(&str, String)> = Vec::new();
    for key in [
        "Theme",
        "Font",
        "UI size",
        "Terminal font",
        "Terminal size",
        "Density",
        "Motion",
        "Jump shortcut",
        "system",
        "dark",
        "light",
        "theme",
        "dense",
        "compact",
        "comfortable",
        "accessible",
        "full",
        "reduced",
        "unknown setting",
    ] {
        rows.push(("setting", format!("{key}={}", super::setting_label(key))));
    }
    for key in [
        "branch",
        "base",
        "agent",
        "worktree",
        "terminals",
        "unknown field",
    ] {
        rows.push(("field", format!("{key}={}", super::field_label(key))));
    }
    check("labels", &rows, 25, LABELS_PIN);
}

#[test]
fn toolkit_catalog_keeps_its_bytes() {
    super::init_toolkit_locale();
    let rows: Vec<(&str, String)> = [
        "Attachment.Remove",
        "Attachment.Retry",
        "Select.placeholder",
        "ComboBox.placeholder",
        "ComboBox.search_placeholder",
        "ComboBox.empty",
        "DropdownButton.more_options",
        "Dock.Unnamed",
        "Dock.Close",
        "Dock.PanelMenu",
        "Dock.Zoom In",
        "Dock.Zoom Out",
        "Dock.Collapse",
        "Dock.Expand",
        "Dock.Unavailable Panel",
        "Dialog.ok",
        "Dialog.cancel",
        "Command.placeholder",
        "Command.empty",
        "List.search_placeholder",
        "Input.Decrement",
        "Input.Increment",
        "Input.Replace",
        "Input.Replace All",
        "Input.Clear",
        "Input.Fold",
        "Input.Unfold",
        "Input.Cut",
        "Input.Copy",
        "Input.Paste",
        "Input.Select All",
        "Input.Go to Definition",
        "Input.Show Code Actions",
        "Input.Show Password",
        "Input.Hide Password",
        "Settings.search_placeholder",
        "Settings.no_results",
        "Settings.no_results_description",
        "Settings.Reset All",
        "Pagination.previous",
        "Pagination.next",
        "Diff.BinaryChanges",
        "Diff.NoTextChanges",
        "Diff.NoChanges",
        "Diff.EmptyFile",
        "Diff.AddedFile",
        "Diff.DeletedFile",
        "Diff.RenamedFile",
        "Diff.CopiedFile",
        "Diff.HiddenLines",
        "Diff.ExpandAbove",
        "Diff.ExpandBelow",
        "Diff.CollapseFile",
        "Diff.ExpandFile",
        "Diff.AddAnnotation",
        "Diff.ConflictedFile",
        "Diff.CurrentChange",
        "Diff.IncomingChange",
        "Diff.BaseChange",
        "Diff.AcceptCurrent",
        "Diff.AcceptIncoming",
        "Diff.AcceptBoth",
        "Diff.AcceptedCurrent",
        "Diff.AcceptedIncoming",
        "Diff.AcceptedBoth",
        "Diff.UndoResolution",
        "Diff.Original",
        "Diff.Modified",
        "Diff.ExpandLines",
        "Diff.SourceLine",
        "Diff.AddedLine",
        "Diff.RemovedLine",
        "Diff.UnchangedLines",
        "Diff.SelectLine",
        "Diff.NoNewline",
        "Diff.Viewer",
        "Diff.ViewerFiles",
    ]
    .into_iter()
    .map(|key| {
        (
            key,
            gpui_kit::component::_rust_i18n_translate("ko", key).to_string(),
        )
    })
    .collect();
    check("toolkit", &rows, 77, TOOLKIT_PIN);
}

const CONSTANTS_PIN: u64 = 14671821289662446785;
const TEMPLATES_PIN: u64 = 2279962709339007975;
const LABELS_PIN: u64 = 5616529307963690001;
const TOOLKIT_PIN: u64 = 17072704162408452631;
