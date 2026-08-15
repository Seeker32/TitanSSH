# Changelog

## [Unreleased]

### Features

- Added backend log file persistence (app log dir, append + 10MB startup truncation) with an in-app log viewer (2s polling) and export via save dialog in Settings → Logging.

## [0.1.4] - 2026-08-15

### Features

- Added host identity verification flow with saved and reused endpoint trust records.
- Added host identity change detection and cross-session trust decision handling.
- Added read-only trusted hosts list with full-chain acceptance.
- Added automatic trust record cleanup tied to the HostConfig lifecycle.
- Added Linux keyring fallback for secure storage.
- Added per-tab terminal connection lifecycle presentation.
- Added SFTP five-channel transfer pool per session.
- Added SFTP separated control and transfer connections to keep directory browsing responsive during transfers.
- Added cross-session fair transfer scheduling and safe uploads with target directory auto-refresh.
- Added download overwrite protection with per-file confirmation.
- Added structured transfer errors displayed in the file browser and task rows.
- Added file context menu in FileExplorer.
- Added English and Chinese translations for application settings and UI elements.

### Changed

- Improved drag-and-drop handling to prevent text selection during sidebar and SFTP panel resizing.
- Updated GitHub Actions to use latest versions of checkout, pnpm, and setup-node.

### Fixed

- Fixed missing metrics being treated as unknown and removed CPU guest double-counting.
- Bundled monitor loop parameters into a struct and fixed clippy warnings.

## [0.1.3] - 2026-08-10

### Added

- Added internationalization support with language selection.
- Added logging level management and UI settings.
- Added stable English error codes for AppError and improved error handling.
- Added new icons for various resolutions and platforms.

### Changed

- Updated identifier to production name and improved legacy host migration.
- Improved sidebar resizing functionality and test coverage.
- Implemented automated release notes extraction and workflow asset uploads.

## [0.1.2] - 2026-08-10

### Changed

- Simplified SSH connection timeout handling and improved diagnostic logging.

## [0.1.1] - 2026-08-10

### Added

- Added automated release packages for macOS and Windows.
- Added independent terminal themes and Chinese documentation.

### Changed

- Limited dependency features to those used by the application.

## [0.1.0] - 2026-08-10

### Added

- Initial TitanSSH release with SSH terminals, SFTP transfers, and server monitoring.
