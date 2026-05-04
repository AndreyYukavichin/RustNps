# RustNps Refactor TODOs

这份清单从“还能补什么”出发，按优先级整理成后续重构任务。

## P0

1. KCP bridge 支持
   - 现状：`npc` 与服务端 bridge 都已接入 KCP transport，不再降级到 tcp。
   - 进展：bridge listener / client connect / mux / visitor / control / health 共享同一 transport 抽象，KCP round-trip 也已补测。
   - 结果：KCP bridge 支持已完成。
   - 主要入口： [src/client.rs](../src/client.rs), [src/server.rs](../src/server.rs), [src/protocol.rs](../src/protocol.rs)

2. Proxy Protocol 全链路确认
   - 现状：TCP 隧道和 host proxy 的 `proto_version` 传递、请求生成和服务端消费都已打通。
   - 进展：已把 `proto_version` 接到客户端 TCP/HTTP 目标连接，host proxy 也会随管理面配置一起下发，并补了 v1/v2 单测和 host 端回归。
   - 结果：Proxy Protocol v1/v2 的全链路确认已完成。
   - 主要入口： [src/config.rs](../src/config.rs), [src/client.rs](../src/client.rs), [src/relay.rs](../src/relay.rs), [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs)

3. 健康检查调度
   - 现状：健康状态已经从客户端上报到服务端，运行时 target 屏蔽列表由报告驱动。
   - 进展：客户端侧 health 监控已补齐，服务端会按上报更新屏蔽列表，client rows 和 dashboard 也暴露了健康摘要。
   - 目标：继续收敛 health check 的恢复语义和管理面展示，让它更贴近 Go 版的运行时体验。
   - 主要入口： [src/config.rs](../src/config.rs), [src/model.rs](../src/model.rs), [src/server.rs](../src/server.rs), [src/store.rs](../src/store.rs)

## P1

4. 任务热更新语义收敛
   - 现状：已有增删改接口，但需要继续对齐 Go 版的启动/停止/删除生命周期。
   - 进展：已实现。客户端编辑会迁移运行时状态，vkey 重命名会同步健康屏蔽与流量计数器；客户端 disable/delete 会清理 health runtime state，health 摘要也会把停用态显示为 disabled；tunnel add/edit/del/start/stop 也已收敛到共享 runtime helper，host 侧也补上了共享 mutation helper 和 runtime refresh hook。
   - 目标：让新增、编辑、删除都能可靠地影响在线监听器和运行时状态。
   - 主要入口： [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs)

5. Dashboard 指标聚合增强
   - 现状：已补上基础 Dashboard、生命周期聚合卡片、health 聚合卡片和周期刷新；历史系统曲线也已在首页保留。
   - 目标：继续补充更细的统计维度时，优先沿用 `dashboard_json_scoped` 的聚合口径。
   - 主要入口： [src/server.rs](../src/server.rs), [src/web.rs](../src/web.rs)



## P2

6. Web 深度持久化
   - 现状：已实现。列表页会记住分页、搜索、排序和列显示状态，刷新后可回填；dashboard 也在轮询刷新中保持聚合视图。
   - 目标：后续扩展更细的 Web 状态时，继续优先复用页面级状态恢复与后端聚合口径。
   - 主要入口： [src/web.rs](../src/web.rs), [src/store.rs](../src/store.rs), [src/server.rs](../src/server.rs)

7. 文档继续收敛到单一事实来源
   - 现状：README、feature、报告文档之间信息有一定重复。
   - 目标：把 README 保持成用户文档，把 feature_map / refactor_todos 保持成工程入口。
   - 主要入口： [README.md](../README.md), [docs/feature.md](feature.md), [docs/feature_map.md](feature_map.md)