# 发布 Starter

仓库推送 `v` 开头的 tag 后，GitHub Actions 会构建并发布三个安装包：Apple Silicon macOS、Intel macOS 和 Windows x86_64。版本号取自 tag，例如 `v0.1.0` 会生成 `0.1.0` 文件名，并自动创建 GitHub Release。

## 第一次配置 GitHub Secrets

没有 Apple Developer 证书时，工作流仍会成功，但 macOS 包只有 ad-hoc 签名。别人下载后首次打开可能看到 Gatekeeper 提示，需要在系统设置里允许打开。给别人长期使用时，建议配置正式签名和公证。

在 Apple Developer 账户中准备：

- `Developer ID Application` 证书，并从钥匙串导出为带密码的 `.p12` 文件。
- Apple Developer Team ID。
- 一个 App-specific password。不要把 Apple ID 主密码放进 GitHub。

在仓库的 **Settings → Secrets and variables → Actions** 中添加：

| Secret | 内容 |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | `.p12` 文件的 base64 文本 |
| `APPLE_CERTIFICATE_PASSWORD` | 导出 `.p12` 时设置的密码 |
| `APPLE_SIGNING_IDENTITY` | 证书名称，例如 `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | 用于公证的 Apple ID 邮箱 |
| `APPLE_TEAM_ID` | Apple Developer Team ID |
| `APPLE_APP_SPECIFIC_PASSWORD` | Apple ID 的 app-specific password |

macOS 上可以这样复制证书的 base64 内容：

```sh
base64 < DeveloperID.p12 | pbcopy
```

六个值都配置后，工作流会启用 hardened runtime、Developer ID 签名，并使用 `notarytool` 公证后把票据 stapling 到 DMG。只配置其中一部分会让构建直接失败，避免发布一个看似正式但实际上无法验证的包。

## 发布流程

先在本地更新 `Cargo.toml` 的版本，运行检查：

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

然后创建并推送 tag：

```sh
git tag v0.1.0
git push origin v0.1.0
```

在 GitHub Actions 页面等待 `Release` 工作流完成。完成后，Release 页面会出现：

- `Starter-0.1.0-macos-arm64.dmg`
- `Starter-0.1.0-macos-x86_64.dmg`
- `Starter-0.1.0-windows-x86_64.zip`

## 本地打包

本地 macOS 包使用 ad-hoc 签名，适合开发测试：

```sh
STARTER_VERSION=0.1.0 bash scripts/package-macos.sh arm64 dist
open dist/Starter-0.1.0-macos-arm64.dmg
```

Windows 原生环境中运行：

```powershell
pwsh scripts/package-windows.ps1 -Version 0.1.0 -OutputDirectory dist
```

安装者打开 DMG 后把 `Starter.app` 拖到 `Applications`。Windows 解压后运行 `Starter.exe`。应用不需要网络、账号或后台服务。
