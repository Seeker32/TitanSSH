# TitanSSH

TitanSSH 是一个桌面 SSH 运维客户端（Tauri + React），提供终端、SFTP、监控与进程管理能力。本文档收录本项目特有的领域词汇与语言约定。

## Language

**私钥路径 (privateKeyPath)**:
使用私钥认证（AuthType.PrivateKey）时指向本地 SSH 私钥文件的路径。仅存储路径，密钥内容不落盘；该路径只能通过系统文件选择器获取，不支持手动输入。
_Avoid_: 密钥路径、key path 的手写形式（`~/.ssh/...` 等未展开路径）
