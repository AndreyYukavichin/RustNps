# 增强功能

## 使用 HTTPS

### 方式一：服务端做 TLS 终止

在 `nps.conf` 中配置：

```ini
https_proxy_port=443
https_just_proxy=false
```

然后在域名路由中为每个域名填写证书和密钥：

- `cert_file_path`
- `key_file_path`

RustNps 会在 HTTPS 监听端口读取客户端的 TLS SNI，并自动选择匹配域名的证书。支持：

- 精确域名证书，例如 `api.example.com`
- 泛域名证书，例如 `*.example.com`
- 多证书并存，按 SNI 动态选择
- PEM 文本直填
- 文件路径加载

### 方式二：内网服务自己处理 HTTPS

在 `nps.conf` 中配置：

```ini
https_proxy_port=443
https_just_proxy=true
```

此时 RustNps 只依据 TLS ClientHello 中的 SNI 找到目标域名路由，然后把 TLS 原始流量透传给内网服务。

适用场景：

- 内网已经有 Nginx / Caddy / Apache
- 内网应用自己管理证书
- 希望在 RustNps 上只做入口和路由，不做 TLS 解密

## 证书填写方式

域名路由支持两种证书填写方式：

1. 文件路径

```text
/etc/ssl/example/fullchain.pem
/etc/ssl/example/privkey.pem
```

2. 直接粘贴 PEM 文本

```text
-----BEGIN CERTIFICATE-----
...
-----END CERTIFICATE-----
```

## 注意事项

- `https_just_proxy=false` 时，如果没有可用证书，TLS 握手不会成功。
- `https_just_proxy=true` 时，SNI 只用于选路，不支持按 HTTP 路径细分 HTTPS 路由。
- 当前 HTTPS 域名代理主要面向 HTTP/1.1 请求；HTTP/2 的某些特性需在客户端或上游处理。

## 数据面增强

RustNps 补齐并延续了 Go 版本常用的数据面能力：

- `compress=true`：链路使用 snappy 帧压缩。
- `crypt=true`：链路使用 TLS relay 加密。
- `rate_limit`：按客户端维度对传输速率限流。
- `flow_limit`：按客户端维度统计总流量并在达到上限后阻断。
- `ip_limit=true`：只允许已经通过 `npc register` 或 Web 登录登记过的来源 IP 访问。

推荐组合：

- 公网服务：`ip_limit=true` + `crypt=true`。
- 弱网或高延迟链路：先观察 CPU 与带宽使用，再决定是否打开 `compress=true`。
- 多租户环境：配合 `rate_limit`、`flow_limit`、`max_conn` 和 `max_tunnel_num` 一起约束资源。

---

## 扩展点与迁移建议

1. 新增隧道类型

	- 在 `src/model.rs` 中定义或约定新的 `Tunnel.mode` 字符串。
	- 在 `src/server.rs::start_tunnel_task` 的 `match` 分支中添加对新类型的处理，调用相应的 `start_*_listener`。
	- 如果数据面协议需要客户端配合，修改 `src/protocol.rs` 中 `LinkKind` 与 `BridgeHello` 的约定，并更新 `npc` 的实现。

2. Web 扩展

	- 页面模板位于 `web/views`，后端通过 `load_view` 做简单的 `{{key}}` 替换。新增页面：在 `src/web.rs` 注册路由并在 `web/views` 下添加模板。
	- 若需复杂前端交互，可以在 `web/static/` 添加 JS 模块并在模板中引用。

3. 持久化格式兼容

	- `src/store.rs::decode_client` 已经兼容多种旧版字段命名，迁移自 Go 的 `clients.json`/`tasks.json` 通常无需额外转换。

4. 性能与监控

	- 系统监控在 `src/server.rs::start_system_monitor` 中采样并写入 `Registry.system_history`，仪表盘 API `/api/dashboard` 使用这些数据。
	- 扩展监控（例如 Prometheus 导出）建议在 `src/web.rs` 新增一个 `/metrics` 路由，导出当前 `Registry` 中感兴趣的计数器。

5. 与 Go nps 的功能差异与迁移建议

	- 大多数配置键名保持兼容，但仍建议在迁移前在测试环境中验证 `clients.json` 与 `tasks.json` 的行为。
	- 如果有自定义脚本或二进制与 Go 版交互（例如管理脚本），请确认它们对 `LastOnlineAddr`、`Id` 等字段的依赖方式。

