# Starter

Starter 是一个使用 Rust 和 GPU 原生渲染的桌面启动器。它常驻菜单栏，通过一个浮动窗口搜索应用、文件和系统操作；数据保存在本机，不使用云同步、分析服务或网页视图。

**macOS** `Option+Space` · **Windows** `Ctrl+Space`

[Starter 0.6.2 release notes](docs/releases/0.6.2.md)

## 功能

- 模糊应用搜索：支持中文名称和错字匹配
- 文件名、文件夹名和文件内容搜索
- 剪贴板历史：文本、网址、图片和文件
- 快速提醒和系统命令
- CPU、内存、启动盘和应用资源监控
- 应用卸载、自动更新和配置管理
- 原生渲染、菜单栏常驻、无需外部终端

## 安装和启动

开发运行需要 Rust 1.90+（2024 edition）。

macOS 应使用 `.app` 包，以获得正确的菜单栏、图标和窗口行为：

```sh
bash scripts/bundle-macos.sh --debug
open target/debug/Starter.app
```

Windows 可以直接运行：

```sh
cargo run --locked
```

启动后按 `Option+Space` 或 `Ctrl+Space` 呼出窗口。设置可通过齿轮按钮、`⌘,` / `Ctrl+,` 或托盘菜单打开。

## 常用操作

| 输入 | 作用 |
| --- | --- |
| `safari`、`微信` | 搜索应用，支持模糊匹配和中文名称 |
| `/f ui` | 搜索已配置目录中的文件和文件夹 |
| `/c launcher_hotkey` | 搜索文件内容 |
| `/uninstall 微信` | 搜索并卸载应用 |
| `/clip needle` | 搜索剪贴板历史 |
| `/remind 10m meeting` | 创建提醒 |
| `Lock`、`Sleep` | 执行系统操作 |
| `Restart`、`Shutdown` | 显示确认后执行系统操作 |
| `monitor`、`监控` | 打开资源监控 |
| `/monitor Chrome` | 按名称筛选资源监控列表 |

`↑↓` 选择，`Enter` 打开，`⌘/Ctrl+Enter` 在 Finder 或资源管理器中显示，`Shift+Enter` 复制路径。收藏应用会排在前面。

资源监控中，`↑↓` 选择应用或后台进程，`Enter` / `⌘K`（Windows 为 `Ctrl+K`）、右键或行末“操作”按钮弹出操作菜单，监控列表保持显示；点击外部或按 Esc 收起。支持正常退出、强制退出、定位和复制路径；macOS 还支持发送 SIGTERM 终止进程。终止进程和强制退出需要再次确认。正常退出允许应用提示保存或取消；列表会继续刷新，发送退出请求不代表应用已经退出。

## 卸载应用和 Mole

macOS 的“卸载并清理关联文件”功能需要单独安装 [Mole](https://github.com/tw93/Mole)：

```sh
brew install mole
```

Mole 是可选依赖，只用于扫描和清理应用的配置、缓存、日志等关联文件。Starter 不会自动安装 Mole。按 `/uninstall` 选择应用后：

- 已安装 Mole：Starter 在自己的任务面板中运行 Mole，显示 Mole 的文件预览、确认、进度和权限提示；不会打开 Kitty、Terminal 等外部终端。
- 未安装 Mole、Mole 版本不兼容或无法唯一匹配应用：Starter 会明确提示原因，并保留“仅移到废纸篓”选项。该选项只移除应用本体，不清理关联数据。
- Windows：继续调用应用注册的官方卸载程序，不依赖 Mole。

Mole 接口目前按 JSON 应用清单工作，已验证 Mole 1.59.0。Mole 的确认仍由 Mole 自己负责，Starter 不会根据终端输出自动确认删除。

## 设置和本地数据

设置包括启动快捷键、应用快捷键组、开机自启动、文件关联、主题、等宽字体、搜索目录、应用别名、自动更新和剪贴板历史。设置修改后立即生效；应用快捷键组可以一次打开多个应用。

数据位置：

- macOS：`~/Library/Application Support/starter`
- Windows：`%APPDATA%\starter`

剪贴板历史最多保存 30 天、200 条记录和 100 MiB；图片保存本地缩略图，文件记录保存原路径。文件搜索遵循 `.gitignore`，跳过隐藏文件、`.git`、`node_modules`、`target` 和 `.cache`。

自动更新每 24 小时检查 GitHub Releases。下载后会校验大小和 SHA-256，再准备安装并重启；更新失败时原应用保持不变。

## 开发和发布

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

macOS 更新器测试：

```sh
bash scripts/bundle-macos.sh --debug
python3 scripts/test-macos-updater.py
```

macOS 全屏窗口、焦点恢复和中文输入回归测试（需要辅助功能权限）：

```sh
xcrun swift scripts/test-macos-fullscreen.swift --ime
```

推送 `v*` 标签会触发 GitHub Actions，构建 Apple Silicon、Intel macOS DMG 以及 Windows 安装包，并创建 GitHub Release。发布说明放在 `docs/releases/<version>.md`。

卸载任务的实现教程见[内嵌卸载终端教程](docs/从零读懂Starter内嵌卸载终端.md)，性能优化说明见[性能优化记录](docs/优化路线.md)。

## 设计边界

Starter 的搜索、提醒、剪贴板和监控功能在本地运行。网络请求只用于 GitHub 更新检查和下载。macOS 的清理卸载由 Mole 负责，Starter 负责应用选择、路径校验、任务面板和进程生命周期；原生废纸篓卸载始终保留为不依赖 Mole 的回退方案。
