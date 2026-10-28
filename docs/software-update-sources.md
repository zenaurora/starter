# 本机软件更新：数据源与执行边界

核对日期：2026-10-27。本文是 Starter 直接集成更新能力的实现依据，命令是待实现的适配器协议，研究时没有执行更新、安装或电脑界面操作，也没有读取或调用旧 `update-all`。在线文档描述的版本可能高于本机版本；必须先探测版本和 `--help`，不能把新参数直接用于所有旧版本。

## 能力模型

每个安装实例使用 `(provider, installation_root, provider_id)` 标识，显示名称只用于展示。多个 Homebrew 前缀、多个 Node 工具链以及 npm/pnpm/Bun 中的同名包不能仅按名字合并。应用路径、bundle identifier、包管理器记录可以关联，但更新权仍属于实际安装来源。

检查结果至少区分：发现更新、未发现更新、固定版本、自带更新器、需手动处理、未检查、检查失败。缺少更新记录、网络失败、不支持检查、解析失败都不能显示为“已是最新”。版本选择交给对应管理器，不能用字符串排序或一套通用 semver 推断所有软件的新旧。

执行计划必须来自已选中的、已验证的安装实例；没有选中项时拒绝执行。更新成功后重新读取该来源，不能仅凭进程退出成功便修改当前版本。命令参数以独立 argv 传入，不拼接 shell，不猜测可执行文件的 `self-update` 参数。

## 包管理器协议

下表中 `<id>`、`<name>`、`<target>` 都必须来自适配器验证过的数据，不能直接接受任意显示文本。

| 来源 | 已安装清单 / 更新检查 | 选中项更新 | 解析与限制 |
| --- | --- | --- | --- |
| Homebrew | `brew info --json=v2 --installed`；`brew outdated --json=v2` | `brew upgrade --formula <full_name>` 或 `brew upgrade --cask <token>` | 都是 JSON；outdated 顶层为 `formulae`、`casks` 数组。默认跳过自带更新器和 `version :latest` 的 cask，不能把跳过视为最新。固定项应单独呈现。[查询接口](https://docs.brew.sh/Querying-Brew)、[命令手册](https://docs.brew.sh/Manpage)、[outdated 源码](https://github.com/Homebrew/brew/blob/main/Library/Homebrew/cmd/outdated.rb) |
| npm 全局包 | `npm ls --global --depth=0 --json`；`npm outdated --global --json --long` | 受原策略约束：`npm update --global <name>`；用户明确选定目标时：`npm install --global <name>@<target>` | 清单为 `dependencies` 映射；outdated 的键对应包名，值包含 `current/wanted/latest/location`。`latest` 是 registry 标签；git、link、别名需保留来源。[ls](https://docs.npmjs.com/cli/v11/commands/npm-ls/)、[outdated](https://docs.npmjs.com/cli/v11/commands/npm-outdated/)、[install](https://docs.npmjs.com/cli/v11/commands/npm-install/) |
| pnpm 全局包 | `pnpm list --global --depth 0 --json`；`pnpm outdated --global --json` | `pnpm update --global <name>`；用户选择跨原范围升级时添加 `--latest` | list 是 JSON 项目列表，outdated 为包记录映射。发现过期项时退出码为 1；不能直接当失败。命名选择仍可能迁移旧全局布局或影响同组依赖。[list](https://pnpm.io/cli/list)、[outdated](https://pnpm.io/cli/outdated)、[update](https://pnpm.io/cli/update) |
| Bun 全局包 | `bun pm ls --global`；`bun outdated --global` | `bun update --global <name>`，跨范围升级显式添加 `--latest` | 官方仅保证清单文本树和 outdated 表格，没有为这些命令承诺 JSON；需按支持版本的固定样本解析，失败保留未知状态。[pm](https://bun.com/docs/pm/cli/pm)、[outdated](https://bun.com/docs/pm/cli/outdated)、[update](https://bun.com/docs/pm/cli/update) |
| uv tool | `uv tool list --show-paths`；可用时加 `--show-version-specifiers` | `uv tool upgrade <name>` | 官方没有 tool list JSON 或 tool outdated 协议。可读已装版本和安装位置，但没有可靠候选时应显示“按需检查/自主管理”。upgrade 会保留原安装约束和设置，不要以重新 install 偷换约束。[CLI](https://docs.astral.sh/uv/reference/cli/)、[工具概念](https://docs.astral.sh/uv/concepts/tools/) |
| pipx | 支持参数时：`pipx list --skip-maintenance --json`；`pipx list --skip-maintenance --outdated --output json` | `pipx upgrade <environment>` | 新旧 JSON 格式不同，见下文。使用环境名，不能假定环境名等于包名；不默认更新注入包。旧版缺少只读检查能力时降为清单/手动。[CLI](https://pipx.pypa.io/stable/reference/cli.html)、[JSON 协议](https://pipx.pypa.io/stable/reference/json-output.html) |
| Cargo 安装的二进制 | `cargo install --list` | 只有来源和选项齐全时：`cargo install <crate> --version <target> --locked`，保留原 root/registry/git/features 等参数 | 官方清单为文本，无通用“已安装二进制 outdated”接口。`cargo outdated` 通常针对项目依赖，不能替代此协议。git/path 来源不能按同名 crates.io 包更新；缺少原安装参数时保留手动处理。[cargo install](https://doc.rust-lang.org/cargo/commands/cargo-install.html) |
| rustup 工具链 | `rustup toolchain list --verbose`；支持时 `rustup check --no-self-update` | `rustup update <channel-host> --no-self-update` | 文本协议；rustup 1.29 起 check 发现更新返回 100，无更新返回 0。精确版本、带日期 nightly、自定义链接工具链应视为固定项，不能替换为 rolling stable。[基础命令](https://rust-lang.github.io/rustup/basics.html)、[工具链](https://rust-lang.github.io/rustup/concepts/toolchains.html)、[变更记录](https://github.com/rust-lang/rustup/blob/main/CHANGELOG.md)、[CLI 源码](https://github.com/rust-lang/rustup/blob/main/src/cli/rustup_mode.rs) |
| Mac App Store / mas | 支持时 `mas list --json`；`mas outdated --json` | `mas update <numeric-id>`，旧版兼容别名 `upgrade` | 当前 JSON 是每行一个对象的流，不能按单个 JSON 数组解析。旧版可能仅文本。依赖 Spotlight；快速检查会受商店 API 信息延迟影响。更新可能需要账户/管理员交互。[mas 官方说明](https://github.com/mas-cli/mas#readme) |
| macOS 系统更新 | `softwareupdate --list` | 精确 label：`softwareupdate --install <label>`；涉及授权/重启优先交给系统设置 | 文本协议，label 不等于 title/version；不要添加 `--all`、`--recommended` 或 `--restart`。`--list-full-installers` 是兼容安装器列表，不代表本机待安装更新。[本机 Apple man page](/usr/share/man/man8/softwareupdate.8)、[Apple 安装器说明](https://support.apple.com/en-us/102662) |

### 必须处理的协议细节

Homebrew 发现检查可设置 `HOMEBREW_NO_AUTO_UPDATE=1`，减少隐式元数据刷新；对应结果必须携带检查时间及“本地元数据可能过期”信息。刷新元数据应作为独立动作。即便指定一个 formula，Homebrew 也可能更新依赖或重装受影响的依赖方；执行摘要要说明管理器会处理依赖。不要为了减少工作量无条件禁用依赖方检查。[Homebrew 手册](https://docs.brew.sh/Manpage)

npm outdated 的退出码 1 可以表示存在过期包，合法 JSON 应优先按协议解码。当前实现会静默跳过 `ETARGET/E403/E404`，也会跳过非 registry 来源；所以空映射不能证明每个已安装项都已验证为最新。相同名称可能映射到多个记录，适配器应支持对象或数组，并核对位置。[npm 实现](https://github.com/npm/cli/blob/latest/lib/commands/outdated.js)

pipx 普通 list 的历史 JSON：

```json
{"pipx_spec_version":"0.1","venvs":{"example":{"metadata":{"main_package":{"package":"example","package_version":"1.0"}}}}}
```

新 `list --outdated --output json` 使用结果 envelope，不能复用普通 list 解码器：

```json
{"pipx_result_version":"1","command":["list"],"status":"success","exit_code":0,"data":{},"errors":[]}
```

以上仅展示 envelope/关键结构，`data` 必须用本机支持版本的真实样本确认；`partial` 不能丢弃已成功的记录或掩盖错误。`--skip-maintenance` 很重要：不加时 pipx 某些操作会自动维护共享库，不能声称是纯检查。不要记录完整 metadata/pip_args，因为可能包含私有索引凭据。[JSON 协议](https://pipx.pypa.io/stable/reference/json-output.html)、[CLI 参考](https://pipx.pypa.io/stable/reference/cli.html)、[元数据说明](https://pipx.pypa.io/stable/reference/metadata.html)

mas 的 `--accurate` 会启动下载过程以获得准确版本，可能弹出验证对话框或挂起；禁止在后台扫描中默认使用。JSON 字段需按已支持版本的样本适配，不能因为官方支持 JSON 就假定所有版本同一 schema。[mas 官方说明](https://github.com/mas-cli/mas#readme)

## 独立 macOS 应用

只读取本机 bundle 的 `Info.plist`，可提取 bundle identifier、`CFBundleVersion`、`CFBundleShortVersionString` 和 Sparkle 的 `SUFeedURL`。前者通常参与 Sparkle 比较，后者用于显示。`SUFeedURL` 是更新源而非官网；应用运行时还能覆写 feed，加入频道、系统兼容性等条件。不能把 RSS 第一项或最大的显示版本直接当作本机可用更新。[Sparkle 入门](https://sparkle-project.org/documentation/)、[发布与 feed 选择](https://sparkle-project.org/documentation/publishing/)、[appcast 顺序](https://sparkle-project.org/documentation/api-reference/Classes/SUAppcast.html)

通用跨应用 Sparkle 安装器不适合首批支持：候选需要校验最小/最大系统版本、架构、最低可升级版本，真正安装还涉及签名与应用自身更新策略。首批可记录“自带更新器”，提供打开应用；仅在可信来源明确提供主页时提供官网操作。没有 `SUFeedURL` 仍可能存在专用自更新机制，不能据此认定不支持更新。[Sparkle appcast item](https://sparkle-project.org/documentation/api-reference/Classes/SUAppcastItem.html)、[更新安全与发布](https://sparkle-project.org/documentation/publishing/)

CLI 自更新也必须逐工具建立白名单协议，并先判断安装所有者。例如包管理器安装的 uv 不允许 `uv self update`；应返回其包管理器。不要让 Brew、npm 和工具自己的自更新器同时控制同一实例。[uv 安装与自更新](https://docs.astral.sh/uv/getting-started/installation/)

### 独立安装 CLI 的 self update 白名单

这些命令只能用于已确认由官方独立安装器管理的实例；`--version` 可以读取当前版本，但不代表已经完成远端检查。没有官方只检查协议时，界面应提供“更新此工具”动作并显示目标未知，不能为了获取候选而偷偷执行更新。

| 工具 | 检查与单工具执行 | 安装归属、频道限制 |
| --- | --- | --- |
| Claude Code | 当前版本：`claude --version`；独立安装更新：`claude update` | 原生安装支持自行更新；Homebrew 由原 cask 更新，stable/latest cask 不同。`claude update` 对包管理器实例可能只输出 up to date，不能把这句话当作远端验证成功。保留 `autoUpdatesChannel`、版本上下限等已有策略。[官方设置与更新](https://code.claude.com/docs/en/setup#update-claude-code) |
| Kimi Code CLI | 当前版本：`kimi --version`；支持该协议时：`kimi upgrade`，由 Starter 明确选择执行后可用 `kimi upgrade --yes` | 新版会按原生/npm/pnpm/yarn/Bun 安装方式选择升级路径，某些方式只打印手动命令。不要自动执行打印出的任意 shell 文本。官方旧 `kimi-cli` 已归档；旧 uv 工具不能自动当成新 `kimi-code` 或静默迁移产品。[当前命令](https://moonshotai.github.io/kimi-code/en/reference/kimi-command)、[旧项目状态与迁移](https://moonshotai.github.io/kimi-cli/en/guides/getting-started.html) |
| OpenCode | 当前版本：`opencode --version`；独立安装：`opencode upgrade` 或 `opencode upgrade <target> --method curl` | `--method` 还支持 npm/pnpm/bun/brew，但 Starter 已知道所有者时应直接派发原管理器，避免重复路径。不要默认加 `--major`：该选项会迁移到 OpenCode 2 并移除 OpenCode 1。[官方 CLI](https://opencode.ai/docs/cli/#upgrade) |
| Bun 本体 | 当前版本：`bun --version`；独立安装：`bun upgrade` | 官方要求 Homebrew 用户用 `brew upgrade bun` 避免冲突。`--canary`/`--stable` 会选频道，不能为了“更新”擅自切换。[官方安装与升级](https://bun.com/docs/installation#upgrading) |
| Deno 本体 | 当前版本：`deno --version`；检查：`deno upgrade --dry-run`；独立安装执行：`deno upgrade`，已确认稳定版目标可用 `--version <target>` | Homebrew 安装走 `brew upgrade deno`。新版支持 stable/LTS/RC/canary；裸版本号选择 stable，即使版本号看起来相同也可能改变 LTS 来源，必须保留频道。[升级协议](https://docs.deno.com/runtime/reference/cli/upgrade/)、[安装归属](https://docs.deno.com/runtime/getting_started/installation/) |
| uv 本体 | 当前版本：`uv --version`；独立安装：`uv self update`，具体目标语法需按该版本 help 探测 | 官方自更新只用于独立安装器；通过 pip、Brew、Cargo 等安装的实例应回到原管理器，不将禁用 self update 当失败后重新下载安装。[官方安装与升级](https://docs.astral.sh/uv/getting-started/installation/#upgrading-uv) |

## 对 Starter 的实现建议

1. 首批优先实现可靠 JSON 来源：Homebrew、npm、pnpm；mas 按版本适配。其余来源先如实展示清单、来源和能力，不把研究覆盖范围伪装成自动更新支持范围。
2. 分开“发现安装实例”“检查候选”“执行选中计划”。清单首屏可以来自缓存；耗时检查后台更新。每个来源一次批量查询，避免为每行单独生成进程。
3. 限制进程并发、执行时间和输出大小；stdout/stderr 分开读取且同时排空。超时取消整个子进程组，更新输出使用有界日志。相同来源/根路径的写操作串行，检查不与其写操作争抢锁。
4. 缓存按来源、可执行文件绝对路径、安装根目录、配置环境和 schema 版本区分；附时间与错误，避免不同 Node/Homebrew 环境串用结果。刷新进行中复用同一个任务，防止重复扫描。
5. 扫描只探测已存在的管理器，不自动安装管理器、修改 PATH、维护共享库或运行自身升级。文本协议固定 locale 并保留原始诊断；解析不完整时不推导“零更新”。
6. 选择状态保存安装 ID，执行前重新验证目标存在、仍由该来源管理且未被固定；拒绝空选择和无目标命令。只刷新实际受影响的来源，避免每次更新后重新扫描整台机器。

以上是实现约束与取舍，不表示这些来源已在 Starter 中全部实现。Nix、MacPorts、Conda 等可沿相同适配器能力模型扩展；未研究和未支持的来源应直接标记未知/手动，不采用猜测命令兜底。


## 当前交互与应用版本检测（2026-10-27）

列表只显示图标、名称、说明、当前版本和更新状态。右键或 Enter 打开分组菜单，首项直接更新；单项执行留在列表，详情按需查看。⌘ 点击或 Shift 点击多选，才显示批量操作。每项仍使用来源校验和独立 argv 执行。

打开面板后后台检查，自动检查不触发 `brew update`；用户主动「检查更新」时才刷新 Homebrew 元数据。在线快照缓存 10 分钟。扫描在执行前取消，刷新不重置正在浏览的位置。

- Sparkle：直接读取 HTTPS appcast，比较内部 build；排除非默认频道、delta、其他平台和不满足系统/架构/最低升级版本的候选。安装交回应用自身，Starter 不替代签名验证。
- App Store：通过 bundle ID 查询 Apple 公开 lookup 接口，只接受 bundle ID 一致的 mac-software 结果。公开信息有地区与缓存限制；查询失败显示检查失败，安装在商店完成。[Apple 接口](https://performance-partners.apple.com/search-api)
- 无公开 feed 的应用：流式读取 [Homebrew Cask API](https://formulae.brew.sh/docs/api/)，要求 `.app` 文件名及 bundle ID 同时精确匹配，拒绝歧义，过滤不满足 macOS 要求的条目。只提供版本信息，不把原安装接管为 Homebrew。未匹配时显示「更新源待确认」，可打开原应用。
- Homebrew 自更新 cask：具体版本也参与比较，读取实际 bundle 版本，不因 auto_updates 标志跳过，也不根据过时 receipt 提供降级。`version latest` 没有可比较的版本时使用应用 feed 或目录补充检测。

所有远端检测均不下载安装包。XML/JSON 和网络响应均有大小、节点数与超时限制；目录只保留本机的匹配候选，不常驻完整远端清单。
