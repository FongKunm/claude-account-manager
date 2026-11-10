#!/bin/bash
# Run with bash on macOS. Does not invoke git or change account/proxy selections.
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  printf '%s\n' '此脚本只能在 macOS 运行。' >&2
  exit 1
fi
project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
case "$project_dir" in
  *OneDrive*|*onedrive*|*CloudStorage*|*'Mobile Documents'*)
    printf '%s\n' '请先把源码复制到 ~/Developer/ClaudeAccountManager，再构建；不要在同步目录装依赖或运行 git。' >&2
    exit 1 ;;
esac
cd "$project_dir"
# SSH and GUI-launched terminals may not have loaded nvm or Cargo yet.
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
if ! command -v node >/dev/null && [[ -r "$HOME/.nvm/nvm.sh" ]]; then
  set +u
  . "$HOME/.nvm/nvm.sh" --no-use
  nvm use 22 >/dev/null
  set -u
fi
# Use the same pnpm version as the package manifest.
if command -v corepack >/dev/null; then
  pnpm() { corepack pnpm@10.33.0 "$@"; }
fi
action="${1:-build}"
architecture="${2:-native}"
case "$action" in build|test|dev|install) ;; *)
  printf '%s\n' '用法：bash scripts/macos.sh build [native|universal|arm64|x64]' \
    '      bash scripts/macos.sh test' '      bash scripts/macos.sh dev' \
    '      bash scripts/macos.sh install [native|universal|arm64|x64]'
  exit 1 ;;
esac
for tool in cargo rustc rustup pnpm xcode-select; do
  command -v "$tool" >/dev/null || { printf '缺少工具：%s\n' "$tool" >&2; exit 1; }
done
xcode-select -p >/dev/null || { printf '%s\n' '请先运行 xcode-select --install。' >&2; exit 1; }
# Keep native build artifacts in the user cache.
export CARGO_TARGET_DIR="$HOME/Library/Caches/ClaudeAccountManager/target"
export MACOSX_DEPLOYMENT_TARGET=11.0
signing_file="$HOME/Library/Application Support/ClaudeAccountManager/signing-identity.txt"
signing_identity="${CAM_MACOS_SIGNING_IDENTITY:-}"
if [[ -z "$signing_identity" && -f "$signing_file" ]]; then
  signing_identity="$(tr -d '\r\n' < "$signing_file")"
  [[ -n "$signing_identity" ]] || { printf '%s\n' '本机签名证书配置为空，未降级为临时签名。' >&2; exit 1; }
fi
if [[ -n "$signing_identity" ]]; then
  [[ "$signing_identity" =~ ^[[:xdigit:]]{40}$ ]] || {
    printf '%s\n' '签名配置必须是证书的 40 位 SHA1 指纹。' >&2; exit 1;
  }
  signing_identity="$(printf '%s' "$signing_identity" | tr '[:lower:]' '[:upper:]')"
  export CAM_MACOS_SIGNING_IDENTITY="$signing_identity"
  export APPLE_SIGNING_IDENTITY="$signing_identity"
fi
verify_fixed_signature() {
  local signed_app="$1" executable identifier requirement team app_team=''
  # securityd uses Team ID only for Apple-recognized developer certificates.
  # A stable self-signed DR still falls back to a version-specific CDHash partition.
  local developer_requirement='anchor apple generic and ((certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13]) or (certificate 1[field.1.2.840.113635.100.6.2.1] and (certificate leaf[field.1.2.840.113635.100.6.1.12] or certificate leaf[field.1.2.840.113635.100.6.1.7])))'
  for identifier in local.claude-account-manager.desktop cam-launcher; do
    executable="$signed_app"
    [[ "$identifier" != cam-launcher ]] || executable="$signed_app/Contents/MacOS/cam-launcher"
    codesign --verify --strict -R "=identifier \"$identifier\" and certificate leaf = H\"$signing_identity\" and ($developer_requirement)" "$executable" || {
      printf '%s\n' '签名验收失败：需要 Apple 签发的开发或 Developer ID 证书；自签证书不能保证钥匙串授权跨版本保留。' >&2; return 1;
    }
    requirement="$(codesign -d -r- "$executable" 2>&1 | sed -n 's/^#\{0,1\} *designated => //p')"
    [[ -n "$requirement" && "$requirement" != *cdhash* ]] || {
      printf '%s\n' '签名仍绑定单一版本，更新后无法沿用授权，未通过验收。' >&2; return 1;
    }
    team="$(codesign -dv --verbose=4 "$executable" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
    [[ "$team" =~ ^[A-Z0-9]{10}$ ]] && { [[ -z "$app_team" ]] || [[ "$team" == "$app_team" ]]; } || {
      printf '%s\n' '应用与终端助手缺少一致的有效 Team ID，未通过签名验收。' >&2; return 1;
    }
    app_team="$team"
  done
}
host_target="$(rustc -vV | sed -n 's/^host: //p')"
case "$host_target" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *)
  printf '%s\n' '需要 macOS 原生 Rust 工具链。' >&2; exit 1 ;;
esac
case "$architecture" in
  native) build_target="$host_target" ;;
  universal) build_target="universal-apple-darwin" ;;
  arm64) build_target="aarch64-apple-darwin" ;;
  x64) build_target="x86_64-apple-darwin" ;;
  *) printf '%s\n' '架构只能是 native、universal、arm64、x64。' >&2; exit 1 ;;
esac

if [[ "$action" == install ]]; then
  app="$CARGO_TARGET_DIR/$build_target/release/bundle/macos/Claude 账号管理器.app"
  [[ -x "$app/Contents/MacOS/cam-launcher" ]] || {
    printf '%s\n' '还没有该架构的完整 .app，请先运行 build。' >&2; exit 1;
  }
  if [[ -n "$signing_identity" ]]; then
    verify_fixed_signature "$app"
  fi
  install_root="$HOME/Applications"
  [[ -w /Applications ]] && install_root=/Applications
  mkdir -p "$install_root"
  installed_app="$install_root/Claude 账号管理器.app"
  if [[ -e "$installed_app" ]]; then
    identifier="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$installed_app/Contents/Info.plist")"
    [[ "$identifier" == local.claude-account-manager.desktop ]] || {
      printf '%s\n' '同名应用不属于换号台，未覆盖。' >&2; exit 1;
    }
  fi
  stage="$(mktemp -d "$install_root/.cam-install.XXXXXX")"
  /usr/bin/ditto "$app" "$stage/Claude 账号管理器.app"
  codesign --verify --deep --strict "$stage/Claude 账号管理器.app"
  previous="$CARGO_TARGET_DIR/previous-app-$(date +%Y%m%d-%H%M%S)-$$.app"
  [[ ! -e "$installed_app" ]] || mv "$installed_app" "$previous"
  if ! mv "$stage/Claude 账号管理器.app" "$installed_app"; then
    [[ ! -e "$previous" ]] || mv "$previous" "$installed_app"
    printf '%s\n' '无法安装应用，原版已尝试恢复。' >&2; exit 1
  fi
  rmdir "$stage"
  if ! "$installed_app/Contents/MacOS/cam-launcher" --repair-hooks; then
    printf '应用已安装到：%s\n' "$installed_app" >&2
    printf '%s\n' '终端入口尚未安装完成。若提示钥匙串权限，请在“钥匙串访问”中解锁 login，再重试 install。' >&2
    exit 1
  fi
  updater_dir="$HOME/.local/bin"
  updater="$updater_dir/cam-update"
  if [[ ! -L "$HOME/.local" && ! -L "$updater_dir" && ! -L "$updater" ]] &&
     { [[ ! -e "$updater" ]] || grep -Fq '# ClaudeAccountManager updater v1' "$updater"; }; then
    mkdir -p "$updater_dir"
    updater_stage="$(mktemp "$updater_dir/.cam-update.XXXXXX")"
    printf '#!/bin/bash\n# ClaudeAccountManager updater v1\nexec /bin/bash %q "$@"\n' \
      "$project_dir/scripts/update-macos.sh" > "$updater_stage"
    chmod 700 "$updater_stage"
    mv "$updater_stage" "$updater"
    printf '%s\n' '以后更新、打包并安装，只需运行：~/.local/bin/cam-update'
  else
    printf '%s\n' '已有其他 cam-update 命令或路径为符号链接，未覆盖；可直接运行 scripts/update-macos.sh。' >&2
  fi
  printf '%s\n' '终端入口已安装。在原来的终端执行一次：' \
    'source "$HOME/Library/Application Support/ClaudeAccountManager/terminal.sh"' \
    '以后切号后直接运行 claude；已有后台对话先 /stop 再 claude --resume。'
  printf '应用位置：%s\n' "$installed_app"
  exit 0
fi

pnpm install --frozen-lockfile
pnpm build
if [[ "$action" == test ]]; then
  (cd src-tauri && cargo test --lib --locked)
  exit 0
fi
if [[ "$action" == dev ]]; then
  (cd src-tauri && cargo build --bin cam-launcher --locked)
  pnpm tauri dev
  exit 0
fi
mkdir -p src-tauri/binaries
if [[ "$build_target" == universal-apple-darwin ]]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    (cd src-tauri && cargo build --bin cam-launcher --release --locked --target "$target")
    cp "$CARGO_TARGET_DIR/$target/release/cam-launcher" "src-tauri/binaries/cam-launcher-$target"
    chmod 700 "src-tauri/binaries/cam-launcher-$target"
  done
  lipo -create "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/cam-launcher" \
    "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/cam-launcher" \
    -output "src-tauri/binaries/cam-launcher-$build_target"
else
  rustup target add "$build_target"
  (cd src-tauri && cargo build --bin cam-launcher --release --locked --target "$build_target")
  cp "$CARGO_TARGET_DIR/$build_target/release/cam-launcher" "src-tauri/binaries/cam-launcher-$build_target"
fi
chmod 700 "src-tauri/binaries/cam-launcher-$build_target"
bundle_config="$project_dir/src-tauri/bundle-macos.json"
if [[ -n "$signing_identity" ]]; then
  security find-identity -v -p codesigning | grep -Fq -- "$signing_identity" || {
    printf '%s\n' '本机签名证书不可用，未降级为临时签名。请在 Mac 桌面终端检查钥匙串后重试。' >&2; exit 1;
  }
  bundle_config="$CARGO_TARGET_DIR/macos-bundle-config.json"
  node - "$project_dir/src-tauri/bundle-macos.json" "$bundle_config" <<'NODE'
const fs = require('fs');
const config = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
config.bundle.macOS = { ...config.bundle.macOS, signingIdentity: process.env.CAM_MACOS_SIGNING_IDENTITY };
fs.writeFileSync(process.argv[3], JSON.stringify(config));
NODE
else
  printf '%s\n' '未配置固定签名：本次使用临时签名，更新后钥匙串可能再次要求授权。' >&2
fi
pnpm tauri build --target "$build_target" --bundles app,dmg --config "$bundle_config"
bundle_dir="$CARGO_TARGET_DIR/$build_target/release/bundle"
app="$bundle_dir/macos/Claude 账号管理器.app"
[[ -x "$app/Contents/MacOS/cam-launcher" ]] || {
  printf '%s\n' '构建缺少终端启动器，不能用于切号。' >&2; exit 1;
}
codesign --verify --deep --strict "$app"
if [[ -n "$signing_identity" ]]; then
  verify_fixed_signature "$app"
fi
printf '\n应用：%s\n' "$app"
for image in "$bundle_dir"/dmg/*.dmg; do
  [[ -f "$image" ]] && shasum -a 256 "$image"
done
printf '%s\n' '首次安装终端入口：'
printf 'bash scripts/macos.sh install %s\n' "$architecture"
printf '%s\n' '然后在原终端执行：' \
  'source "$HOME/Library/Application Support/ClaudeAccountManager/terminal.sh"'
