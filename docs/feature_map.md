# RustNps 功能映射

这份文档把《NPS内网穿透系统Rust重构功能对标分析报告》里的功能项，映射到 RustNps 当前的实现位置，并标记状态。

状态说明：

- 已实现：RustNps 中已经有可运行实现，或者至少有等价的主路径实现。
- 部分实现：主路径存在，但与 Go 版相比仍有差异，或者只覆盖了常见场景。
- 未实现：当前代码里没有对应实现，或者只剩接口/注释/帮助文本。

## 控制面与启动链路

| 报告项 | 状态 | 代码位置 |
| --- | --- | --- |
| 服务端 nps / 客户端 npc | 已实现 | [src/bin/nps.rs](../src/bin/nps.rs), [src/bin/npc.rs](../src/bin/npc.rs) |
| `nps.conf` / `npc.conf` 配置解析 | 已实现 | [src/config.rs](../src/config.rs), [src/model.rs](../src/model.rs) |
| bridge 控制连接 | 已实现 | [src/protocol.rs](../src/protocol.rs), [src/server.rs](../src/server.rs), [src/client.rs](../src/client.rs) |
| 客户端配置上报 | 已实现 | [src/protocol.rs](../src/protocol.rs), [src/client.rs](../src/client.rs) |
| 多路复用数据面 | 已实现 | [src/mux.rs](../src/mux.rs), [src/relay.rs](../src/relay.rs) |
| 控制连接与数据连接分离 | 已实现 | [src/server.rs](../src/server.rs), [src/client.rs](../src/client.rs) |

## 数据面与代理模式

| 报告项 | 状态 | 代码位置 |
| --- | --- | --- |
| TCP 隧道 | 已实现 | [src/server.rs](../src/server.rs), [src/client.rs](../src/client.rs) |
| UDP 隧道 | 已实现 | [src/server.rs](../src/server.rs), [src/client.rs](../src/client.rs) |
| HTTP 正向代理 | 已实现 | [src/server.rs](../src/server.rs), [src/relay.rs](../src/relay.rs) |
| 域名代理 / URL location 路由 | 已实现 | [src/server.rs](../src/server.rs) |
| HTTPS 域名代理 / SNI 多证书 | 已实现 | [src/server.rs](../src/server.rs), [src/tls.rs](../src/tls.rs) |
| SOCKS5 代理 | 已实现 | [src/socks5.rs](../src/socks5.rs), [src/server.rs](../src/server.rs) |
| file 模式 | 已实现 | [src/client.rs](../src/client.rs) |
| secret 模式 | 已实现 | [src/client.rs](../src/client.rs), [src/server.rs](../src/server.rs) |
| p2p 模式 | 部分实现 | [src/client.rs](../src/client.rs), [src/server.rs](../src/server.rs) |

## Web 管理面

| 报告项 | 状态 | 代码位置 |
| --- | --- | --- |
| Web 管理页 | 已实现 | [src/web.rs](../src/web.rs), [src/server.rs](../src/server.rs) |
| Dashboard / API | 已实现 | [src/web.rs](../src/web.rs), [src/server.rs](../src/server.rs) |
| 登录验证码 | 已实现 | [src/web.rs](../src/web.rs), [src/server.rs](../src/server.rs) |
| 客户端 / 隧道 / 域名的增删改查 | 已实现 | [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs) |
| 自动分配隧道端口 | 已实现 | [src/server.rs](../src/server.rs) |
| API 返回 ID | 已实现 | [src/server.rs](../src/server.rs) |
| 唯一验证密钥展示 | 已实现 | [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs) |

## 安全与配额

| 报告项 | 状态 | 代码位置 |
| --- | --- | --- |
| 客户端黑名单 IP | 已实现 | [src/config.rs](../src/config.rs), [src/server.rs](../src/server.rs) |
| 全局黑名单 IP | 已实现 | [src/config.rs](../src/config.rs), [src/server.rs](../src/server.rs) |
| `ip_limit` 注册与登录联动 | 已实现 | [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs) |
| `allow_rate_limit` / `allow_flow_limit` | 已实现 | [src/relay.rs](../src/relay.rs), [src/server.rs](../src/server.rs) |
| `max_conn` / `max_tunnel_num` / `allow_ports` | 已实现 | [src/server.rs](../src/server.rs) |
| HTTP Basic 认证 | 已实现 | [src/relay.rs](../src/relay.rs), [src/server.rs](../src/server.rs) |
| Proxy Protocol | 已实现 | [src/config.rs](../src/config.rs), [src/server.rs](../src/server.rs), [src/relay.rs](../src/relay.rs), [src/client.rs](../src/client.rs) |

## 运行时与兼容性

| 报告项 | 状态 | 代码位置 |
| --- | --- | --- |
| TCP / TLS bridge | 已实现 | [src/client.rs](../src/client.rs), [src/server.rs](../src/server.rs), [src/tls.rs](../src/tls.rs) |
| KCP bridge | 已实现 | [src/bridge_transport.rs](../src/bridge_transport.rs), [src/client.rs](../src/client.rs), [src/server.rs](../src/server.rs), [src/mux.rs](../src/mux.rs) |
| 压缩 / 加密数据面包装 | 已实现 | [src/relay.rs](../src/relay.rs), [src/tls.rs](../src/tls.rs) |
| 健康检查 | 部分实现 | [src/config.rs](../src/config.rs), [src/model.rs](../src/model.rs), [src/store.rs](../src/store.rs) |
| 反向多路复用流关闭语义 | 已实现 | [src/relay.rs](../src/relay.rs), [src/mux.rs](../src/mux.rs) |
| Go Web UI 深度持久化 | 已实现 | [src/web.rs](../src/web.rs), [src/server.rs](../src/server.rs), [web/static/js/table-state.js](../web/static/js/table-state.js) |

## 结论

RustNps 已经覆盖了绝大多数常用穿透场景。当前最值得继续推进的缺口是健康检查调度、更完整的运维指标聚合，以及 p2p 的真实 NAT 打洞能力。详细 todo 请看 [refactor_todos.md](refactor_todos.md)。