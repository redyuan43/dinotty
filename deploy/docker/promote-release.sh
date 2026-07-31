#!/usr/bin/env bash
set -Eeuo pipefail

bundle_dir="${1:?usage: promote-release.sh BUNDLE_DIR}"
root_dir="${DINOTTY_ROOT:-/opt/dinotty}"
env_file="${DINOTTY_ENV_FILE:-$root_dir/.env}"
app_dir="${DINOTTY_APP_DIR:-$root_dir/app}"
public_url="${DINOTTY_PUBLIC_URL:-https://tmd.yuanspaces.com}"
proxy_container="${DINOTTY_PROXY_CONTAINER:-myownerim-caddy-1}"

revision="$(tr -d '[:space:]' < "$bundle_dir/REVISION")"
build_version="$(tr -d '[:space:]' < "$bundle_dir/BUILD_VERSION")"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]]
[[ -n "$build_version" ]]
test -f "$env_file"

short_revision="${revision:0:12}"
image="dinotty:${revision}"
candidate="dinotty-candidate-${short_revision}"
candidate_volume="dinotty-candidate-${short_revision}-data"
rollback_image="dinotty:rollback-$(date -u +%Y%m%dT%H%M%SZ)"
compose_files=(
  -f "$bundle_dir/deploy/docker/docker-compose.yml"
  -f "$bundle_dir/deploy/docker/docker-compose.production.yml"
)

exec 9>"$root_dir/.deploy.lock"
flock -n 9

cleanup_candidate() {
  docker rm -f "$candidate" >/dev/null 2>&1 || true
  docker volume rm "$candidate_volume" >/dev/null 2>&1 || true
}
trap cleanup_candidate EXIT

docker build \
  --build-arg "DINOTTY_REVISION=$revision" \
  --build-arg "DINOTTY_BUILD_VERSION=$build_version" \
  --label "org.opencontainers.image.revision=$revision" \
  --label "org.opencontainers.image.version=$build_version" \
  -f "$bundle_dir/deploy/docker/Dockerfile.binary" \
  -t "$image" \
  "$bundle_dir"

production_volume="$(
  docker inspect dinotty \
    --format '{{range .Mounts}}{{if eq .Destination "/home/dinotty"}}{{.Name}}{{end}}{{end}}'
)"
test -n "$production_volume"

cleanup_candidate
docker volume create "$candidate_volume" >/dev/null
docker run --rm \
  -v "$production_volume:/from:ro" \
  -v "$candidate_volume:/to" \
  alpine:3.21 \
  sh -c 'cd /from && tar cf - . | tar xf - -C /to'

docker run -d \
  --name "$candidate" \
  --network none \
  --env-file "$env_file" \
  -e DINOTTY_PORT=8999 \
  -e DINOTTY_COOKIE_SECURE=true \
  -v "$candidate_volume:/home/dinotty" \
  -v "$root_dir/workspace:/workspace:ro" \
  "$image" >/dev/null

candidate_info=""
for _ in $(seq 1 45); do
  if candidate_info="$(
    docker exec "$candidate" sh -c \
      'curl -fsS -H "Authorization: Bearer ${DINOTTY_TOKEN}" http://127.0.0.1:8999/api/info'
  )"; then
    break
  fi
  sleep 2
done
grep -Fq "\"version\":\"$build_version\"" <<<"$candidate_info"
docker exec "$candidate" sh -c \
  'curl -fsS -H "Authorization: Bearer ${DINOTTY_TOKEN}" http://127.0.0.1:8999/api/settings >/dev/null'
docker exec "$candidate" sh -c \
  'curl -fsS http://127.0.0.1:8999/ | grep -qi "<html"'
if docker logs "$candidate" 2>&1 | grep -Eiq 'panic|fatal'; then
  echo "candidate emitted a fatal log" >&2
  exit 1
fi

previous_image_id="$(docker inspect dinotty --format '{{.Image}}')"
docker tag "$previous_image_id" "$rollback_image"

rollback() {
  echo "production verification failed; restoring $rollback_image" >&2
  DINOTTY_IMAGE="$rollback_image" docker compose \
    --env-file "$env_file" "${compose_files[@]}" -p dinotty up --no-build -d
  for _ in $(seq 1 45); do
    if [[ "$(docker inspect --format '{{.State.Health.Status}}' dinotty 2>/dev/null)" == healthy ]]; then
      return
    fi
    sleep 2
  done
  echo "rollback container did not become healthy" >&2
}

DINOTTY_IMAGE="$image" docker compose \
  --env-file "$env_file" "${compose_files[@]}" -p dinotty config -q
DINOTTY_IMAGE="$image" docker compose \
  --env-file "$env_file" "${compose_files[@]}" -p dinotty up --no-build -d

production_ok=false
for _ in $(seq 1 45); do
  if [[ "$(docker inspect --format '{{.State.Health.Status}}' dinotty 2>/dev/null)" == healthy ]]; then
    production_ok=true
    break
  fi
  sleep 2
done

if [[ "$production_ok" != true ]] ||
  [[ -n "$(docker port dinotty)" ]] ||
  ! docker exec "$proxy_container" wget -qO- http://dinotty:8999/api/token-configured |
    grep -q '"configured":true' ||
  ! curl -fsS "$public_url/api/token-configured" | grep -q '"configured":true'; then
  rollback
  exit 1
fi

production_info="$(
  docker exec dinotty sh -c \
    'curl -fsS -H "Authorization: Bearer ${DINOTTY_TOKEN}" http://127.0.0.1:8999/api/info'
)"
if ! grep -Fq "\"version\":\"$build_version\"" <<<"$production_info"; then
  rollback
  exit 1
fi

rm -rf "$app_dir/.deploy-artifacts" "$app_dir/deploy"
cp -a "$bundle_dir/.deploy-artifacts" "$app_dir/"
cp -a "$bundle_dir/deploy" "$app_dir/"
cp "$bundle_dir/REVISION" "$bundle_dir/BUILD_VERSION" "$app_dir/"
printf '%s\n' "$image" > "$root_dir/CURRENT_IMAGE"

printf 'revision=%s\nversion=%s\nimage=%s\nrollback=%s\n' \
  "$revision" "$build_version" "$image" "$rollback_image"
