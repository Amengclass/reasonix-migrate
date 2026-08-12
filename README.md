<div align="center">

# Reasonix Migrate

**A lossless session / memory / config migration tool for the Reasonix desktop app.**

[![version](https://img.shields.io/github/v/release/Amengclass/reasonix-migrate?color=blue&label=version)](https://github.com/Amengclass/reasonix-migrate/releases)
[![stars](https://img.shields.io/github/stars/Amengclass/reasonix-migrate?style=social)](https://github.com/Amengclass/reasonix-migrate)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![platform](https://img.shields.io/badge/platform-Windows-lightgrey.svg)](#)
[![built with](https://img.shields.io/badge/built%20with-Rust%20%7C%20React%20%7C%20TypeScript-orange.svg)](#)
[![PRs](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](#)

English | [中文](README_ZH.md) | [Changelog](CHANGELOG.md)

</div>

A desktop GUI (Tauri 2 + React) that helps you move **Reasonix** data between machines and workspaces — one session at a time, or a whole backup at once.

## ✨ Highlights

- **Lossless by design** — copies at the *directory level*; never touches the internal session format (jsonl event stream, revision chain, content digest). A JSON conversion is lossy; a directory copy is not.
- **Single-session migration** — pick one session and drop it into any target workspace. The tool computes the project slug, rewrites `meta` ownership, and auto-registers the project in `desktop-projects.json`, so it shows up in Reasonix immediately.
- **Whole-machine backup** — export your entire Reasonix data to a zip, restore it on another machine. Optional project / session / date filters, `.env` excluded by default.
- **Verification** — every backup ships with a manifest of SHA-256 hashes; verify the zip before importing, and re-check on import.
- **Consistent with the Reasonix sidebar** — session lists are filtered by the `desktop-projects.json` registry, so you only see what Reasonix actually shows (no orphaned recovery branches or deleted sessions).

## Architecture

```
 ┌─────────────────┐   scan/select   ┌──────────────────┐   copy    ┌──────────────────┐
 │ Reasonix data   │ ──────────────▶ │  reasonix-migrate │ ────────▶ │ target workspace │
 │ (REASONIX_HOME) │                 │      (GUI)       │           │ (projects/<slug>)│
 └─────────────────┘                 └──────────────────┘           └──────────────────┘
                                           │  export / import
                                           ▼
                                    ┌──────────────┐
                                    │ backup .zip  │
                                    └──────────────┘
```

## Quick Start

> Requires [Node.js](https://nodejs.org) + [pnpm](https://pnpm.io) and the [Rust toolchain](https://rustup.rs).

```bash
# 1. Clone
git clone https://github.com/Amengclass/reasonix-migrate.git
cd reasonix-migrate/reasonix-migrate-tauri

# 2. Install frontend dependencies
pnpm install

# 3. Build (frontend → dist, then Rust → self-contained exe)
pnpm build:renderer
cd src-tauri && cargo build --features custom-protocol

# 4. Run
./src-tauri/target/debug/reasonix-migrate-tauri.exe
```

The GUI has four tabs:

| Tab | What it does |
|---|---|
| **Migrate** | Copy one session from a Reasonix home / backup zip / sessions dir into a target workspace (auto slug + meta fix + project registration). |
| **Export** | Pack a Reasonix data dir into a backup zip, optionally filtered by project / session / date. |
| **Import** | Restore a backup zip into a target Reasonix home (slug remap, conflict skip, hash re-verify). |
| **Verify** | Check a backup zip for integrity (file count + per-file SHA-256). |

## Configuration

| Variable | Default | Description |
|---|---|---|
| `REASONIX_HOME` | *(auto-detected)* | Path to the Reasonix data directory (`desktop-projects.json`, `projects/*/sessions` live here). |

## FAQ

<details>
<summary>Does migration move or copy the session?</summary>

It **copies** by default — the source session stays intact. Check the **"Delete source after migration"** option to actually move it (irreversible).

</details>

<details>
<summary>Why does the session list not show everything on disk?</summary>

The list is filtered by the `desktop-projects.json` registry — the same registry that drives the Reasonix sidebar. Orphaned recovery branches and deleted sessions are intentionally hidden.

</details>

<details>
<summary>Should I quit the Reasonix desktop app before exporting?</summary>

Yes, recommended. The desktop app writes to its data directory continuously; quitting first gives you a complete snapshot.

</details>

<details>
<summary>Does migrating affect session history?</summary>

No — the active session (current version) is preserved. Historical *recovery branches* are kept as-is and are not modified by this tool.

</details>

## Development

```bash
pnpm install
pnpm typecheck          # TS type check

pnpm build:renderer     # frontend → dist/
cd src-tauri
cargo build --features custom-protocol   # Rust → self-contained debug exe
```

> **Note:** with the `custom-protocol` feature, the frontend is embedded into the exe at compile time. After changing frontend code you must **re-run `cargo build`** (not just `build:renderer`), otherwise the exe runs the old UI.
> On Windows with Huorong/antivirus file-lock issues (`LNK1105`), use the provided retry scripts: `.\build.ps1 debug` (dev) or `.\tauri-build.ps1` (release).

## Project Structure

```text
reasonix-migrate/
├── reasonix-migrate-tauri/       # the Tauri app
│   ├── src/                      # React frontend (4 tabs: Migrate / Export / Import / Verify)
│   └── src-tauri/
│       ├── src/core/             # Rust core: common / catalog / export / import / one
│       └── src/lib.rs            # Tauri commands
├── .gitignore
├── CHANGELOG.md
└── LICENSE
```

## Contributing

PRs welcome! For bugs or feature requests, open an [issue](https://github.com/Amengclass/reasonix-migrate/issues). Keep changes focused; the tool is designed to be **directory-level and lossless** — don't add session-format rewriting.

## License

[MIT](LICENSE)
