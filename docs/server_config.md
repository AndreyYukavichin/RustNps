# 服务端配置

RustNps 服务端配置文件通常是 `conf/nps.conf`。

常用字段如下：

名称 | 含义
---|---
`web_port` | Web 管理端口
`web_username` | Web 管理账号
`web_password` | Web 管理密码
`web_base_url` | Web 子路径部署前缀
`bridge_ip` | bridge 监听 IP
`bridge_port` | 客户端控制连接和 mux 连接监听端口
`bridge_type` | 连接类型，当前主要使用 `tcp`
`http_proxy_ip` | HTTP/HTTPS 域名代理监听 IP
`http_proxy_port` | HTTP 域名代理监听端口
`https_proxy_port` | HTTPS 域名代理监听端口
`https_just_proxy` | 为 `true` 时透传 HTTPS 到内网，不在服务端解密 TLS
`public_vkey` | 配置文件模式下客户端默认密钥
`disconnect_timeout` | 客户端/链路超时时间（秒）
`allow_user_login` | 是否允许客户端用户登录 Web
`allow_user_register` | 是否允许用户注册
`ip_limit` | 为 `true` 时，只允许已登记来源 IP 建立访问
`allow_local_proxy` | 是否允许代理到服务端本地地址
`allow_multi_ip` | 是否允许隧道监听指定服务端 IP
`allow_rate_limit` | 是否允许客户端配置运行时带宽限速
`allow_flow_limit` | 是否允许客户端配置总流量封顶
`allow_tunnel_num_limit` | 是否允许客户端配置最大隧道数
`allow_connection_num_limit` | 是否允许客户端配置最大并发连接数
`allow_ports` | 允许监听的服务端端口或端口段，逗号分隔
`tls_enable` | bridge TLS 开关元数据，当前 HTTPS 域名代理与其独立
`tls_bridge_port` | bridge TLS 端口预留字段
`p2p_ip` | p2p 使用的服务端 IP
`p2p_port` | p2p 使用的 UDP 端口
`log_level` | 日志等级
`log_path` | 日志路径

## HTTPS 相关

### 1. 服务端 TLS 终止

```ini
https_proxy_port=443
https_just_proxy=false
```

然后在域名路由中为每个域名配置：

- `scheme=https` 或 `scheme=all`
- `cert_file_path`
- `key_file_path`

证书和密钥支持两种形式：

- 文件路径
- 直接填写 PEM 文本

### 2. HTTPS 透传

```ini
https_proxy_port=443
https_just_proxy=true
```

此时 RustNps 只读取 ClientHello 中的 SNI 做路由，不在服务端解密 TLS，后端内网服务需要自己处理 HTTPS 证书。

## `ip_limit` 访问控制

```ini
ip_limit=true
allow_user_login=true
allow_user_register=true
```

开启后，服务端只接受已经登记过的来源 IP。

- 配置文件模式客户端可以执行 `npc register -server=<server_addr> -vkey=<vkey>` 进行登记
- 允许 Web 登录时，登录成功的来源 IP 会自动放行 2 小时
- 超时后需要重新登录或再次执行 `npc register`

这项能力适合放在公网环境中，把控制面和用户访问面都收紧到临时授权 IP。

## 运行时限速与流量封顶

```ini
allow_rate_limit=true
allow_flow_limit=true
allow_connection_num_limit=true
allow_tunnel_num_limit=true
```

这些开关决定客户端是否可以声明自己的运行时资源限制：

- `rate_limit`：每秒带宽上限，单位 KB/s
- `flow_limit`：累计总流量上限，单位 MB
- `max_conn`：客户端最大并发连接数
- `max_tunnel_num`：客户端最大隧道/域名数量

其中 `flow_limit` 命中后，后续新连接会被拒绝，已有连接在继续传输时也会被中止。

## `compress` / `crypt` 数据面语义

客户端可以在 `npc.conf` 或 Web 管理页里为某个 client 打开：

- `compress=true`：数据面使用 snappy 帧压缩
- `crypt=true`：数据面使用 TLS relay 加密

行为与 Go 版约定保持一致：

- 文件模式不包装压缩或加密层
- `crypt=true` 时优先使用 TLS relay
- 仅在普通数据面链路上包装，控制面协议仍走 RustNps 自己的 bridge 帧