# LiteLAN v0.1.13

Overlay `src/main.rs` and `Cargo.toml` on the existing v0.1.12 project. Retain the existing assets, core patches, build scripts, GitHub workflow and README.

- Navigation: completed the Connect user-plus contour; consistent vector stroke and rounded curves.
- Device actions: Quick actions, Connection (only when applicable), Device management and Danger zone. Separate copy-ID icon, rounded retry and delete icons. Both removal descriptions are inside the respective outlined clickable rows. Remove without blocking and block confirmation retain their existing handlers.
- Connect: Create invitation remains the only invitation-creation action. The link is automatically copied; removed the manual device link button and optional peer alias field. The saved local name is shown with a Settings link. Invitation parsing still uses the remote name if no alias is supplied. Blocked devices and Unblock remain.
- Settings: replaced the old checkbox and malformed scaling widget with a single theme toggle. Removed manual scale and reset-scale controls; UI zoom fixed at 100% relative to system DPI. Device name and factory reset are unchanged.
- Overview, window minimum dimensions, core worker, networking, profile reset, authentication and retries unchanged.

Verification: compared source sections against v0.1.12, checked manifest version and lexical delimiter balance, verified deleted controls are absent. No Rust toolchain or Windows runtime available; compilation and native visual/interaction tests remain required. This is a source update, not a compiled executable.
