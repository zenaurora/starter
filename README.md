# Starter

A native desktop launcher built with Rust and GPU rendering. Fast, lightweight, and fully local — no web views, no cloud services, no telemetry.

**macOS** `Option+Space` · **Windows** `Ctrl+Space`

## What it does

Launch apps, search files, grab clipboard history, and manage quick reminders — all from one floating window that stays out of your way.

- **Fuzzy app search** with typo tolerance and Chinese name support
- **File and content search** in your configured directories
- **Clipboard manager** for text, images, URLs, and files
- **Quick reminders** with natural time parsing
- **System commands** — lock, sleep, restart, shutdown, settings shortcuts
- **App uninstall** with confirmation and system integration
- **Auto-updates** with download progress and rollback
- Runs from the menu bar; stays fast with native rendering

## Get started

Clone the repo and run it. Requires Rust 1.90+ (2024 edition).

**macOS** (uses `.app` bundle for proper tray and icon support):
```sh
bash scripts/bundle-macos.sh --debug
open target/debug/Starter.app
```

**Windows**:
```sh
cargo run --locked
```

First launch: the window appears centered. Hit `Option+Space` / `Ctrl+Space` to bring it back anytime.

## What you can do

Type to search apps, or use these shortcuts:

| What you type | What happens |
|---------------|--------------|
| `safari` | Finds Safari (fuzzy match, typo-tolerant) |
| `微信` | Finds WeChat by Chinese name |
| `/f ui` | Searches file/folder names in your configured directories |
| `/c launcher_hotkey` | Searches file contents (case-insensitive) |
| `/uninstall 微信` | Uninstalls WeChat after confirmation |
| `/clip needle` | Searches clipboard history by keyword |
| `/clip 图片 2026-10` | Filters clipboard images by type and date |
| `Lock` / `Sleep` | Locks screen or sleeps the computer immediately |
| `Restart` / `Shutdown` | Shows confirmation before system actions |
| `bluetooth` / `display` | Opens system settings directly |
| `/remind 10m meeting` | Creates reminder in 10 minutes |
| `/remind tomorrow 15:00 call` | Creates reminder tomorrow at 3 PM |

Click mode buttons to browse by category. `↑↓` to select, `Enter` to open, `⌘/Ctrl+Enter` to reveal in Finder/Explorer, `Shift+Enter` to copy path. Star apps to favorite them — they float to the top.

## Settings

Open settings with the gear icon, `⌘,` / `Ctrl+,`, or via the tray menu.

- **Hotkeys** — customize the launcher shortcut and create your own app shortcuts with a visual key picker; one shortcut can open multiple apps together (for example, `Cmd+Enter` opens kitty and ChatGPT)
- **Startup** — optionally launch at login and stay in the tray until you summon Starter; disabled by default and applied when you save settings
- **File associations** — set which app opens which file extension from search results
- **Appearance** — pick a theme (Catppuccin Mocha, Everforest, Gruvbox, Latte) and monospace font
- **Search directories** — add folders to index for file search
- **App aliases** — add Chinese or custom names for any app
- **Auto-update** — checks GitHub releases every 24h; downloads, verifies, installs, and restarts automatically
- **Clipboard** — toggle history recording; clear all or delete individual entries

Changes apply immediately. No restart needed.

The hotkey page separates Starter's launcher shortcut from your app shortcut groups. Click **修改** to change the launcher shortcut, or **新建** / **编辑** to open an app group editor. Terminals such as kitty are configured as ordinary apps; Starter reserves no terminal shortcut. Use **添加应用** to add targets, then save settings. **取消编辑** discards just the current group edit. Existing single-app shortcuts continue to work. If one target fails to launch, the other apps still open and Starter reports the failed targets.

You can also configure a group in `config.toml`:

```toml
[[shortcuts]]
hotkey = "Super+Enter"
applications = ["kitty", "ChatGPT"]
enabled = true
```

## How it works

All data stays local — config lives in `~/Library/Application Support/starter` (macOS) or `%APPDATA%\starter` (Windows). No cloud sync, no analytics, no external requests except GitHub release checks.

Clipboard history saves the last 30 days (max 200 entries, 100 MiB total). Images are thumbnailed and stored locally. Files save paths, not contents — if you move the original, restore will fail.

File search respects `.gitignore` and skips hidden files, `.git`, `node_modules`, `target`, `.cache`. Indexes up to 100K files in memory. Refresh from the tray menu when you add/remove files.

Reminders run entirely local in `reminders.json`. The app must stay running to send notifications on time. If you quit or sleep, missed reminders show up when you reopen.

System commands use native APIs — macOS uses system preference URLs and runtime-resolved interfaces; Windows uses `ms-settings:` and official uninstallers. No shell command injection, no forced quits.

## Why it exists

Most launchers are either slow web apps, cloud-dependent services, or feature-bloated tools. Starter is none of those. It's a single native binary with GPU rendering, instant startup, and zero network dependencies. Built for daily use without friction.

## Development

```sh
# Format, lint, test
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets

# Test macOS updater with a temp app copy
bash scripts/bundle-macos.sh --debug
python3 scripts/test-macos-updater.py
```

Push a tag starting with `v` to trigger GitHub Actions — builds macOS (Apple Silicon + Intel) and Windows installers automatically.

Icon generation: `swift scripts/generate-icons.swift` on macOS.

Tested on Apple Silicon macOS with native settings, uninstall flow, DMG updates, clipboard isolation, and reminder notifications. Windows builds pass CI lint/test but full MSI updates and clipboard require native validation.

## Roadmap

- Browser bookmarks integration
- Calculator and history
- Window snapping and half-screen management

---

**Local-first. Zero bloat. Built for humans.**
