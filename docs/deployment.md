# 部署指南

## 推荐发布流程（CI/CD）

发布和部署优先使用仓库里的 `Package` workflow（`.github/workflows/package.yml`），不要手动在本机跑构建脚本作为正式产物来源。

- 手动打包：进入 GitHub Actions → `Package` → `Run workflow`，选择 `dev` 或 `main`；手动运行只上传 Actions artifacts。
- 正式发布：在 `main` 上推送 `v*` tag；CI 会构建包并发布到 GitHub Release。
- CI 产物：`dinotty-macos` 包含 `.dmg`，`dinotty-linux` 包含桌面 `.deb` / `.AppImage` 和服务端 `dinotty-server_*.deb`，`dinotty-windows` 包含 NSIS 安装包和 portable `.exe`。
- 产物暂存：CI 会把包复制到 `dist/package-artifacts/` 后上传，手动运行的 artifacts 默认保留 14 天。

## Dev 分支公网部署

`dev` 分支额外提供 `.github/workflows/deploy-dev.yml`。这是面向长期运行实例的
DevOps 流程，不替代 `Package` 的正式发布职责：

1. GitHub 托管 runner 构建前端和静态 `dinotty-server` 二进制。
2. workflow 将运行时所需的 Compose 文件、入口脚本和二进制打成短期 artifact。
3. 标有 `self-hosted`、`linux`、`x64`、`dinotty-prod` 的 runner 通过私有 SOCKS
   代理下载 artifact。
4. 自托管 runner 解包到 `/opt/dinotty/app`，用 Docker Compose 重建 Dinotty。
5. workflow 等待健康检查，并验证容器没有发布主机端口、反向代理私有网络可访问应用。

推送到 `dev` 会自动触发部署；也可以在 GitHub Actions 中手动运行 `Deploy Dev`。
它只部署当前 `dev` 提交，不会把 Token、SSH 私钥或工作区数据打进
artifact 或提交到仓库。

### 首次准备

首次准备由有宿主机管理员权限的人完成一次：

- 安装 Docker Engine 和 Docker Compose plugin。
- 注册 GitHub self-hosted runner，并添加标签 `dinotty-prod`。
- 创建 `/opt/dinotty/.env`，至少设置 `DINOTTY_TOKEN`、`WORKSPACE_DIR`、
  `TZ` 和 `DINOTTY_CADDY_NETWORK`；该文件权限应为 `0600`，绝不能提交。
- 创建宿主机工作区目录，例如 `/opt/dinotty/workspace`。
- 准备反向代理已有的私有 Docker 网络，并让 Caddy 使用
  `deploy/caddy/tmd.yuanspaces.com.Caddyfile` 的模式代理 Dinotty。
- 如果 runner 无法直接访问 GitHub，提供 workflow 当前使用的本地 SOCKS 代理
  `127.0.0.1:17890`，或同步修改 workflow 中的下载配置。

公网实例必须只让 Caddy 发布 HTTPS；不要在 Compose 中重新加入 `8999:8999` 之类
的主机端口映射。浏览器访问由 Dinotty 的强随机 Token 认证；Caddy 负责 HTTPS、
安全响应头和到私有 Docker 网络的反向代理，不应再要求重复输入 HTTP Basic Auth。

### 日常发布、验证与回滚

日常发布只需合并或推送到 `dev`，然后在 Actions 中确认 `Deploy Dev` 的 build 和
deploy job 都成功。部署后的最小验证应包括：

```bash
docker inspect --format '{{.State.Health.Status}}' dinotty
docker port dinotty
docker exec <caddy-container> wget -qO- http://dinotty:8999/api/token-configured
```

预期结果是容器状态为 `healthy`、`docker port dinotty` 没有输出，且最后一个请求返回
`"configured":true`。随后从浏览器验证 HTTPS、Dinotty Token 登录和 WebSocket 会话。

需要回滚时，优先在 Git 中 revert 有问题的提交并推送 `dev`，让同一 workflow 部署
已知正确版本。不要手工修改 `/opt/dinotty/app/.deploy-artifacts`，因为下一次部署会
覆盖它。Token、工作区、Docker volume 和 Caddy 配置不在 artifact 中，回滚代码不会
自动重置这些持久化状态。

## 本地脚本定位

`./scripts/build.sh` 和 `./scripts/build-linux-deb.sh` 只用于本地修改代码后的临时构建、验证或排障；正式部署和发布请走上面的 CI/CD 流程。

```bash
# macOS，在仓库根目录运行；仅用于本地改代码后的临时构建
./scripts/build.sh native
./scripts/build.sh list

# 远程构建 Linux deb；仅用于本地改代码后的临时排障
./scripts/build-linux-deb.sh
```

## Linux systemd 部署（推荐使用 CI deb）

从 `Package` workflow 的 `dinotty-linux` artifact 或 GitHub Release 下载服务端 deb 后安装：

```bash
sudo apt install ./dinotty-server_*.deb

# 管理命令
systemctl status dinotty       # 查看状态
systemctl restart dinotty      # 重启
systemctl stop dinotty         # 停止
journalctl -u dinotty -f       # 查看实时日志

# 修改配置后重启
sudo vim /etc/dinotty/env      # 编辑端口、Token、日志级别
sudo systemctl restart dinotty
```

deb 安装后会部署 `dinotty-server`、systemd unit 和 `/etc/dinotty/env.example`，并启用/启动 `dinotty.service`。

如果只是本地改代码后的临时二进制验证，可以显式传入本地构建产物：

```bash
sudo bash deploy/systemd/install.sh --bin target/release/dinotty-server --token your-secret-token
sudo bash deploy/systemd/uninstall.sh
```

## Linux 桌面包

从 CI 的 `dinotty-linux` artifact 或 GitHub Release 获取桌面包：

```bash
# deb 安装包
sudo apt install ./Dinotty*.deb

# 或直接运行 AppImage
chmod +x ./Dinotty*.AppImage
./Dinotty*.AppImage
```

## macOS 桌面包

从 CI 的 `dinotty-macos` artifact 或 GitHub Release 下载 `.dmg`，打开后按系统提示安装。

## Windows 桌面包

从 CI 的 `dinotty-windows` artifact 或 GitHub Release 下载：

- NSIS 安装包：适合正常安装和卸载。
- portable `.exe`：适合免安装测试。

如需开机自启，可以使用 Windows 任务计划程序、NSSM 或 WinSW 包装 portable 可执行文件。

## Docker 部署

Docker 镜像当前仍按本地 Compose 流程构建：

```bash
cd deploy/docker

# 配置环境变量
cp .env.example .env
# 编辑 .env 设置 DINOTTY_TOKEN、WORKSPACE_DIR 等

# 构建并启动（支持 amd64 和 arm64）
docker compose up -d --build

# 管理命令
docker compose logs -f         # 查看日志
docker compose restart         # 重启
docker compose down            # 停止并移除

# 多架构构建并推送
docker buildx build --platform linux/amd64,linux/arm64 \
  -t your-registry/dinotty:latest --push \
  -f deploy/docker/Dockerfile .
```

### 公网 Docker 部署

不要直接映射 Dinotty 的端口到公网。生产覆盖文件会使用由 CI 或可信构建机准备
的静态二进制，移除主机端口映射，并让容器只加入 HTTPS 反向代理所在的私有 Docker
网络：

```bash
cd deploy/docker
cp .env.production.example .env
# CI/构建机先生成 .deploy-artifacts/dinotty-server
docker compose -f docker-compose.yml -f docker-compose.production.yml up -d --build
```

生产覆盖会启用 `DINOTTY_COOKIE_SECURE=true`，使浏览器会话 Cookie 只能通过
HTTPS 发送。设置 `DINOTTY_CADDY_NETWORK` 为现有反向代理网络名；反向代理
还应提供独立认证层，并保留 Dinotty 的强随机 Token。

Windows 上可通过 Docker Desktop 使用 Linux 容器部署；`.env` 中的工作区路径需要按 Docker Desktop 的挂载路径填写。

## 跨平台包

跨平台桌面包由 `Package` workflow 的 matrix 统一生成：

| 平台 | CI 环境 | 产物 |
|------|---------|------|
| macOS | `macos-latest` | `.dmg` |
| Linux | `ubuntu-22.04` | 桌面 `.deb` / `.AppImage`、服务端 `dinotty-server_*.deb` |
| Windows | `windows-latest` | NSIS `.exe`、portable `.exe` |

## 配置说明

| 参数 | 方式 | 默认值 | 说明 |
|------|------|--------|------|
| 端口 | `--port` / `-p` | 8999 | 服务监听端口 |
| Token | `DINOTTY_TOKEN` 环境变量或配置文件 | 未配置 / 首次设置 | 访问认证令牌，为空时进入首次设置流程 |
| 日志级别 | `RUST_LOG` 环境变量 | info | trace / debug / info / warn / error |
| Shell | Unix: `SHELL`；Windows: `DINOTTY_SHELL` | 自动检测 | Windows 优先 `DINOTTY_SHELL`，再尝试 `pwsh.exe`、`powershell.exe`、`%ComSpec%` / `cmd.exe` |
| Secure Cookie | `DINOTTY_COOKIE_SECURE` | `false` | 公网 HTTPS 反向代理部署设为 `true`，为浏览器会话 Cookie 添加 `Secure` 属性 |
| 分离会话回收时间 | `DINOTTY_DETACH_REAP_SECS` 环境变量 | 5400（90 分钟） | seconds a detached (disconnected) session is kept before the cleanup task reaps it; default 5400 (90 minutes) |

### 配置与数据目录

| 平台 | 配置目录 | 插件目录 |
|------|----------|----------|
| Linux | `~/.config/dinotty` | `~/.dinotty/plugins` |
| macOS | `~/Library/Application Support/dinotty` | `~/.dinotty/plugins` |
| Windows | `%APPDATA%\dinotty` | `%USERPROFILE%\.dinotty\plugins` |

Token、`settings.json`、审计日志和 webhook secrets 存放在配置目录；插件持久化数据存放在用户目录下的 `.dinotty/plugin-data`。
