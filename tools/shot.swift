// Capture the society window by window id, with no focus or z-order change.
//
// macOS ScreenCaptureKit captures one window's contents directly, so the
// result is identical whether the window is frontmost or completely covered.
// This is the observer's screenshot path: the app never has to be clicked,
// raised, or activated.
//
// Usage: shot-bin [output.png] [delay-seconds] [frames] [interval-ms]
//
// With [frames] > 1 the output path is a prefix and the helper writes
// `<stem>-0000.png`, `<stem>-0001.png`, ... — the frames a demo GIF is built
// from, captured from one process so the interval does not include a fresh
// ScreenCaptureKit handshake per frame.

import AppKit
import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(2)
}

// ScreenCaptureKit requires a GUI-initialized process even with no window.
// .prohibited keeps this helper out of the Dock and out of the app switcher.
_ = NSApplication.shared
NSApp.setActivationPolicy(.prohibited)

let args = CommandLine.arguments
let outPath = args.count > 1 ? args[1] : "/tmp/society.png"
let delay = args.count > 2 ? (Double(args[2]) ?? 0) : 0
let frames = args.count > 3 ? (Int(args[3]) ?? 1) : 1
let intervalMs = args.count > 4 ? (Int(args[4]) ?? 100) : 100
if delay > 0 { Thread.sleep(forTimeInterval: delay) }

func framePath(_ index: Int) -> String {
    // A single capture (no frame count given) is the documented screenshot
    // path: it writes exactly the path the caller named.
    guard args.count > 3 else { return outPath }
    let url = URL(fileURLWithPath: outPath)
    let stem = url.deletingPathExtension().lastPathComponent
    let ext = url.pathExtension.isEmpty ? "png" : url.pathExtension
    return url.deletingLastPathComponent()
        .appendingPathComponent(String(format: "%@-%04d.%@", stem, index, ext)).path
}

let sem = DispatchSemaphore(value: 0)
var rc: Int32 = 0

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: false)

        // The app window is the large titled one. Smaller same-process windows
        // are menu-bar and offscreen surfaces, so filter by title and area.
        let candidates = content.windows.filter {
            let app = ($0.owningApplication?.applicationName ?? "").lowercased()
            let titled = ($0.title ?? "").contains("society")
            return app.contains("society") && titled
                && $0.frame.width > 400 && $0.frame.height > 300
        }
        guard let win = candidates.max(by: {
            ($0.frame.width * $0.frame.height) < ($1.frame.width * $1.frame.height)
        }) else { fail("society window not found; is the app running?") }

        // A desktop-independent filter isolates this window: no backdrop, no
        // other app's pixels, and no need for the window to be visible.
        let filter = SCContentFilter(desktopIndependentWindow: win)
        let cfg = SCStreamConfiguration()
        cfg.width = Int(win.frame.width * 2)
        cfg.height = Int(win.frame.height * 2)
        cfg.showsCursor = false
        cfg.captureResolution = .best
        cfg.ignoreShadowsSingleWindow = true

        for index in 0..<max(frames, 1) {
            if index > 0 {
                try await Task.sleep(nanoseconds: UInt64(max(intervalMs, 1)) * 1_000_000)
            }
            let img = try await SCScreenshotManager.captureImage(
                contentFilter: filter, configuration: cfg)
            let path = framePath(index)
            let url = URL(fileURLWithPath: path)
            guard let dest = CGImageDestinationCreateWithURL(
                url as CFURL, UTType.png.identifier as CFString, 1, nil)
            else { fail("could not create PNG encoder") }
            CGImageDestinationAddImage(dest, img, nil)
            guard CGImageDestinationFinalize(dest) else { fail("could not write \(path)") }
            print("\(path) \(img.width)x\(img.height)")
        }
    } catch {
        FileHandle.standardError.write("capture failed: \(error)\n".data(using: .utf8)!)
        rc = 1
    }
    sem.signal()
}
sem.wait()
exit(rc)
