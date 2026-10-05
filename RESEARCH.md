# 轻量桌面启动器：需求与技术调研

调研日期：2026-10-05。目标平台：macOS、Windows。本文是产品和架构决策，不是实现承诺；性能、内存和兼容性数字需在目标机器上实测。

## 结论

先做一个**键盘优先的轻量入口**：全局快捷键呼出，输入应用名或别名，回车启动；另设 `Option+Enter`（macOS）直接打开用户选定的终端。它应当常驻、离线可用、无需账号。参考 [Omarchy 的终端直达和应用菜单](https://github.com/omacom/omarchy/blob/quattro/manual/07-hotkeys.md)，借鉴短路径和可记忆的键位，不复制它的 Linux 桌面环境、安装器和完整平铺窗口管理。

搜索入口采用显式模式：默认搜索应用；`/f 查询` 只搜文件和文件夹名称；`/c 查询` 只搜选定目录中的文本内容。先交付应用入口，再交付 `/f`，然后 `/c`。这避免文件内容扫描拖慢打开应用的常用路径，也不需要一开始维护全盘内容索引。

技术路线改为 **Rust + [GPUI Kit](https://github.com/longbridge/gpui-kit)** 优先。你指的是 Longbridge 的组件框架，而不只是 Zed 裸 GPUI；上一版按裸 GPUI 评价组件成熟度不准确。GPUI Kit 将匹配版本的 GPUI、`gpui-base` 和 `gpui-component` 组织在一起，提供输入、列表、可访问性和 UI 集成测试能力；其 `gpui-shell` 是可选的 JavaScript 扩展层，本项目不需要。[GPUI Kit 架构与用法](https://github.com/longbridge/gpui-kit#framework-architecture) UI 框架仍不等于操作系统外壳：全局热键、托盘、应用枚举、开机启动和打包仍需单独适配。MyGo 保留为技术试验不通过时的替代方案，尤其是它已有这些桌面 API。[MyGo README](https://github.com/egoist/mygo)

这里的“原生”指本机进程/窗口、系统输入与热键和 GPU 绘制，不等于控件由 AppKit/WinUI 原生绘制。GPUI Kit 的较高关注度和公开产品案例增加了信心，但不能代替这个常驻启动器在两平台上的实测。[GPUI Kit README](https://github.com/longbridge/gpui-kit) GPUI 底层仍是 pre-1.0，需锁定明确版本。[GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)

## 用户任务与首版范围

主要任务：人在任何应用中按 `Option+Space`（macOS）或可配置的 Windows 快捷键，马上输入少量字符并打开应用。`Option+Enter` 是**全局直接打开终端**，无需先呼出搜索框；终端目标可配置，若已运行则优先激活还是新开窗口需在试验中确定一致行为。Windows 的 `Alt+Space` 常用于系统菜单，不宜未经实测直接设为默认值。启动器应可用 Escape 收起、上下键选中、Enter 打开；失去焦点后收起。热键冲突需提示并允许重设。[Microsoft RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey)

首版验收建议：

1. macOS 与 Windows 都可发现、启动已安装应用；应用新增或卸载后能更新列表。扫描位置和刷新方式由各平台适配，不把路径规则写进 UI。
2. 在任意应用中呼出后，输入法可立即接收文字；窗口出现在当前屏幕的合理位置；关闭与再次呼出不丢焦点、不误吞快捷键。
3. 空查询显示少量最近使用/置顶应用。查询结果有稳定排序；名称、常见缩写、用户别名可匹配。错一字或相邻两字颠倒时，在合理候选范围内仍能找到目标，但正确拼写的结果优先。
4. 可更改呼出和终端直达快捷键；终端未安装、改名或绑定失败时给出清晰提示。任意应用的直接热键可延后，不必首版做完整快捷键管理器。
5. 基础设置包含开机启动、两枚全局热键、终端目标、应用别名和退出。配置本地存储，无账号和联网要求。

确认纳入后续范围：浏览器书签、剪贴板历史、带历史的计算器。明确排除：AI、PDF/Office 内容解析、通用插件市场、完整窗口管理和 Omarchy 式安装/显示器设置。左右半屏单独做可行性试验，若成本合理再加两个动作，不扩成平铺窗口管理器。Raycast 的根搜索能力可作交互参考，不作为功能清单。[Raycast Search Bar](https://manual.raycast.com/search-bar)

## 搜索路线

### 第一阶段：应用名称

将候选来源和排序器分开。候选含应用 ID、显示名、路径、图标、别名和启动动作；排序综合精确/前缀、词首/驼峰缩写、子序列模糊匹配、用户别名、最近使用和置顶。错字容忍要单独处理：`fzf`/Telescope 式子序列匹配允许跳字，却未必允许替换或颠倒字符。应先在候选集上用轻量模糊匹配，再对少量高可能候选做编辑距离补救，限制查询长度与候选数，避免每击键全量计算。测试用例要包含英文缩写、大小写、拼写错误、转置以及中文/日文输入法组合态。Rust 可评估 `nucleo-matcher`（子序列匹配）和 `strsim` 的 Damerau-Levenshtein；两者职责不同。[Nucleo](https://github.com/helix-editor/nucleo) [strsim](https://github.com/rapidfuzz/strsim-rs)

### 第二阶段：文件/文件夹名称

输入 `/f 查询` 进入文件名模式，搜索文件和文件夹；允许配置根目录（如 `~/Documents`、项目目录），默认排除构建目录、版本控制目录和大体积缓存。结果展示名称、父目录、类型、最近修改时间，支持回车打开、定位到文件管理器、复制路径。先做限定目录搜索；不要承诺一开始就在所有磁盘、网络盘和云盘上实时搜索。

实现上抽象 `FileCandidateSource`，保留两条路线：先基于用户指定目录后台枚举并维护轻量文件名缓存；若以后需要整机范围和低延迟，再考虑系统索引适配器。macOS 的 `NSMetadataQuery` 可查询 Spotlight 索引并接收异步更新；Windows Search 有自己的查询与索引语义，覆盖范围由系统设置决定。[Apple NSMetadataQuery](https://developer.apple.com/documentation/foundation/nsmetadataquery) [Microsoft Windows Search](https://learn.microsoft.com/en-us/windows/win32/search/-search-sql-where) 这两者不能被视为完全一致的跨平台搜索后端。不要把 Everything 设为 Windows 的硬依赖；其命令行客户端依赖正在运行的 Everything 服务。[Everything ES](https://github.com/voidtools/ES)

### 第三阶段：文件内容

输入 `/c 查询` 只搜索用户选定根目录的**文本文件内容**，显示匹配行、行号和路径。搜索任务需支持取消、结果流式返回、大小/类型限制和忽略规则；输入变化时取消旧任务。Rust 可以直接复用 `ripgrep` 的 `ignore`、`grep-searcher` 等 crates，默认尊重 `.gitignore`、`.ignore` 等规则并跳过隐藏/二进制文件；这很适合开发目录，但对一般文档目录可能需要单独的默认规则与可见说明。[ripgrep Guide](https://github.com/BurntSushi/ripgrep/blob/master/GUIDE.md) PDF、Office 和语义搜索不在计划内。

模式解析只把开头的独立 `/f`、`/c`（后接空格或单独输入）当命令，避免把路径或 URL 误判为模式。Escape 可先清除当前查询，再退出模式。内容搜索与文件名搜索各有独立的结果类型和操作，不合并成一个持续扫描的默认结果流。

## 补充功能的边界

**浏览器书签**：这是真实浏览器中的书签，不以手工 Quicklink 冒充。第一步做用户主动导入浏览器导出的 HTML 书签文件，支持标题、URL、文件夹路径、按名称或域名搜索及在默认浏览器打开；记录导入时间并允许重新导入。之后再评估 Chrome、Edge、Firefox 的授权式实时同步；浏览器扩展 API 需要书签权限，Safari 则要单独评估，故不承诺首版自动同步。[Chrome Bookmarks API](https://developer.chrome.com/docs/extensions/reference/api/bookmarks) [Firefox Bookmarks API](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Work_with_the_Bookmarks_API)

**计算器**：用 `=表达式` 进入计算模式，显示结果、复制结果、查看与清除本地计算历史。表达式交给现有 Rust 数学解析库（如 [fend](https://github.com/printfn/fend)），不自写语法/求值器；先验证精度、单位和本地化输入是否符合预期。该模式不发起网络请求。

**剪贴板历史**：初版只记录文本和 URL，提供搜索、复制回剪贴板、单条删除、全部清除、暂停记录和容量/保留期设置。数据只保存在本机；首次启用前说明会记录复制的内容。macOS 可观察 `NSPasteboard.changeCount`，Windows 可接收剪贴板更新通知；两平台的后台监听与隐私过滤需单独实测。[NSPasteboard.changeCount](https://developer.apple.com/documentation/appkit/nspasteboard/changecount) [Windows Clipboard](https://learn.microsoft.com/en-us/windows/win32/dataxchg/using-the-clipboard)

**左右半屏**：仅考虑把当前窗口移到屏幕左/右半边的两个命令，可绑定快捷键；不做布局保存、自动平铺或多窗口规则。Windows 可用 `SetWindowPos`；macOS 操作其他应用窗口通常要经 Accessibility API 和授权。GPUI Kit 的 Dock 是**本应用内部面板布局**，与外部窗口管理无关。这个功能应在核心稳定后单独试验，尤其验证多显示器、最大化窗口和权限处理；若成本超出两个动作的价值就延期。[SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos) [AXUIElement](https://developer.apple.com/documentation/applicationservices/axuielement)

## 建议架构

单进程、模块化。窗口隐藏时进程常驻；应用目录在后台刷新；每次输入只在内存候选上排序。文件内容搜索与剪贴板监听不得阻塞 UI 线程。初期不引入插件、JS 扩展、后台服务或统一索引数据库。

```text
展示层      GPUI Kit 输入框、结果列表、设置界面
            |
应用层      LauncherController / QuerySession / HotkeyActions
            |
领域层      ModeParser / Candidate / Action / Matcher / Ranker
            |
数据适配层  AppCatalog / FileCandidateSource / ContentSearcher
            |       BookmarkStore / ClipboardStore / HistoryStore
            |
系统集成层  macOS / Windows：热键、焦点/窗口、应用枚举/启动、
            托盘、开机启动、剪贴板；半屏窗口操作可选
```

这里的接口只围绕真实变化点：平台 API、候选来源、内容搜索。`Candidate` 统一承载显示与可执行动作，但应用、书签、文件名和内容命中不混成一个无边界的大列表。UI 状态只保存当前模式、查询、选中项、候选快照和运行中的任务；使用记录与设置持久化，文件内容只保存当前搜索的短暂片段。搜索更新使用查询序号或取消令牌，防止旧结果覆盖新查询。GPUI Kit 用其输入、列表和可访问性组件即可；内置 Dock、代码编辑器及可选 `gpui-shell` 不属于此工具的依赖目标。[GPUI Kit README](https://github.com/longbridge/gpui-kit)

## GPUI Kit 与 MyGo 的取舍

| 标准 | GPUI Kit | MyGo 原生 UI |
| --- | --- | --- |
| 主语言 | Rust | Go |
| UI 与搜索 | 现成输入/列表/可访问性组件；可直接复用 `nucleo-matcher` 与 ripgrep crates | 官方列有原生 UI；文件名匹配需评估 Go 库，内容搜索可封装打包的 `rg` |
| 启动器系统 API | 窗口与输入在框架范围内；全局热键、托盘、应用枚举需另行适配 | 官方列有全局快捷键、托盘、窗口、开机启动等，仍需两平台实测 |
| 主要风险 | 底层 GPUI pre-1.0；热键/托盘与事件循环配合需要验证 | 仓库较新，原生 UI 和平台细节需要实机验证；搜索工具链分属两种语言/进程 |

选择 GPUI Kit 的依据是你指定的框架，加上它已有启动器需要的输入与列表组件、Rust 搜索库能直接复用。先做 macOS 和 Windows 的小型技术试验：隐藏窗口反复呼出、失焦收起、输入法组合输入、多屏定位、快捷键冲突、应用枚举与启动、睡眠唤醒后恢复。重点验证 `global-hotkey`、`tray-icon` 的主线程/事件循环要求如何与 GPUI 配合，不能假定直接拼接即可。[global-hotkey](https://github.com/tauri-apps/global-hotkey#platform-specific-notes) [tray-icon](https://github.com/tauri-apps/tray-icon#platform-specific-notes) 若 GPUI Kit 在这一关键路径上确实需要大量平台补丁，再评估 MyGo，保持同一需求边界。

## 推荐实施顺序

1. 技术试验：GPUI Kit 窗口与系统热键在两平台的呼出/隐藏/焦点/输入法/睡眠唤醒行为，记录实测启动时间和常驻资源。
2. V0：应用发现、模糊搜索、启动、最近使用/置顶、基础设置，以及 `Option+Enter` 终端直达。
3. V1：`/f` 搜用户配置目录的文件/文件夹名称，打开、定位、复制路径。
4. V2：`/c` 搜所选目录的文本内容，带取消、流式结果和忽略规则。
5. V3：浏览器 HTML 书签导入及搜索；`=表达式` 计算器与本地历史。
6. V4：可暂停、可清理的文本剪贴板历史。左右半屏作为单独的可行性试验，验证后再决定是否并入。

V0 完成的判断标准是：从其他应用按键到启动目标应用的路径稳定、输入法和焦点可靠、错字情况下仍能找到常用应用，而不是做出最多命令。每阶段都保留 macOS/Windows 同一行为契约；若某平台差异明显，在设置或结果中明确呈现。
