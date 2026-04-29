控制面与 Bridge 协议
=====================

概述
----
RustNps 使用自定义的 Bridge 协议在客户端（npc）与服务端（nps）之间建立控制和数据通道。协议位于 `src/protocol.rs`，基于一个带长度前缀和固定 4 字节魔数的简单二进制帧，帧载荷为 JSON 序列化的消息。

消息类型
--------
- BridgeHello（客户端发起）
  - `Control { vkey, version, core_version }`：建立控制连接，用于接收服务端下发的 `ServerMessage`。
  - `Mux { vkey, version, core_version }`：建立多路复用（Mux）连接，后续在该连接上可复用多个数据流。
  - `Config { vkey, version, core_version, config }`：一次性下发完整运行时配置（`ClientRuntimeConfig`），服务端会合并并启动隧道。
  - `Data { vkey, link_id }`：数据连接，表示客户端为某个 link_id 提供数据通道。
  - `SecretVisitor` / `P2pVisitor`：用于 secret/p2p 模式的来访 socket。
  - `RegisterIp { vkey, hours }`：请求注册来源 IP 到服务端授权列表。

- ServerMessage（服务端下发）
  - `Ok { message }` / `Error { message }`：通用应答。
  - `Open { link_id, link }`：要求客户端为 link_id 建立数据连接并连接到 `link.target`。
  - `Ping` / `Stop { reason }`：健康检查或关闭指令。

Link 与数据转发
----------------
`Link` 包含：`kind`（tcp/http/udp/file/secret/p2p）、`target`（目标地址）、`remote_addr`（服务端看到的入站地址）、`crypt`、`compress`、`local_proxy` 等字段。服务端把入站连接参数封装为 `Link`，通过 `Open` 下发给客户端；客户端建立数据连接后双方进行 `copy_bidirectional` 的流量透传。对于 UDP，数据先写入一个二进制 blob 并读取回包。

帧结构
------
- 4 字节魔数：`RNP1`
- 4 字节 LE 长度（u32）
- N 字节 JSON 消息体

实现代码
--------
- 消息序列化/反序列化：`src/protocol.rs` 的 `read_message` / `write_message`。
- 数据 blob：`read_blob` / `write_blob` 用于 UDP 载荷。
- 链路请求：`src/server.rs::request_client_stream` 会根据 `ServerMessage::Open` 的 `link` 字段请求客户端建立 `BridgeHello::Data`。 

安全与兼容性
------------
- `vkey`（VerifyKey）用于把控制/配置与某个 client 关联。
- `Link.remote_addr` 由服务端观察并填充，供 web UI 展示客户端来源 IP。
- 协议中对 `Mode`、`Mode` 等字符串保持兼容旧版实现的自由扩展（例如 `tcptrans`、`file`）。

参考实现：
- `src/protocol.rs`
- 服务器侧消息生成：`src/server.rs`
- 客户端侧实现：`src/client.rs`