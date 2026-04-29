# RustNps 文档

RustNps 是 Go 版 `nps/npc` 的 Rust 重构工程，目标是保留原版的使用方式和主要配置习惯，同时逐步把控制面、数据面、Web 管理和协议能力迁移到 Rust 实现。

当前版本已经覆盖以下核心能力：

- `nps` 服务端 / `npc` 客户端基础运行
- TCP、UDP、HTTP 代理、SOCKS5、secret、p2p fallback、file 模式
- Web 管理页与基础 API
- 域名代理 HTTP 路由
- HTTPS 监听、TLS 终止、SNI 多证书选择
- `https_just_proxy=true` 时按 SNI 透传 HTTPS 到内网服务
- Rust 版 bridge mux 连接复用
- `compress` / `crypt` 数据面包装，支持 snappy 压缩和 TLS relay
- `ip_limit` 访问控制，以及 `npc register` / Web 登录自动登记来源 IP
- `allow_rate_limit` / `allow_flow_limit` 的运行时限速和流量封顶

建议阅读顺序：

1. [install.md](install.md)
2. [run.md](run.md)
3. [server_config.md](server_config.md)
4. [example.md](example.md)
5. [nps_extend.md](nps_extend.md)

说明：

- RustNps 的配置字段尽量兼容 Go 版 `nps.conf` / `npc.conf`
- 当前 HTTPS 域名代理优先支持 HTTP/1.1，浏览器通常会自动回落到 HTTP/1.1
- `https_just_proxy=true` 模式下，SNI 只用于选路，不做 TLS 解密