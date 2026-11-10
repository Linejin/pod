// Shared by integration tests that need an external tool (codex, ssh,
// rust-analyzer, a built pod-hook.exe, ...). Included with
// `#[path = "../../../tools/test-support/require_tools.rs"] mod require_tools;`.
//
// Without `POD_REQUIRE_TOOLS=1` a missing tool skips the check with a note on
// stderr (developer machines). With it (CI) a missing tool fails the test, so
// a green run never hides an unchecked path (quality-07).

/// Name of the environment variable that turns skips into failures.
#[allow(dead_code)]
pub const ENV: &str = "POD_REQUIRE_TOOLS";

/// Call where a test would skip because `what` is unavailable. Returns
/// normally (caller skips) unless `POD_REQUIRE_TOOLS=1`, then panics.
#[allow(dead_code)]
pub fn missing(what: &str) {
    missing_with(what, std::env::var(ENV).ok().as_deref());
}

fn missing_with(what: &str, require: Option<&str>) {
    if require.is_some_and(|v| v.trim() == "1") {
        panic!("{what} is unavailable and {ENV}=1: refusing to skip");
    }
    eprintln!("{what} is unavailable: skipped (set {ENV}=1 to fail instead)");
}

#[test]
fn require_tools_skips_without_env() {
    missing_with("x", None);
    missing_with("x", Some("0"));
    missing_with("x", Some(""));
}

#[test]
#[should_panic(expected = "POD_REQUIRE_TOOLS=1")]
fn require_tools_fails_with_env() {
    missing_with("x", Some("1"));
}
