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

给别人发布时，推送 `v` 开头的 tag 即可触发 GitHub Actions，自动生成 Apple Silicon macOS、Intel macOS 和 Windows 安装包，并创建 GitHub Release。配置 Apple Developer 证书及公证 secrets 后，发布工作流支持正式签名与公证。

图标使用折线「S」。macOS 菜单栏嵌入 72 px 单色模板，由系统按 18 pt 显示并适配深浅外观；应用包包含 16–1024 px 的 `.icns`。图标资源位于 `resources/icons`，在 macOS 上运行 `swift scripts/generate-icons.swift` 可重新生成。

## 使用与设置

默认 macOS `Option+Space` 呼出，Windows `Ctrl+Space` 呼出。`Option/Alt+Enter` 全局打开终端（macOS 默认 Terminal，Windows 默认 wt.exe）。系统里如果已有相同热键，请修改冲突方或在设置里更换；注册失败会显示具体错误。

点击右上角设置图标、按 `⌘,` / `Ctrl+,`，或通过托盘「设置…」进入独立设置页：

- **快捷键**：通过按键选择框配置呼出、终端和任意应用的全局组合键，例如 `[Cmd] + [K]`。点击「＋」增加格子，点击「×」移除格子，支持搜索按键；每组为一个普通按键加修饰键，最多 5 格，重复或不完整的组合会提示。应用可以从已安装列表搜索选择，或浏览路径。应用快捷键支持编辑、移除和停用；终端快捷键清空可停用。选择过程不监听组合键，保存设置后生效。
- **打开方式**：按扩展名指定应用（例如 `md` → Visual Studio Code），`folder` 指定文件夹应用，`*` 指定其余文件。规则仅用于 Starter 打开搜索结果；移除规则后恢复系统默认。精确扩展名优先于 `*`。
- **外观**：Catppuccin Mocha、Everforest、Gruvbox、Catppuccin Latte；点选预览，取消恢复，保存持久化。可设置已安装的等宽字体。
- **搜索目录**：支持原生文件夹选择器和手工输入路径，一行一个，支持 `~`。
- **应用别名**：填写应用名称和逗号分隔的别名，点击添加或更新；可移除。
- **更新**：默认自动检查正式版本，可关闭或立即检查。启动时及持续运行期间每 24 小时检查 GitHub Release；发现新版后点击「下载并更新」，自动下载、校验、安装并重启；显示下载进度，下载期间可以取消，失败可以重试。macOS 按运行架构选择 DMG 并替换当前 `.app`；Windows 安装版使用 MSI，便携版使用 ZIP。详见 [自动更新设计](docs/UPDATING.md)。
- **剪贴板**：可暂停或开启后台记录，保存后生效；历史只保存在本机，可删除单条或确认清空全部。保留最近 30 天、最多 200 条，总量最多 100 MiB，单条最多 16 MiB。

保存时验证热键、目录和必填项；热键注册失败保留原设置。改动立即应用，无须重启。配置文件入口保留给高级编辑，手工修改后从托盘重新加载。

`⌘W`（Windows `Ctrl+W`）在搜索页和设置页都可收起窗口，保留托盘和全局快捷键；再次使用呼出快捷键回到搜索页，设置中未保存的改动会取消。macOS 用 `⌘Q` 完整退出，也可使用托盘「退出」。

窗口呼出并激活后，搜索框会重新获得焦点。顶部显示「可直接输入 · ↑↓ 选择」，搜索框下方同时显示强调色线条；焦点离开搜索框时提示「点击搜索框输入」，点击提示即可恢复。输入法正在选词时，上下键交给输入法处理。

| 查询 | 行为 |
| --- | --- |
| `safrai` | 搜索应用；模糊匹配，并容忍少量错字及相邻字母交换 |
| `微信` | 匹配 WeChat；自动索引应用的中文名称和内置中文别名 |
| `/f ui` | 在配置目录内，模糊匹配文件和文件夹名称 |
| `/c launcher_hotkey` | 在配置目录内，按字面查找文本内容，忽略大小写 |
| `/uninstall 微信` | 搜索可卸载应用；选择后确认，macOS 移到废纸篓，Windows 启动官方卸载向导 |
| `/clip needle` | 回搜复制过的文本、网址、文件名及路径，支持多个关键词 |
| `/clip 图片 2026-10` | 按类型和复制日期筛选图片，列表显示缩略图 |
| `Lock` / `锁屏`、`Sleep` / `睡眠` | 锁定会话或让电脑睡眠 |
| `Restart` / `重启`、`Shutdown` / `关机` | 显示确认页；明确确认后发送系统操作 |
| `bluetooth` / `display` / `sound` / `battery` | 直接找到对应的原生系统设置页，Enter 打开 |
| `/system` / `/settings` | 仅浏览系统命令与设置；支持 Wi-Fi、网络、键盘、通知、隐私 |
| `/remind 10m 开会` | 预览十分钟后的具体时间，Enter 保存提醒 |
| `/remind 明天 15:00 开会` | 预览明天下午三点，Enter 保存提醒 |
| `/remind` / `/reminder` | 浏览提醒，Enter 或「提醒」按钮进入创建与管理面板 |

也可直接点击搜索模式按钮。`↑↓` 选择、`Enter` 打开、`⌘/Ctrl+Enter` 在文件管理器定位、`Shift+Enter` 复制路径；`Esc` 逐级清空查询、退出模式、隐藏。点击应用行右侧星标可收藏或取消收藏。默认列表依次按收藏、启动次数、最近启动时间排序；输入查询后先按匹配准确度排序，再以收藏和启动次数打破平局。独立的「最近使用」区域显示最近打开的 5 个应用，可直接点击启动。收藏与使用记录保存在本地，隐藏后仍保留托盘和全局快捷键。

剪贴板模式也可输入 `/clipboard`。`Enter` 把选中内容按原类型复制并收起窗口，回到其他应用后粘贴；`Shift+Enter` 复制并保持窗口打开。支持文本、HTTP/HTTPS 链接、PNG/JPEG/TIFF/WebP/GIF/BMP 图片和单个/多个文件；文件恢复为原生文件列表，可粘贴到 Finder 或资源管理器。文件历史保存路径，不备份文件内容；原文件移动或删除后会提示无法复制。图片按类型、尺寸、复制日期检索，目前不做图片文字 OCR。记录从 Starter 启动后的新复制动作开始，重复内容合并并移到最新位置；系统标为私密或临时的内容会跳过。关闭 Starter 时不记录，无法追回开启记录前的旧剪贴板。

卸载模式排除 Starter 自身。macOS 仅处理 `/Applications` 和 `~/Applications` 下的应用包，排除系统应用与符号链接；请先退出目标应用，确认后移到废纸篓，保留文稿和个人设置。Windows 从用户及系统的 32/64 位卸载登记中读取可用卸载程序，排除系统组件；MSI 的维护命令转换为 `/x` 卸载，其他程序运行登记的卸载器，不删除开始菜单快捷方式。无卸载登记的便携应用及商店应用请从系统设置卸载。Windows 向导可能要求管理员确认。执行前会重新验证目标，确认期间路径或登记变化会要求刷新后重试。

系统命令和系统设置同时参与默认搜索，无须记住前缀；空查询继续展示收藏和常用应用。「系统」按钮或 `/system` 用于集中浏览这些操作。蓝牙、显示器、声音、电池可用英文或中文搜索。锁屏、睡眠立即发送；重启、关机先展示确认页，普通 Enter 不会连续执行，需要点击明确的确认按钮或按 `⌘Enter` / `Ctrl+Enter`，Esc 取消并回到搜索框。系统操作不使用用户输入拼接 shell 命令，不强制关闭未保存的应用。macOS 设置使用系统偏好设置 URL，Windows 使用 `ms-settings:`；系统策略或设备不支持的操作会提示失败。macOS 锁屏通过运行时解析的系统 login 接口实现，该接口属于私有接口，系统变更后可能失效；缺失时会提示使用 `Control+Command+Q`。

快速提醒支持搜索框单行创建，也可从右上角「提醒」、模式按钮或托盘进入面板。面板包含事项和时间两个输入框、5/15/30 分钟、1 小时、明早 9 点的快捷按钮；显示解析后的具体日期和时间，按 Enter 创建。已到时的提醒排在列表前面，可「完成」或延后 5 分钟，未到时的提醒可取消。顶部提醒入口显示到时数量。时间输入支持 `10m`、`2h`、`3d`、`10分钟后`、`15:00`、`明天 9:00`、`明天下午3点半`、`明早9点`、`YYYY-MM-DD HH:MM`；单行命令先写时间、再空格写事项。无日期的钟点表示下一次该时刻，明确写今天或日期且已过期则拒绝创建；不会猜测「一会儿」等模糊时间。最多保存 500 条待处理提醒，事项最多 200 字，时间最远为 366 天。

提醒只保存在本机配置目录的 `reminders.json`，以原子写入保存；保存失败不会报告成功或丢掉原记录。Starter 需保持后台运行才能准时发送系统通知，窗口收起不影响计时；退出或睡眠期间到时的提醒在恢复运行后显示并补发一次，已经送达的提醒不会在每次启动时重复通知。未处理的到时提醒一直保留在应用内。macOS 首次创建时请求系统通知许可，通过 `.app` 启动才能发送；拒绝通知、专注模式或前台抑制不会移除应用内提醒。Windows 使用 Starter 自己的通知标识，并写入 MSI 开始菜单快捷方式；不借用 PowerShell 的通知身份。当前不支持周期提醒、退出应用后的系统定时、点击通知跳回提醒或日历同步。

中文查询支持应用名称、文件名和 UTF-8 文本内容。macOS 从 `Info.plist` 及中文 `InfoPlist.strings` 读取简体和繁体名称；常见英文应用另有内置中文别名（如微信、企业微信、腾讯会议、钉钉、飞书、终端、系统设置），与设置页中的自定义别名合并，已有配置自动生效。搜索结果保留原应用名称。没有中文元数据或内置别名的应用，可在设置页添加中文别名。

默认不配置搜索目录，避免遍历整个用户目录。文件遍历遵循 `.gitignore` / `.ignore`，跳过隐藏文件、`.git`、`node_modules`、`target` 和 `.cache`。最多索引 100,000 个文件与目录，显示最多 100 条结果。内容搜索跳过二进制、无效 UTF-8 和大于 2 MiB 的文件，不解析 PDF、Office。目录索引保存在内存中，新建或删除文件后可从托盘刷新。

## 结构

- `src/ui.rs`：启动器界面、键盘交互、结果列表。
- `src/settings.rs`：设置页、校验和草稿；保存或取消后返回搜索。
- `src/settings/hotkey_editor.rs`：分格选择组合键、增删格子、组合预览与校验。
- `src/appearance.rs`：统一配色。深色配色参考 [Omarchy 官方主题](https://github.com/omacom/omarchy/tree/quattro/themes)，浅色来自 Catppuccin Latte。
- `src/icons.rs`：后台读取系统应用/文件图标，按需加载，有界缓存；缺失时使用 Lucide 类型图标。额外图标仅嵌入实际使用的条目。
- `src/platform.rs`：托盘、全局快捷键注册与事件路由、定位、窗口显示隐藏。
- `src/hotkeys.rs`：组合键解析、重复校验、注册失败回滚。
- `src/opening.rs`：打开规则与应用启动；macOS 使用 `open -a`，Windows 使用 ShellExecute（支持 `.lnk`）。
- `src/search.rs`、`src/catalog.rs`：可独立测试的搜索和应用发现。
- `src/worker.rs`：后台目录遍历、可取消搜索、80 ms 防抖、分批返回。
- `src/config.rs`、`src/history.rs`：本地 TOML 配置和 JSON 使用记录。
- `src/updates.rs`：正式版本检查、平台安装包选择、下载进度和 SHA-256 校验。
- `src/updates/install.rs`：安装准备、独立更新助手、进程退出等待、替换与启动失败回滚。
- `src/uninstall.rs`、`src/uninstall/`：可卸载应用发现、目标复核、macOS 废纸篓及 Windows 官方卸载器。
- `src/clipboard.rs`、`src/clipboard/native/`：原生剪贴板读写、后台变更监测、有界存储、缩略图、搜索和恢复。
- `src/system_commands.rs`、`src/system_commands/native.rs`：类型化系统命令目录、搜索别名、设置目标和双平台执行。
- `src/reminders.rs`、`src/reminders/time.rs`、`src/reminders/notifications.rs`：时间解析、本地提醒持久化、到时恢复与原生通知。
- `src/ui/launcher_commands.rs`、`src/ui/reminder_panel.rs`：命令交互、系统操作确认、快捷提醒面板。

macOS 配置在 `~/Library/Application Support/starter/config.toml`，Windows 在 `%APPDATA%\starter\config.toml`。使用记录位于同目录 `usage.json`。更新检查只请求 GitHub 版本信息，不发送查询、目录、配置或使用记录。无遥测、网络索引或云端同步。

剪贴板历史位于配置目录的 `clipboard/`，包括索引、原图和缩略图；`clipboard_history = false` 可暂停记录。清空历史删除这些记录与图片，不清空当前系统剪贴板、不删除原文件。macOS 历史目录仅当前用户可读，文件以私有权限原子保存。

## 验证与当前范围

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets

# macOS 更新助手的真实应用包测试（使用临时副本）
bash scripts/bundle-macos.sh --debug
python3 scripts/test-macos-updater.py
```

已在当前 Apple Silicon macOS 环境编译运行，验证原生设置界面、卸载确认与取消、DMG 准备、更新助手替换与重启。测试覆盖临时应用包移入废纸篓并恢复、隔离剪贴板的文本/图片/文件往返、历史持久化/去重/过期/清空、校验失败、下载取消、安装/启动失败回滚、快捷键冲突恢复与焦点回归。新增 Windows 核心模块通过独立交叉编译和 Clippy 检查；完整 Windows 交叉构建受本机 Windows SDK 和 `llvm-rc` 缺失阻断，MSI 自动更新、卸载与剪贴板读写仍需 Windows 原生运行验证。双平台 CI 继续运行格式、Clippy、测试和安装包检查。

本次新增的系统命令和提醒通过自动化测试、格式检查、严格 Clippy 与 macOS 构建验证；未操作真实电脑进行锁屏、睡眠、重启、关机，也未手动验证设置跳转或通知展示。Windows 核心模块仅做交叉编译检查，运行行为由 Windows 原生测试确认。

当前实现应用启动、文件/内容搜索、终端热键、主题、设置界面、收藏与使用次数排序、最近使用区域、应用卸载、多类型剪贴板历史、系统命令、系统设置搜索及本地快速提醒。浏览器书签、计算器历史、半屏窗口管理尚未实现。
