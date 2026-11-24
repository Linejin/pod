# Pod patches to gpui-pre

Source: crates.io gpui-pre 0.3.11, Apache-2.0; upstream LICENSE-APACHE,
README and published package sources retained. No global registry cache edits.
Only Cargo's extraction marker `.cargo-ok` is omitted from the published copy.

`aria_readonly(bool)` writes or clears AccessKit's read-only flag on the
existing element's node. No additional node, role or disabled state is created.

`Hitbox::native_child_bounds` reads the last rendered frame so an application
hosting a native child window can match its bounds to clipping and hide it when
it is covered by an interactive GPUI overlay or removed from the frame.

Modified files (Apache-2.0 4(b) notices; checked by tools/check-vendor-notices.ps1):

- `src/elements/div.rs`
- `src/window.rs`

Regression: pod-editor/tests/save_ui.rs checks the public node attribute
transition and actual input-node flags, actions and editing policy.
Remove this patch when the upstream dependency exposes this state.
