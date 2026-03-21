# TitanSSH — Desktop SSH Operations Client

---

## 1. Purpose

TitanSSH is a **desktop DevOps client** built with:

* Tauri (Rust backend)
* Vue 3 + TypeScript (strict)
* Event-driven architecture

It provides:

> SSH + File Transfer + Monitoring + Process Management

This is a **long-term engineering system**, not a demo tool.

---

## 2. Core Architecture Rules (NON-NEGOTIABLE)

### 2.1 Session ≠ UI

* Session = runtime entity
* Tab = view only

❌ Tab owns connection
✅ Session lifecycle is independent

---

### 2.2 Frontend is View-Only

❌ Parsing shell output in Vue
✅ Rust returns structured JSON only

---

### 2.3 Communication Contract

* `invoke` → request/response
* `event` → streaming

Must be:

* typed
* structured
* version-safe

---

### 2.4 Service Boundaries (MANDATORY SPLIT)

* `terminal_service`
* `sftp_service`
* `monitor_service`
* `process_service`

❌ No “god service”

---

### 2.5 Long Task System

All long operations MUST:

* have `taskId`
* include state:

```
pending → running → done | failed
```

---

## 3. TDD DEVELOPMENT RULES (CRITICAL)

---

### 3.1 Test First (MANDATORY)

Every feature must follow:

```
1. Write tests
2. Run tests (fail)
3. Implement
4. Refactor
```

❌ No test → code is invalid
❌ Skipping failing tests is forbidden

---

### 3.2 Required Test Layers

#### Unit Tests

* Pure logic (Rust + TS)
* Edge cases + error paths REQUIRED

---

#### Integration Tests

* Service-to-service behavior
* invoke/event contract validation

---

#### E2E Tests

Must cover:

* SSH connection lifecycle
* Terminal interaction
* File transfer flow
* Monitoring updates

---

### 3.3 Test Scope Requirements

Every feature MUST include:

* success path
* failure path
* retry / edge cases (if applicable)

---

### 3.4 Backend Test Focus (Rust)

* session lifecycle
* service isolation
* async behavior
* error propagation (`Result`)

---

### 3.5 Frontend Test Focus

* state transitions (Pinia)
* event handling correctness
* NO business logic tests in components

---

## 4. Data Model Rules

* JSON-serializable only
* camelCase
* timestamp = milliseconds

Core models:

* HostConfig
* SessionInfo
* TerminalTab
* FileTransferTask
* MonitorSnapshot
* ProcessInfo

---

## 5. Security Rules

* ❌ No plaintext secrets
* ✅ Use OS secure storage

Private key:

* store path only
* passphrase secured

---

## 6. Terminal Rules

* xterm.js rendering only
* Rust handles IO

❌ No terminal simulation in frontend
❌ No buffer logic in Vue

---

## 7. Monitoring Rules

* backend collects ALL metrics
* single payload per update

❌ No frontend aggregation
❌ No per-chart requests

---

## 8. SFTP Rules

Must support:

* upload / download
* progress tracking
* task queue

---

## 9. Code Rules

### Rust

* no excessive `unwrap`
* proper `Result` handling
* clear module boundaries

---

### Frontend

* `<script setup>`
* strict TypeScript
* no business logic in components

---

### Mandatory Comment Rule

Every method MUST include a **Chinese comment** explaining:

* purpose
* key parameters (if needed)
* side effects (if any)

---

## 10. Performance Constraints

* terminal = streaming
* charts = bounded buffer
* avoid redundant invoke
* avoid unnecessary re-render

---

## 11. AI Enforcement Rules

AI MUST:

* follow architecture strictly
* respect service boundaries
* generate COMPLETE code + tests

AI MUST NOT:

* write demo-style code
* mix layers
* skip TDD
* break event model

---

## 12. Out of Scope (Early Stage)

* jump host
* docker integration
* plugin system
* cloud sync

---

## Final Rule

> If it is not tested, it does not exist.
