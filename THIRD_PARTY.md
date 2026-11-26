# Third-party software

LiteLAN is an independent native frontend, not an official AnywhereLAN release.

## AnywhereLAN (AWL)

- Upstream version: v0.21.0
- License: MPL-2.0
- Upstream source: https://github.com/anywherelan/awl/tree/v0.21.0
- Upstream license: https://github.com/anywherelan/awl/blob/v0.21.0/LICENSE
- LiteLAN source and build patches: https://github.com/S0koloff/LiteLAN

The default LiteLAN build uses a modified AWL core. The corresponding patch scripts and added Go sources are available in the LiteLAN repository under `patches/` and are included with the patched core in the binary distribution. The upstream license is included as `core/AWL-LICENSE.txt` in the default patched build. The optional `-OriginalCore` build uses the original upstream binary instead.

## Other dependencies

Rust dependencies are listed in `Cargo.toml`; their respective licenses apply. Wintun and other upstream components retain their respective licenses and notices.
