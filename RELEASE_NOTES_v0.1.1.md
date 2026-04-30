# RustNps v0.1.1

RustNps v0.1.1 是一个面向可发布与可迁移的整理版本，重点补齐了 Web 管理体验、配置路径联动，以及 GitHub Release 多架构产物发布链路。

## 本次更新

- 新增管理面板登录验证码支持，`open_captcha=true` 时登录页会显示验证码并在登录时校验。
- 新增隧道自动分配服务端口，新增、复制、编辑隧道时端口为空或 `0` 会自动回填可用端口。
- 支持 `-conf_path` / `--conf-path` 自定义配置路径，并自动按配置目录解析对应的 `web` 资源目录。
- 管理面板与 API 保留新增对象返回 `id` 的行为，便于前端和脚本联动。
- 客户端列表补齐 `LastOnlineTime` 和 `LastOnlineAddr` 展示。
- 保留客户端黑名单 IP、全局黑名单 IP 与 `ip_limit` 注册/登录联动能力。
- 新增 GitHub Actions Release 工作流，可自动构建多平台发布包并上传到 GitHub Releases。

## 发布产物

本版本会为以下目标生成独立的 `rnps` 与 `rnpc` 压缩包：

- Linux `x86_64-unknown-linux-gnu`
- Linux `aarch64-unknown-linux-gnu`
- Linux `armv7-unknown-linux-gnueabihf`
- Windows `x86_64-pc-windows-msvc`
- Windows `aarch64-pc-windows-msvc`
- macOS `x86_64-apple-darwin`
- macOS `aarch64-apple-darwin`

其中服务端压缩包包含：

- `rnps` 可执行文件
- `conf/nps.conf`
- `web/` 管理面板静态资源

客户端压缩包包含：

- `rnpc` 可执行文件
- `conf/npc.conf`

## 升级说明

- 从 `v0.1.0` 升级到 `v0.1.1` 无需调整协议层使用方式。
- 如果你通过自定义目录启动服务端，建议统一使用 `-conf_path`，这样配置文件与 Web 资源会按同一目录解析。
- 如果你启用 Web 管理面板，建议同时开启验证码并检查 `web_username`、`web_password` 配置。

## 当前仍保留的差异

- 与 Go 版 `nps_mux` 的完全 wire compatibility 仍未完成。
- P2P 仍以服务端中继 fallback 为主，尚未完全实现真正的 UDP NAT hole punching。
- Web 管理面板的深度持久化能力仍在继续完善。

## 致谢

RustNps 参考并致敬以下项目：

- [ehang-io/nps](https://github.com/ehang-io/nps)
- [yisier/nps](https://github.com/yisier/nps)