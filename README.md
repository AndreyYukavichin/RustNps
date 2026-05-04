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

## 主要能力

RustNps 覆盖了 Go 版 nps / npc 的核心使用路径：服务端与客户端、常见隧道模式、HTTP / HTTPS / SOCKS5 / file / secret / p2p、Web 管理页、验证码登录、自动端口分配、环境变量渲染、压缩与加密包装、限速与流量封顶、IP 注册联动、以及 KCP bridge 支持。

这里优先描述用户能直接使用的能力和启动方式；实现进展、功能对标和后续 TODO 请看 [docs/feature_map.md](docs/feature_map.md) 与 [docs/refactor_todos.md](docs/refactor_todos.md)。

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

### 3. 使用 Docker 镜像

RustNps 也可以直接打包成多架构镜像并推送到 Docker Hub。仓库内提供了对应的 `Dockerfile` 和 GitHub Actions 发布流程，镜像名为 `andreiyhub/rustnps`。

本地运行服务端示例：

```bash
docker run --rm \
  -p 8081:8081 -p 8024:8024 -p 80:80 -p 443:443 \
  -v "$PWD/conf:/etc/rustnps" \
  andreiyhub/rustnps:latest
```

启动客户端示例：

```bash
docker run --rm \
  -v "$PWD/conf:/etc/rustnps" \
  andreiyhub/rustnps:latest rnpc -config=/etc/rustnps/npc.conf
```

如果需要指定架构，可以在 `docker run` 前加 `--platform`，例如 `linux/amd64`、`linux/arm64`、`linux/arm/v7` 或 `linux/386`。

### 4. 启动服务端

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

### 5. 启动客户端

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
web_port=8081
bridge_port=18024
```

然后分别启动：

```bash
./target/release/rnps -conf_path=conf/nps.conf
./target/release/rnpc -config=conf/npc.conf
```
