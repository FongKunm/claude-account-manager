Claude 换号台 · macOS

在本机非同步目录准备源码，安装 Xcode 命令行工具、Rust 1.90 或更高版本、Node.js 22.12 或更高版本和 pnpm 10.33.0。
在仓库根目录运行

  bash scripts/macos.sh test
  bash scripts/macos.sh build

默认构建当前 Mac 的架构。如需通用包，可使用 bash scripts/macos.sh build universal。
构建脚本会显示产物位置。要安装应用和终端入口，请使用与构建时相同的架构参数。

  bash scripts/macos.sh install

应用将安装至可写的 Applications 目录。打开新的 zsh 或 bash，或者在已打开的终端执行

  source "$HOME/Library/Application Support/ClaudeAccountManager/terminal.sh"

原生签名为可选配置。需要固定签名时，将自己的 Apple 开发者或 Developer ID 证书指纹通过 CAM_MACOS_SIGNING_IDENTITY 提供给构建脚本。
钥匙串授权和数据加密与本机用户相关。登录、恢复已有对话和卸载终端入口的步骤见 src-tauri/HELP-mac.txt。
