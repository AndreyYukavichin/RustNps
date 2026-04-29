持久化与数据迁移
=================

摘要
----
RustNps 的运行时可将客户端、隧道与 host 等配置持久化到 `conf/clients.json`、`conf/tasks.json`、`conf/hosts.json`。持久化支持两种格式：

- JSON 数组（完整结构）
- 行分隔的 JSON 对象（每条记录之间以 `*#*` 分隔，兼容历史记录）

实现
----
持久化实现集中在 `src/store.rs`：

- `PersistentStore::load` 读取三份文件并调用 `decode_client` / `decode_tunnel` / `decode_host`。
- `PersistentStore::save_all` 会把 `clients` 遍历并写出 `clients.json`、`tasks.json`、`hosts.json`。
- `decode_*` 函数支持读取多版本字段，能把老版本的 `Id`、`VerifyKey` 等字段转换为当前 `ClientRuntimeConfig`、`Tunnel`、`Host` 结构。

迁移要点
-------
- 从 Go `nps` 导出的 `clients.json`（数组格式）通常可以直接复制到 `RustNps/conf/` 下使用。
- 如果遇到老格式（字段名驼峰或 PascalCase），`decode_client` 已包含兼容读取逻辑。
- `LastOnlineAddr` 字段会被解析并存进 `ClientRuntimeConfig.last_online_addr`，用于 Web 列表显示客户端最后来源地址。

冲突与注意
---------
- `PersistentStore` 在写入时会替换现有文件（原子写入），请确保磁盘空间充足。
- `save_all` 会过滤 `no_store` 标记，只有 `no_store=false` 的实体才会被写入。
- 在并发修改 `clients` 的场景下，请停止 `nps` 后再手动编辑 `conf/*.json`，避免运行时覆盖。

参考实现：
- [src/store.rs](src/store.rs)
- `ClientRuntimeConfig` 定义： [src/model.rs](src/model.rs)