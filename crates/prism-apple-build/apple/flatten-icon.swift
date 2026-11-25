// Redraws an image without an alpha channel, on white: App Store Connect
// rejects app icons that have one (ITMS-90717), even fully opaque ones.
//
//   xcrun --sdk macosx swift flatten-icon.swift <in.png> <out.png>
//
// Prints `transparent` when some pixel of the image was not opaque (what
// shows white now), nothing otherwise.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("flatten-icon: \(message)\n".utf8))
    exit(1)
}

let args = CommandLine.arguments
guard args.count == 3 else { fail("usage: flatten-icon.swift <in.png> <out.png>") }
guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: args[1]) as CFURL, nil),
      let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
else { fail("cannot read \(args[1])") }
let (width, height) = (image.width, image.height)
let rect = CGRect(x: 0, y: 0, width: width, height: height)
let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

// Any pixel not opaque?
var transparent = false
if image.alphaInfo != .none && image.alphaInfo != .noneSkipLast && image.alphaInfo != .noneSkipFirst {
    var pixels = [UInt8](repeating: 0, count: width * height * 4)
    pixels.withUnsafeMutableBytes { buffer in
        guard let rgba = CGContext(
            data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: srgb,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { fail("cannot read the pixels of \(args[1])") }
        rgba.draw(image, in: rect)
    }
    transparent = stride(from: 3, to: pixels.count, by: 4).contains { pixels[$0] < 255 }
}

guard let context = CGContext(
    data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
    space: srgb, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
else { fail("cannot draw \(args[1])") }
context.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1))
context.fill(rect)
context.draw(image, in: rect)
guard let flat = context.makeImage(),
      let out = CGImageDestinationCreateWithURL(
          URL(fileURLWithPath: args[2]) as CFURL, UTType.png.identifier as CFString, 1, nil)
else { fail("cannot write \(args[2])") }
CGImageDestinationAddImage(out, flat, nil)
guard CGImageDestinationFinalize(out) else { fail("cannot write \(args[2])") }
if transparent { print("transparent") }
