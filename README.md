# Claude 换号台

在本机管理自己的 Claude Code 订阅账号，查看各账号的额度，并在切换时接回正在使用的对话。桌面界面使用 Tauri 2、React 和 TypeScript。这是社区维护的独立项目。

## 使用方式

- 为账号填写本机 HTTP 代理和固定出口。换号台可检测出口，并在启动或切换前核对绑定。
- Windows 支持普通登录和 Setup Token 账号。切换预览会列出受管终端、阻止退出的原因，以及恢复时使用的模型、思考强度和权限。确认切换后，支持的窗口按各自的会话 ID 恢复。
- macOS 可为新启动的 Claude Code 选择账号。已有会话需要先退出，再用 `claude --resume <会话 ID>` 接回。macOS 的功能范围见使用说明。
- Windows 受管会话可从 Claude Code 的状态栏接收额度。点击手动查询会启动一次独立的 Claude Code 请求，并消耗少量额度。

账号数据由桌面应用在当前操作系统用户的数据目录维护。Windows 使用 DPAPI，macOS 使用钥匙串保护加密密钥。手动导出的 CPA JSON 含有可用凭据，请保存在自己的设备上。

[Windows 使用说明](src-tauri/HELP.txt) · [macOS 使用说明](src-tauri/HELP-mac.txt)

## 预览界面

安装 Node.js 22.12 或更高版本和 pnpm 10.33.0，在仓库根目录运行以下命令。

```sh
pnpm install --frozen-lockfile
pnpm dev
```

浏览器预览使用虚构的演示账号。要使用自己的 Claude Code 账号，请构建桌面应用。

## Windows 构建

需要 Rust 1.90 或更高版本、MSVC C++ 构建工具、Windows SDK 和 WebView2。先安装上面的前端依赖，再从仓库根目录运行以下 PowerShell 命令。

```powershell
pnpm build
cd src-tauri
cargo test --locked --lib
cargo build --locked --release --bins --features custom-protocol
```

默认产物目录是 `src-tauri/target/release/`。将 `claude-account-manager.exe` 和 `cam-launcher.exe` 放在同一目录，运行前者即可打开换号台。要管理真实会话还需要安装官方 Claude Code。

## macOS 构建

安装 Xcode 命令行工具、Rust、Node.js 和 pnpm，将源码放在本机的非云同步目录，在仓库根目录运行以下命令。

```sh
bash scripts/macos.sh test
bash scripts/macos.sh build
```

`bash scripts/macos.sh install` 会安装构建好的应用和终端入口。[macOS 构建和安装说明](README-MAC.txt)

## 参与维护

欢迎提交 Issue 和 Pull Request。请写明复现步骤和验证结果，涉及账号的示例使用虚构数据。维护者会审阅改动，通过测试后决定是否合并。[贡献说明](CONTRIBUTING.md)

本项目采用 [MIT 许可证](LICENSE)，允许商用、修改和再分发。分发本软件的副本或实质部分时，请保留版权和许可声明。
