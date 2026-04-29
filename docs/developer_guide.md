开发指南
========

仓库结构（高层）
----------------
- `src/`：Rust 源码
  - `server.rs`：服务端主逻辑与 bridge 监听
  - `client.rs`：客户端实现
  - `web.rs`：Web 路由与模板渲染
  - `protocol.rs`：桥接协议定义与序列化
  - `relay.rs`：数据转发与 IO 抽象
  - `store.rs`：持久化实现
  - `model.rs`：数据模型定义
- `web/`：Web 静态内容与模板
- `conf/`：示例配置与持久化文件
- `docs/`：项目文档（本目录）

快速开发流程
-------------
1. 构建：`cargo build` 或 `cargo build --release`。
2. 运行服务端：`cargo run --bin nps -- -conf_path conf/nps.conf`。
3. 运行客户端：`cargo run --bin npc -- -config conf/npc.conf`。
4. 运行测试：`cargo test`（已通过若干单元测试）。

添加新隧道类型（示例）
-------------------
1. 在 `src/model.rs` 中确定新的 `mode` 字符串（例如 `myproto`）。
2. 在 `src/server.rs::start_tunnel_task` 的 `match mode.as_str()` 分支中添加对应的 `start_myproto_listener` 调用。
3. 新增 `start_myproto_listener` 函数，复用 `start_tcp_listener` 的连接接受和 `request_client_stream` 流程。
4. 如果需要客户端支持，请在 `src/protocol.rs::LinkKind` 添加枚举并在客户端实现对应的逻辑。

Web 模板开发
------------
- 模板位于 `web/views`，后端通过 `load_view` 读取并替换 `{{key}}`。增加新页面需要：
  1. 在 `src/web.rs` 添加路由函数并在 `start_web_manager` 注册路由。
  2. 编写 `web/views/your_page.html` 模板并在 `render_layout` 中通过 `load_view` 渲染。

注意事项
-------
- 大多数共享结构使用 `Arc<Registry>` 与 `Mutex` 保护，请注意死锁与锁粒度。
- `PersistentStore::save_all` 的写入是原子操作，会覆盖原文件，避免在写入时对文件手动修改。
- 新增公共 API 时，注意 Web 层 `current_session` 与 `authorize_mutation` 的权限控制。

提交指南
-------
- 保持改动最小化并包含必要的单元测试。
- 代码风格：遵循 Rust 的 idiomatic 风格，使用 `cargo fmt` 保持格式一致。
- 文档：为重要变更更新 `docs/` 相应页面。