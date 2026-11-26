<div align="center">

# LiteLAN

**Play together. Connect directly.**

A lightweight native Windows frontend for AnywhereLAN, built with Rust.

[![Release](https://img.shields.io/github/v/release/S0koloff/LiteLAN?style=flat-square)](https://github.com/S0koloff/LiteLAN/releases/latest)
[![Windows](https://img.shields.io/badge/Windows-10%20%2F%2011-0078D4?style=flat-square)](https://github.com/S0koloff/LiteLAN/releases)
[![Rust](https://img.shields.io/badge/GUI-Rust-orange?style=flat-square)](https://www.rust-lang.org/)
[![Build](https://img.shields.io/github/actions/workflow/status/S0koloff/LiteLAN/windows.yml?branch=main&style=flat-square)](https://github.com/S0koloff/LiteLAN/actions)

[**Download**](https://github.com/S0koloff/LiteLAN/releases/latest) · [Getting started](#getting-started) · [Report an issue](https://github.com/S0koloff/LiteLAN/issues)

</div>

## Overview

LiteLAN brings a standalone desktop interface to [AnywhereLAN](https://github.com/anywherelan/awl), making it easier to connect computers over a private virtual network for multiplayer games and other IP-based applications.

No browser window, no account, and no self-hosted server required. Connections use AnywhereLAN's peer-to-peer networking and relay infrastructure when available.

<p align="center">
  <img src="assets/LiteLAN%20Network%20Overview.png" width="420" alt="LiteLAN v0.1.11 Overview showing network status, virtual IP and connected devices">
  <br>
  <em>LiteLAN v0.1.17 · Network Overview</em>
</p>

## Features

- **Native desktop UI** — Rust and egui, without Electron or WebView.
- **Peer-to-peer networking** — direct connections when possible, with relay support.
- **Simple invitations** — add friends using private, one-time invitation links.
- **Connection overview** — view peers, virtual IP addresses and connection types.
- **Portable distribution** — extract and run on Windows x64.
- **Focused configuration** — no exit node, built-in DNS or SOCKS5 proxy enabled by default.
- **Built-in diagnostics** — view core logs without leaving the application.

## Getting started

1. Download `LiteLAN-win-x64.zip` from the [latest release](https://github.com/S0koloff/LiteLAN/releases/latest).
2. Extract the entire archive to a folder.
3. Run `LiteLAN.exe` as administrator (or use the included `run-admin.bat`).
4. Once the network core is running, select **Create & copy one-time invitation**.
5. Send the invitation privately to your friend. They can paste it into **Add friend by invitation link** in their own LiteLAN client.
6. Accept any incoming request. Once connected, use the host's virtual IP shown in the guest's peer list to join the game.

Both players should use the same release. Windows administrator privileges are required to create the virtual network adapter.

## Requirements

- Windows 10 or 11, x64
- Internet connection
- Administrator privileges

## Build from source

Install [Rust](https://rustup.rs/) and [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the Desktop development with C++ workload.

```powershell
git clone https://github.com/S0koloff/LiteLAN.git
cd LiteLAN
.\build.bat
```

The build script downloads the upstream AnywhereLAN core, verifies its published SHA-256 checksum, runs tests and builds the Windows application. The output is placed in `dist/` and packaged as `LiteLAN-win-x64.zip`.

## Architecture

| Component | Technology |
| --- | --- |
| Desktop interface | Rust / egui |
| Network core | AnywhereLAN v0.21.0 |
| Peer discovery | libp2p / DHT |
| Virtual network adapter | Wintun |
| Core communication | Local HTTP API, bypassing system proxy |

LiteLAN uses a separate configuration profile and does not intentionally modify system proxy settings. Avoid running another AnywhereLAN instance alongside LiteLAN.

## Credits

LiteLAN is an independent frontend for [AnywhereLAN](https://github.com/anywherelan/awl), not an official AnywhereLAN release.

- The default build bundles a core built from the [AnywhereLAN v0.21.0 source](https://github.com/anywherelan/awl/tree/v0.21.0) under the [MPL-2.0 license](https://github.com/anywherelan/awl/blob/v0.21.0/LICENSE), with a local patch for outgoing authorization retries and retry/cancel API actions. The patch does not change transport, Wintun, routing, encryption, or approval policy.
- To reproduce the modified core, see [`patches/build-core.ps1`](patches/build-core.ps1) and [`patches/apply-auth-retry.ps1`](patches/apply-auth-retry.ps1). The release archive also includes the patch alongside the core. `build.ps1 -OriginalCore` uses the unchanged upstream binary for diagnosis; manual Retry and Cancel require the patched core.
- The bundle includes the official [Wintun 0.14.1](https://www.wintun.net/) Windows AMD64 DLL. Rust dependencies are listed in [`Cargo.toml`](Cargo.toml) and are covered by their respective licenses.

<div align="center">

Built for simpler LAN gaming.

</div>
