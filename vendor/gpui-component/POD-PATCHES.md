# Pod toolkit patch

Source: crates.io gpui-component 0.9.0, Apache-2.0 (LICENSE retained).
Changes: dialog close uses the existing Dock.Close locale key; dock tabs copy
their displayed tab name into the accessible label. No registry cache is edited.
Legacy SlideDown and deferred focus return use the theme motion duration, so
the host reduced-motion preference also suppresses the legacy animation/delay.
Single-panel headers expose their tab name and selection. Dock menu, close,
zoom and collapse controls have names and keyboard tab stops.
Dock panel content bypasses GPUI's view cache while accessibility is active:
GPUI 0.3.11 replays cached paint/hitboxes without accessibility nodes, so partial
frames (including foreground activation) dropped the panel from UIA. The
absolute full-size slot and the cache without an accessibility client remain.
Regression: Pod's `qa/ui-wave0/dock-activation.ps1` and
`tools/visual/uia-required.ps1` check settings content after three activations.
Code-editor fold chevrons (gutter) have names (`Input.Fold` / `Input.Unfold`,
new English locale keys; Pod supplies Korean) for the P3 editor integration.
Dialogs without an `on_ok` handler (custom footers, as every Pod dialog
uses) no longer close on Enter while a control inside them has focus: Enter
propagates to that control, so a focused button gets its keyboard click and a
text field its own Enter (`dialog.rs`, native-QA keyboard flow). Enter on the
dialog surface itself still confirms/closes as before.
`dialog::set_dialog_bounds_observer` (host UI tests only): when set, each
dialog surface reports its painted bounds through a paint-free absolute
`canvas`, so Pod's native-QA tests can check dialog placement under
`cargo test --release` (GPUI's `debug_selector` bounds need `debug_assertions`).
Unset (the app), the dialog renders exactly as before.
Modified files (each starts with a `Pod patch:` change notice, Apache-2.0
4(b); `tools/check-vendor-notices.ps1` checks this list):

- `src/dialog/dialog.rs`
- `src/dock/tab_panel.rs`
- `src/input/input.rs`
- `src/root.rs`
- `locales/ui.yml`

Remove this patch once these fixes are available in the upstream dependency.
