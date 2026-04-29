Web 管理界面（Web UI）
=====================

简介
----
RustNps 自带一个轻量的 Web 管理页，便于查看客户端/隧道状态、修改配置、查看仪表盘与系统监控数据。Web 实现使用 `axum` 提供路由，模板为简单的文件替换，视图位于 `web/views`，静态文件位于 `web/static`。

关键实现位置
-------------
- 路由与渲染： [src/web.rs](src/web.rs)
- 模板视图： [web/views/layout.html](web/views/layout.html)、[web/views/dashboard.html](web/views/dashboard.html)
- 静态资源（JS/CSS）： [web/static/](web/static)
- 后端 API： `/client/list`、`/index/gettunnel`、`/api/dashboard` 等由 `src/web.rs` 调用 `src/server.rs` 的函数生成 JSON

用户与权限
---------
- `web_username` 为空时，服务端会默认以 `admin` 权限开放所有页面（适合内网测试环境）。
- 支持普通用户登录：`WebSession` 包含 `is_admin` 与 `client_id`，非管理员用户的请求会被限定到其 `client_id` 范围内。

常用页面概览
-----------
- Dashboard（`/` 或 `/index/index`）
  - 显示系统负载、CPU、内存、带宽、以及客户端/隧道的一些统计。
  - API：`/api/dashboard`（返回 `SystemSnapshot` 序列和汇总数据）。
- Client List（`/client/list`）
  - 显示已注册客户端的表格（VKey、Remark、在线状态、Addr、InletFlow/ExportFlow 等）。
  - API：`/client/list`（GET 页面，POST 返回表格数据）。
- Tunnel Pages（`/index/tcp` `/index/udp` `/index/all`）
  - 根据隧道 `mode` 进行筛选，也有 `All Tunnels` 页面显示全部。
  - 注意：`All Tunnels` 前端查询时不要传 `type=all` 给后端，以免错误过滤（`src/web.rs` 中 `handle_tunnel_add` 做了兼容处理）。
- 表单页（添加/编辑 client/host/tunnel）
  - 表单由 `render_form` 动态构建（`src/web.rs::render_tunnel_fields`、`render_client_fields` 等），表单字段名与后端变更接口一致，例如 `type`/`port`/`target` 等。

前端实现要点
-------------
- 模板渲染：函数 `load_view` 从 `web/views` 读取 HTML 模板，并用 `{{key}}` 占位替换变量（参见 `render_layout` 的 `vars`）。
- 表格与交互：采用 `bootstrap-table`、`jQuery` 和 `echarts`。Dashboard 的数据由 `/api/dashboard` 周期性轮询并刷新图表/进度条。
- 内存显示修复：Dashboard 模块使用服务器返回的 `total_mem` 与 `virtual_mem` 计算占用百分比并格式化为 GB/MB，两位小数显示。参考视图代码： [web/views/dashboard.html](web/views/dashboard.html)

修改表单字段（例如添加 `模式` 下拉）
---------------------------------
表单字段通过 `render_tunnel_fields`、`render_client_fields` 等在后端动态生成。如果需要添加或修改字段：

1. 编辑 `src/web.rs::render_tunnel_fields`，在生成 HTML 的 `select` 中添加或调整选项（例如将 `type` 值与显示标签对应）。
2. 如果要更改前端模板（静态布局），编辑 `web/views/form.html` 或对应视图。
3. 后端的变更处理在 `handle_post_mutation` 中统一路由到 `src/server.rs` 的 `mutate_*` 函数，确保表单 `name` 与后端参数名一致。

常见问题与调试
--------------
- 页面显示“管理员”而非登录用户名：`layout.html` 使用 `{{username}}` 占位，确保 `render_layout` 传入正确的 `session.username`（见 `src/web.rs::render_layout`）。
- Dashboard 内存进度条显示 100%：请确认 `SystemSnapshot.total_mem` 已由 `start_system_monitor` 填充（MB 单位），并由前端计算 `% = virtual_mem / total_mem`。
- 隧道类型显示为 `all`：确保 `tunnel` 的 `mode` 字段在后端保存为具体类型（如 `tcp`），并且前端在查询 `All Tunnels` 时不把 `type=all` 作为过滤值。

扩展
---
- 新增页面：在 `src/web.rs` 添加路由并创建对应 `web/views/*.html`。
- 国际化：页面使用 `langtag` 标记，静态资源中包含语言映射（修改 `web/static` 下语言文件）。

参考：
- 路由实现：[src/web.rs](src/web.rs)
- 模板视图： [web/views/layout.html](web/views/layout.html)
- Dashboard JS： [web/views/dashboard.html](web/views/dashboard.html)