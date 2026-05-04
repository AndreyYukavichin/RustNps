# 安装

## 概览

本章介绍在常见平台上构建与运行 `RustNps` 的步骤、常见注意事项以及示例服务运行方式。

如果你只想快速试用：在 `RustNps` 目录执行 `cargo build --release`，然后运行 `target/release/rnps -conf_path conf/nps.conf` 即可启动服务端。

---

## 先决条件

- 安装 Rust（通过 `rustup` 管理工具链）。推荐使用稳定通道：

	- Linux / macOS:

		```bash
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
		```

	- Windows:

		使用 `rustup` 安装器或通过 `winget`/`choco` 安装 `rustup`。

- 推荐安装 `cargo`, `rustc`（由 `rustup` 提供）。
- 在部分平台上若启用 TLS 相关功能，可能需要系统的 OpenSSL dev 包（Linux：`libssl-dev` / `openssl-devel`，Windows：通过 vcpkg 或 OpenSSL MSI 安装）。如果构建失败，请参考编译错误提示安装对应的系统依赖。

---

## 获取源码

从仓库根目录进入 `RustNps` 子目录：

```bash
cd RustNps
```

（本说明假定你在 `RustNps` 目录下执行后续命令）

---

## 构建

- 开发构建：

```bash
cargo build
```

- 发布构建：

```bash
cargo build --release
```

编译成功后可在 `target/release/` 下找到二进制：

- Windows: `target\\release\\rnps.exe`、`target\\release\\rnpc.exe`
- Linux/macOS: `target/release/rnps`、`target/release/rnpc`

---

## 运行

- 直接运行（示例）：

```bash
# 启动服务端（使用仓库内示例配置）
./target/release/rnps -conf_path conf/nps.conf

# 启动客户端（同一台机器用于本地联调）
./target/release/rnpc -config conf/npc.conf
```

- 使用 `cargo run`（用于开发调试）：

```bash
cargo run --bin rnps -- -conf_path conf/nps.conf
cargo run --bin rnpc -- -config conf/npc.conf
```

Windows PowerShell 示例：

```powershell
.\target\\release\\rnps.exe -conf_path conf\\nps.conf
.\target\\release\\rnpc.exe -config conf\\npc.conf
```

---

## 常用命令（验证 & 调试）

```bash
cargo fmt     # 格式化
cargo check   # 快速类型检查
cargo test    # 运行单元测试
```

若构建出错，请先运行 `cargo check` 并根据提示安装缺失的系统依赖。

---

## 运行在生产环境（建议）

- 在 Linux 系统上可以使用 systemd 管理 `rnps`：

示例 `nps.service`：

```ini
[Unit]
Description=RustNps Service
After=network.target

[Service]
Type=simple
User=nps
WorkingDirectory=/opt/RustNps
ExecStart=/opt/RustNps/target/release/rnps -conf_path /opt/RustNps/conf/nps.conf
Restart=on-failure
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

- Windows 上可使用 NSSM、sc create 或类似工具注册为服务。

---

## 交叉编译（常见示例）

安装目标并交叉编译示例：

```bash
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

交叉编译过程中若遇到依赖的 C 库（如 OpenSSL），需要为目标平台准备相应的交叉链接库与头文件。

## Docker（可选）

如果你更倾向容器部署，可以直接查看 [docker.md](docker.md)，其中包含本仓库的 Docker 镜像构建与 Docker Hub 发布说明。

---

## 常见问题与排查

- 构建失败提示找不到 OpenSSL：安装系统的 OpenSSL 开发包（如 `libssl-dev`），或在 Windows 上使用 vcpkg/指定环境变量指向 OpenSSL 安装目录。
- 运行时报端口被占用：确认 `conf/nps.conf` 中的端口设置（`bridge_port`、`web_port`、`http_proxy_port` 等），或停止占用进程。
- Web 登录异常：确认 `web_username` / `web_password` 在 `conf/nps.conf` 中配置正确（若留空将以 admin 权限开放）。

---

