import AppKit
import ApplicationServices
import Carbon

// Run with Starter using Alt+Space and the invoking terminal allowed Accessibility:
// xcrun swift scripts/test-macos-fullscreen.swift [--ime]
// A temporary fullscreen host drives the real global hotkey and restores the
// previous application/input source. --ime also checks Chinese composition.

let app = NSApplication.shared
let previous = NSWorkspace.shared.frontmostApplication
let starters = NSWorkspace.shared.runningApplications.filter { $0.bundleIdentifier == "dev.starter.launcher" }
guard starters.count == 1, let starter = starters.first, AXIsProcessTrusted() else {
    print("ERROR: run exactly one Starter and enable Accessibility for the invoking terminal")
    exit(2)
}
let originalSource = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
let checkIME = CommandLine.arguments.contains("--ime")
let host = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 800, height: 600), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
host.title = "Starter fullscreen regression host"
host.collectionBehavior = [.fullScreenPrimary]
host.isReleasedWhenClosed = false
let label = NSTextField(labelWithString: "Starter fullscreen regression test")
label.frame = NSRect(x: 40, y: 250, width: 600, height: 40)
host.contentView?.addSubview(label)
app.setActivationPolicy(.regular)

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}
func visibleStarter() -> Bool {
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return windows.contains { ($0[kCGWindowOwnerPID as String] as? Int32) == starter.processIdentifier && ($0[kCGWindowLayer as String] as? Int) == 3 && ($0[kCGWindowAlpha as String] as? Double ?? 0) > 0 }
}
func keyStarter() -> Bool {
    let ax = AXUIElementCreateApplication(starter.processIdentifier)
    return attribute(ax, kAXFocusedWindowAttribute) != nil
}
func press(_ code: CGKeyCode, flags: CGEventFlags = []) {
    let source = CGEventSource(stateID: .hidSystemState)
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down)!
        event.flags = flags
        event.post(tap: .cghidEventTap)
    }
}
func finish(_ code: Int32) {
    TISSelectInputSource(originalSource)
    host.orderOut(nil)
    previous?.activate(options: [])
    exit(code)
}
func searchValue(_ element: AXUIElement, depth: Int = 0) -> String? {
    guard depth < 10 else { return nil }
    if attribute(element, kAXRoleAttribute) as? String == kAXTextFieldRole {
        return attribute(element, kAXValueAttribute) as? String
    }
    let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
    for child in children {
        if let value = searchValue(child, depth: depth + 1) { return value }
    }
    return nil
}
func checkCommit(_ completion: @escaping () -> Void, attempts: Int = 10) {
    let value = searchValue(AXUIElementCreateApplication(starter.processIdentifier))
    if value == "你" {
        print("PASS: fullscreen Chinese candidates and commit")
        TISSelectInputSource(originalSource)
        completion()
    } else if value == nil && attempts > 0 {
        // GPUI initializes its accessibility tree lazily when first queried.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
            checkCommit(completion, attempts: attempts - 1)
        }
    } else {
        print("FAIL: Chinese commit did not reach the search field (value=\(value ?? "missing"))")
        finish(1)
    }
}
func checkInput(_ completion: @escaping () -> Void) {
    // Request the tree before composition so a fresh Starter also exposes input.
    _ = searchValue(AXUIElementCreateApplication(starter.processIdentifier))
    let sources = TISCreateInputSourceList(nil, false).takeRetainedValue() as! [TISInputSource]
    let chinese = sources.first { source in
        guard let value = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return false }
        let id = Unmanaged<CFString>.fromOpaque(value).takeUnretainedValue() as String
        return id == "com.apple.inputmethod.SCIM.ITABC" || id == "com.apple.inputmethod.SCIM.Shuangpin"
    }
    guard let chinese, TISSelectInputSource(chinese) == noErr else {
        print("ERROR: --ime requires an enabled Apple Pinyin or Shuangpin input source")
        finish(2); return
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
        press(45); press(34) // n, i: identical in Pinyin and Shuangpin.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
            let candidates = windows.contains {
                ($0[kCGWindowOwnerPID as String] as? Int32) == starter.processIdentifier &&
                ($0[kCGWindowLayer as String] as? Int) == 20 &&
                ($0[kCGWindowAlpha as String] as? Double ?? 0) > 0
            }
            guard candidates && visibleStarter() && host.isOnActiveSpace else {
                print("FAIL: Chinese candidates are not visible above the fullscreen launcher")
                finish(1); return
            }
            press(49) // Commit the first candidate.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                checkCommit(completion)
            }
        }
    }
}
func checkHide(_ round: Int) {
    press(49, flags: .maskAlternate)
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
        guard !visibleStarter() && host.isOnActiveSpace && host.isKeyWindow &&
            NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier else {
            print("FAIL: second Option+Space did not hide Starter and restore focus to the fullscreen host")
            finish(1); return
        }
        if round < 3 { checkShow(round + 1) } else { print("PASS: fullscreen show/focus/hide x3"); finish(0) }
    }
}
func checkShow(_ round: Int) {
    press(49, flags: .maskAlternate)
    DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
        let visible = visibleStarter()
        let sameSpace = host.isOnActiveSpace
        let key = keyStarter()
        print("round=\(round) starterVisible=\(visible) hostOnActiveSpace=\(sameSpace) starterFocusedWindow=\(key) starterHidden=\(starter.isHidden) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "unknown")")
        fflush(stdout)
        guard visible && sameSpace && key else { print("FAIL: Option+Space did not show and focus Starter over the current fullscreen Space"); finish(1); return }
        if round == 1 && checkIME { checkInput { checkHide(round) } }
        else { checkHide(round) }
    }
}

print("accessibility=\(AXIsProcessTrusted()) pid=\(starter.processIdentifier)")
host.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { host.toggleFullScreen(nil) }
DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
    guard host.styleMask.contains(.fullScreen) && host.isOnActiveSpace else { print("FAIL: host not in fullscreen"); finish(2); return }
    print("fullscreen host ready; before hotkey starterVisible=\(visibleStarter())")
    fflush(stdout)
    checkShow(1)
}
app.run()
