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
