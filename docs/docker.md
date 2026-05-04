# RustNps Docker 镜像

RustNps 可以直接构建为多架构 Docker 镜像，并发布到 Docker Hub。仓库默认的发布镜像名为 `andreiyhub/rustnps`。

## 构建

本仓库根目录提供了 [Dockerfile](../Dockerfile) 和 GitHub Actions 工作流 [docker-publish.yml](../.github/workflows/docker-publish.yml)。

本地构建示例：

```bash
docker build -t andreiyhub/rustnps:dev .
```

## 运行

启动服务端：

```bash
docker run --rm \
  -p 8081:8081 -p 8024:8024 -p 80:80 -p 443:443 \
  -v "$PWD/conf:/etc/rustnps" \
  andreiyhub/rustnps:latest
```

启动客户端：

```bash
docker run --rm \
  -v "$PWD/conf:/etc/rustnps" \
  andreiyhub/rustnps:latest rnpc -config=/etc/rustnps/npc.conf
```

默认会优先使用挂载目录里的 `nps.conf` / `npc.conf`。如果需要指定架构，可以给 `docker run` 加 `--platform`，例如：`linux/amd64`、`linux/386`、`linux/arm64`、`linux/arm/v7`。

## 发布流程

GitHub Actions 会在打 `v*` 标签时自动构建并推送多架构镜像；也可以通过手动触发输入自定义 tag。发布前需要在仓库 Secrets 中配置：

- `DOCKERHUB_USERNAME`
- `DOCKERHUB_TOKEN`