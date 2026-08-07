# TitanSSH — Desktop SSH Operations Client

Tauri + React 19 + TS(strict) + event-driven desktop DevOps client: SSH, File Transfer, Monitoring, Process Management. Long-term engineering system, not a demo.

## Architecture (non-negotiable)

- **Session ≠ UI**: session = runtime entity; tab = view only. Tab never owns the connection
- **View-only frontend**: never parse shell output in React; Rust returns structured JSON only
- **Communication**: invoke = request/response, event = streaming; typed, structured, version-safe
- **Services**: terminal_service / sftp_service / monitor_service / process_service. No god service
- **Long tasks**: require taskId; state `pending → running → done | failed`

## TDD (mandatory)

1. Tests first → run (fail) → implement → refactor. Untested code is invalid; never skip failing tests
2. Layers:
   - Unit: pure logic (Rust + TS), edge cases + error paths required
   - Integration: service-to-service, invoke/event contract validation
   - E2E: SSH lifecycle, terminal interaction, file transfer, monitoring updates
3. Every feature: success path + failure path + retry/edge cases (if applicable)
4. Rust focus: session lifecycle, service isolation, async, `Result` propagation
5. Frontend focus: Zustand state transitions, event handling; no business-logic tests in components

## Data model

JSON-serializable, camelCase, timestamps in ms. Models: HostConfig, SessionInfo, TerminalTab, FileTransferTask, MonitorSnapshot, ProcessInfo

## Security

- No plaintext secrets; use OS secure storage
- Private keys: store path only, passphrase secured

## Feature rules

- Terminal: xterm.js renders, Rust handles IO; no frontend simulation/buffer logic
- Monitoring: backend collects all metrics, one payload per update; no frontend aggregation, no per-chart requests
- SFTP: upload/download, progress tracking, task queue

## Code rules

- Rust: no excessive unwrap, proper Result, clear module boundaries
- Frontend: React function components + Hooks, immutable Zustand updates, strict TS, no business logic in components
- Every method: Chinese comment (purpose, key params, side effects)

## Performance

Terminal streaming, bounded chart buffers, no redundant invoke, no unnecessary re-renders

## AI rules

Must: follow architecture, respect service boundaries, complete code + tests
Must not: demo code, mix layers, skip TDD, break the event model

## Out of scope (early stage)

Jump host, docker, plugins, cloud sync

> Final rule: if it isn't tested, it doesn't exist.
