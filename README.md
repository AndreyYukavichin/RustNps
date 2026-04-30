# RustNps

<p align="center">
  <img src="web/static/img/RustNps.png" alt="RustNps Logo" height="150" />
</p>

**致敬与鸣谢：**
本项目参考并部分基于以下优秀的开源项目，特此致敬：
- [ehang-io/nps](https://github.com/ehang-io/nps)
- [yisier/nps](https://github.com/yisier/nps)

RustNps 是对 Go 版 nps/npc 的 Rust 重构工程。当前工程保留 nps 的核心模型：一个公网服务端 `rnps`、多个内网客户端 `rnpc`、服务端统一接收入站流量，再通过客户端主动建立的控制连接和数据连接把流量转发到内网目标。

参考与致谢：
**https://github.com/ehang-io/nps**
**https://github.com/yisier/nps**


> Design note: this Rust edition focuses on a safe, explicit control-plane + data-plane split. The wire protocol is RustNps native (`RNP1`) and is intended for RustNps `rnps` and `rnpc` pairs, not for mixed Go/Rust runtime compatibility.

## 功能覆盖

当前 RustNps 已实现以下核心能力：

| Go nps 功能 | RustNps 模块 | 状态 |
| --- | --- | --- |
| 服务端 nps / 客户端 npc | `src/bin/nps.rs`, `src/bin/npc.rs` | 已实现 |
| `nps.conf` / `npc.conf` 配置解析 | `src/config.rs` | 已实现，兼容常用字段 |
| bridge 控制连接 | `src/protocol.rs`, `src/server.rs`, `src/client.rs` | 已实现 |
| 客户端配置上报 | `BridgeHello::Config` | 已实现 |
| TCP 隧道 | `start_tcp_listener` | 已实现 |
| UDP 隧道 | `start_udp_listener` + `serve_udp` | 已实现，按请求短连接转发 |
| HTTP 正向代理 | `start_http_proxy_listener` | 已实现，支持 CONNECT |
| 域名代理 / URL location 路由 | `start_http_host_listener` | 已实现 HTTP |
| HTTPS 域名代理 / SNI 多证书 | `src/tls.rs`, `start_https_host_listener` | 已实现 |
| SOCKS5 代理 | `src/socks5.rs` | 已实现 no-auth CONNECT |
| file 模式 | `serve_file` | 已实现客户端本地文件读取 |
| secret 模式 | `SecretVisitor` relay | 已实现 |
| p2p 模式 | `P2pVisitor` relay fallback | 已实现中继兜底 |
| Web 管理页 | `start_web_manager` | 已实现轻量 Dashboard/API |
| 管理面板登录验证码 | `open_captcha`, `/captcha/`, `login_captcha_block` | 已实现，开启后登录需验证验证码 |
| Go 版 Web 静态资源 | `web/static` | 已迁移 Bootstrap、FontAwesome、ECharts、语言包、图片等 |
| Go 版 Web 操作体验 | `src/server.rs` Web 兼容层 | 已实现登录、侧栏、Dashboard、客户端/隧道/域名列表和主要操作按钮 |
| 新增/复制/编辑隧道自动分配端口 | `ensure_tunnel_server_port`, `generate_server_port` | 已实现，端口为空或 0 时自动回填可用端口 |
| 端口范围映射 | `expand_ports`, `expand_runtime_tunnels` | 已实现 |
| 环境变量渲染 | `{{.ENV_NAME}}` | 已实现 |
| bridge mux 连接复用 | `src/mux.rs`, `src/client.rs`, `src/server.rs` | 已实现 |
| `compress` / `crypt` 数据面包装 | `src/relay.rs`, `src/tls.rs` | 已实现，支持 snappy / TLS relay |
| `allow_rate_limit` / `allow_flow_limit` | `src/relay.rs`, `src/server.rs` | 已实现运行时限速和流量封顶 |
| `ip_limit` 注册与登录联动 | `src/server.rs`, `src/web.rs` | 已实现 |
| `max_conn` / `max_tunnel_num` / `allow_ports` | `src/server.rs` | 已实现 |

### 当前对齐说明

下面这些 Go 版 nps 的近期能力，RustNps 目前已经补齐或保持兼容：

- 管理面板登录验证码：配置 `open_captcha=true` 后，登录页会显示验证码并在登录时校验。
- 隧道自动生成服务端口：新增、复制、编辑隧道时，如果端口为空或 0，会自动生成一个可用端口并写回任务。
- API 返回 ID：新增客户端、主机、隧道时会返回对应的 `id`，方便前端或脚本继续操作。
- 唯一验证密钥显示：域名解析页和隧道列表页已保留 `VerifyKey` / `VKey` 的展示与查询。
- 客户端黑名单 IP：新增或编辑客户端时可配置多个黑名单 IP。
- `ip_limit` 注册与登录联动：已保留注册授权 IP 逻辑，管理面板登录也会授权当前 IP。

这部分功能已经可以按 Go 版的常见使用方式继续迁移；如果你在 Go 版里主要依赖的是上面这些能力，RustNps 现在已经覆盖了大部分日常管理路径。

暂未完全等价 Go 版的高级实现：

- KCP 与 Go 版 `nps_mux` wire compatibility。
- Go Web UI 的深度持久化能力仍在演进；当前 Web 端已提供 Go 风格页面和内存态增删改查兼容层。
- 真正 UDP NAT hole punching 的 P2P；当前 p2p 使用服务端中继 fallback，优先保证可用性。

## 快速开始

### 1. 安装 Rust

Windows、Debian、RedHat 系都建议使用 rustup：

```powershell
winget install Rustlang.Rustup
```

Linux：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

确认工具链：

```bash
rustc --version
cargo --version
```

### 2. 编译

在 `RustNps` 目录执行：

```bash
cargo build --release
```

产物位置：

- Windows: `target\release\rnps.exe`, `target\release\rnpc.exe`
- Linux: `target/release/rnps`, `target/release/rnpc`

交叉编译示例：

```bash
rustup target add x86_64-pc-windows-msvc
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-pc-windows-msvc
cargo build --release --target x86_64-unknown-linux-gnu
```

### 3. 启动服务端

可直接复用 Go 版 `conf/nps.conf` 的常用字段：

```bash
./target/release/rnps -conf_path=../nps/conf/nps.conf
```

默认端口：

- `bridge_port=8024`: npc 控制连接和数据连接入口
- `http_proxy_port=80`: HTTP 域名代理入口
- `web_port=8081`: 轻量 Web Dashboard

Web Dashboard 默认账号密码来自 `nps.conf`：

```ini
web_username=admin
web_password=123
```

访问：

```text
http://127.0.0.1:8081/
http://127.0.0.1:8081/api/dashboard
```

RustNps Web 端复用了 Go 版 nps 的 `/static` 前端资源，并兼容以下常用路由：

- `/login/index`, `/login/verify`, `/login/out`
- `/index/index`
- `/client/list`
- `/index/tcp`, `/index/udp`, `/index/http`, `/index/socks5`, `/index/secret`, `/index/p2p`, `/index/file`
- `/index/hostlist`
- `/global/index`

Bootstrap Table 数据接口保持 Go 版习惯，返回 `rows` 和 `total`：

- `POST /client/list`
- `POST /index/gettunnel`
- `POST /index/hostlist`

### 4. 启动客户端

配置文件模式：

```bash
./target/release/rnpc -config=../nps/conf/npc.conf
```

无配置文件模式：

```bash
./target/release/rnpc -server=SERVER_IP:8024 -vkey=123
```

客户端会先把 `npc.conf` 里的 hosts/tunnels 上报给服务端，然后保持一条控制连接。服务端收到外部访问时，会通过控制连接通知客户端新建数据连接，再由客户端拨内网目标。

## 配置示例

### TCP 隧道

```ini
[common]
server_addr=127.0.0.1:8024
conn_type=tcp
vkey=123
auto_reconnection=true

[ssh]
mode=tcp
server_port=10022
target_addr=127.0.0.1:22
```

外部访问 `SERVER_IP:10022` 会转发到客户端机器的 `127.0.0.1:22`。

### HTTP 域名代理

```ini
[web]
host=dev.example.com
target_addr=127.0.0.1:8080
location=/
```

将 `dev.example.com` 解析到服务端 IP 后，访问服务端 `http_proxy_port` 即可代理到客户端内网服务。

### SOCKS5

```ini
[socks5]
mode=socks5
server_port=19009
```

浏览器或系统代理指向 `SERVER_IP:19009`。

### UDP

```ini
[dns]
mode=udp
server_port=12253
target_addr=114.114.114.114:53
```

### Secret / P2P Relay

服务提供方：

```ini
[ssh_secret]
mode=secret
password=ssh2
target_addr=127.0.0.1:22
```

访问方：

```ini
[secret_ssh]
local_port=2001
password=ssh2
```

访问方连接 `127.0.0.1:2001`，服务端按密码匹配 provider，再中继到 provider 的 `target_addr`。

## 项目结构

```text
RustNps
├─Cargo.toml
├─README.md
└─src
   ├─bin
   │  ├─nps.rs          # server executable
   │  └─npc.rs          # client executable
   ├─client.rs          # npc runtime: config upload, control loop, target dial
   ├─config.rs          # nps.conf / npc.conf parser and port expansion
   ├─lib.rs
   ├─model.rs           # shared config/runtime data structures
   ├─protocol.rs        # RNP1 framed control protocol
   ├─relay.rs           # TCP copy, HTTP header parser, helpers
   ├─server.rs          # nps runtime: bridge, listeners, dashboard
   └─socks5.rs          # minimal SOCKS5 CONNECT implementation
```

## 核心架构

```mermaid
flowchart LR
    user["External user"] --> listener["nps proxy listener"]
    listener --> control["npc control connection"]
    control --> open["Open(link_id, target)"]
    open --> data["npc data connection"]
    data --> target["Intranet target"]
    listener <--> data
```

RustNps 不让服务端主动连接客户端，因为客户端通常在 NAT 内。所有连接都由 npc 主动拨出：

1. npc 建立 control connection。
2. nps 收到外部 TCP/UDP/HTTP/SOCKS 请求。
3. nps 通过 control connection 发送 `Open { link_id, link }`。
4. npc 新建 data connection 到 nps，声明同一个 `link_id`。
5. nps 配对入站连接和 data connection，开始双向复制。

This design keeps ownership and failure boundaries explicit: every public connection has one link id, one data connection, and a clear timeout.

## Rust 优势使用点

- 明确的数据结构：`model.rs` 用强类型表达 Client、Tunnel、Host、Target。
- 明确错误返回：核心函数返回 `io::Result`，避免隐藏 panic。
- 所有权模型：数据连接被配对后移动到 relay 线程，生命周期清晰。
- 跨平台标准库：TCP/UDP/file path 逻辑基于 Rust std，可覆盖 Windows/Linux x86_64。
- 小依赖面：除 `serde`/`serde_json`/`base64`/`md5` 外没有引入重量级运行时。

## 部署建议

Linux systemd 示例：

```ini
[Unit]
Description=RustNps Server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/opt/rustnps/nps -conf_path=/etc/rustnps/nps.conf
Restart=always
RestartSec=5
User=nobody
AmbientCapabilities=CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
```

Windows 可以使用 NSSM 或 PowerShell 注册服务：

```powershell
New-Service -Name RustNps -BinaryPathName "C:\RustNps\rnps.exe -conf_path=C:\RustNps\conf\nps.conf"
Start-Service RustNps
```

## 开发验证

```bash
cargo fmt
cargo check
cargo test
cargo build --release
```

建议先用高端口验证，避免 Windows/Linux 下 80/443 端口权限问题：

```ini
http_proxy_port=18080
web_port=18081
bridge_port=18024
```

然后分别启动：

```bash
./target/release/rnps -conf_path=conf/nps.conf
./target/release/rnpc -config=conf/npc.conf
```
