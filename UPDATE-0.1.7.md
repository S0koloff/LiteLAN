# LiteLAN 0.1.7

Native compact interface based on the approved prototype. Window 440 × 660 logical points, bottom navigation, device rows, connection progress and toast notifications. System DPI is respected; UI zoom is adjustable. Includes Windows executable icon and UAC manifest. Existing profiles are preserved. No changes to Wintun, MTU, Windows firewall, system proxy or VPN routing.

## Build
Extract this update into the existing project root, overwriting files. Keep your existing README and build.bat. Commit and tag v0.1.7. The existing Windows workflow builds the pinned AWL v0.21.0 source with the local authorization patch, then builds/tests Rust and publishes the ZIP for a version tag. `build.ps1 -OriginalCore` is a fallback for testing upstream, but manual Retry/Cancel endpoints require the patched core.

## Authorization
Create a one-time invitation; the recipient pastes it and presses Connect. A valid token permits automatic authorization by the upstream core. The retry patch keeps the original permission checks and reuses a saved pending auth request at a 20-second ticker interval. The Try again action queues a targeted attempt (bounded to 30 seconds), not a new device. After 60 seconds, the progress window warns about prolonged waiting; it may be hidden. Cancel attempts forgets the pending local request; it cannot retract one already delivered to the remote device. A token that is invalid or no longer usable may require a new invitation or manual approval. Previously authorized devices reconnect using the original mechanisms. No blanket automatic approval.

## Acceptance checks (Windows, two PCs)
- Upgrade both clients; leave the existing working profile intact. Try a fresh invitation with a previously unknown peer to prove one-way authorization (existing confirmed peers do not test it).
- Check that both lists show a different device name, its assigned IP, and Direct or Relay. Verify real traffic in the game.
- Keep a recipient offline, then bring it back: verify automatic retry, Try again, background mode, and cancellation.
- Check invalid links, rejection, incoming manual requests, and blocked/unblocked peers.
- Check window layout and button hit areas with 100%, 125%, 150% Windows DPI and different UI zoom. Check light/dark mode and long names.
- Verify the GitHub build before distributing the executable.

## Verification status
This archive contains source updates, not a compiled Windows binary. Source API and retry flow were reviewed against upstream v0.21.0. Rust compilation, patched Go integration tests, rendered Windows UI and fresh one-way connection have not been executed here (Rust/PowerShell unavailable; upstream download blocked in the local environment). The proposed retry fix is not a guarantee of network reachability. The existing working connection is not modified by updating files.
