RustNps 架构概览
=================

目标
----
本章描述 RustNps 的总体架构与模块划分，帮助读者理解控制面（Control Plane）和数据面（Data Plane）如何在代码中实现，以及各个子模块之间的交互关系。

总体分层
--------
- 管理层（Web UI / API）
  - 负责用户认证、展示系统/客户端/隧道信息、触发运行时操作（启动/停止/删除）等。
  - 主要实现：`src/web.rs`（路由与渲染）、`web/views/`（模板）、`web/static/`（静态资源）。

- 控制面（Control Plane）
  - 负责客户端注册、隧道安装、配置下发、控制消息的收发与会话维护。
  - 主要实现：`src/server.rs`（桥接接入、桥心逻辑）、`src/protocol.rs`（消息格式）
  - 控制通道通过 BridgeHello 中的 `Control`/`Mux`/`Config` 等消息完成认证与配置同步。

- 数据面（Data Plane）
  - 执行流量转发（TCP/UDP/HTTP/SOCKS5/Secret/P2P 等），通过请求客户端建立数据通道并在两端做拷贝。
  - 主要实现：`src/relay.rs`、`src/socks5.rs`、`src/server.rs` 中的 listener 启动函数（`start_tcp_listener`、`start_udp_listener`、`start_http_proxy_listener` 等）。

- 会话与持久化
  - 运行时会把客户端、隧道、host 等信息保存在内存结构 `Registry` 中，并由 `PersistentStore` 将可持久化数据读写到 `conf/*.json`。
  - 相关文件：`src/store.rs`、`src/model.rs`。

- 多路复用（Mux）
  - 为了节省连接资源，客户端与服务端支持 bridge mux（控制 + 多个数据流复用）。Mux 会话在服务端以 `MuxSession` 对象管理。
  - 相关代码：`src/mux.rs`、`src/server.rs`（mux_accept_loop）。

主要数据结构
------------
- `Registry`（`src/server.rs`）
  - 运行时中心对象，包含 `clients`、`controls`（活跃 control 句柄）、`sessions`（Web 会话）、`system_history`（系统快照）等。

- `ClientRuntimeConfig`（`src/model.rs`）
  - 一个客户端的运行时配置，含 `common`（基础信息），`hosts`、`tunnels` 等。

- `Tunnel`（`src/model.rs`）
  - 表示一个隧道任务，包含 `mode`（tcp/udp/httpProxy/socks5/secret/p2p/file 等）、`server_ip`、`server_port`、`target` 等字段。

控制流（简要）
------------
1. 客户端与服务端建立 bridge 连接并发送 `BridgeHello::Control` 或 `BridgeHello::Mux`。
2. 服务端在 `handle_bridge_conn` 中处理控制消息、创建 `ControlHandle` 或 `MuxSession` 并存入 `Registry::controls`/`Registry::mux_sessions`。
3. 当客户端发送 `BridgeHello::Config`（带 tunnels/hosts）时，服务端调用 `install_client_config`：分配 id、校验、调用 `expand_runtime_tunnels` 并将监听器（listener）传给 `start_*_listener` 启动 data plane。
4. Web UI 通过 API（例如 `/index/gettunnel`）读取 `Registry` 中的 `clients`/`tunnels` 信息并渲染到页面；发起操作时调用服务端 mutation 接口（POST），服务端修改 `Registry` 并持久化到 `PersistentStore`（如果需要）。

网络与安全
----------
- 通过 `ip_limit` 与授权列表进行来源 IP 控制；支持 `register_ip` 命令将客户端公网 IP 注册到 server 的授权列表。
- 支持 `compress` 与 `crypt` 标记，对数据通道进行 snappy 压缩或 TLS 加密包装。

扩展点
------
- 新增隧道类型：在 `src/model.rs` 中定义 `Tunnel.mode` 的新字符串并在 `start_tunnel_task` 中补充对应的 listener 分支。
- Web 扩展：模板位于 `web/views`，采用简单的字符串替换渲染（`load_view`），增加表单与 API 后端对应即可。

参考代码位置
-----------
- Web/模板/静态资源：`web/views/`、`web/static/`
- Web 路由/渲染：`src/web.rs`
- 控制/桥接：`src/server.rs`
- 数据转发/Relay：`src/relay.rs`、`src/socks5.rs`
- 协议定义：`src/protocol.rs`
- 持久化：`src/store.rs`、`src/model.rs`
- Mux：`src/mux.rs`
- 客户端：`src/client.rs`

本章结语
--------
本文件仅为概览；后续章节将深入每一部分的配置、实现细节与常见调整方法。