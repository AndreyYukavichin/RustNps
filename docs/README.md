# RustNps 文档

这组文档是 RustNps 的技术入口页，重点放在配置、架构、协议、持久化和 Web 管理面的实现说明。用户手册和快速开始请优先看仓库根目录的 [README.md](../README.md)。

## 适合先读的页面

1. [install.md](install.md)
2. [run.md](run.md)
3. [server_config.md](server_config.md)
4. [example.md](example.md)
5. [nps_extend.md](nps_extend.md)

## 状态入口

- 功能对标和实现位置： [feature_map.md](feature_map.md)
- 重构待办和优先级： [refactor_todos.md](refactor_todos.md)
- 架构总览： [architecture.md](architecture.md)
- Web 管理页说明： [web_ui.md](web_ui.md)
- 持久化与迁移： [persistence.md](persistence.md)
- Docker 镜像与发布： [docker.md](docker.md)

## 说明

- RustNps 的配置字段尽量兼容 Go 版 `nps.conf` / `npc.conf`
- 当前 HTTPS 域名代理优先支持 HTTP/1.1，浏览器通常会自动回落到 HTTP/1.1
- `https_just_proxy=true` 模式下，SNI 只用于选路，不做 TLS 解密