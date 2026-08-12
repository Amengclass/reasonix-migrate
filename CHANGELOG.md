# Changelog

## [0.1.0] - 2026-08-13

### Added
- Initial public release.
- Four-tab GUI (Tauri 2 + React): **Migrate** / **Export** / **Import** / **Verify**.
- Single-session migration: auto project slug, `meta` ownership fix, auto project registration.
- Whole-machine backup: zip export with project / session / date filters (`.env` excluded by default).
- Import with slug remap, conflict skip, and SHA-256 re-verification.
- Backup integrity verify (manifest SHA-256).
- Session lists consistent with the Reasonix sidebar (filtered by `desktop-projects.json`).
- Auto project-list refresh on dropdown open; per-project session refresh on change.
- `--features custom-protocol` self-contained debug build (no vite dev server needed).

[0.1.0]: https://github.com/Amengclass/reasonix-migrate/releases/tag/v0.1.0
