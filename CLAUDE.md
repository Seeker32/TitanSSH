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

## Code rules

- Rust: no excessive unwrap, proper Result, clear module boundaries
- Frontend: React function components + Hooks, immutable Zustand updates, strict TS
- Every method: Chinese comment (purpose, key params, side effects)
- Dependencies: enable only the features used by the code; do not use umbrella features such as `full` unless every included feature is required and the reason is documented.
- Use `cargo fmt` to format rust files
- Commit messages must be written in English

## Performance

Terminal streaming, bounded chart buffers, no redundant invoke, no unnecessary re-renders

## AI rules

Must: follow architecture, respect service boundaries, complete code + tests
Must not: demo code, mix layers, skip TDD

> Final rule: if it isn't tested, it doesn't exist.

## Agent skills

### Issue tracker

Issues live in GitHub Issues (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context — one `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
