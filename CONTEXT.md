# TitanSSH

TitanSSH 是一个桌面 SSH 运维客户端（Tauri + React），提供终端、SFTP、监控与进程管理能力。本文档收录本项目特有的领域词汇与语言约定。

## Language

**私钥路径 (privateKeyPath)**:
使用私钥认证（AuthType.PrivateKey）时指向本地 SSH 私钥文件的路径。仅存储路径，密钥内容不落盘；该路径只能通过系统文件选择器获取，不支持手动输入。
_Avoid_: 密钥路径、key path 的手写形式（`~/.ssh/...` 等未展开路径）

**网卡接口 (network interface)**:
服务器操作系统暴露的网络接口，是网络上行与下行流量的归属对象；以接口名区分，监控语境中不包括回环接口 `lo`。
_Avoid_: 网络端口、TCP/UDP 端口、交换机端口

**上行速率 (transmit rate)**:
从被监控服务器视角，网卡接口每秒发送的字节数（TX bytes/s）。
_Avoid_: 上传速度、出口带宽

**下行速率 (receive rate)**:
从被监控服务器视角，网卡接口每秒接收的字节数（RX bytes/s）。
_Avoid_: 下载速度、入口带宽

**冲突策略 (conflict strategy)**:
下载最终目标已存在时的处理方式。Reject 拒绝并返回结构化冲突错误（缺省值，绝不覆盖本地文件）；Overwrite 仅在用户对单个文件确认覆盖后使用，经同目录临时文件原子替换。
_Avoid_: 覆盖模式、force、强写

**安全发布 (safe publish)**:
下载数据先写入与最终目标同目录、命名含 taskId 的临时文件，flush 与关闭成功后才按冲突策略发布（no-clobber 重命名或原子替换）；失败、取消或清理失败不破坏原有本地文件。
_Avoid_: 先删后写、直接原地覆盖写入
