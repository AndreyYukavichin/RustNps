# RustNps v0.1.2 — Release Notes

Release date: 2026-04-30

Summary
-------
本次小版本聚焦于管理面板的用户体验修复与前端稳定性改进，同时完善了跨平台构建/发布流程说明，便于生成多平台二进制包供发布。

Highlights
----------
- 前端（Web UI）
  - 登录页验证码改进：点击验证码图片只刷新验证码（AJAX），不会导致整页刷新；同时会重置输入框，避免误提交。
  - 登录防重提交：移除重复提交导致的“验证码错误”假阳性问题。
  - 侧栏导航改为局部加载：左侧菜单点击采用 AJAX + history.pushState，只替换正文区域（`#router-view`），提升交互响应速度并避免整页刷新。
  - 仪表盘修复：修复在 AJAX 导航后 `ibox` 的折叠/关闭按钮失效问题；为所有图表面板（Load/CPU/Memory/Connections/Bandwidth/Traffic/Type）补齐 `ibox-tools`（统一的折叠和关闭按钮），并将 `inspinia.js` 里对这些按钮的事件绑定改为事件委托，确保动态注入的内容也能响应。

- 构建与发布
  - 文档中新增了详细的本机与跨目标构建示例（Rust/Cargo 与 Go），并给出了打包、生成校验和与 GitHub Release 上传的示例步骤。

- 其它
  - 若干小的稳定性修复与国际化加载初始化顺序调整。

Compatibility / Breaking Changes
--------------------------------
无破坏性 API 变更。本次主要为前端行为和构建/发布说明更新，使用者无需更改运行时配置。

Upgrade Notes
-------------
- 如果你在生产环境部署了旧版前端资源（浏览器缓存），建议在发布后通知运维/用户清除浏览器缓存或通过 CDN 添加版本缓存字段以确保新版静态资源生效。

Build instructions
------------------
下面给出 `RustNps`（Rust）和 `nps`（Go）项目的常见构建、跨平台构建与打包示例。

Prerequisites
* Rust + Cargo (stable)。推荐安装 `cross`（用于跨编译）：

```bash
cargo install cross
```

* Go toolchain（用于 `nps` Go 项目，若需要构建 Go 版本）：

```bash
# 推荐 Go 1.20+
go version
```

A. 构建 RustNps（二进制）

1) 本机（调试/发布）

```bash
cd RustNps
# 本机 release
cargo build --release
# 生成的二进制位于 target/release/nps 和 target/release/npc
```

2) 使用 `cross` 构建常见 Linux 目标（在 Linux 或 CI 中使用）

```bash
cd RustNps
cross build --release --target x86_64-unknown-linux-gnu
cross build --release --target aarch64-unknown-linux-gnu
```

3) Windows 构建（在 Windows 或使用 cross）

```bash
cross build --release --target x86_64-pc-windows-gnu
# 生成 .exe 文件
```

4) macOS 构建

macOS 原生目标建议在 macOS runner / 机器上构建：

```bash
# Intel macOS
cargo build --release --target x86_64-apple-darwin
# Apple Silicon
cargo build --release --target aarch64-apple-darwin
```

注意：跨编译 macOS 目标从非 macOS 主机较为复杂，推荐在 macOS CI runner 上构建。

5) 打包示例（Linux x86_64）

```bash
cd RustNps
mkdir -p dist/rnps-v0.1.2-linux-amd64
cp target/x86_64-unknown-linux-gnu/release/nps dist/rnps-v0.1.2-linux-amd64/rnps
cp target/x86_64-unknown-linux-gnu/release/npc dist/rnps-v0.1.2-linux-amd64/rnpc
cp -r conf dist/rnps-v0.1.2-linux-amd64/conf
tar -czf rnps-v0.1.2-linux-amd64.tar.gz -C dist rnps-v0.1.2-linux-amd64
# 生成 sha256
sha256sum rnps-v0.1.2-linux-amd64.tar.gz > rnps-v0.1.2-linux-amd64.tar.gz.sha256
```

B. 构建 Go `nps`（可选）

项目中仍保留原生 Go 实现 `nps`：

```bash
cd nps
# 若仓库自带 Makefile
make
# 或手动构建
GOOS=linux GOARCH=amd64 go build -o nps ./cmd/nps
GOOS=linux GOARCH=amd64 go build -o npc ./cmd/npc

# cross-build arm64
GOOS=linux GOARCH=arm64 go build -o nps-arm64 ./cmd/nps
```

同样打包为 tar/zip，并生成 sha256 校验。

Release / 发布步骤（建议）
--------------------------------
下面示例以手动/半自动方式说明如何发布 `v0.1.2`：

1. 更新版本与变更说明

- 在 `RustNps/Cargo.toml`（如有手动版本字段）或 README 中更新版本号（可选）。
- 将本文件 `RELEASE_NOTES_v0.1.2.md` 作为发布说明保存。

2. 提交並打 tag

```bash
git add -A
git commit -m "release: v0.1.2 — UI fixes, dashboard & build docs"
git tag -a v0.1.2 -m "RustNps v0.1.2"
git push origin main
git push origin v0.1.2
```

3. 使用 CI（推荐）或本地构建生成多平台二进制

- 如果你已在仓库配置了 GitHub Actions 流水线，推送 tag 通常会触发 release workflow（自动构建并上传 artifacts）。
- 否则在本地/CI 上运行上文中的构建与打包命令，生成 `dist/` 下的归档文件。

4. 创建 GitHub Release 并上传二进制与校验文件

可以使用 GitHub CLI（`gh`）快速上传：

```bash
# 示例：将所有 dist 下的包上传到 release
gh release create v0.1.2 --title "v0.1.2" --notes-file RustNps/RELEASE_NOTES_v0.1.2.md dist/*
```

或在 GitHub Web UI 中创建 release，粘贴本文件内容为 Release Notes，并手动上传打包文件及 `.sha256`。

5. 验证发布

- 在发布页面下载一个二进制，运行 `sha256sum -c <file>.sha256` 验证完整性。
- 在目标平台上启动并做一次快速 smoke test（登录面板、打开仪表盘、检查验证码刷新行为）。

6. 后续

- 如需将 macOS 二进制做 notarize，请在 macOS 环境上进行代码签名与 notarization（不在本说明详细展开）。
- 在 README 或项目主页更新下载链接与版本号。

Appendix: 常用命令速览
--------------------------------
```bash
# 快速构建（Rust 本机）
cd RustNps && cargo build --release

# cross (Linux x86_64 / aarch64)
cross build --release --target x86_64-unknown-linux-gnu
cross build --release --target aarch64-unknown-linux-gnu

# Go build
cd nps
GOOS=linux GOARCH=amd64 go build -o nps ./cmd/nps

# 创建 Git tag 并触发 CI
git tag -a v0.1.2 -m "v0.1.2" && git push origin v0.1.2

# 使用 gh 上传 release 说明与二进制
gh release create v0.1.2 --title "v0.1.2" --notes-file RustNps/RELEASE_NOTES_v0.1.2.md dist/*
```

感谢
------
感谢为本版本贡献修复与测试的同学。如发现回归或构建问题，请在仓库中开 issue 并附上平台与构建日志。
