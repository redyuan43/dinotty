# Deployment Guide

## Recommended Release Flow (CI/CD)

Use the repository `Package` workflow (`.github/workflows/package.yml`) for release and deployment artifacts. Do not treat local script output as the official release source.

- Manual package: open GitHub Actions → `Package` → `Run workflow`, then choose `dev` or `main`; manual runs upload Actions artifacts only.
- Official release: push a `v*` tag on `main`; CI builds packages and publishes GitHub Release assets.
- CI artifacts: `dinotty-macos` contains `.dmg`, `dinotty-linux` contains desktop `.deb` / `.AppImage` and the server `dinotty-server_*.deb`, and `dinotty-windows` contains the NSIS installer and portable `.exe`.
- Artifact staging: CI copies packages to `dist/package-artifacts/` before upload. Manual-run artifacts are retained for 14 days by default.

## Public Deployment From `dev`

`dev` also has `.github/workflows/deploy-dev.yml`. This is the DevOps path for
a long-running instance; it does not replace the `Package` workflow's release
responsibilities:

1. A GitHub-hosted runner builds the frontend and a static `dinotty-server` binary.
2. The workflow packages the runtime Compose files, entrypoint, and binary into a
   short-lived artifact.
3. A runner labelled `self-hosted`, `linux`, `x64`, and `dinotty-prod` downloads
   that artifact through a private SOCKS proxy.
4. The self-hosted runner extracts it into `/opt/dinotty/app` and recreates
   Dinotty with Docker Compose.
5. The workflow waits for health checks and verifies that Dinotty has no published
   host port while the reverse proxy can reach it on the private Docker network.

Every push to `dev` starts this deployment. It can also be started manually from
the `Deploy Dev` workflow in GitHub Actions. The artifact and repository never
contain the Token, SSH private keys, or workspace data.

### One-Time Host Setup

An administrator prepares the host once:

- Install Docker Engine and the Docker Compose plugin.
- Register a GitHub self-hosted runner with the `dinotty-prod` label.
- Create `/opt/dinotty/.env` with at least `DINOTTY_TOKEN`, `WORKSPACE_DIR`,
  `TZ`, and `DINOTTY_CADDY_NETWORK`. Keep it mode `0600` and out of Git.
- Create the host workspace, for example `/opt/dinotty/workspace`.
- Provide the reverse proxy's private Docker network and configure Caddy using
  the pattern in `deploy/caddy/tmd.yuanspaces.com.Caddyfile`.
- If the runner cannot reach GitHub directly, provide the local SOCKS proxy
  currently expected at `127.0.0.1:17890`, or update the workflow's download
  configuration to match the environment.

The public instance must expose HTTPS only through Caddy. Do not restore a host
port mapping such as `8999:8999`. Browser access is authenticated by Dinotty's
strong random Token. Caddy provides HTTPS, security headers, and reverse proxying
to the private Docker network; it should not prompt for HTTP Basic Auth again.

### Deploy, Verify, and Roll Back

For routine releases, merge or push to `dev`, then confirm that the `Deploy Dev`
build and deploy jobs succeed in GitHub Actions. The minimum post-deploy checks
are:

```bash
docker inspect --format '{{.State.Health.Status}}' dinotty
docker port dinotty
docker exec <caddy-container> wget -qO- http://dinotty:8999/api/token-configured
```

The container should be `healthy`, `docker port dinotty` should print nothing,
and the final request should include `"configured":true`. Then verify HTTPS,
the Dinotty Token login, and a WebSocket session from a browser.

To roll back, revert the bad commit in Git and push `dev`, allowing the same
workflow to deploy the known-good revision. Do not edit
`/opt/dinotty/app/.deploy-artifacts` by hand because the next deployment
overwrites it. The Token, workspace, Docker volume, and Caddy configuration are
outside the artifact, so a code rollback does not reset persistent state.

## Local Script Scope

`./scripts/build.sh` and `./scripts/build-linux-deb.sh` are only for temporary local builds, verification, or troubleshooting after changing code. Use the CI/CD flow above for deployment and releases.

```bash
# macOS, run from the repository root; only for temporary local builds after code changes
./scripts/build.sh native
./scripts/build.sh list

# Remote Linux deb build; only for local troubleshooting after code changes
./scripts/build-linux-deb.sh
```

## Linux systemd Deploy (Use CI deb)

Download the server deb from the `Package` workflow `dinotty-linux` artifact or from GitHub Releases, then install it:

```bash
sudo apt install ./dinotty-server_*.deb

# Management commands
systemctl status dinotty       # Check status
systemctl restart dinotty      # Restart
systemctl stop dinotty         # Stop
journalctl -u dinotty -f       # View live logs

# Update config and restart
sudo vim /etc/dinotty/env      # Edit port, token, log level
sudo systemctl restart dinotty
```

Installing the deb deploys `dinotty-server`, the systemd unit, and `/etc/dinotty/env.example`, then enables and starts `dinotty.service`.

For temporary local binary validation after changing code, pass the local build output explicitly:

```bash
sudo bash deploy/systemd/install.sh --bin target/release/dinotty-server --token your-secret-token
sudo bash deploy/systemd/uninstall.sh
```

## Linux Desktop Package

Download desktop packages from the CI `dinotty-linux` artifact or from GitHub Releases:

```bash
# deb installer
sudo apt install ./Dinotty*.deb

# Or run the AppImage directly
chmod +x ./Dinotty*.AppImage
./Dinotty*.AppImage
```

## macOS Desktop Package

Download the `.dmg` from the CI `dinotty-macos` artifact or from GitHub Releases, then open it and follow the system installer prompts.

## Windows Desktop Package

Download packages from the CI `dinotty-windows` artifact or from GitHub Releases:

- NSIS installer: suitable for normal install and uninstall flows.
- Portable `.exe`: suitable for install-free testing.

For auto-start on Windows, wrap the portable executable with Task Scheduler, NSSM, or WinSW.

## Docker Deploy

Docker images are still built through the local Compose flow:

```bash
cd deploy/docker

# Configure environment variables
cp .env.example .env
# Edit .env to set DINOTTY_TOKEN, WORKSPACE_DIR, etc.

# Build and start (supports amd64 and arm64)
docker compose up -d --build

# Management commands
docker compose logs -f         # View logs
docker compose restart         # Restart
docker compose down            # Stop and remove

# Multi-arch build and push
docker buildx build --platform linux/amd64,linux/arm64 \
  -t your-registry/dinotty:latest --push \
  -f deploy/docker/Dockerfile .
```

### Public Docker deployment

Do not publish Dinotty's port directly to the internet. The production override
uses a static binary prepared by CI or a trusted build machine, removes the
host port mapping, and joins the reverse proxy's private Docker network:

```bash
cd deploy/docker
cp .env.production.example .env
# CI/build machine first creates .deploy-artifacts/dinotty-server
docker compose -f docker-compose.yml -f docker-compose.production.yml up -d --build
```

The production override sets `DINOTTY_COOKIE_SECURE=true`, so browser session
cookies are sent only over HTTPS. Set `DINOTTY_CADDY_NETWORK` to the existing
reverse-proxy network name, keep Dinotty's strong random Token, and add a
separate authentication layer at the reverse proxy.

On Windows, use Docker Desktop with Linux containers. Set workspace paths in `.env` using paths visible inside Docker Desktop mounts.

## Cross-Platform Packages

Cross-platform desktop packages are generated by the `Package` workflow matrix:

| Platform | CI runner | Artifacts |
|----------|-----------|-----------|
| macOS | `macos-latest` | `.dmg` |
| Linux | `ubuntu-22.04` | desktop `.deb` / `.AppImage`, server `dinotty-server_*.deb` |
| Windows | `windows-latest` | NSIS `.exe`, portable `.exe` |

## Configuration

| Parameter | Method | Default | Description |
|-----------|--------|---------|-------------|
| Port | `--port` / `-p` | 8999 | Server listen port |
| Token | `DINOTTY_TOKEN` env var or config file | Unconfigured / first-time setup | Access auth token; when empty, Dinotty starts the first-time setup flow |
| Log level | `RUST_LOG` env var | info | trace / debug / info / warn / error |
| Shell | Unix: `SHELL`; Windows: `DINOTTY_SHELL` | Auto-detect | Windows tries `DINOTTY_SHELL`, then `pwsh.exe`, `powershell.exe`, `%ComSpec%` / `cmd.exe` |
| Secure Cookie | `DINOTTY_COOKIE_SECURE` | `false` | Set to `true` behind a public HTTPS reverse proxy to add the `Secure` attribute to browser session cookies |

### Config And Data Directories

| Platform | Config directory | Plugin directory |
|----------|------------------|------------------|
| Linux | `~/.config/dinotty` | `~/.dinotty/plugins` |
| macOS | `~/Library/Application Support/dinotty` | `~/.dinotty/plugins` |
| Windows | `%APPDATA%\dinotty` | `%USERPROFILE%\.dinotty\plugins` |

Tokens, `settings.json`, audit logs, and webhook secrets are stored in the config directory. Plugin persistent data lives under `.dinotty/plugin-data` in the user's home directory.
