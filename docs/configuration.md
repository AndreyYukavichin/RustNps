配置参考（Server & Client）
=========================

本章列出 RustNps 常用的服务端和客户端配置项，并给出示例。本仓库保持与 Go 版 `nps` / `npc` 的兼容性，配置文件位于 `RustNps/conf/`。

服务端（`conf/nps.conf`）
------------------------
RustNps 服务端使用 key=value 的 INI 风格配置（顶层键值对）。常用项：

- `bridge_ip` / `bridge_port`：bridge 控制与 mux 连接的监听地址与端口。
- `http_proxy_ip` / `http_proxy_port`：用于域名代理（HTTP）的监听地址/端口。
- `https_proxy_port`：HTTPS 域名代理端口。
- `web_ip` / `web_port`：Web 管理界面监听地址/端口。
- `web_username` / `web_password`：Web 管理默认管理员用户名/密码。
- `web_base_url`：如果把 Web 部署在子路径时设置该前缀（例如 /rustnps）。
- `public_vkey`：默认的客户端密钥（仅作示例/兼容）。
- `allow_user_login` / `allow_user_register`：是否允许 Web 用户登录与注册。
- `ip_limit`：开启后只允许已登记来源 IP 访问控制面。
- `allow_flow_limit` / `allow_rate_limit` / `allow_connection_num_limit` / `allow_tunnel_num_limit`：运行时策略的开关。
- `allow_local_proxy` / `allow_multi_ip`：控制是否允许代理到本地/绑定多 IP。
- `p2p_ip` / `p2p_port`：P2P 功能使用的服务端 IP/端口。

示例（最简）

```ini
# conf/nps.conf
web_ip=0.0.0.0
web_port=8081
web_username=admin
web_password=123
bridge_ip=0.0.0.0
bridge_port=18024
http_proxy_ip=0.0.0.0
http_proxy_port=18080
https_proxy_port=18082
public_vkey=123
allow_user_login=true
allow_user_register=false
ip_limit=false
```

更多字段和说明请参阅：[RustNps/docs/server_config.md](server_config.md)

客户端（`conf/npc.conf`）
------------------------
客户端配置采用 INI 风格，并以分段描述多个隧道。最顶层的 `[common]` 段定义与服务端的连接方式与通用选项，其他段（如 `[tcp]`、`[udp]`）定义隧道。示例见仓库 `conf/npc.conf`。

示例（摘录）

```ini
[common]
server_addr=127.0.0.1:18024
conn_type=tcp
vkey=123
auto_reconnection=true
max_conn=1000
crypt=false
compress=false

auto_reconnection=true

[tcp]
mode=tcp
target_addr=127.0.0.1:22
server_port=10022

[socks5]
mode=socks5
server_port=19009
```

常用字段说明：

- `server_addr`：服务端地址（含端口）。
- `vkey`：客户端 VerifyKey，用于将配置与服务端登记的 client 对应。
- `conn_type`：`tcp` 或 `tls` 等。
- `crypt` / `compress`：数据通道是否加密或压缩（在服务端允许时生效）。
- 每个隧道段内：`mode`（tcp/udp/httpProxy/socks5/secret/p2p/file）、`server_port`（服务端监听端口）、`target_addr`（替换/目标地址）等。

多端口与多个目标
----------------
- 使用 `ports` 或 `server_port` 支持单端口或逗号/范围定义（参见 `expand_ports` 实现）。
- 支持在 `target_addr` 中设置多个行（换行分隔），与 `ports` 一一对应或按轮询选择。

运行时配置下发
--------------
- 客户端可以通过 `BridgeHello::Config` 将完整的 `ClientRuntimeConfig` 下发到服务端，服务端会把 `tunnels`/`hosts` 等合并入运行时并启动对应监听器（参见 `src/protocol.rs` 与 `src/server.rs` 的 `install_client_config`）。

持久化
------
- 可持久化的配置会写入 `conf/clients.json`、`conf/tasks.json`、`conf/hosts.json`，由 `PersistentStore` 管理，支持 JSON 行分隔和数组两种格式（参见 `src/store.rs`）。

参考文件：
- 服务端默认示例： [conf/nps.conf](conf/nps.conf)
- 客户端示例： [conf/npc.conf](conf/npc.conf)
- 持久化实现： [src/store.rs](src/store.rs)
- 数据模型： [src/model.rs](src/model.rs)