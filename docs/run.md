# 启动

## 服务端

使用配置文件启动：

```bash
./target/release/rnps -conf_path=conf/nps.conf
```

Windows：

```powershell
.\target\release\rnps.exe -conf_path=conf\nps.conf
```

默认关键端口：

- `bridge_port`：客户端控制连接和 mux 数据连接入口
- `http_proxy_port`：HTTP 域名代理入口
- `https_proxy_port`：HTTPS 域名代理入口
- `web_port`：Web 管理端口

## 客户端

配置文件模式：

```bash
./target/release/rnpc -config=conf/npc.conf
```

Windows：

```powershell
.\target\release\rnpc.exe -config=conf\npc.conf
```

无配置文件模式：

```bash
./target/release/rnpc -server=SERVER_IP:8024 -vkey=123
```

## 启动顺序建议

1. 先启动 `rnps`
2. 确认 `bridge_port` / `web_port` 已监听
3. 再启动 `rnpc`
4. 访问 Web 管理页面检查客户端是否在线

## Web 管理

默认访问地址：

```text
http://127.0.0.1:8081/
```

默认账号密码由 `nps.conf` 决定，例如：

```ini
web_username=admin
web_password=123
```

## 常见排错

- 端口被占用：修改 `nps.conf` 中的监听端口后重新启动
- 客户端离线：先检查 `bridge_port` 是否开放，再检查 `vkey` 是否一致
- HTTP 正常但 HTTPS 不通：检查 `https_proxy_port` 是否开启，以及域名路由是否填了证书和密钥
- 访问 HTTPS 报证书错误：确认 `cert_file_path` / `key_file_path` 是否匹配，证书域名是否与访问域名一致

## E2E 联调

仓库内置了一套最小联调配置，位于 `test-e2e/`。

推荐顺序：

1. 启动本地 backend：`powershell -ExecutionPolicy Bypass -File .\test-e2e\start-backend.ps1`
2. 启动服务端：`.\target\debug\rnps.exe -conf_path .\test-e2e\nps-e2e.conf`
3. 按需启动客户端：`.\target\debug\rnpc.exe -config .\test-e2e\npc-raw-rate.conf` 等

这套联调默认使用本地 backend `127.0.0.1:28081`，对应脚本和配置已经统一到同一端口，避免旧的 `28080` 残留监听误导结果。

已验证的最小端到端结果：

- `20084` raw tunnel 可返回 `200/16B`
- `20081` compress tunnel 可返回 `200/16B`
- `20082` crypt tunnel 可返回 `200/16B`
- `20083` flow-limit tunnel 在达到封顶前可返回 `200/16B`

已验证的运行时策略结果：

- `crypt-rate-key` 在 Web 客户端列表统计中可观察到持续增长的流量计数，`RateLimit=32`
- `flow-key` 在累计导出流量约 `1.04MB` 后，新请求会被卡住/拒绝，且 Web 统计不再继续增长

如果需要复核流量统计，可登录 Web 管理页 `http://127.0.0.1:19081/`，使用 `nps-e2e.conf` 里的账号密码 `admin / 123`，查看 Client List 页面中的 `InletFlow` / `ExportFlow`。