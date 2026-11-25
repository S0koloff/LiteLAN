# Third-party software
LiteLAN is an independent native frontend, not an official AnywhereLAN release.
The default build uses AnywhereLAN v0.21.0 (MPL-2.0) with a local outgoing authorization retry and retry/cancel API patch. It does not change the transport, Wintun, routing, encryption or approval policy.

- Upstream source: https://github.com/anywherelan/awl/tree/v0.21.0
- License: https://github.com/anywherelan/awl/blob/v0.21.0/LICENSE
- Modified source: apply `patches/apply-auth-retry.ps1` to that tag; this script is also included with the binary. `patches/build-core.ps1` reproduces the build.
- Wintun: https://www.wintun.net/ (official 0.14.1 Windows AMD64 DLL)
- The build can use the unchanged upstream binary with `build.ps1 -OriginalCore` for diagnosis; manual Retry/Cancel actions require the patched core.

Rust dependencies are listed in Cargo.toml; their respective licenses apply.
