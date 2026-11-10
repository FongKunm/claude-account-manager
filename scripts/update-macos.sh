#!/bin/bash
# User-invoked update of this independent, non-synced Mac clone only.
set -euo pipefail
[[ "$(uname -s)" == Darwin ]] || { printf '%s\n' '此命令只用于 Mac。' >&2; exit 1; }
project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
case "$project_dir" in *OneDrive*|*onedrive*|*CloudStorage*|*'Mobile Documents'*)
  printf '%s\n' '请在 OneDrive / iCloud 外的独立克隆更新。' >&2; exit 1 ;;
esac
cd "$project_dir"
[[ "$(git rev-parse --show-toplevel)" == "$project_dir" ]] || exit 1
[[ -z "$(git status --porcelain)" ]] || {
  printf '%s\n' '源码有本地修改，请先提交或自行处理；更新命令不会覆盖这些修改。' >&2; exit 1;
}
git pull --ff-only
bash scripts/macos.sh test
bash scripts/macos.sh build
# Restart only the manager's own bundled GUI, never Claude Code or terminals.
pids="$(pgrep -f '/Claude 账号管理器[.]app/Contents/MacOS/claude-account-manager$' || true)"
if [[ -n "$pids" ]]; then
  printf '%s\n' '构建成功，正在重启换号台以安装新版。'
  while IFS= read -r app_pid; do
    kill -TERM "$app_pid" 2>/dev/null || true
  done <<< "$pids"
  for attempt in {1..20}; do
    pgrep -f '/Claude 账号管理器[.]app/Contents/MacOS/claude-account-manager$' >/dev/null || break
    sleep 0.25
  done
  if pgrep -f '/Claude 账号管理器[.]app/Contents/MacOS/claude-account-manager$' >/dev/null; then
    printf '%s\n' '换号台尚未退出，请关闭它后重新运行本命令。' >&2; exit 1
  fi
fi
bash scripts/macos.sh install
installed_app="$HOME/Applications/Claude 账号管理器.app"
[[ -w /Applications ]] && installed_app='/Applications/Claude 账号管理器.app'
open "$installed_app"
printf '%s\n' '更新、安装完成，账号数据已保留。'
