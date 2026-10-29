# Starter

Starter 是一个使用 Rust 和 GPU 原生渲染的桌面启动器。它常驻菜单栏，通过一个浮动窗口搜索应用、文件和系统操作；数据保存在本机，不使用云同步、分析服务或网页视图。

**macOS** `Option+Space` · **Windows** `Ctrl+Space`

[Starter 0.8.0 release notes](docs/releases/0.8.0.md)

## 功能

- 模糊应用搜索：支持中文名称和错字匹配
- 文件名、文件夹名和文件内容搜索
- 剪贴板历史：文本、网址、图片和文件
- 快速提醒和系统命令
- CPU、内存、启动盘和应用资源监控
- 应用卸载、自动更新和配置管理
- 本机软件更新：多来源清单、右键更新、自动版本检测与按项执行（macOS）
- 常用收藏：直接输入或一键收藏剪贴板，自动命名，无需分类；支持搜索、置顶和复制
- 轻量待办：快速添加、完成、恢复和删除撤销，与定时提醒独立
- 端口排查：按端口 / 进程 / PID 查找 TCP 监听与 UDP 绑定，核实身份后操作进程
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

输入 `/` 列出全部可用功能与系统命令，每行右侧显示命令写法。继续输入英文前缀或中文名称筛选，例如 `/mon`、`/待办`、`/发票`；`↑↓` 选择，`Enter` 进入，`Esc` 返回。列出命令不扫描文件或启动资源监控。软件更新入口仅在 macOS 提供。

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
| `/update`、`软件更新` | 搜索并打开本机软件更新面板 |
| `/save 发票`、`常用收藏` | 打开收藏并搜索内容 |
| `/todo 买牛奶`、`待办` | 打开待办，带入输入内容，再按 Enter 添加 |
| `/ports 3000`、`端口排查` | 查看指定端口的占用进程 |

`↑↓` 选择，`Enter` 打开，`⌘/Ctrl+Enter` 在 Finder 或资源管理器中显示，`Shift+Enter` 复制路径。收藏应用会排在前面。

资源监控中，`↑↓` 选择应用或后台进程，`Enter` / `⌘K`（Windows 为 `Ctrl+K`）、右键或行末“操作”按钮弹出操作菜单，监控列表保持显示；点击外部或按 Esc 收起。支持正常退出、强制退出、定位和复制路径；macOS 还支持发送 SIGTERM 终止进程。终止进程和强制退出需要再次确认。正常退出允许应用提示保存或取消；列表会继续刷新，发送退出请求不代表应用已经退出。

## 工作台：收藏、待办与端口

首页点击「工作台」，或用上面的搜索命令进入。收藏无需分类：在页面上方直接输入或粘贴内容，点击「收藏」或按 `⌘Enter` / `Ctrl+Enter` 保存；「收藏剪贴板」可一步保存剪贴板文本。名称自动取自内容首行，需要时再编辑名称与内容。`⌘N` / `Ctrl+N` 聚焦输入框。收藏左侧选择、右侧预览，`Enter` 复制，多行文本原样保留；保存和复制后均有反馈。右键或行末菜单可以编辑、置顶、删除和加入待办。未保存的内容在切换窗口时保留，放弃编辑修改需要确认。

待办输入后按 Enter 添加，点击圆圈完成；「已完成」中可恢复。完成与删除后可点击「撤销」。收藏加入待办时保留内容与链接，重复加入会恢复已有的阅读事项。

收藏与待办存在配置目录内的 `organizer.json`，仅本地存储。后台串行保存、原子替换；文件读取失败时保留原文件并显示原因，可点击重新读取。单条文本最多 256 KB，总计最多 5000 项 / 16 MB。

端口仅在首次进入、手动刷新（`⌘R` / `Ctrl+R`）和进程操作后扫描，不持续轮询。显示当前用户可见的 TCP 监听和 UDP 绑定，不含普通 TCP 客户端连接。纯数字搜索匹配完整端口号或 PID。右键复制地址、PID 或路径；终止、强制结束前显示影响范围并再次核对进程身份。macOS 支持 SIGTERM 和强制结束，Windows 支持强制结束；不能核实身份的进程仅允许查看和复制。

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

## 本机软件更新

macOS 点击首页「软件更新」，或搜索 `/update` 打开。此功能直接集成在 Starter 中，不依赖旧的 `update-all` 脚本。

- 直接读取 Homebrew、npm/pnpm/Bun 全局包、uv tool、pipx、Cargo 和 rustup 的安装记录；支持多个已知 PATH 中的 Homebrew/Node 管理器实例。按安装来源和根路径标识项目，不按名称合并不同环境的同名包。
- 右键菜单首项直接更新单个软件，进度留在列表，按需展开详情。`⌘` 点击或 `Shift` 点击多选后才出现「更新所选」，批量执行前可查看所选项目；没有自动勾选或全量升级。每项使用独立终端任务，任一项失败或中断便停止后续队列，保留逐项结果。执行前核对安装来源、当前版本、固定状态和原安装参数。
- 打开面板后自动后台查询版本；「检查更新」或 `⌘R` 还会刷新 Homebrew 元数据。支持 npm/pnpm/rustup、Sparkle appcast、App Store 公开接口，以及应用文件名和 bundle ID 双重匹配的 Homebrew Cask 目录。自更新 cask 对比实际应用版本，避免被旧安装记录误导。检查失败、更新源待确认和未检查分别显示，不把网络失败当成已是最新。
- Homebrew 固定项不能更新；Node 更新交给原管理器、不强制 `--latest`，链接、别名和自定义来源交回原环境；uv/pipx 保留原安装策略；Cargo 仅在标准 registry 来源及安装参数完整时可执行，保留原 binaries、features、target、profile 和版本约束。
- 独立 CLI 只在已确认的原生安装位置或安装收据匹配时开放自更新，避免与包管理器重复管理。独立 Deno 的频道、Kimi 的产品版本或其他安装来源未确认时提供官方处理入口。App Store、macOS、Sparkle 和其他应用使用原更新入口；未知官网不猜测，不自动下载或替换应用。Conda/MacPorts/Nix/TeX/RubyGems/Yarn 目前只提供检测和说明入口，不承诺逐包自动更新。

右键菜单分组提供更新、固定 Homebrew 版本、多选、主页、访达定位和复制名称/路径，支持方向键、回车、Esc，并在窗口边缘自动避让。终端支持输入、复制与中断；返回列表后释放终端缓冲，仍可查看上次逐项结果。管理器可能调整依赖或同组包，计划页会提示。

性能上复用启动器的应用清单；本地清单按运行环境缓存 2 分钟，在线检查结果缓存 10 分钟，重复打开直接复用；最多 3 个检测进程并行，带超时、输出上限和取消回收。公开应用目录流式解析，只保留匹配记录；网络请求有超时、读取上限和取消检查。更新会取消正在进行的扫描，后台刷新保留选中项与滚动位置。终端只在开始执行时创建，搜索文本预先索引，列表虚拟化。路径来自进程环境和常见安装位置，不执行用户 shell 初始化脚本，不扫描所有项目虚拟环境或所有历史 Node 版本。更新完成后重新读取本地清单，不再次自动联网检查。

各来源的协议和限制见 [software-update-sources.md](docs/software-update-sources.md)。

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

Starter 的搜索、提醒、剪贴板和监控功能在本地运行。网络请求用于 GitHub 更新检查和下载，以及打开软件更新面板后的公开版本检测、用户发起的软件源刷新和软件更新；软件更新由各软件原有的管理器或自身更新入口完成。macOS 的清理卸载由 Mole 负责，Starter 负责应用选择、路径校验、任务面板和进程生命周期；原生废纸篓卸载始终保留为不依赖 Mole 的回退方案。
