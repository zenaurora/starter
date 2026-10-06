# Starter

Rust + [GPUI Kit](https://github.com/longbridge/gpui-kit) 桌面启动器，面向 macOS 和 Windows。使用原生窗口与 GPU 渲染，无 WebView、无 AI 服务。

## 运行

需要 Rust 1.90 或更新版本（2024 edition）。macOS 需要 Xcode Command Line Tools；Windows 需要 Visual Studio C++ Build Tools 和 Windows SDK。

```sh
# macOS 开发运行（生成带身份标识的原生 .app）
bash scripts/bundle-macos.sh --debug
open target/debug/Starter.app

# macOS 发布构建
bash scripts/bundle-macos.sh --release
open target/release/Starter.app

# Windows
cargo run --locked
```

macOS 使用 `.app` 运行，以便托盘、窗口激活和原生图标正确工作。打包脚本使用本地 ad-hoc 签名，还不是公证的分发包。

图标使用折线「S」。macOS 菜单栏嵌入 72 px 单色模板，由系统按 18 pt 显示并适配深浅外观；应用包包含 16–1024 px 的 `.icns`。图标资源位于 `resources/icons`，在 macOS 上运行 `swift scripts/generate-icons.swift` 可重新生成。

## 使用与设置

默认 macOS `Option+Space` 呼出，Windows `Ctrl+Space` 呼出。`Option/Alt+Enter` 全局打开终端（macOS 默认 Terminal，Windows 默认 wt.exe）。系统里如果已有相同热键，请修改冲突方或在设置里更换；注册失败会显示具体错误。

点击右上角设置图标、按 `⌘,` / `Ctrl+,`，或通过托盘「设置…」进入独立设置页：

- **快捷键与终端**：修改两个全局快捷键和终端应用。
- **外观**：Catppuccin Mocha、Everforest、Gruvbox、Catppuccin Latte；点选预览，取消恢复，保存持久化。可设置已安装的等宽字体。
- **搜索目录**：支持原生文件夹选择器和手工输入路径，一行一个，支持 `~`。
- **应用别名**：填写应用名称和逗号分隔的别名，点击添加或更新；可移除。

保存时验证热键、目录和必填项；热键注册失败保留原设置。改动立即应用，无须重启。配置文件入口保留给高级编辑，手工修改后从托盘重新加载。

`⌘W`（Windows `Ctrl+W`）在搜索页和设置页都可收起窗口，保留托盘和全局快捷键；再次使用呼出快捷键回到搜索页，设置中未保存的改动会取消。macOS 用 `⌘Q` 完整退出，也可使用托盘「退出」。

| 查询 | 行为 |
| --- | --- |
| `safrai` | 搜索应用；模糊匹配，并容忍少量错字及相邻字母交换 |
| `微信` | 匹配 WeChat；自动索引应用的中文名称和内置中文别名 |
| `/f ui` | 在配置目录内，模糊匹配文件和文件夹名称 |
| `/c launcher_hotkey` | 在配置目录内，按字面查找文本内容，忽略大小写 |

也可直接点击搜索模式按钮。`↑↓` 选择、`Enter` 打开、`⌘/Ctrl+Enter` 在文件管理器定位、`Shift+Enter` 复制路径；`Esc` 逐级清空查询、退出模式、隐藏。点击应用行右侧星标可收藏或取消收藏。默认列表依次按收藏、启动次数、最近启动时间排序；输入查询后先按匹配准确度排序，再以收藏和启动次数打破平局。独立的「最近使用」区域显示最近打开的 5 个应用，可直接点击启动。收藏与使用记录保存在本地，隐藏后仍保留托盘和全局快捷键。

中文查询支持应用名称、文件名和 UTF-8 文本内容。macOS 从 `Info.plist` 及中文 `InfoPlist.strings` 读取简体和繁体名称；常见英文应用另有内置中文别名（如微信、企业微信、腾讯会议、钉钉、飞书、终端、系统设置），与设置页中的自定义别名合并，已有配置自动生效。搜索结果保留原应用名称。没有中文元数据或内置别名的应用，可在设置页添加中文别名。

默认不配置搜索目录，避免遍历整个用户目录。文件遍历遵循 `.gitignore` / `.ignore`，跳过隐藏文件、`.git`、`node_modules`、`target` 和 `.cache`。最多索引 100,000 个文件与目录，显示最多 100 条结果。内容搜索跳过二进制、无效 UTF-8 和大于 2 MiB 的文件，不解析 PDF、Office。目录索引保存在内存中，新建或删除文件后可从托盘刷新。

## 结构

- `src/ui.rs`：启动器界面、键盘交互、结果列表。
- `src/settings.rs`：设置页、校验和草稿；保存或取消后返回搜索。
- `src/appearance.rs`：统一配色。深色配色参考 [Omarchy 官方主题](https://github.com/omacom/omarchy/tree/quattro/themes)，浅色来自 Catppuccin Latte。
- `src/icons.rs`：后台读取系统应用/文件图标，按需加载，有界缓存；缺失时使用 Lucide 类型图标。额外图标仅嵌入实际使用的条目。
- `src/platform.rs`：托盘、全局快捷键、打开/定位、窗口显示隐藏。
- `src/search.rs`、`src/catalog.rs`：可独立测试的搜索和应用发现。
- `src/worker.rs`：后台目录遍历、可取消搜索、80 ms 防抖、分批返回。
- `src/config.rs`、`src/history.rs`：本地 TOML 配置和 JSON 使用记录。

macOS 配置在 `~/Library/Application Support/starter/config.toml`，Windows 在 `%APPDATA%\starter\config.toml`。使用记录位于同目录 `usage.json`。无遥测、网络索引或云端同步。

## 验证与当前范围

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

已在当前 Apple Silicon macOS 环境编译运行，并检查原生界面。Windows 代码及双平台 GitHub Actions 工作流已准备；当前 Mac 上交叉检查被依赖的 Windows 资源编译器 `llvm-rc` 缺失阻断，仍需 Windows 原生构建和运行验证。

当前实现应用启动、文件/内容搜索、终端热键、主题、设置界面、收藏与使用次数排序、最近使用区域。浏览器书签、剪切板历史、计算器历史、半屏窗口管理尚未实现。
