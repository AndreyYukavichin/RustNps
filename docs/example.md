# 使用示例

## TCP 隧道

客户端 `npc.conf`：

```ini
[common]
server_addr=127.0.0.1:18024
vkey=123
compress=true
crypt=false
rate_limit=512
flow_limit=1024
max_conn=32
max_tunnel_num=16

[ssh]
mode=tcp
server_port=10022
target_addr=127.0.0.1:22
```

效果：访问服务端 `10022` 会转发到客户端机器的 `22` 端口。

说明：

- `compress=true` 时数据面会走 snappy 压缩。
- `crypt=true` 时数据面会切到 TLS relay。
- `rate_limit=512` 表示整客户端总带宽限制为 512 KB/s。
- `flow_limit=1024` 表示累计流量达到 1024 MB 后停止继续转发。

## HTTP 域名代理

```ini
[web]
host=dev.local
target_addr=127.0.0.1:8080
location=/
```

然后把 `dev.local` 解析到 RustNps 服务端 IP，访问 `http://dev.local` 即可。

## HTTPS 域名代理

在 Web 或持久化数据中配置域名路由（可通过 Web UI 添加 Host）：

- `host=api.example.com`
- `target=127.0.0.1:8080`
- `scheme=https`
- `cert_file_path=/path/to/fullchain.pem`
- `key_file_path=/path/to/privkey.pem`

当 `https_just_proxy=false` 时，RustNps 会根据 TLS SNI 自动选择对应证书并在服务端做 TLS 终止。

## HTTPS 透传

如果内网服务自己处理 TLS：

```ini
https_proxy_port=443
https_just_proxy=true
```

然后配置域名路由：

- `host=app.example.com`
- `target=127.0.0.1:8443`
- `scheme=https`

此时外部 TLS 会原样透传到内网 `8443`。

## SOCKS5

```ini
[socks5]
mode=socks5
server_port=19009
```

浏览器或系统代理指向服务端 `19009` 即可。

## UDP

```ini
[dns]
mode=udp
server_port=12253
target_addr=114.114.114.114:53
```

## Secret

服务提供方：

```ini
[ssh_secret]
mode=secret
password=ssh2
target_addr=127.0.0.1:22
```

访问方（示例工具或本地转发）：

```ini
[secret_ssh]
local_port=2001
password=ssh2
```

## `ip_limit` + `npc register`

如果服务端开启了：

```ini
ip_limit=true
```

那么访问来源 IP 必须先登记。客户端可以执行：

```bash
# 向服务端登记当前来源 IP（示例）
./target/release/npc register -server=SERVER_IP:18024 -vkey=123
```

成功后，该来源 IP 会被服务端放行（通常带有时限）；如果启用了 Web 登录，登录成功的浏览器来源 IP 也会自动加入放行列表。

## Web 管理页设置 client 运行时参数

`RustNps` 的 Web 管理页支持：

- 查看/编辑客户端运行时参数（`Compress` / `Crypt` / `Rate limit` / `Flow limit` 等）。
- 通过表单在线添加或编辑隧道（`type`/`port`/`target` 等字段）。

这适合在无需修改 `npc.conf` 的情况下对某个 client 做临时或即时调整。

---

## 快速联调（本地 E2E）

1. 在一台机器上启动服务端：

```bash
cd RustNps
cargo run --bin nps -- -conf_path conf/nps.conf
```

2. 在另一终端启动客户端（同机也可）：

```bash
cargo run --bin npc -- -config conf/npc.conf
```

3. 打开 Web 管理页：`http://127.0.0.1:18081/`（默认端口可能不同，参照 `conf/nps.conf`）

4. 在 Client List / Tunnel 页面检查已注册的隧道并发起连接测试。

---

