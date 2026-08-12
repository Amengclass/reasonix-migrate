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

### Fixed
- Support the v1.24 **double-hex recovery-branch naming** (`-recovery-<hex>-<hex>`); such branches were previously misclassified as independent sessions, inflating the export session count and the manifest `recovery_branch_count`.
- Session-catalog pre-seed now targets the **newest** `cache/session-catalog/v*.sqlite` (Reasonix v1.24.2 uses v4) and fills the v4 `recovery_role` / `ordinary_visible` columns so migrated sessions stay visible in the Reasonix sidebar.
- Compatibility note updated: built and tested against Reasonix desktop **v1.24.1 / v1.24.2** (verified on v1.24.2).

[0.1.0]: https://github.com/Amengclass/reasonix-migrate/releases/tag/v0.1.0
