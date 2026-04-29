系统监控与仪表盘
=================

简介
----
RustNps 定期采集主机的 CPU、内存、负载及网络 IO，用于 Web 仪表盘显示与历史趋势。采集逻辑在 `src/server.rs` 的 `start_system_monitor` 中实现，使用 `sysinfo` crate 获取系统指标。

采样与历史
-----------
- 采样周期：3 秒。
- 保留历史：10 条最近采样数据（`Registry.system_history`）。
- 指标：CPU 使用率（%）、已用内存（MB）、总内存（MB）、Swap（MB）、Load 平均值（1/5/15）、网络每秒发送/接收速率（B/s）、估算的 TCP/UDP 隧道数量。

ServerSnapshot 字段
--------------------
- `time`：采样时间字符串（HH:MM:SS）
- `load1`、`load5`、`load15`：系统负载平均
- `cpu`：CPU 使用百分比
- `virtual_mem`：已用内存，单位 MB
- `total_mem`：总内存，单位 MB
- `swap_mem`：已用 Swap，单位 MB
- `tcp`、`udp`：估算的监听隧道数量
- `io_send`、`io_recv`：每秒网络平均发送/接收速率（B/s）

Web 显示要点
-------------
- 内存进度条：前端从 `/api/dashboard` 获取 `virtual_mem` 与 `total_mem`，并按 `percent = virtual_mem / total_mem` 计算宽度。
- 数值格式化：前端展示将 `virtual_mem`/`total_mem` 格式化为 GB/MB 两位小数，例如 `27.57 GB / 63.66 GB`。
- 带宽显示：以 B/s 为单位展示上行/下行，也可以在前端按需转换为 KB/s、MB/s。

调试提示
-------
- 如果内存进度条一直为 100%：请确认 `start_system_monitor` 中 `total_mem` 正确赋值（单位 MB），以及 `/api/dashboard` 返回的 JSON 中包含 `total_mem` 字段。
- 在 Windows 上，`sysinfo` 的一些指标可能与 Linux 行为不同，测试时请留意单位差异。

参考：
- 采集实现： [src/server.rs::start_system_monitor](src/server.rs#L150)
- Dashboard 前端： [web/views/dashboard.html](web/views/dashboard.html)