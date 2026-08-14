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
传输最终目标已存在时的处理方式（上传与下载共用）。Reject 拒绝并返回结构化冲突错误（缺省值，绝不覆盖已有目标）；Overwrite 仅在用户对单个文件确认覆盖后使用，经同目录临时文件安全发布。
_Avoid_: 覆盖模式、force、强写

**安全发布 (safe publish)**:
传输数据先写入与最终目标同目录、命名含 taskId 的唯一临时文件（下载为本地、上传为远端），flush 与关闭成功后才按冲突策略发布（no-clobber 重命名或原子替换）；失败、取消或清理失败不破坏原有目标文件。远端无法保证安全替换时保留旧目标并让任务失败。
_Avoid_: 先删后写、直接原地覆盖写入

**endpoint (endpoint)**:
实际建立 SSH 连接所用的 `HostConfig.host + port` 精确组合，是主机身份与信任记录的归属键。不做小写、尾点、别名或解析 IP 归一化；不同拼写或不同端口是不同的 endpoint。
_Avoid_: 目标机、服务器地址、host

**主机身份 (host identity)**:
SSH 服务器在握手时呈现的主机公钥（算法 + 完整公钥 blob）。指纹（OpenSSH 风格 `SHA256:base64`）由后端从公钥 blob 计算，仅用于展示，不做比较键。
_Avoid_: 指纹、密钥指纹、fingerprint（单独使用时）

**主机身份确认 (host-identity challenge)**:
认证前必须解决的一次性信任决策：未知主机或已保存 key 不一致时，后端阻塞所有等待连接并在所属 Terminal 标签内联展示确认卡，用户选择接受并保存、仅本次接受或拒绝。保存失败时 challenge 保持未决。
_Avoid_: 信任弹窗、全局确认框、TOFU 提示

**信任记录 (trust record)**:
持久化在 TitanSSH 独立 `known_hosts` 文件中的单条记录：精确 endpoint + 当前算法 + 完整公钥；每个 endpoint 至多一条。已保存 key 精确匹配时在认证前静默放行，不再产生 challenge。
_Avoid_: 指纹缓存、白名单、host key 列表

**信任存储 (trust store)**:
TitanSSH 在应用数据目录维护的标准 OpenSSH `known_hosts` 格式文件及其读写逻辑。不读取系统 `~/.ssh/known_hosts`，不使用 keyring；读写串行化并经同目录临时文件安全发布。文件缺失视为空信任存储，不可读/不可解析时 fail-closed。
_Avoid_: known_hosts 数据库、系统信任库、密钥库

**信任记录清理 (trust-record cleanup)**:
HostConfig 保存或删除后自动移除不再被任何配置引用的 endpoint 信任记录：endpoint 精确比较 host 字符串 + port，不做归一化；仅当更新后的配置集合不再引用旧 endpoint 时删除。不终止运行中的 Runtime Session（其临时信任持续到 Session 关闭），新 Session 将旧 endpoint 视为未知并重新确认；清理失败以结构化错误显式返回。
_Avoid_: 信任吊销、手动删除信任记录、known_hosts 编辑

**仅本次接受 (accept once)**:
只把当前 Runtime Session 的 endpoint + 呈现 key 写入临时信任的决策；覆盖该 Session 的 Terminal、SFTP、Monitoring 及重连，Session 关闭即清除，不落盘。
_Avoid_: 临时信任（单独使用时）、跳过验证、记住本次

**接受并保存 (accept and save)**:
把 challenge 快照的算法与完整公钥持久化到信任存储并放行当前 Session 的决策；保存失败保持 challenge 未决并结构化报错，绝不静默降级为仅本次接受。
_Avoid_: 永久信任、自动保存、记住密码式保存
