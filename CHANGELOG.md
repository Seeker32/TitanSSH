# Changelog

## [0.1.6] - 2026-08-25

### Features

- Added process monitoring: full process snapshots with delta CPU sampling, delivered by a new process service and shared exec connection registry.
- Introduced the tab view model (ADR-0002): the tab bar and terminal pane render from a tab list; the terminal tab is the session anchor, and other tabs are pure views that reference sessions without owning connections.
- Added memory usage metrics to the monitoring snapshot and the ServerStatusPanel.
- Added a RecentTransfers component listing terminal-state transfers from closed sessions.

### Changed

- Removed the legacy activeView session-view state in favor of tab-based view selection.
- Added a cargo fmt check gate to the release workflow and tidied code formatting in session and host models.
- Updated architecture documentation; added process monitoring and tab view model ADRs.

### Fixed

- Closing a tab after a connection failure (e.g. "No route to host") no longer gets stuck: the frontend removes the session projection even when the backend has already reaped the session and close_session returns SessionNotFound, with store and e2e regression tests covering the user-facing entry point.

## [0.1.5] - 2026-08-18

### Features

- Made host identity and session management commands asynchronous to prevent blocking the main thread.
- Made monitoring commands asynchronous to improve performance and prevent blocking.
- Added backend log file persistence (app log dir, append + 10MB startup truncation) with an in-app log viewer (2s polling) and export via save dialog in Settings → Logging.
- Enhanced log export functionality and error handling.
- Improved SFTP entry validation and macOS Keychain deletion logic.
- Improved error handling and security in storage and IPC.
- Hardened legacy host migration with error handling and backup for corrupt configurations.
- Improved terminal command handling and UTF-8 data preservation in terminal sessions.
- Enhanced logging and session management with early panic handling and cleanup capabilities.
- Added release process documentation and pre-tag checklist.

### Changed

- Required English commit messages in agent rules.
- Refactored trust store tests into a separate file.

### Fixed

- Hardened host identity verification: revocation events, deadlock fixes, and bounded cancel flags.
- Hardened host config persistence: atomic writes, three-state credential input, and boundary validation.
- Restored overwritten legacy credentials via failure compensation; parsed only auth-related credentials.
- Hardened the remote monitoring collection pipeline against silent degradation.
- Completed monitor task lifecycle terminal states and exception protection.
- Typed monitor command errors to distinguish missing task, session, and snapshot.
- Serialized hosts.json concurrent read-modify-write with a shared lock and made host commands asynchronous.
- Fixed log viewer breakage for log files larger than 64 KiB and single-line format corruption from multiline messages.
- Simplified SFTP temporary path assertions in tests and improved readability.

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
