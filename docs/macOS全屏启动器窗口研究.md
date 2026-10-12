# macOS 全屏启动器窗口研究

> 研究目标：解释为什么 Starter 在浏览器全屏 Space 中按下快捷键后可能切换 Space、闪烁或需要再次按键，并整理 Raycast 类启动器可以借鉴的公开实现。Raycast 核心代码闭源，本文不把未公开的内部实现当作事实。

## 结论先行

macOS 中“当前前台应用”和“当前 Space 上可以显示的窗口”是两套状态。一个普通的 GPUI `Floating` 窗口通常属于 Starter 自己的应用集合；浏览器全屏时，Starter 的窗口可能不在当前全屏 Space，于是系统会切换 Space，或者 GPUI 的激活回调在切换过程中把窗口隐藏。

当前代码已经设置了 `CanJoinAllSpaces | FullScreenAuxiliary`，这能覆盖一部分 Spaces/fullscreen 场景，但 Apple 在 macOS 13 引入了更直接的 `CanJoinAllApplications`：它用于浮动窗口和系统叠层，允许窗口在符合条件时加入其他应用的全屏 Space。要模拟 Raycast 的“在当前全屏应用上出现”，应在 macOS 13+ 评估 `CanJoinAllApplications`，同时重新审查激活回调，不要把“应用没有 active”直接当成“弹窗应该消失”。

中文输入不要求 Starter 把整个应用激活为普通应用。正确方向是保留 `NSPanel` 的 non-activating 行为，但让面板可以成为 key window，并让输入控件继续实现 AppKit 的 `NSTextInputClient`/GPUI 输入链路。Maccy、Sol 和 Leader Key 都公开采用了这个形态；这些源码证明的是窗口和输入控件的设计方向，不能替代在本项目上的中文输入法实测。

## Apple 官方事实

以下内容来自当前 macOS SDK 26.5 的 AppKit 头文件和 Apple Developer Documentation；本地头文件路径是 `/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk/System/Library/Frameworks/AppKit.framework/Headers/NSWindow.h`。

### 1. Collection Behavior 控制 Spaces、全屏和 Stage Manager

Apple 将 `collectionBehavior` 定义为窗口在 Mission Control、Spaces 和 Stage Manager 中的显示偏好：[NSWindow.CollectionBehavior](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct)。这些选项不是一个“切换到窗口所属桌面”的 API，而是告诉 WindowServer 该窗口可以加入哪些窗口集合。

- [`canJoinAllSpaces`](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct/canjoinallspaces)：窗口可以出现在所有 Spaces。
- [`fullScreenAuxiliary`](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct/fullscreenauxiliary)：窗口可以和全屏窗口显示在同一个 Space。
- [`moveToActiveSpace`](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct/movetoactivespace)：窗口变为 active 时，移动到 active Space；Apple 的定义是“becomes active 时移动”，不是“永远加入所有 Space”。
- [`canJoinAllApplications`](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct/canjoinallapplications)：macOS 13+；用于能加入其他应用集合、包括符合条件的全屏 Space 的浮动窗口和系统叠层。Apple 特别说明这种窗口不参与 Stage Manager 的布局，但可以加入其他应用的全屏 Space。

Apple 文档同时说明 `primary`、`auxiliary`、`canJoinAllApplications` 三者一次只能指定一个。不要把 `canJoinAllApplications` 当成 `auxiliary` 的别名，也不要同时设置这三个互斥选项。`canJoinAllSpaces` 和 `moveToActiveSpace` 代表不同的 Space 策略；工程中应选择一种明确策略并在真实全屏场景验证，而不是盲目叠加。

SDK 头文件中的原文还说明：`fullScreenAuxiliary` 的含义是“与全屏窗口显示在同一 Space”，`canJoinAllApplications` 的含义是“允许加入其他应用的集合及全屏 Space（符合条件时）”。这就是 Starter 当前组合值得重新检查的原因。

### 2. Nonactivating Panel 仍然可以接收键盘输入

[`NSWindow.StyleMask.nonactivatingPanel`](https://developer.apple.com/documentation/appkit/nswindow/stylemask-swift.struct/nonactivatingpanel) 的官方定义是：这是一个不会激活所属应用的 `NSPanel` 或其子类。SDK 头文件也明确写着该 style mask 只适用于 `NSPanel`（或子类）。

这解决了“显示面板时不要把用户正在用的应用彻底切走”的一半问题，但它不代表面板不能成为 key window。AppKit 的 [`canBecomeKey`](https://developer.apple.com/documentation/appkit/nswindow/canbecomekey) 文档说明，若窗口不含 titled mask，子类可以覆写该属性返回 `true`；公开的启动器实现正是这么做的。

`NSPanel.becomesKeyOnlyIfNeeded` 的文档进一步说明：对于 non-activating panel，只有命中视图返回 `needsPanelToBecomeKey == true` 时才成为 key。这是一个可选的精细策略；如果启动器主要工作就是搜索输入，直接确保搜索输入控件获得 key/focus 更简单。

### 3. 激活是异步状态，不应立即假定结果

[`NSApplication.activate(ignoringOtherApps:)`](https://developer.apple.com/documentation/appkit/nsapplication/activate(ignoringotherapps:)) 的官方文档明确说，发送激活消息后可能存在时间延迟，不能假定应用会立即 active。Apple 也说明，通常不需要手动调用它，AppKit 会处理正常激活流程。

因此，`show()` 后立刻读取 `is_active`、`is_key_window` 并据此隐藏或聚焦，容易遇到竞态。应把显示、成为 key、输入焦点和失去焦点当作状态变化处理，必要时在主线程下一轮事件循环重新确认。

### 4. 中文输入必须走 NSTextInputClient 链路

Apple 的 [`NSTextInputClient`](https://developer.apple.com/documentation/appkit/nstextinputclient) 文档说明，自定义文本视图要实现该协议，才能和 macOS 文本输入管理系统正确交互。其 [`setMarkedText`](https://developer.apple.com/documentation/appkit/nstextinputclient/setmarkedtext(_:selectedrange:replacementrange:)) 文档解释了拼音组合文本、选区和最终提交文本的关系。

[`interpretKeyEvents`](https://developer.apple.com/documentation/appkit/nsresponder/interpretkeyevents(_:)) 会把键盘事件交给系统输入管理器，输入管理器再回调 `insertText` 或 `doCommand`。因此，模拟 Raycast 的窗口层级时，不能为了“不激活应用”而绕开 GPUI 的文本输入处理；应保持当前 GPUI 输入控件/`EntityInputHandler` 的 IME 路径，只改变原生窗口的 Space 和焦点策略。

## 开源启动器的实际做法

### Maccy（公开源码，commit `a92c11ae3e7a86a57dc6359bad59e330e4625ede`）

源码：[FloatingPanel.swift](https://github.com/p0deje/Maccy/blob/a92c11ae3e7a86a57dc6359bad59e330e4625ede/Maccy/FloatingPanel.swift)。它的 `FloatingPanel` 是 `NSPanel` 子类：

```swift
styleMask = [.nonactivatingPanel, .resizable, .closable, .fullSizeContentView]
isFloatingPanel = true
level = .statusBar
collectionBehavior = [.auxiliary, .stationary, .moveToActiveSpace, .fullScreenAuxiliary]
hidesOnDeactivate = false
override var canBecomeKey: Bool { true }
```

它在 `open()` 中调用 `orderFrontRegardless()` 和 `makeKey()`，没有在显示弹窗时调用 `NSApp.activate`。失去 key 时由 `resignKey()` 关闭面板。搜索框使用 SwiftUI `TextField`：[SearchFieldView.swift](https://github.com/p0deje/Maccy/blob/a92c11ae3e7a86a57dc6359bad59e330e4625ede/Maccy/Views/SearchFieldView.swift)。

这里能借鉴三点：面板不是普通文档窗口；显示路径不依赖激活整个应用；搜索框仍是原生文本控件，因此把 IME 交给系统。Maccy 使用 `.statusBar` level 是它自身的产品选择，并不意味着 Starter 应直接照搬高层级。

### Sol（公开源码，commit `bf78ccadca30b0130c2706ff39a7215a9facd326`）

源码：[Panel.swift](https://github.com/ospfranco/sol/blob/bf78ccadca30b0130c2706ff39a7215a9facd326/macos/sol-macOS/views/Panel.swift)。它使用：

```swift
styleMask: [.titled, .fullSizeContentView, .nonactivatingPanel]
level = .floating
collectionBehavior.insert(.fullScreenAuxiliary)
collectionBehavior.insert(.canJoinAllSpaces)
override var canBecomeKey: Bool { true }
override var canBecomeMain: Bool { true }
```

失去 key 后通过 `windowDidResignKey` 隐藏。它的 [PanelManager](https://github.com/ospfranco/sol/blob/bf78ccadca30b0130c2706ff39a7215a9facd326/macos/sol-macOS/managers/PanelManager.swift) 会根据 frontmost window 选择屏幕，并调用 `makeKeyAndOrderFront`；当面板自己已经是 key 时，会复用面板所在屏幕，避免把面板误判成“当前前台应用”。Sol 的输入组件使用 `react-native-macos` 的 `TextInput`：[MainInput.tsx](https://github.com/ospfranco/sol/blob/bf78ccadca30b0130c2706ff39a7215a9facd326/src/components/MainInput.tsx)。

### Leader Key（公开源码，commit `16bcb307dcc5309fbc3a00fe398d913e1f7ddc51`）

源码：[MainWindow.swift](https://github.com/mikker/LeaderKey.app/blob/16bcb307dcc5309fbc3a00fe398d913e1f7ddc51/Leader%20Key/MainWindow.swift)。它使用 `NSPanel`、`.nonactivatingPanel`、`canBecomeKey = true` 和 `canBecomeMain = true`，并在 `windowDidResignKey` 关闭。这个更小的实现说明“non-activating + 可成为 key + 失去 key 隐藏”是一种常见面板模型。

这些项目都没有公开 Raycast 的内部实现；本文只引用它们公开的窗口/输入代码。Raycast 的具体层级、私有 API 或焦点竞态处理不能从其产品行为反推出。

## 对 Starter 当前代码的诊断

Starter 当前在 [`configure_launcher_window`](/Users/mkh/code1/starter/src/platform.rs:185) 中通过原生消息给 GPUI 窗口加入 `CanJoinAllSpaces` 和 `FullScreenAuxiliary`。GPUI 的 macOS 实现中，`WindowKind::Floating` 使用 `NSPanel` 和 floating level，但不会自动加入 `NSWindowStyleMaskNonactivatingPanel`；代码创建窗口处也明确注释“使用可激活的浮动窗口以兼容输入法”。这解释了为什么中文输入目前有基础，也说明当前窗口在激活/Space 切换上仍可能走普通应用的路径。

更可疑的是 [`observe_activation`](/Users/mkh/code1/starter/src/ui.rs:369)：普通启动器在 `!window.is_window_active()` 且仍可见时调用 `hide()`。在全屏 Space 切换、激活延迟、快捷键回调和 GPUI 窗口 active 状态不同步的瞬间，这个分支可能先隐藏窗口；用户手动滑回去看到一次闪烁，再按第二次才能打开，符合“显示动作触发了，但随后被失活观察者撤销”的竞态特征。

卸载任务分支已经特殊处理了 macOS activation：它在 active 时聚焦任务面板，并直接 `return`，没有走普通启动器的失活隐藏分支。普通搜索启动器仍需单独设计失活策略，不能只复制任务面板的逻辑。

## 建议的实现实验顺序

这是实现建议，不是 Apple 对 Raycast 内部行为的证明：

1. 先保留 `WindowKind::Floating`、现有 GPUI 输入控件和 `FullScreenAuxiliary`，只记录 show/hide、`window.is_window_active()`、key/focus 的时间顺序，复现浏览器全屏问题。
2. 在 macOS 13+ 为启动器加入 `CanJoinAllApplications`，并移除互斥的 `Auxiliary/Primary`（若 GPUI 默认设置了其中之一）。保留 `FullScreenAuxiliary` 是否必要要通过真实全屏测试决定。
3. 不要在每一次 `!is_window_active()` 时立即隐藏。打开流程中设置“正在显示/等待激活”的短暂状态；只有确认用户点击了外部窗口、按下 Escape 或明确取消时才隐藏。若产品要求点击外部自动收起，应基于 key/resign 事件和任务状态判断，而不是只读应用 active。
4. 打开时先把窗口放到 frontmost screen/current Space，再 `orderFrontRegardless`/`makeKey`，下一轮主线程事件中恢复输入控件 focus。不要把 `NSApp.activate(true)` 当成跨 Space 的唯一策略；如果输入法依赖可激活的面板，可以保留激活调用，同时由 collection behavior 负责 Space 归属。
5. 每次改窗口策略都回归中文：拼音组合、候选框、提交中文、快捷键、全屏浏览器、普通窗口、多个显示器和 Stage Manager。输入法候选框是可见性/焦点策略的真实验收标准。

## 本次实现

Starter 保留 `WindowKind::Floating`，因此没有改变此前已经验证过的中文拼音候选框路径。窗口创建后增加 `CanJoinAllApplications`（macOS 13 及以上）、`CanJoinAllSpaces` 和 `FullScreenAuxiliary`，并清理互斥的 `Primary/Auxiliary` 位。`hidesOnDeactivate` 关闭，显示/隐藏由 Starter 自己管理。

UI 激活观察器不再在第一次失活回调中立即隐藏窗口，而是等待 150 毫秒并再次检查 key/active 状态；重新激活或再次显示会使旧的关闭任务失效。这样全屏 Space 切换的瞬时失活不会造成闪烁，真正点击外部应用后仍会自动收起。一个最小的全屏 host probe 已验证窗口保持在当前全屏 Space，并且第一次显示即可成为 key window。

## 局限

- 本研究没有运行 Raycast，也没有读取其私有实现；Raycast 只作为用户体验目标。
- 开源项目的代码能证明它们的 API 选择，不能证明所有 macOS 版本、输入法和 Stage Manager 配置都相同。
- `CanJoinAllApplications`、`FullScreenAuxiliary`、`CanJoinAllSpaces` 的最终表现仍受 macOS WindowServer、显示器布局、全屏应用和窗口 level 影响，必须在目标系统实测。
- Raycast 的私有实现仍不可见；这里模拟的是它可观察到的 Space/overlay 行为。
