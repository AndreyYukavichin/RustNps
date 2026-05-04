# RustNps Refactor TODOs

这份清单从“还能补什么”出发，按优先级整理成后续重构任务。

## P0

1. KCP bridge 支持
   - 现状：`npc` 帮助文本仍然只声明 TCP 可用，KCP 没有运行时实现。
   - 目标：补齐 KCP transport 或明确降级策略，让 bridge 连接类型与 Go 版更接近。
   - 主要入口： [src/client.rs](../src/client.rs), [src/server.rs](../src/server.rs), [src/protocol.rs](../src/protocol.rs)

2. Proxy Protocol 全链路确认
   - 现状：配置字段和部分元数据已经存在，但还需要明确握手、透传和服务端消费是否完整覆盖。
   - 进展：已把 `proto_version` 接到客户端 TCP/HTTP 目标连接，并补了 v1/v2 单测。
   - 目标：保证 TCP 隧道和 host proxy 场景都能正确生成和使用 Proxy Protocol v1/v2。
   - 主要入口： [src/config.rs](../src/config.rs), [src/relay.rs](../src/relay.rs), [src/server.rs](../src/server.rs)

3. 健康检查调度
   - 现状：健康状态已经从客户端上报到服务端，运行时 target 屏蔽列表由报告驱动。
   - 进展：客户端侧 health 监控已补齐，服务端会按上报更新屏蔽列表，client rows 和 dashboard 也暴露了健康摘要。
   - 目标：继续收敛 health check 的恢复语义和管理面展示，让它更贴近 Go 版的运行时体验。
   - 主要入口： [src/config.rs](../src/config.rs), [src/model.rs](../src/model.rs), [src/server.rs](../src/server.rs), [src/store.rs](../src/store.rs)

## P1

4. 任务热更新语义收敛
   - 现状：已有增删改接口，但需要继续对齐 Go 版的启动/停止/删除生命周期。
   - 目标：让新增、编辑、删除都能可靠地影响在线监听器和运行时状态。
   - 主要入口： [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs)

5. Dashboard 指标聚合增强
   - 现状：基础 Dashboard 已有，但更深的统计维度仍可补齐。
   - 目标：补上更稳定的实时统计和历史视图。
   - 主要入口： [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs)



## P2

6. Web 深度持久化
   - 现状：内存态和 JSON 持久化已经能支撑常见操作，但还可以更完整地对齐 Go 版运维体验。
   - 目标：把 Web 端的状态同步、页面回填和局部刷新做得更稳定。
   - 主要入口： [src/web.rs](../src/web.rs), [src/store.rs](../src/store.rs), [src/server.rs](../src/server.rs)

7. 文档继续收敛到单一事实来源
   - 现状：README、feature、报告文档之间信息有一定重复。
   - 目标：把 README 保持成用户文档，把 feature_map / refactor_todos 保持成工程入口。
   - 主要入口： [README.md](../README.md), [docs/feature.md](feature.md), [docs/feature_map.md](feature_map.md)