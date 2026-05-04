# RustNps 功能说明

RustNps 是对 Go 版 nps/npc 的 Rust 重构实现，目标不是逐字复刻全部历史行为，而是把控制面、数据面和 Web 管理面做成安全、清晰、可维护的 Rust 版本。

本文保留一个面向迁移和重构的入口页，详细对标关系和缺口请继续看 [feature_map.md](feature_map.md) 与 [refactor_todos.md](refactor_todos.md)。

## 当前状态

RustNps 已经覆盖了大部分高频使用场景：TCP、UDP、HTTP、HTTPS、SOCKS5、file、secret、p2p、Web 管理页、登录验证码、端口自动分配、IP 白名单/黑名单、流量与连接限制、基础 Dashboard/API 都已存在。

仍然需要持续补齐的部分主要集中在：KCP/bridge 兼容、Proxy Protocol、健康检查调度、Go 版更深的运行时任务工厂、以及更完整的 Web 持久化和运维体验。

## 迁移建议

如果你正在从 Go 版 nps 迁移到 RustNps，可以先按下面顺序使用：

1. `nps.conf` / `npc.conf` 的基础字段。
2. 客户端、隧道、域名解析的常规增删改查操作。
3. HTTP、HTTPS、SOCKS5、TCP、UDP、file、secret、p2p 等常用转发模式。
4. Web 面板的登录、列表和 Dashboard 操作路径。

如果你只需要做日常穿透与面板管理，RustNps 目前已经能够覆盖大部分高频使用场景；如果你在追求与 yisier/nps 的最新行为一致，则需要继续阅读后面的映射和 todo。