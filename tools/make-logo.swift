// draws the lanlord logo, app icon and setup panel. run through tools/make-assets.sh.
// usage: make-logo <output dir>
// everything is drawn with coregraphics so the art can be rebuilt without any network.

import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

let space = CGColorSpaceCreateDeviceRGB()

func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        colorSpace: space,
        components: [
            CGFloat((hex >> 16) & 255) / 255, CGFloat((hex >> 8) & 255) / 255,
            CGFloat(hex & 255) / 255, alpha,
        ])!
}

func gradient(_ stops: [(CGFloat, UInt32)]) -> CGGradient {
    CGGradient(
        colorsSpace: space, colors: stops.map { rgb($0.1) } as CFArray,
        locations: stops.map { $0.0 })!
}

/// a bitmap context with y pointing down, like the screen.
func canvas(_ w: Int, _ h: Int) -> CGContext {
    let ctx = CGContext(
        data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.translateBy(x: 0, y: CGFloat(h))
    ctx.scaleBy(x: 1, y: -1)
    ctx.interpolationQuality = .high
    return ctx
}

func save(_ ctx: CGContext, _ path: String) {
    let url = URL(fileURLWithPath: path)
    try? FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, ctx.makeImage()!, nil)
    CGImageDestinationFinalize(dest)
}

func pngData(_ ctx: CGContext) -> Data {
    let data = NSMutableData()
    let dest = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, ctx.makeImage()!, nil)
    CGImageDestinationFinalize(dest)
    return data as Data
}

// MARK: helpers

func fill(_ ctx: CGContext, _ path: CGPath, _ g: CGGradient, _ a: CGPoint, _ b: CGPoint) {
    ctx.saveGState()
    ctx.addPath(path)
    ctx.clip()
    ctx.drawLinearGradient(g, start: a, end: b, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()
}

func outline(_ ctx: CGContext, _ path: CGPath, _ color: CGColor, _ width: CGFloat) {
    ctx.saveGState()
    ctx.addPath(path)
    ctx.setStrokeColor(color)
    ctx.setLineWidth(width)
    ctx.setLineJoin(.round)
    ctx.setLineCap(.round)
    ctx.strokePath()
    ctx.restoreGState()
}

func round(_ r: CGRect, _ radius: CGFloat) -> CGPath {
    CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
}

func circle(_ x: CGFloat, _ y: CGFloat, _ r: CGFloat) -> CGPath {
    CGPath(ellipseIn: CGRect(x: x - r, y: y - r, width: 2 * r, height: 2 * r), transform: nil)
}

func line(_ ctx: CGContext, _ pts: [(CGFloat, CGFloat)], _ color: CGColor, _ width: CGFloat) {
    ctx.saveGState()
    ctx.setStrokeColor(color)
    ctx.setLineWidth(width)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.move(to: CGPoint(x: pts[0].0, y: pts[0].1))
    for p in pts.dropFirst() { ctx.addLine(to: CGPoint(x: p.0, y: p.1)) }
    ctx.strokePath()
    ctx.restoreGState()
}

/// a glossy ball: lit from the top left.
func jewel(_ ctx: CGContext, _ x: CGFloat, _ y: CGFloat, _ r: CGFloat, _ light: UInt32, _ mid: UInt32, _ dark: UInt32, _ edge: UInt32, _ lw: CGFloat) {
    let p = circle(x, y, r)
    ctx.saveGState()
    ctx.addPath(p)
    ctx.clip()
    let g = CGGradient(
        colorsSpace: space, colors: [rgb(light), rgb(mid), rgb(dark)] as CFArray, locations: [0, 0.45, 1])!
    ctx.drawRadialGradient(
        g, startCenter: CGPoint(x: x - r * 0.35, y: y - r * 0.4), startRadius: 0,
        endCenter: CGPoint(x: x, y: y), endRadius: r * 1.15, options: [.drawsAfterEndLocation])
    ctx.restoreGState()
    outline(ctx, p, rgb(edge), lw)
    ctx.setFillColor(rgb(0xFFFFFF, 0.75))
    ctx.fillEllipse(in: CGRect(x: x - r * 0.5, y: y - r * 0.62, width: r * 0.5, height: r * 0.3))
}

// MARK: the emblem: a crown on a crt that shows a small network

/// draws into a 1000 x 1000 box whose top left is (ox, oy), scaled to `size` pixels.
func emblem(_ ctx: CGContext, _ ox: CGFloat, _ oy: CGFloat, _ size: CGFloat, thick: CGFloat = 1) {
    ctx.saveGState()
    ctx.translateBy(x: ox, y: oy)
    ctx.scaleBy(x: size / 1000, y: size / 1000)
    let ink = rgb(0x14131C)
    let w = 16 * thick

    // soft shadow on the ground
    ctx.saveGState()
    ctx.setShadow(offset: .zero, blur: 40, color: rgb(0x000000, 0.55))
    ctx.setFillColor(rgb(0x000000, 0.45))
    ctx.fillEllipse(in: CGRect(x: 230, y: 940, width: 540, height: 38))
    ctx.restoreGState()

    // stand
    let neck = CGPath(rect: CGRect(x: 405, y: 880, width: 190, height: 56), transform: nil)
    fill(ctx, neck, gradient([(0, 0xB9B5A4), (1, 0x6E6A5A)]), CGPoint(x: 405, y: 0), CGPoint(x: 595, y: 0))
    outline(ctx, neck, ink, w)
    let base = round(CGRect(x: 290, y: 925, width: 420, height: 56), 22)
    fill(ctx, base, gradient([(0, 0xEDE9DA), (0.5, 0xC2BEAC), (1, 0x7D7967)]), CGPoint(x: 290, y: 925), CGPoint(x: 560, y: 990))
    outline(ctx, base, ink, w)
    line(ctx, [(320, 940), (680, 940)], rgb(0xFFFFFF, 0.7), 6 * thick)

    // the monitor body
    let body = round(CGRect(x: 130, y: 410, width: 740, height: 490), 54)
    fill(ctx, body, gradient([(0, 0xF6F3E7), (0.4, 0xD3CEBC), (1, 0x8A8674)]), CGPoint(x: 130, y: 410), CGPoint(x: 870, y: 900))
    outline(ctx, body, ink, w)
    line(ctx, [(176, 446), (824, 446)], rgb(0xFFFFFF, 0.9), 8 * thick)
    line(ctx, [(160, 480), (160, 840)], rgb(0xFFFFFF, 0.75), 8 * thick)
    line(ctx, [(840, 480), (840, 850)], rgb(0x3E3B30, 0.35), 8 * thick)
    line(ctx, [(190, 880), (810, 880)], rgb(0x3E3B30, 0.35), 8 * thick)

    // the sunken screen frame and the glass
    let well = round(CGRect(x: 205, y: 480, width: 590, height: 340), 34)
    fill(ctx, well, gradient([(0, 0x55525A), (1, 0xB8B6BC)]), CGPoint(x: 205, y: 480), CGPoint(x: 795, y: 820))
    outline(ctx, well, ink, 8 * thick)
    let glass = round(CGRect(x: 232, y: 506, width: 536, height: 288), 24)
    fill(ctx, glass, gradient([(0, 0x2342C8), (0.45, 0x081660), (1, 0x00001E)]), CGPoint(x: 232, y: 506), CGPoint(x: 768, y: 794))
    outline(ctx, glass, rgb(0x000012), 7 * thick)

    // a small network on the glass
    ctx.saveGState()
    ctx.addPath(glass)
    ctx.clip()
    let nodes: [(CGFloat, CGFloat)] = [(400, 610), (610, 600), (505, 735)]
    ctx.setShadow(offset: .zero, blur: 22, color: rgb(0x58FF8C, 0.9))
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        line(ctx, [nodes[a], nodes[b]], rgb(0x6CFF9A), 11 * thick)
    }
    for (x, y) in nodes {
        jewel(ctx, x, y, 40, 0xFFFFFF, 0x4CE887, 0x0B6B33, 0x003B17, 6 * thick)
    }
    ctx.restoreGState()
    // a reflection across the glass
    ctx.saveGState()
    ctx.addPath(glass)
    ctx.clip()
    ctx.move(to: CGPoint(x: 240, y: 512))
    ctx.addLine(to: CGPoint(x: 600, y: 512))
    ctx.addLine(to: CGPoint(x: 240, y: 700))
    ctx.closePath()
    ctx.setFillColor(rgb(0xFFFFFF, 0.14))
    ctx.fillPath()
    ctx.restoreGState()
    // power light and a vent
    jewel(ctx, 780, 862, 12, 0xC8FFD6, 0x38D060, 0x0A5A22, 0x06280F, 3 * thick)
    for i in 0..<4 {
        line(ctx, [(300 + CGFloat(i) * 30, 850), (300 + CGFloat(i) * 30, 872)], rgb(0x3E3B30, 0.55), 7 * thick)
    }

    // the crown
    let crown = CGMutablePath()
    crown.move(to: CGPoint(x: 262, y: 452))
    crown.addLine(to: CGPoint(x: 232, y: 196))
    crown.addLine(to: CGPoint(x: 384, y: 336))
    crown.addLine(to: CGPoint(x: 500, y: 138))
    crown.addLine(to: CGPoint(x: 616, y: 336))
    crown.addLine(to: CGPoint(x: 768, y: 196))
    crown.addLine(to: CGPoint(x: 738, y: 452))
    crown.closeSubpath()
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 10), blur: 26, color: rgb(0x000000, 0.5))
    ctx.addPath(crown)
    ctx.setFillColor(rgb(0xC98A00))
    ctx.fillPath()
    ctx.restoreGState()
    fill(ctx, crown, gradient([(0, 0xFFF7B8), (0.3, 0xF9C81C), (0.65, 0xD48E00), (1, 0x7E4B00)]), CGPoint(x: 232, y: 130), CGPoint(x: 770, y: 460))
    outline(ctx, crown, rgb(0x3A2100), 20 * thick)
    // lit edges along the left of each point
    line(ctx, [(258, 244), (372, 338)], rgb(0xFFFFFF, 0.8), 9 * thick)
    line(ctx, [(486, 190), (396, 330)], rgb(0xFFFFFF, 0.7), 9 * thick)
    line(ctx, [(628, 340), (514, 190)], rgb(0x6A3F00, 0.35), 9 * thick)
    for (x, y) in [(232, 188), (500, 128), (768, 188)] {
        jewel(ctx, CGFloat(x), CGFloat(y), 36, 0xFFFFFF, 0xFFD43A, 0xB06E00, 0x3A2100, 12 * thick)
    }
    // the band and its stones
    let band = round(CGRect(x: 258, y: 372, width: 484, height: 92), 24)
    fill(ctx, band, gradient([(0, 0xFFE46A), (0.45, 0xE9A400), (1, 0x8A5400)]), CGPoint(x: 258, y: 372), CGPoint(x: 258, y: 464))
    outline(ctx, band, rgb(0x3A2100), 16 * thick)
    line(ctx, [(286, 386), (714, 386)], rgb(0xFFFFFF, 0.75), 7 * thick)
    jewel(ctx, 380, 424, 24, 0xDCE6FF, 0x3C6CFF, 0x0A2490, 0x061450, 8 * thick)
    jewel(ctx, 500, 424, 30, 0xFFD6D6, 0xF03030, 0x8A0A0A, 0x400000, 9 * thick)
    jewel(ctx, 620, 424, 24, 0xDCE6FF, 0x3C6CFF, 0x0A2490, 0x061450, 8 * thick)
    ctx.restoreGState()
}

// MARK: backgrounds

struct Rng {
    var state: UInt64
    mutating func next() -> CGFloat {
        state = state &* 6364136223846793005 &+ 1442695040888963407
        return CGFloat((state >> 33) & 0xFFFFF) / CGFloat(0xFFFFF)
    }
}

func stars(_ ctx: CGContext, _ w: CGFloat, _ h: CGFloat, count: Int, scale: CGFloat, seed: UInt64, avoid: CGRect = .null) {
    var rng = Rng(state: seed)
    for _ in 0..<count {
        let x = rng.next() * w
        let y = rng.next() * h
        if avoid.contains(CGPoint(x: x, y: y)) { _ = rng.next(); continue }
        let r = (0.7 + rng.next() * 1.6) * scale
        let a = 0.35 + rng.next() * 0.6
        ctx.setFillColor(rgb(0xFFFFFF, a))
        ctx.fillEllipse(in: CGRect(x: x - r, y: y - r, width: 2 * r, height: 2 * r))
    }
}

/// the dark navy of the nt setup screens: a vertical gradient, a glow, and a fine dither.
func navy(_ ctx: CGContext, _ w: CGFloat, _ h: CGFloat, scale: CGFloat, glow: CGPoint?) {
    ctx.drawLinearGradient(
        gradient([(0, 0x00007E), (0.55, 0x00004A), (1, 0x000014)]),
        start: .zero, end: CGPoint(x: 0, y: h), options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    if let glow = glow {
        let g = CGGradient(
            colorsSpace: space, colors: [rgb(0x3345E0, 0.55), rgb(0x3345E0, 0)] as CFArray, locations: [0, 1])!
        ctx.drawRadialGradient(
            g, startCenter: glow, startRadius: 0, endCenter: glow, endRadius: h * 0.9, options: [])
    }
    // a 2 pixel checker, barely there, like the dithered backdrops of the time
    let step = max(2, 2 * scale)
    ctx.setFillColor(rgb(0x000000, 0.10))
    var y: CGFloat = 0
    var row = 0
    while y < h {
        var x: CGFloat = row % 2 == 0 ? 0 : step
        while x < w {
            ctx.fill(CGRect(x: x, y: y, width: step, height: step))
            x += 2 * step
        }
        y += step
        row += 1
    }
}

// MARK: the word

func wordPath(_ text: String, size: CGFloat, italic: Bool = false) -> CGPath {
    let name = italic ? "TimesNewRomanPS-BoldItalicMT" : "TimesNewRomanPS-BoldMT"
    let font = CTFontCreateWithName(name as CFString, size, nil)
    let attr = NSAttributedString(
        string: text,
        attributes: [
            NSAttributedString.Key(rawValue: kCTFontAttributeName as String): font,
            NSAttributedString.Key(rawValue: kCTKernAttributeName as String): size * 0.01,
        ])
    let line = CTLineCreateWithAttributedString(attr)
    let path = CGMutablePath()
    for run in CTLineGetGlyphRuns(line) as! [CTRun] {
        let runFont = (CTRunGetAttributes(run) as NSDictionary)[kCTFontAttributeName] as! CTFont
        let n = CTRunGetGlyphCount(run)
        var glyphs = [CGGlyph](repeating: 0, count: n)
        var pos = [CGPoint](repeating: .zero, count: n)
        CTRunGetGlyphs(run, CFRange(location: 0, length: n), &glyphs)
        CTRunGetPositions(run, CFRange(location: 0, length: n), &pos)
        for i in 0..<n {
            if let g = CTFontCreatePathForGlyph(runFont, glyphs[i], nil) {
                path.addPath(g, transform: CGAffineTransform(translationX: pos[i].x, y: pos[i].y))
            }
        }
    }
    return path
}

/// puts a glyph path (y up, baseline at 0) at (x, baseline) in a y-down context.
func place(_ p: CGPath, _ x: CGFloat, _ baseline: CGFloat) -> CGPath {
    var t = CGAffineTransform(translationX: x, y: baseline).scaledBy(x: 1, y: -1)
    return p.copy(using: &t)!
}

/// the wordmark: soft shadow, a lit top left edge, a vertical gradient and a thin dark edge.
func wordmark(_ ctx: CGContext, _ text: CGPath, light: Bool, size: CGFloat, finish: Int = 0) {
    let box = text.boundingBox
    ctx.saveGState()
    ctx.setShadow(
        offset: CGSize(width: size * 0.03, height: size * 0.04), blur: size * 0.06,
        color: rgb(light ? 0x000040 : 0x000000, light ? 0.35 : 0.8))
    ctx.addPath(text)
    ctx.setFillColor(rgb(light ? 0x1C2A9A : 0x000000))
    ctx.fillPath()
    ctx.restoreGState()
    // a bright edge up and to the left, then the gradient face over it
    var up = CGAffineTransform(translationX: -size * 0.012, y: -size * 0.012)
    ctx.addPath(text.copy(using: &up)!)
    ctx.setFillColor(rgb(0xFFFFFF, light ? 0.9 : 0.95))
    ctx.fillPath()
    var face = light
        ? gradient([(0, 0x4A5CE8), (0.5, 0x0A1A8C), (1, 0x00004A)])
        : gradient([(0, 0xFFFFFF), (0.45, 0xD2E0FF), (1, 0x6F8FE0)])
    if !light && finish == 1 { face = gradient([(0, 0xFFF6B0), (0.45, 0xF4BE12), (1, 0x8A5300)]) }
    if !light && finish == 2 {
        face = gradient([(0, 0xFFFFFF), (0.46, 0xB4BEDA), (0.5, 0x3C4670), (1, 0xC6D0EC)])
    }
    fill(ctx, text, face, CGPoint(x: 0, y: box.minY), CGPoint(x: 0, y: box.maxY))
    outline(ctx, text, rgb(light ? 0x00002E : 0x00003A, 0.85), size * 0.012)
}

// MARK: the banner

func banner(_ scale: CGFloat, light: Bool, finish: Int = 0) -> CGContext {
    let w = 1280 * scale
    let h = 400 * scale
    let ctx = canvas(Int(w), Int(h))
    if !light {
        navy(ctx, w, h, scale: scale, glow: CGPoint(x: 250 * scale, y: 190 * scale))
        stars(
            ctx, w, h, count: 70, scale: scale, seed: 7,
            avoid: CGRect(x: 420 * scale, y: 70 * scale, width: 800 * scale, height: 290 * scale))
    }
    emblem(ctx, 52 * scale, 30 * scale, 340 * scale)
    let size = 188 * scale
    let word = place(wordPath("LANlord", size: size), 430 * scale, 238 * scale)
    wordmark(ctx, word, light: light, size: size, finish: finish)
    let tag = place(wordPath("all your LAN are belong to you", size: 42 * scale, italic: true), 436 * scale, 322 * scale)
    ctx.addPath(tag)
    ctx.setFillColor(rgb(light ? 0x1C2A9A : 0xA8BCFF, 0.95))
    ctx.fillPath()
    return ctx
}

// MARK: the setup panel (the tall starry strip beside the nt setup wizard)

func setupPanel(_ scale: CGFloat) -> CGContext {
    let w = 164 * scale
    let h = 312 * scale
    let ctx = canvas(Int(w), Int(h))
    ctx.drawLinearGradient(
        gradient([(0, 0x00006A), (0.5, 0x000038), (1, 0x00000E)]), start: .zero,
        end: CGPoint(x: 0, y: h), options: [])
    stars(ctx, w, h, count: 46, scale: scale * 0.8, seed: 21)
    emblem(ctx, 8 * scale, 104 * scale, 148 * scale)
    return ctx
}

// MARK: the app icon tile

func tile(_ size: CGFloat, margin: CGFloat, thick: CGFloat = 1) -> CGContext {
    let ctx = canvas(Int(size), Int(size))
    let m = size * margin
    let box = CGRect(x: m, y: m, width: size - 2 * m, height: size - 2 * m)
    let shape = round(box, box.width * 0.215)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: size * 0.012), blur: size * 0.03, color: rgb(0x000000, margin > 0.06 ? 0.5 : 0.35))
    ctx.addPath(shape)
    ctx.setFillColor(rgb(0x00004A))
    ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    ctx.drawLinearGradient(
        gradient([(0, 0x2A3CD4), (0.35, 0x0A1A96), (1, 0x00001E)]),
        start: CGPoint(x: box.minX, y: box.minY), end: CGPoint(x: box.maxX * 0.8, y: box.maxY),
        options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    stars(ctx, size, size, count: Int(14 * size / 512), scale: size / 1024, seed: 5)
    ctx.restoreGState()
    // a lit edge top left, a dark one bottom right, like a raised button
    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    outline(ctx, shape, rgb(0xFFFFFF, 0.35), size * 0.012)
    ctx.restoreGState()
    let e = box.width * 0.8
    emblem(ctx, box.midX - e / 2, box.minY + box.height * 0.1, e, thick: thick)
    return ctx
}

// MARK: hand set pixel icons for 16 and 32 pixels

struct Pix {
    let w: Int
    let h: Int
    var px: [UInt32?]
    init(_ w: Int, _ h: Int) {
        self.w = w
        self.h = h
        px = [UInt32?](repeating: nil, count: w * h)
    }
    subscript(x: Int, y: Int) -> UInt32? {
        get { (0..<w).contains(x) && (0..<h).contains(y) ? px[y * w + x] : nil }
        set { if (0..<w).contains(x) && (0..<h).contains(y) { px[y * w + x] = newValue } }
    }
    mutating func rect(_ x: Int, _ y: Int, _ rw: Int, _ rh: Int, _ c: UInt32) {
        for j in y..<(y + rh) { for i in x..<(x + rw) { self[i, j] = c } }
    }
    /// fill a polygon by pixel centres, then draw an outline and light it from the top left.
    mutating func shape(_ pts: [(Double, Double)], hi: UInt32, mid: UInt32, lo: UInt32, ink: UInt32) {
        func inside(_ x: Int, _ y: Int) -> Bool {
            let px = Double(x) + 0.5
            let py = Double(y) + 0.5
            var c = false
            var j = pts.count - 1
            for i in 0..<pts.count {
                let (xi, yi) = pts[i]
                let (xj, yj) = pts[j]
                if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi { c = !c }
                j = i
            }
            return c
        }
        var mask = [Bool](repeating: false, count: w * h)
        for y in 0..<h { for x in 0..<w { mask[y * w + x] = inside(x, y) } }
        func m(_ x: Int, _ y: Int) -> Bool { (0..<w).contains(x) && (0..<h).contains(y) && mask[y * w + x] }
        func edge(_ x: Int, _ y: Int) -> Bool {
            m(x, y) && (!m(x - 1, y) || !m(x + 1, y) || !m(x, y - 1) || !m(x, y + 1))
        }
        for y in 0..<h {
            for x in 0..<w where m(x, y) {
                if edge(x, y) {
                    self[x, y] = ink
                } else if edge(x - 1, y) || edge(x, y - 1) {
                    self[x, y] = hi
                } else if edge(x + 1, y) || edge(x, y + 1) {
                    self[x, y] = lo
                } else {
                    self[x, y] = mid
                }
            }
        }
    }
    func image() -> CGContext {
        let ctx = canvas(w, h)
        ctx.setShouldAntialias(false)
        for y in 0..<h {
            for x in 0..<w {
                if let c = px[y * w + x] {
                    ctx.setFillColor(rgb(c))
                    ctx.fill(CGRect(x: x, y: y, width: 1, height: 1))
                }
            }
        }
        return ctx
    }
}

let gold = (hi: UInt32(0xFFF8A8), mid: UInt32(0xF6BE10), lo: UInt32(0xB06E00), ink: UInt32(0x3A2100))
let beige = (hi: UInt32(0xFFFFFF), mid: UInt32(0xCFCAB8), lo: UInt32(0x85816F), ink: UInt32(0x15141C))

func pixelIcon32() -> CGContext {
    var p = Pix(32, 32)
    // the navy tile with a raised bevel
    p.rect(0, 0, 32, 32, 0x00007A)
    p.rect(0, 0, 32, 1, 0x4B58E0)
    p.rect(0, 0, 1, 32, 0x4B58E0)
    p.rect(0, 31, 32, 1, 0x000024)
    p.rect(31, 0, 1, 32, 0x000024)
    p.rect(1, 1, 30, 1, 0x1B28B4)
    p.rect(1, 30, 30, 1, 0x00004A)
    for (x, y) in [(4, 5), (27, 8), (5, 26), (28, 22), (2, 15)] { p[x, y] = 0xFFFFFF }
    // the stand and the monitor
    p.rect(13, 27, 6, 1, 0x7D7967)
    p.rect(9, 28, 14, 2, beige.ink)
    p.rect(10, 28, 12, 1, 0xC2BEAC)
    p.rect(10, 29, 12, 1, 0x7D7967)
    p.shape([(5, 14), (27, 14), (27, 27), (5, 27)], hi: beige.hi, mid: beige.mid, lo: beige.lo, ink: beige.ink)
    p.rect(8, 17, 16, 8, 0x15141C)
    p.rect(9, 18, 14, 6, 0x0A1A78)
    p.rect(9, 18, 14, 1, 0x2A44C8)
    // the network on the glass
    let wire: UInt32 = 0x2FB05A
    p.rect(13, 19, 6, 1, wire)
    p[13, 20] = wire
    p[14, 21] = wire
    p[18, 20] = wire
    p[17, 21] = wire
    for (x, y) in [(11, 19), (19, 19), (15, 21)] {
        p.rect(x, y, 2, 2, 0x6CFF9C)
        p[x, y] = 0xFFFFFF
    }
    p[24, 26] = 0x38D060
    // the crown
    p.shape(
        [(6, 15), (5, 3), (11, 8), (16, 1), (21, 8), (27, 3), (26, 15)], hi: gold.hi, mid: gold.mid, lo: gold.lo,
        ink: gold.ink)
    p.rect(8, 11, 16, 1, gold.lo)
    p.rect(11, 12, 2, 1, 0x3C6CFF)
    p.rect(15, 12, 2, 1, 0xF03030)
    p.rect(19, 12, 2, 1, 0x3C6CFF)
    return p.image()
}

func pixelIcon16() -> CGContext {
    var p = Pix(16, 16)
    p.rect(0, 0, 16, 16, 0x00007A)
    p.rect(0, 0, 16, 1, 0x4B58E0)
    p.rect(0, 0, 1, 16, 0x4B58E0)
    p.rect(0, 15, 16, 1, 0x000024)
    p.rect(15, 0, 1, 16, 0x000024)
    p.rect(6, 14, 4, 1, beige.ink)
    p.shape([(3, 7), (13, 7), (13, 14), (3, 14)], hi: beige.hi, mid: beige.mid, lo: beige.lo, ink: beige.ink)
    p.rect(5, 9, 6, 3, 0x0A1A78)
    p.rect(6, 10, 1, 1, 0x5CFF8C)
    p.rect(9, 10, 1, 1, 0x5CFF8C)
    p.shape([(3, 8), (2, 2), (6, 5), (8, 1), (10, 5), (14, 2), (13, 8)], hi: gold.hi, mid: gold.mid, lo: gold.lo, ink: 0x6A3C00)
    p[8, 6] = 0xF03030
    return p.image()
}

// MARK: files

func scaled(_ src: CGContext, to size: Int) -> CGContext {
    let ctx = canvas(size, size)
    ctx.interpolationQuality = .none
    ctx.setShouldAntialias(false)
    // the canvas is flipped, so flip back to draw an image upright
    ctx.translateBy(x: 0, y: CGFloat(size))
    ctx.scaleBy(x: 1, y: -1)
    ctx.draw(src.makeImage()!, in: CGRect(x: 0, y: 0, width: size, height: size))
    return ctx
}

func ico(_ images: [(Int, Data)]) -> Data {
    var d = Data()
    func u16(_ v: Int) { d.append(contentsOf: [UInt8(v & 255), UInt8((v >> 8) & 255)]) }
    func u32(_ v: Int) { d.append(contentsOf: [UInt8(v & 255), UInt8((v >> 8) & 255), UInt8((v >> 16) & 255), UInt8((v >> 24) & 255)]) }
    u16(0); u16(1); u16(images.count)
    var offset = 6 + 16 * images.count
    for (size, png) in images {
        d.append(UInt8(size >= 256 ? 0 : size)); d.append(UInt8(size >= 256 ? 0 : size)); d.append(0); d.append(0)
        u16(1); u16(32); u32(png.count); u32(offset)
        offset += png.count
    }
    for (_, png) in images { d.append(png) }
    return d
}

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "assets"
let ui = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "crates/ui/assets"

save(banner(1, light: false), "\(out)/logo.png")
save(banner(1, light: true), "\(out)/logo-light.png")
save(setupPanel(1), "\(out)/setup-panel.png")
save(setupPanel(2), "\(out)/setup-panel@2x.png")
save(setupPanel(2), "\(ui)/setup-panel.png")

// the macos icon has a margin around the tile; the windows and window icons nearly fill it
save(tile(512, margin: 0.098), "\(out)/icon-512.png")
save(tile(512, margin: 0.04), "\(ui)/icon.png")
save(pixelIcon32(), "\(out)/icon-32.png")

let iconset = "\(out)/icon.iconset"
try? FileManager.default.removeItem(atPath: iconset)
for (name, px, big) in [
    ("icon_16x16", 16, false), ("icon_16x16@2x", 32, false), ("icon_32x32", 32, false), ("icon_32x32@2x", 64, false),
    ("icon_128x128", 128, true), ("icon_128x128@2x", 256, true), ("icon_256x256", 256, true),
    ("icon_256x256@2x", 512, true), ("icon_512x512", 512, true), ("icon_512x512@2x", 1024, true),
] {
    let ctx: CGContext
    if px == 16 { ctx = pixelIcon16() }
    else if px == 32 && !big { ctx = pixelIcon32() }
    else if px == 64 { ctx = scaled(pixelIcon32(), to: 64) }
    else { ctx = tile(CGFloat(px), margin: 0.098) }
    save(ctx, "\(iconset)/\(name).png")
}

var entries: [(Int, Data)] = []
entries.append((16, pngData(pixelIcon16())))
entries.append((32, pngData(pixelIcon32())))
entries.append((48, pngData(tile(48, margin: 0.03, thick: 2.2))))
entries.append((256, pngData(tile(256, margin: 0.03))))
try? ico(entries).write(to: URL(fileURLWithPath: "\(out)/icon.ico"))
if CommandLine.arguments.count > 3 {
    let prev = CommandLine.arguments[3]
    save(scaled(pixelIcon16(), to: 256), "\(prev)/zoom-16.png")
    save(scaled(pixelIcon32(), to: 256), "\(prev)/zoom-32.png")
    save(tile(48, margin: 0.03, thick: 2.2), "\(prev)/icon-48.png")
}
if CommandLine.arguments.count > 4 {
    let dir = CommandLine.arguments[4]
    save(banner(1, light: false, finish: 0), "\(dir)/a-white-blue.png")
    save(banner(1, light: false, finish: 1), "\(dir)/b-gold.png")
    save(banner(1, light: false, finish: 2), "\(dir)/c-chrome.png")
}
print("wrote the logo and icons to \(out)")
