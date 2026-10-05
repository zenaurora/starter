# Architecture Refactoring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the codebase to improve modularity, fix performance issues, add error dialog for config failures, and improve maintainability.

**Architecture:** Split the monolithic `ui.rs` into focused modules, improve error handling with visual feedback, add bounded channels to prevent memory growth, implement proper icon cache limits, and reduce debounce delay for better responsiveness.

**Tech Stack:** Rust, GPUI Kit, async-channel

## Global Constraints

- Maintain compatibility with existing config files and data formats
- No breaking changes to user-facing behavior except improvements
- All error messages remain in Chinese
- Preserve existing keyboard shortcuts and UI interactions
- Maintain macOS and Windows cross-platform support patterns

---

### Task 1: Add Error Dialog Component

**Files:**
- Create: `src/ui/error_dialog.rs`
- Modify: `src/ui.rs:1-60` (add imports and error dialog field)
- Modify: `src/main.rs:1-10` (import new module)

**Interfaces:**
- Consumes: `gpui_kit` component APIs
- Produces: `ErrorDialog` struct with methods:
  - `new(message: String, window: &mut Window, cx: &mut Context<Self>) -> Self`
  - `render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement`

- [ ] **Step 1: Create error dialog module skeleton**

```rust
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

pub enum Event {
    Close,
}

pub struct ErrorDialog {
    message: String,
}

impl EventEmitter<Event> for ErrorDialog {}

impl ErrorDialog {
    pub fn new(message: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { message }
    }
}

impl Render for ErrorDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child("placeholder")
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS with no errors

- [ ] **Step 3: Implement error dialog UI with dark overlay and centered modal**

```rust
impl Render for ErrorDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .child(
                div()
                    .w(px(480.))
                    .max_w_full()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.destructive)
                            .child("配置加载失败"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.foreground)
                            .line_height(relative(1.5))
                            .child(self.message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(
                                Button::new("close")
                                    .primary()
                                    .child("确定")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(Event::Close);
                                    })),
                            ),
                    ),
            )
    }
}
```

- [ ] **Step 4: Add error dialog to main ui module**

In `src/ui.rs`, add after line 6:

```rust
mod error_dialog;
use error_dialog::ErrorDialog;
```

- [ ] **Step 5: Add error dialog field to Launcher struct**

In `src/ui.rs`, add to `Launcher` struct after `settings_subscription` field (around line 33):

```rust
error_dialog: Option<Entity<ErrorDialog>>,
error_dialog_subscription: Option<Subscription>,
```

- [ ] **Step 6: Initialize error dialog fields in Launcher::new**

In `src/ui.rs`, add to `Launcher::new` initialization (around line 181):

```rust
error_dialog: None,
error_dialog_subscription: None,
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS

- [ ] **Step 8: Commit error dialog component**

```bash
git add src/ui/error_dialog.rs src/ui.rs src/main.rs
git commit -m "feat: add error dialog component for startup failures"
```

---

### Task 2: Show Error Dialog on Config Load Failure

**Files:**
- Modify: `src/ui.rs:57-75` (Launcher::new error handling)
- Modify: `src/ui.rs:831-900` (Render implementation)

**Interfaces:**
- Consumes: `ErrorDialog::new(message: String, window: &mut Window, cx: &mut Context<Self>) -> ErrorDialog`
- Produces: Updated `Launcher::new` that shows error dialog instead of storing error string

- [ ] **Step 1: Replace error string accumulation with immediate dialog**

In `src/ui.rs` `Launcher::new` method, replace lines 59-75 with:

```rust
let mut error = None;
let (config, config_path) = match config::load_or_create() {
    Ok(value) => value,
    Err(problem) => {
        error = Some(format!("配置读取失败：{problem:#}\n\n程序将使用默认设置运行。"));
        (
            Config::default(),
            config::config_path().unwrap_or_else(|_| PathBuf::from("config.toml")),
        )
    }
};
appearance::apply(config.theme, &config, window, cx);
let history_path = config_path.with_file_name("usage.json");
let history = History::load(history_path.clone()).unwrap_or_else(|problem| {
    let msg = format!("使用记录读取失败：{problem:#}\n\n程序将使用空白历史记录。");
    if let Some(existing) = &mut error {
        existing.push_str("\n\n");
        existing.push_str(&msg);
    } else {
        error = Some(msg);
    }
    History::empty(history_path)
});
```

- [ ] **Step 2: Create error dialog entity after setup completes**

In `src/ui.rs` after Shell setup (around line 168), add:

```rust
let mut error_dialog = None;
let mut error_dialog_subscription = None;
if let Some(message) = error {
    let dialog = cx.new(|cx| ErrorDialog::new(message, window, cx));
    error_dialog_subscription = Some(cx.subscribe_in(
        &dialog,
        window,
        |this, _, event, _window, cx| match event {
            error_dialog::Event::Close => {
                this.error_dialog = None;
                this.error_dialog_subscription = None;
                cx.notify();
            }
        },
    ));
    error_dialog = Some(dialog);
}
```

- [ ] **Step 3: Update Launcher struct initialization**

In `src/ui.rs` `Launcher::new`, update the struct initialization (around line 181) to use the new variables:

```rust
error_dialog,
error_dialog_subscription,
```

- [ ] **Step 4: Update Render implementation to show error dialog**

In `src/ui.rs` `Render for Launcher`, update the `render` method (around line 832):

```rust
fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if let Some(dialog) = &self.error_dialog {
        return div()
            .size_full()
            .capture_key_down(cx.listener(Self::key_down))
            .child(dialog.clone())
            .into_any_element();
    }
    if let Some(settings) = &self.settings {
        return div()
            .size_full()
            .capture_key_down(cx.listener(Self::key_down))
            .child(settings.clone())
            .into_any_element();
    }
    // ... rest of render code unchanged
```

- [ ] **Step 5: Remove unused error field**

In `src/ui.rs`, remove the `error: Option<String>` field from the `Launcher` struct (around line 50).

- [ ] **Step 6: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS

- [ ] **Step 7: Test with broken config**

```bash
# Backup config
cp ~/Library/Application\ Support/starter/config.toml ~/config.toml.bak 2>/dev/null || true

# Create invalid config
mkdir -p ~/Library/Application\ Support/starter
echo "invalid toml [[[ content" > ~/Library/Application\ Support/starter/config.toml

# Run app
bash scripts/bundle-macos.sh --debug
open target/debug/Starter.app
```

Expected: Error dialog appears with readable message

- [ ] **Step 8: Restore config and verify normal operation**

```bash
# Restore or remove
mv ~/config.toml.bak ~/Library/Application\ Support/starter/config.toml 2>/dev/null || rm ~/Library/Application\ Support/starter/config.toml
```

- [ ] **Step 9: Commit error dialog integration**

```bash
git add src/ui.rs
git commit -m "feat: show error dialog on config/history load failures"
```

---

### Task 3: Fix Worker Channel Memory Issue

**Files:**
- Modify: `src/worker.rs:47-50`

**Interfaces:**
- Consumes: `async_channel` bounded channel API
- Produces: Updated `start()` function with bounded command channel

- [ ] **Step 1: Replace unbounded command channel with bounded**

In `src/worker.rs`, replace line 48:

```rust
let (sender, commands) = async_channel::bounded(8);
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS

- [ ] **Step 3: Commit channel bound fix**

```bash
git add src/worker.rs
git commit -m "fix: bound worker command channel to prevent memory growth"
```

---

### Task 4: Improve Search Responsiveness

**Files:**
- Modify: `src/worker.rs:79`

**Interfaces:**
- Consumes: Standard library `Duration`
- Produces: Updated debounce timing

- [ ] **Step 1: Reduce debounce delay from 80ms to 50ms**

In `src/worker.rs`, replace line 79:

```rust
thread::sleep(Duration::from_millis(50));
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS

- [ ] **Step 3: Commit debounce improvement**

```bash
git add src/worker.rs
git commit -m "perf: reduce search debounce from 80ms to 50ms for better responsiveness"
```

---

### Task 5: Fix Icon Cache Unbounded Growth

**Files:**
- Modify: `src/icons.rs:54-59`
- Modify: `src/icons.rs:12-15`

**Interfaces:**
- Consumes: Standard library `HashMap` and `VecDeque`
- Produces: LRU-style cache with proper capacity management

- [ ] **Step 1: Add VecDeque import for LRU tracking**

In `src/icons.rs`, update imports at line 4:

```rust
use std::{collections::{HashMap, VecDeque}, path::PathBuf, sync::Arc};
```

- [ ] **Step 2: Add LRU queue field to Icons struct**

In `src/icons.rs`, update the `Icons` struct (line 12):

```rust
pub struct Icons {
    sender: Sender<PathBuf>,
    cache: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    lru: VecDeque<PathBuf>,
}
```

- [ ] **Step 3: Initialize LRU queue in Icons::new**

In `src/icons.rs`, update the struct construction in `Icons::new` (around line 36):

```rust
Self {
    sender,
    cache: HashMap::new(),
    lru: VecDeque::new(),
}
```

- [ ] **Step 4: Update get method to track LRU access**

In `src/icons.rs`, replace the `get` method (lines 44-52):

```rust
pub fn get(&mut self, path: &PathBuf) -> Option<Arc<RenderImage>> {
    if let Some(image) = self.cache.get(path) {
        // Move to back (most recently used)
        if let Some(pos) = self.lru.iter().position(|p| p == path) {
            self.lru.remove(pos);
        }
        self.lru.push_back(path.clone());
        return image.clone();
    }
    if self.sender.try_send(path.clone()).is_ok() {
        self.cache.insert(path.clone(), None);
        self.lru.push_back(path.clone());
    }
    None
}
```

- [ ] **Step 5: Update insert method to evict old entries**

In `src/icons.rs`, replace the `insert` method (lines 54-59):

```rust
pub fn insert(&mut self, loaded: Loaded) {
    const MAX_CACHE_SIZE: usize = 512;
    
    // Evict least recently used if at capacity
    while self.cache.len() >= MAX_CACHE_SIZE && !self.lru.is_empty() {
        if let Some(old_path) = self.lru.pop_front() {
            self.cache.remove(&old_path);
        }
    }
    
    self.cache.insert(loaded.path.clone(), loaded.image);
    
    // Add to LRU if not already present
    if !self.lru.iter().any(|p| p == &loaded.path) {
        self.lru.push_back(loaded.path);
    }
}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS

- [ ] **Step 7: Run clippy to check for issues**

Run: `cargo clippy --locked --all-targets -- -D warnings`
Expected: SUCCESS with no warnings

- [ ] **Step 8: Commit icon cache LRU implementation**

```bash
git add src/icons.rs
git commit -m "fix: implement proper LRU cache for icons to prevent unbounded growth"
```

---

### Task 6: Add Logging for Icon Loading Failures

**Files:**
- Modify: `src/ui.rs:107-119` (icon event handler)

**Interfaces:**
- Consumes: Standard library `eprintln!` macro
- Produces: Logged error messages for debugging

- [ ] **Step 1: Add debug logging when icon channel closes**

In `src/ui.rs`, update the icon event handler (around line 107):

```rust
cx.spawn_in(window, async move |this, cx| {
    while let Ok(icon) = icon_events.recv().await {
        if this
            .update_in(cx, |this, _, cx| {
                this.icons.insert(icon);
                cx.notify();
            })
            .is_err()
        {
            eprintln!("Icon loader: main window closed, stopping icon worker");
            break;
        }
    }
})
```

- [ ] **Step 2: Add similar logging for worker and shell event handlers**

In `src/ui.rs`, update worker event handler (around line 120):

```rust
cx.spawn_in(window, async move |this, cx| {
    while let Ok(event) = worker_events.recv().await {
        if this
            .update_in(cx, |this, _, cx| this.worker_event(event, cx))
            .is_err()
        {
            eprintln!("Search worker: main window closed, stopping worker");
            break;
        }
    }
})
```

- [ ] **Step 3: Update shell event handler**

In `src/ui.rs`, update shell event handler (around line 130):

```rust
cx.spawn_in(window, async move |this, cx| {
    while let Ok(event) = shell_events.recv().await {
        if this
            .update_in(cx, |this, window, cx| this.shell_event(event, window, cx))
            .is_err()
        {
            eprintln!("Shell events: main window closed, stopping shell handler");
            break;
        }
    }
})
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check --locked`
Expected: SUCCESS

- [ ] **Step 5: Commit logging improvements**

```bash
git add src/ui.rs
git commit -m "feat: add debug logging for background task shutdowns"
```

---

### Task 7: Document Architecture Decisions

**Files:**
- Create: `docs/ARCHITECTURE.md`

**Interfaces:**
- Consumes: None
- Produces: Architecture documentation file

- [ ] **Step 1: Create architecture documentation**

```markdown
# Starter Architecture

## Overview

Starter is a keyboard-first desktop launcher built with Rust and GPUI Kit. This document describes the high-level architecture and key design decisions.

## Module Structure

### Core Modules (`src/lib.rs`)

Independent, testable business logic with no UI dependencies:

- `catalog.rs` - Application discovery and indexing
- `config.rs` - Configuration file management
- `history.rs` - Usage tracking and persistence
- `search.rs` - Search algorithms and ranking

### UI Modules (`src/ui/`)

GPUI-dependent presentation layer:

- `ui.rs` - Main launcher window and orchestration (will be split further)
- `error_dialog.rs` - Error dialog component for startup failures
- `settings.rs` - Settings page component

### System Integration (`src/`)

Platform-specific integration:

- `platform.rs` - Tray, hotkeys, window management, file operations
- `icons.rs` - Async icon loading with LRU cache (512 items max)
- `worker.rs` - Background search worker with bounded channels

### Application Entry (`src/main.rs`)

Application bootstrap and asset loading.

## Threading Model

- **Main thread**: UI rendering, event handling
- **Icon worker**: One dedicated thread for icon extraction (bounded queue: 64 requests, 32 results)
- **Search worker**: One dedicated thread for file scanning and searching (bounded queue: 8 commands, 32 events)

### Channel Bounds

All async channels are bounded to prevent memory growth:
- Icon requests: 64 in-flight max
- Icon results: 32 buffered max
- Worker commands: 8 queued max
- Worker events: 32 buffered max

## Error Handling Strategy

### Startup Errors

Configuration and history loading errors show a modal error dialog with:
- Clear error message in Chinese
- Fallback to defaults (user can continue using the app)
- Single "确定" button to dismiss

### Runtime Errors

- Tray/hotkey failures: Show error in status bar, disable affected features
- Search errors: Display in status text at bottom of window
- Icon loading failures: Silently fall back to generic icons

## Performance Optimizations

1. **Search debounce**: 50ms delay before disk operations
2. **Icon cache**: LRU cache with 512-item limit
3. **Cancellation tokens**: `AtomicBool` for instant search cancellation
4. **Batch rendering**: Results delivered in batches during content search

## Configuration

- Config path: `~/Library/Application Support/starter/config.toml` (macOS)
- History path: `~/Library/Application Support/starter/usage.json`
- Hot-reload: Supported via tray menu

## Future Refactoring

The `ui.rs` file (1051 lines) should be split into:
- `ui/launcher.rs` - Core launcher logic
- `ui/result_list.rs` - Result rendering
- `ui/keyboard.rs` - Keyboard event handling
- `ui/state.rs` - Application state management
```

- [ ] **Step 2: Commit architecture documentation**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: add architecture documentation"
```

---

### Task 8: Final Verification and Testing

**Files:**
- None (verification only)

**Interfaces:**
- Consumes: All previous changes
- Produces: Verified working application

- [ ] **Step 1: Run full build**

Run: `cargo build --locked --release`
Expected: SUCCESS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --locked --all-targets -- -D warnings`
Expected: SUCCESS with no warnings

- [ ] **Step 3: Run formatter check**

Run: `cargo fmt --check`
Expected: All files formatted correctly

- [ ] **Step 4: Build macOS app bundle**

Run: `bash scripts/bundle-macos.sh --debug`
Expected: SUCCESS, bundle created

- [ ] **Step 5: Test normal operation**

```bash
open target/debug/Starter.app
```

Expected: 
- App launches without errors
- Can search for applications
- Settings page works
- Hotkeys work

- [ ] **Step 6: Test error dialog with broken config**

```bash
# Backup and break config
cp ~/Library/Application\ Support/starter/config.toml ~/config.toml.bak 2>/dev/null || true
echo "broken: [[[ toml" > ~/Library/Application\ Support/starter/config.toml

# Launch app
open target/debug/Starter.app
```

Expected: Error dialog appears with clear message, app continues to work with defaults

- [ ] **Step 7: Restore config**

```bash
mv ~/config.toml.bak ~/Library/Application\ Support/starter/config.toml 2>/dev/null || rm ~/Library/Application\ Support/starter/config.toml
```

- [ ] **Step 8: Test rapid typing for debounce**

Launch app, type rapidly in search box.

Expected: Smooth, responsive, no lag or queue buildup

- [ ] **Step 9: Final commit**

```bash
git status
# Verify all changes committed
```

Expected: Working tree clean
