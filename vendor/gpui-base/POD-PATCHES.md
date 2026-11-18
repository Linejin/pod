# Pod input accessibility patch

Source: crates.io gpui-base 0.9.0, Apache-2.0; upstream LICENSE-APACHE,
README and published package sources retained. No global registry cache edits.
Only Cargo's extraction marker `.cargo-ok` is omitted from the published copy.

The actual input node now projects `self.readonly` every render through
GPUI's `aria_readonly` attribute. This preserves node identity, focus and the
existing SetValue/input guards; a safe reload clears read-only state.

Modified files (Apache-2.0 4(b) notices; checked by tools/check-vendor-notices.ps1):

- `src/input/base/state.rs`

Regression: pod-editor/tests/save_ui.rs observes the actual emitted input
node and preserves native-input, dirty-state, exact-disk and reload checks;
gpui-component's existing accessibility action test also asserts the flag.
Remove this patch when the upstream dependency exposes this state.
