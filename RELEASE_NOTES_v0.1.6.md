# RustNps v0.1.6 — Release Notes

Release date: 2026-05-04

Summary
-------
本次版本主要聚焦于可发布性、Web 管理体验和运行时可观测性补齐。RustNps 现在已经可以通过 GitHub Release 和 Docker Hub 两条路径对外发布，同时继续收敛 Dashboard 聚合、健康状态与 Web 列表状态回填等能力。

Highlights
----------
- 发布与打包
  - 新增 Docker 多架构发布流水线，镜像名为 `andreiyhub/rustnps`，支持 `linux/amd64`、`linux/386`、`linux/arm64` 和 `linux/arm/v7`。
  - 新增容器启动入口，支持通过同一个镜像运行 `rnps` 或 `rnpc`。
  - 补充 Docker 使用文档，方便本地构建、运行和发布到 Docker Hub。

- Web 管理面
  - Dashboard 增加生命周期聚合与健康聚合展示。
  - 列表页支持分页、搜索、排序和列显示状态的本地持久化回填。
  - Web 管理页与文档默认端口统一修正为 `8081`。

- 运行时状态与指标
  - 健康检查汇总继续收敛到服务端统一聚合接口。
  - 客户端停用、删除以及相关生命周期变更会更严格地清理运行时健康状态。
  - Dashboard 继续沿用 `dashboard_json_scoped` 作为聚合入口，便于后续扩展更多统计维度。

- 协议与能力
  - KCP bridge、Proxy Protocol、Web 深度持久化等前期完成项继续保留，并随文档整理同步更新。

Compatibility / Breaking Changes
--------------------------------
本次没有引入破坏性协议变更。管理面和 Web 行为有所增强，但现有配置文件仍可继续使用。

Upgrade Notes
-------------
- 如果你使用的是旧版 Web 管理页缓存，首次升级后建议刷新浏览器缓存，以便加载新的前端状态恢复逻辑。
- 如果你计划直接使用 Docker 镜像发布，请在仓库 Secrets 中准备 `DOCKERHUB_USERNAME` 和 `DOCKERHUB_TOKEN`。
- 现有示例配置中的 Web 端口以 `8081` 为准，请检查自定义文档或部署脚本是否仍引用旧端口 `18081`。

Release / 发布步骤（手动）
--------------------------------
建议按下面顺序完成 `v0.1.6` 发布：

1. 提交变更并打 tag

```bash
git add -A
git commit -m "release: v0.1.6"
git tag -a v0.1.6 -m "RustNps v0.1.6"
git push origin main
git push origin v0.1.6
```

2. 创建 GitHub Release

```bash
gh release create v0.1.6 --title "v0.1.6" --notes-file RustNps/RELEASE_NOTES_v0.1.6.md
```

3. 如果需要上传构建产物，可以在同一条命令后附加二进制包或压缩包路径，例如：

```bash
gh release create v0.1.6 --title "v0.1.6" --notes-file RustNps/RELEASE_NOTES_v0.1.6.md dist/*
```

4. 推送 tag 后，Docker 发布工作流会自动构建并推送 `andreiyhub/rustnps` 多架构镜像。

感谢
------
感谢为本版本参与测试、反馈和验证的同学。如发现回归或发布问题，请附上平台、构建日志和复现步骤。