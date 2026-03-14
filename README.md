# SSH Terminal Manager

A cross-platform desktop application for managing SSH server connections and terminal sessions, built with Tauri + Vue 3 + Rust.

## Tech Stack

### Frontend
- **Framework**: Vue 3 + TypeScript
- **Build Tool**: Vite
- **State Management**: Pinia
- **Terminal**: xterm.js + xterm-addon-fit
- **Testing**: Vitest + @vue/test-utils + fast-check

### Backend
- **Framework**: Tauri 2.x
- **Language**: Rust (Edition 2024)
- **Async Runtime**: Tokio
- **SSH Library**: ssh2 (libssh2 bindings)
- **Security**: keyring (OS keychain integration)
- **Testing**: proptest + mockall

## Project Structure

```
ssh-terminal-manager/
├── src/                   # Vue 3 frontend application
│   ├── components/        # Vue components
│   │   ├── host/          # Server configuration UI
│   │   ├── terminal/      # Terminal components
│   │   ├── status/        # Status monitoring UI
│   │   └── layout/        # Layout components
│   ├── stores/            # Pinia stores
│   ├── types/             # TypeScript type definitions
│   ├── pages/             # Page components
│   └── test/              # Test setup
├── src-tauri/             # Rust backend application
│   ├── src/
│   │   ├── commands/      # Tauri command handlers
│   │   ├── core/          # Core business logic
│   │   │   ├── session_manager.rs
│   │   │   ├── ssh_client.rs
│   │   │   ├── monitor_worker.rs
│   │   │   └── terminal_bridge.rs
│   │   ├── models/        # Data models
│   │   ├── storage/       # Data persistence
│   │   └── errors/        # Error types
│   └── Cargo.toml
├── public/                # Static assets
├── dist/                  # Build output
├── package.json           # Frontend dependencies
├── vite.config.ts         # Vite configuration
├── vitest.config.ts       # Vitest configuration
└── README.md
```

## Getting Started

### Prerequisites

- **Node.js**: v18 or higher
- **pnpm**: Latest version (or npm/yarn)
- **Rust**: Latest stable (install via [rustup](https://rustup.rs/))
- **System Dependencies**:
  - macOS: Xcode Command Line Tools
  - Linux: libssl-dev, pkg-config
  - Windows: Visual Studio Build Tools

### Installation

1. Clone the repository:
```bash
git clone <repository-url>
cd ssh-terminal-manager
```

2. Install frontend dependencies:
```bash
pnpm install
```

3. Rust dependencies will be automatically handled by Cargo.

### Development

Run the development server:
```bash
pnpm tauri dev
```

This will:
- Start the Vite dev server on port 5173
- Launch the Tauri application
- Enable hot module replacement (HMR)

### Testing

Run frontend tests:
```bash
pnpm test              # Run once
pnpm test:watch        # Watch mode
```

Run backend tests:
```bash
cd src-tauri
cargo test             # All tests
cargo test --lib       # Unit tests only
```

### Building

Build the application for production:
```bash
pnpm tauri build
```

The built application will be in `src-tauri/target/release/bundle/`.

## Features (Phase 1)

### Server Configuration Management
- ✅ Create, edit, and delete server configurations
- ✅ Support for password authentication
- ✅ Support for private key authentication
- ✅ Secure password storage using OS keychain
- ✅ Persistent configuration storage

### Terminal Sessions
- ✅ Multi-tab terminal interface
- ✅ Real-time bidirectional communication
- ✅ Terminal resize support
- ✅ ANSI color and control sequence support
- ✅ Session state management

### Server Monitoring
- ✅ Real-time CPU usage
- ✅ Memory and swap usage
- ✅ Load average (1, 5, 15 minutes)
- ✅ Server uptime
- ✅ Automatic monitoring on session connect

### User Interface
- ✅ Clean, intuitive layout
- ✅ Server list sidebar
- ✅ Status monitoring panel
- ✅ Multi-tab terminal area
- ✅ Resizable panels

## Architecture

### Communication Flow

```
Frontend (Vue 3)
    ↓ Tauri Commands
Backend (Rust)
    ↓ SSH Protocol
Remote Server
    ↑ PTY Output
Backend (Rust)
    ↑ Tauri Events
Frontend (Vue 3)
```

### Key Components

- **SessionManager**: Manages all active SSH sessions
- **SshClient**: Handles SSH connections and authentication
- **MonitorWorker**: Collects server status periodically
- **HostStore**: Persists server configurations
- **Pinia Stores**: Frontend state management

## Development Guidelines

### Code Style
- Frontend: Follow Vue 3 Composition API best practices
- Backend: Follow Rust standard conventions
- Use TypeScript strict mode
- Write tests for critical functionality

### Testing Strategy
- **Unit Tests**: Test individual functions and components
- **Property Tests**: Verify correctness properties with random inputs
- **Integration Tests**: Test end-to-end workflows

## Troubleshooting

### Common Issues

**Port 5173 already in use:**
```bash
# Kill the process using the port
lsof -ti:5173 | xargs kill -9
```

**Rust compilation errors:**
```bash
# Update Rust toolchain
rustup update stable
```

**SSH connection fails:**
- Check firewall settings
- Verify SSH server is running
- Confirm credentials are correct

## License

MIT

## Contributing

Contributions are welcome! Please read the design document in `.kiro/specs/ssh-terminal-manager/` for detailed architecture information.

