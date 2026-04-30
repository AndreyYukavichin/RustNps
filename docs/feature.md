# RustNps 功能说明

RustNps 是对 Go 版 nps/npc 的 Rust 重构实现，目标不是逐字复刻全部历史行为，而是优先把控制面、数据面和 Web 管理面做成安全、清晰、可维护的 Rust 版本。

本文整理了 RustNps 当前已经对齐 Go 版 nps 的核心能力，以及仍然保留的差异点，方便发布、迁移和对外说明。

## 已对齐的核心能力

| Go nps 功能 | RustNps 现状 | 说明 |
| --- | --- | --- |
| 服务端 nps / 客户端 npc | 已实现 | `src/bin/nps.rs`、`src/bin/npc.rs` |
| `nps.conf` / `npc.conf` 配置解析 | 已实现 | 兼容常用字段与目录布局 |
| bridge 控制连接 | 已实现 | 控制连接与数据连接分离 |
| 客户端配置上报 | 已实现 | 客户端启动后会上报 hosts / tunnels |
| TCP / UDP 隧道 | 已实现 | 支持常规端口映射与短连接 UDP 转发 |
| HTTP 正向代理 | 已实现 | 支持 `CONNECT` |
| 域名代理 / URL location 路由 | 已实现 | 支持 HTTP 域名路由 |
| HTTPS 域名代理 / SNI 多证书 | 已实现 | 支持证书与 SNI 解析 |
| SOCKS5 代理 | 已实现 | 支持 no-auth CONNECT |
| file 模式 | 已实现 | 客户端本地文件读取 |
| secret / p2p | 已实现 | 当前以服务端中继兜底保证可用性 |
| Web 管理页 | 已实现 | 提供 Go 风格 Dashboard 与 API |
| 管理面板登录验证码 | 已实现 | `open_captcha=true` 时启用 |
| 自动分配隧道端口 | 已实现 | 新增、复制、编辑时端口为空或 0 会自动生成 |
| 自定义配置路径与 Web 目录 | 已实现 | `-conf_path` 可指定配置文件，Web 目录会随配置路径自动解析 |
| API 返回 ID | 已实现 | 新增客户端、主机、隧道时返回对应 `id` |
| 唯一验证密钥显示 | 已实现 | 客户端列表、域名列表、隧道列表均可查看 |
| 客户端黑名单 IP | 已实现 | 支持多个黑名单 IP |
| 全局黑名单 IP | 已实现 | 支持全局黑名单配置与访问拦截 |
| 客户端上次在线时间 | 已实现 | 客户端列表会展示 `LastOnlineTime` |
| `ip_limit` 注册与登录联动 | 已实现 | 注册授权与登录授权逻辑已保留 |
| `allow_rate_limit` / `allow_flow_limit` | 已实现 | 支持运行时限速与流量封顶 |
| `max_conn` / `max_tunnel_num` / `allow_ports` | 已实现 | 支持连接数、隧道数与端口白名单控制 |

## 最近对齐的发布点

以下能力是 RustNps 近期重点补齐，并且已经能按 Go 版的常见使用方式迁移：

### 管理面板登录验证码

当 `nps.conf` 中设置 `open_captcha=true` 时，RustNps 会在登录页显示验证码，并在提交登录时先校验验证码内容。这个能力用于增强 Web 管理面板的基础安全性。

### 隧道自动生成服务端口

新增、复制或编辑隧道时，如果端口没有填写或填写为 `0`，RustNps 会自动分配一个可用端口并回填到任务里。这样可以保持和 Go 版 nps 一致的“可空端口”管理体验。

### 自定义配置路径与 Web 目录

RustNps 支持 `-conf_path` 或 `--conf-path` 启动参数。指定配置文件后，RustNps 会优先按该配置文件所在目录自动寻找对应的 `web` 目录，从而同时加载配置与 Web 静态资源。

### 全局黑名单 IP

RustNps 已实现全局黑名单功能。全局黑名单会在访问控制阶段被统一拦截，适合用来阻断已知的恶意扫描或攻击源。

### 客户端上次在线时间

RustNps 在客户端列表中保留了 `LastOnlineTime` 和 `LastOnlineAddr`，便于在管理面板里快速判断客户端最近一次在线的时间与来源地址。

### API 返回 ID

新增客户端、主机和隧道时，会返回对应的 `id`，便于前端页面继续联动刷新，也方便脚本或外部系统做后续管理。

### 唯一验证密钥显示

客户端列表、域名解析列表和隧道列表中，RustNps 保留了 `VerifyKey` / `VKey` 的展示和查询逻辑，便于快速定位某个客户端对应的配置。

### 客户端黑名单 IP 与 IP 限制联动

RustNps 保留了 Go 版的客户端黑名单 IP、多 IP 白名单与 `ip_limit` 注册授权逻辑。管理面板登录成功后，也会对当前 IP 做授权处理，便于在开启 `ip_limit` 后正常使用面板。

## 与 Go 版仍有差异的部分

- KCP 与 Go 版 `nps_mux` 的 wire compatibility 仍未完全对齐。
- Go Web UI 的深度持久化能力还在演进中；当前 RustNps 提供的是可用的兼容层。
- 真正的 UDP NAT hole punching P2P 仍未完全实现；当前以服务端中继 fallback 为主，优先保证稳定性。

## 迁移建议

如果你正在从 Go 版 nps 迁移到 RustNps，可以优先沿用以下配置与习惯：

- `nps.conf` / `npc.conf` 的基础字段。
- 客户端、隧道、域名解析的常规增删改查操作。
- HTTP、HTTPS、SOCKS5、TCP、UDP、file、secret 等常用转发模式。
- Web 面板的登录、列表和 Dashboard 操作路径。

如果你只需要做日常穿透与面板管理，RustNps 目前已经覆盖了大部分高频使用场景。