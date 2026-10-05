// Run from the repository root: swift scripts/generate-icons.swift
import AppKit
import Foundation
import ImageIO
import UniformTypeIdentifiers

let output = URL(fileURLWithPath: "resources/icons", isDirectory: true)
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
let iconset = FileManager.default.temporaryDirectory
    .appendingPathComponent(UUID().uuidString + ".iconset", isDirectory: true)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: iconset) }

// One continuous folded S, shared by the application and menu bar artwork.
let points: [(CGFloat, CGFloat)] = [(18, 4), (9, 4), (4, 9), (9, 12),
                                    (15, 12), (20, 15), (15, 20), (6, 20)]
let pathData = "M " + points.map { "\($0.0) \($0.1)" }.joined(separator: " L ")
func color(_ red: CGFloat, _ green: CGFloat, _ blue: CGFloat, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: red / 255, green: green / 255, blue: blue / 255, alpha: alpha)
}
func gradient(_ colors: [CGColor]) -> CGGradient {
    CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
               colors: colors as CFArray, locations: nil)!
}
func mark(in context: CGContext, origin: CGFloat, scale: CGFloat, width: CGFloat) {
    context.saveGState()
    context.translateBy(x: origin, y: origin)
    context.scaleBy(x: scale, y: scale)
    context.beginPath()
    context.move(to: CGPoint(x: points[0].0, y: points[0].1))
    for point in points.dropFirst() {
        context.addLine(to: CGPoint(x: point.0, y: point.1))
    }
    context.setLineWidth(width)
    context.setLineCap(.round)
    context.setLineJoin(.round)
    context.replacePathWithStrokedPath()
    context.clip()
    context.drawLinearGradient(gradient([color(157, 220, 187), color(243, 232, 208)]),
                               start: CGPoint(x: 12, y: 4), end: CGPoint(x: 12, y: 20),
                               options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    context.restoreGState()
}
func render(size: Int, template: Bool = false) throws -> Data {
    let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
                            bytesPerRow: size * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    // Design coordinates use a top-left origin.
    context.translateBy(x: 0, y: CGFloat(size))
    context.scaleBy(x: CGFloat(size) / 1024, y: -CGFloat(size) / 1024)
    if template {
        context.scaleBy(x: 1024 / 24, y: 1024 / 24)
        context.move(to: CGPoint(x: points[0].0, y: points[0].1))
        for point in points.dropFirst() { context.addLine(to: CGPoint(x: point.0, y: point.1)) }
        context.setLineWidth(2.6)
        context.setLineCap(.round)
        context.setLineJoin(.round)
        context.setStrokeColor(color(0, 0, 0))
        context.strokePath()
    } else {
        let plate = CGPath(roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824),
                           cornerWidth: 184, cornerHeight: 184, transform: nil)
        context.saveGState()
        context.setShadow(offset: CGSize(width: 0, height: 14), blur: 22,
                          color: color(0, 0, 0, 0.24))
        context.setFillColor(color(28, 38, 34))
        context.addPath(plate)
        context.fillPath()
        context.restoreGState()
        context.saveGState()
        context.addPath(plate)
        context.clip()
        context.drawLinearGradient(gradient([color(49, 66, 57), color(24, 32, 30)]),
                                   start: CGPoint(x: 250, y: 100), end: CGPoint(x: 760, y: 924),
                                   options: [])
        context.restoreGState()
        context.addPath(plate)
        context.setStrokeColor(color(207, 232, 214, 0.12))
        context.setLineWidth(2)
        context.strokePath()
        mark(in: context, origin: 152, scale: 30, width: 3.1)
    }
    let data = NSMutableData()
    let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, context.makeImage()!, nil)
    guard CGImageDestinationFinalize(destination) else { fatalError("Could not encode icon") }
    return data as Data
}

// Render each size from paths so small icons retain clean edges.
for size in [16, 32, 128, 256, 512] {
    try render(size: size).write(to: iconset.appendingPathComponent("icon_\(size)x\(size).png"))
    try render(size: size * 2).write(to: iconset.appendingPathComponent("icon_\(size)x\(size)@2x.png"))
}
try render(size: 1024).write(to: output.appendingPathComponent("starter-1024.png"))
try render(size: 64).write(to: output.appendingPathComponent("starter-64.png"))
try render(size: 72, template: true).write(to: output.appendingPathComponent("tray-template.png"))

let templateSVG = """
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <path d="\(pathData)" fill="none" stroke="#000" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>
</svg>

"""
try templateSVG.write(to: output.appendingPathComponent("tray-template.svg"), atomically: true, encoding: .utf8)
let appSVG = """
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="plate" x1="0" y1="0" x2="1" y2="1"><stop stop-color="#314239"/><stop offset="1" stop-color="#18201e"/></linearGradient>
    <linearGradient id="ink" x1="0" y1="0" x2="0" y2="1"><stop stop-color="#9ddcbb"/><stop offset="1" stop-color="#f3e8d0"/></linearGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="184" fill="url(#plate)" stroke="#cfe8d6" stroke-opacity=".12" stroke-width="2"/>
  <path d="\(pathData)" transform="translate(152 152) scale(30)" fill="none" stroke="url(#ink)" stroke-width="3.1" stroke-linecap="round" stroke-linejoin="round"/>
</svg>

"""
try appSVG.write(to: output.appendingPathComponent("starter.svg"), atomically: true, encoding: .utf8)
let process = Process()
process.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
process.arguments = ["-c", "icns", "-o", output.appendingPathComponent("Starter.icns").path, iconset.path]
try process.run()
process.waitUntilExit()
guard process.terminationStatus == 0 else { fatalError("iconutil failed") }
print("Created application and Retina menu bar icons in resources/icons")
