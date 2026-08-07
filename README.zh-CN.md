# TitanSSH — 桌面 SSH 运维客户端

跨平台桌面 DevOps 客户端：SSH 终端、文件传输（SFTP）、服务器监控。基于 **Tauri 2 + React 19 + Rust** 的事件驱动架构，长期工程化系统，非演示项目。

## 功能特性

- **主机管理**：主机配置的增删改查、密码 / 私钥认证、密钥口令安全存储
- **终端会话**：多标签终端、实时双向通信、窗口尺寸自适应、ANSI 颜色与控制序列支持
- **服务器监控**：CPU / 内存 / 交换分区 / 负载 / 运行时长实时监控，连接后自动采集
- **文件传输（SFTP）**：上传 / 下载、进度跟踪、任务队列
- **安全存储**：密码经 OS 系统钥匙串（keyring）加密保存，私钥只存路径

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | React 19 + TypeScript（strict）、Vite、Zustand、xterm.js、Ant Design 6 |
| 后端 | Tauri 2.10、Rust（edition 2024）、Tokio、ssh2（libssh2）、keyring |
| 测试 | Vitest + React Testing Library + Playwright（前端）；proptest + mockall（Rust） |

## 目录结构

```
Titan/
├── src/                      # React 前端
│   ├── components/           # 组件（host / terminal / sftp / status / layout / home）
│   ├── stores/               # Zustand 状态（host / session / monitor / sftp / layout / theme）
│   ├── pages/                # 页面（HomePage）
│   ├── types/                # 与后端共享的类型定义（camelCase、JSON 序列化）
│   └── test/                 # 测试环境配置
├── src-tauri/                # Rust 后端
│   ├── src/
│   │   ├── commands/         # Tauri 命令（invoke 入口：host / session / monitor / sftp）
│   │   ├── core/             # 服务层（见下方"服务边界"）
│   │   ├── models/           # 数据模型（HostConfig / SessionInfo / TerminalTab / FileTransferTask / MonitorSnapshot / ProcessInfo）
│   │   ├── storage/          # 持久化（host_store、secure_store）
│   │   └── errors/           # 统一错误类型（AppError）
│   └── Cargo.toml
├── e2e/                      # Playwright E2E 测试
├── docs/                     # 设计文档
└── package.json
```

## 架构设计

### 通信模型（事件驱动）

- **invoke**：请求 / 响应，用于一次性操作
- **event**：流式推送，用于终端输出、监控快照等持续数据

所有通信必须是**类型化、结构化、版本安全**的 JSON。

### Session ≠ UI

Session 是运行时实体，Tab 只是视图。Tab 不拥有连接，Session 生命周期独立于界面，可被多视图复用。

### 服务边界

后端按领域拆分，禁止"上帝服务"：

- `terminal_service`：终端 IO（xterm.js 只负责渲染，Rust 处理所有 IO 与缓冲）
- `sftp_service`：文件上传 / 下载、进度与任务队列
- `monitor_service` + `monitor_worker`：后端采集全部指标，每次更新一个完整 payload，前端不做聚合
- `session_manager` + `ssh_client`：会话生命周期与 SSH 连接

### 长任务

所有长操作必须带 `taskId`，状态机：`pending → running → done | failed`

## 快速开始

### 环境要求

- Node.js ≥ 22.13、pnpm
- Rust stable（[rustup](https://rustup.rs/)）
- 系统依赖：macOS 需 Xcode Command Line Tools；Linux 需 `libssl-dev`、`pkg-config`；Windows 需 Visual Studio Build Tools

### 安装与开发

```bash
pnpm install        # 安装前端依赖（Rust 依赖由 Cargo 自动管理）
pnpm tauri dev      # 启动 Vite + Tauri 应用（支持 HMR）
```

### 测试

```bash
pnpm test                       # 前端单测（Vitest）
pnpm test:watch                 # 监听模式
pnpm exec playwright install chromium   # 首次运行 E2E 前安装浏览器
pnpm test:e2e                   # Playwright E2E（mock Tauri 层）
cd src-tauri && cargo test      # Rust 后端测试
```

### 构建

```bash
pnpm tauri build
```

产物位于 `src-tauri/target/release/bundle/`。

## 开发规范（强制）

- **TDD**：先写测试 → 跑失败 → 实现 → 重构；未测试的代码无效
- **测试分层**：单元（纯逻辑 + 边界/错误路径）、集成（服务间 + invoke/event 契约）、E2E（SSH 生命周期 / 终端交互 / 文件传输 / 监控更新）
- **每个特性**：成功路径 + 失败路径 + 重试/边界
- **前端**：函数组件 + Hooks、Zustand 不可变更新、strict TS、组件内无业务逻辑
- **Rust**：不滥用 unwrap、正确 `Result` 传播、模块边界清晰
- **注释**：每个方法必须有中文注释，说明目的、关键参数、副作用
- **性能**：终端流式渲染、图表有界缓冲、避免冗余 invoke 与不必要的重渲染

## 常见问题

**端口 5173 被占用：**

```bash
lsof -ti:5173 | xargs kill -9
```

**SSH 连接失败：** 检查防火墙、确认远端 SSH 服务运行、核对凭据。

## License

MIT
