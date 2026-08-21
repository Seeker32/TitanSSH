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
认证前必须解决的一次性信任决策：未知主机或已保存 key 不一致时，后端阻塞所有等待连接并在所属 Terminal 标签内联展示确认卡，用户选择接受并保存（未知主机）、仅本次接受、替换记录（已保存 key 不一致，需第二次内联确认）或拒绝。保存/替换失败时 challenge 保持未决。
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

**可信主机清单 (trusted hosts list)**:
Settings 中的只读区域，按 host 字典序 + port 稳定顺序展示每条信任记录的 endpoint、算法与 SHA-256 指纹。主机 key 与 `known_hosts` 文本由后端解析，React 只消费 typed JSON；不提供删除、编辑、导入或导出操作，信任记录随 HostConfig 生命周期自动管理。空信任存储、读取失败与解析失败是三种不同状态，错误绝不伪装成空列表。
_Avoid_: 信任记录管理页、known_hosts 编辑器、指纹白名单页面

**进程快照 (process snapshot)**:
单次全量进程采样的结构化结果：所属会话 ID、毫秒时间戳、全部进程条目与总数。后端以固定节奏（2 秒）恒定推送全量快照（`process:snapshot` 事件），前端从同一份数据派生 top-5 摘要与全量列表，不做增量、差分或自适应裁剪推送。
_Avoid_: 进程增量、top-N 推送、按需拉取进程列表

**差值采样 (delta sampling)**:
进程 CPU% 的计算方式：worker 在内存中保存每个进程上次采样的 CPU 时间（utime+stime），用相邻两次采样的增量除以间隔得到当前占用率；内存直接取 RSS。与 `ps` 输出的生命周期平均 %CPU 相对立。
_Avoid_: ps %CPU、生命周期平均占用、瞬时 CPU

**终端标签 (terminal tab)**:
以 Runtime Session 为锚点的标签视图：一个会话恰有一个终端标签，关闭终端标签即关闭会话，触发完整 teardown（终端、SFTP、主机监控、进程监控与共享采样连接）；标签只是会话的视图锚点，连接生命周期归 SessionManager。
_Avoid_: 标签即连接、会话恢复标签、多终端标签共连

**进程标签 (process tab)**:
展示某会话全量进程列表的纯视图标签：引用会话但不拥有连接，随时打开/关闭且不影响采样（采样跟随会话生命周期）；关闭后重开立即渲染缓存的最新进程快照。
_Avoid_: 进程弹窗、独立进程会话、进程监控页（路由式）

**会话锚点 (session anchor)**:
标签模型中承担会话生命周期归属的角色，由终端标签担任；其他标签（进程标签等）只引用会话、不拥有连接，因此不能独立决定会话的存亡。
_Avoid_: 标签拥有连接、会话挂在视图上、标签级重连
