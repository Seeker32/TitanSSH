# TitanSSH

TitanSSH 是一款桌面 SSH 运维工具。你可以把常用服务器放在一起管理，在多个终端标签之间切换，传输文件，并随时查看服务器的基本状态。

## 可以做什么

- 保存并整理服务器连接
- 使用密码或 SSH 私钥登录
- 同时打开多个终端会话
- 单独调整终端主题，不影响应用其他界面
- 上传、下载文件并查看进度
- 查看 CPU、内存、磁盘、网络活动和运行时长

密码由操作系统的安全钥匙串保存。私钥始终留在你的电脑上，TitanSSH 只记录私钥文件的位置。

## 开始使用

需要准备 Node.js 22.13 或更高版本、pnpm 和当前稳定版 Rust。macOS 还需要 Xcode Command Line Tools；Linux 与 Windows 可能需要安装常用的 C/C++ 编译工具。

```bash
pnpm install
pnpm tauri dev
```

应用会以开发模式打开。通过侧边栏添加服务器后，双击服务器即可打开终端会话。

## 终端主题

点击侧边栏的“设置”可选择终端主题：浅色、深色、One Dark、Dracula、Solarized Light 和 Solarized Dark。选择会保存在本机，只影响终端内容，不会改变应用本身的明暗外观。

## 检查与构建

```bash
pnpm test
pnpm tauri build
```

构建后的应用位于 `src-tauri/target/release/bundle/`。

## 常见问题

**无法连接服务器？** 请检查服务器地址、SSH 服务、防火墙、用户名和认证信息。

**开发应用无法启动？** 请确认已安装上方列出的 Node.js、pnpm、Rust 和平台编译工具。

## 许可证

MIT
