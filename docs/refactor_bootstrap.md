# Refactor Bootstrap

这份文档给下一步代码重构一个最小启动面，避免后续工作继续散落在口头清单里。

## Phase 1

- 先把 P0 缺口做成独立测试或显式 stub。
- 每个缺口都要有一个可运行检查，避免后续重构时再次回退。
- 优先顺序：Health Check -> 任务/Host 生命周期。KCP 和 Proxy Protocol 已完成。

## Phase 2

- 把任务生命周期收敛到统一入口。
- 将 add/edit/del/start/stop 的 side effects 从 Web 层剥离到 server 层。
- tunnel add/edit/del/start/stop 已开始收敛到共享 runtime helper，host 侧也补上了共享 mutation helper 和 runtime refresh hook。
- 保持现有接口不变，只调整内部 orchestration。

## Phase 3


- 最后统一整理 README、feature_map 和 todo 文档，保证文档和代码保持同步。