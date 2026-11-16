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

// MARK: the emblem: a flat server tower dressed as a lord, in the spirit of nt and backoffice branding

/// which emblem to draw: "a" has the staff leaning against the right side of the tower, "b" against the left.
var emblemStyle = "a"
/// how dark the little ground shadow under the tower is; lighter on a white page.
var shadowAlpha: CGFloat = 0.28

let gold: UInt32 = 0xD4AF37
let goldDark: UInt32 = 0xA9841A
let ink: UInt32 = 0x23232F

struct Cloth {
    let front: UInt32
    let side: UInt32
    let top: UInt32
}

let burgundy = Cloth(front: 0x8E1B38, side: 0x5C1024, top: 0xA82A4A)

func poly(_ pts: [(CGFloat, CGFloat)]) -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: pts[0].0, y: pts[0].1))
    for q in pts.dropFirst() { p.addLine(to: CGPoint(x: q.0, y: q.1)) }
    p.closeSubpath()
    return p
}

func flat(_ ctx: CGContext, _ path: CGPath, _ color: UInt32) {
    ctx.addPath(path)
    ctx.setFillColor(rgb(color))
    ctx.fillPath()
}

func box(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> CGPath {
    CGPath(rect: CGRect(x: x, y: y, width: w, height: h), transform: nil)
}

func ellipse(_ cx: CGFloat, _ cy: CGFloat, _ rx: CGFloat, _ ry: CGFloat) -> CGPath {
    CGPath(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: 2 * rx, height: 2 * ry), transform: nil)
}

func stroke(_ ctx: CGContext, _ path: CGPath, _ color: UInt32, _ width: CGFloat, alpha: CGFloat = 1) {
    ctx.addPath(path)
    ctx.setStrokeColor(rgb(color, alpha))
    ctx.setLineWidth(width)
    ctx.setLineCap(.butt)
    ctx.setLineJoin(.miter)
    ctx.strokePath()
}

func segment(_ x1: CGFloat, _ y1: CGFloat, _ x2: CGFloat, _ y2: CGFloat) -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: x1, y: y1))
    p.addLine(to: CGPoint(x: x2, y: y2))
    return p
}

/// draws into a 1000 x 1000 box whose top left is (ox, oy), scaled to `size` pixels.
func emblem(_ ctx: CGContext, _ ox: CGFloat, _ oy: CGFloat, _ size: CGFloat, thick: CGFloat = 1) {
    ctx.saveGState()
    ctx.translateBy(x: ox, y: oy)
    ctx.scaleBy(x: size / 1000, y: size / 1000)
    ctx.translateBy(x: 500, y: 540)
    ctx.scaleBy(x: 1.17, y: 1.17)
    ctx.translateBy(x: emblemStyle == "b" ? -432 : -513, y: -548)
    gentleman(ctx, burgundy, shadow: true)
    ctx.restoreGState()
}

/// the lord's staff: a separate slim object standing on the ground and leaning a few degrees against the
/// tower, its gold ball resting on the edge of the tower, with a soft contact shadow under its foot.
func staff(_ ctx: CGContext) {
    let top: (CGFloat, CGFloat) = emblemStyle == "b" ? (326, 376) : (606, 376)
    let foot: (CGFloat, CGFloat) = emblemStyle == "b" ? (268, 926) : (676, 926)
    ctx.setFillColor(rgb(0x000000, shadowAlpha))
    ctx.fillEllipse(in: CGRect(x: foot.0 - 28, y: 920, width: 56, height: 14))
    let dx = foot.0 - top.0
    let dy = foot.1 - top.1
    let len = (dx * dx + dy * dy).squareRoot()
    let ux = dx / len
    let uy = dy / len
    stroke(ctx, segment(top.0 + ux * 14, top.1 + uy * 14, foot.0 - ux * 24, foot.1 - uy * 24), 0xB98A52, 10)
    stroke(ctx, segment(top.0 + ux * 14 - 3, top.1 + uy * 14, foot.0 - ux * 24 - 3, foot.1 - uy * 24), 0xFFFFFF, 2, alpha: 0.35)
    stroke(ctx, segment(foot.0 - ux * 26, foot.1 - uy * 26, foot.0, foot.1), gold, 10)
    flat(ctx, circle(top.0, top.1, 20), gold)
    flat(ctx, circle(top.0 - 6, top.1 - 7, 6), 0xF4DC86)
}

/// a tall top hat tipped a little to one side: a wide brim, a satin band and a lit left edge.
func topHat(_ ctx: CGContext) {
    ctx.saveGState()
    ctx.translateBy(x: 470, y: 346)
    ctx.rotate(by: (emblemStyle == "b" ? 12 : -12) * .pi / 180)
    ctx.translateBy(x: -470, y: -346)
    flat(ctx, ellipse(470, 340, 134, 24), 0x1A1A26)
    flat(ctx, poly([(393, 342), (399, 152), (541, 152), (547, 342)]), 0x262634)
    flat(ctx, ellipse(470, 152, 71, 13), 0x383849)
    flat(ctx, poly([(394, 318), (546, 318), (547, 342), (393, 342)]), gold)
    stroke(ctx, segment(398, 160, 394, 318), 0xFFFFFF, 5, alpha: 0.6)
    ctx.restoreGState()
}

/// a slender tower under a softly draped robe: a big fold falling from the left shoulder, a narrow one on
/// the right, and the side face wrapped in cloth. Gold only on the collar edge.
func gentleman(_ ctx: CGContext, _ robe: Cloth, shadow: Bool) {
    if shadow {
        ctx.setFillColor(rgb(0x000000, shadowAlpha))
        ctx.fillEllipse(in: CGRect(x: 335, y: 902, width: 290, height: 44))
    }
    flat(ctx, poly([(355, 360), (515, 360), (515, 920), (355, 920)]), 0xD7DBE8)
    flat(ctx, poly([(515, 360), (585, 324), (585, 884), (515, 920)]), robe.side)
    flat(ctx, poly([(355, 360), (425, 324), (585, 324), (515, 360)]), robe.top)
    // the bays down the open middle, where the robe falls away
    for i in 0..<3 {
        let y = 600 + CGFloat(i) * 52
        flat(ctx, box(426, y, 62, 34), 0x1F2A6B)
        flat(ctx, circle(438, y + 17, 6), 0x3FD07A)
        flat(ctx, box(450, y + 13, 30, 8), 0x39458F)
    }
    // the side face: one broad fold of cloth
    flat(ctx, poly([(515, 372), (548, 356), (578, 640), (556, 886), (515, 912)]), robe.front)
    // the robe: a large fold falling from the left shoulder, and a narrow one at the right edge
    flat(ctx, poly([(346, 352), (474, 350), (446, 470), (420, 640), (398, 790), (382, 916), (346, 916)]), robe.front)
    flat(ctx, poly([(366, 364), (398, 362), (390, 560), (376, 912), (356, 912)]), robe.side)
    flat(ctx, poly([(515, 358), (490, 356), (494, 560), (500, 720), (498, 916), (515, 916)]), robe.front)
    // gold on the collar edge only
    stroke(ctx, segment(474, 350, 446, 470), gold, 5)
    // one lit edge
    stroke(ctx, segment(346, 352, 425, 324), 0xFFFFFF, 5, alpha: 0.8)
    staff(ctx)
    topHat(ctx)
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

func wordPath(_ text: String, size: CGFloat, italic: Bool = false, spacing: CGFloat = 0.01) -> CGPath {
    let name = italic ? "TimesNewRomanPS-ItalicMT" : "TimesNewRomanPS-BoldMT"
    let font = CTFontCreateWithName(name as CFString, size, nil)
    let attr = NSAttributedString(
        string: text,
        attributes: [
            NSAttributedString.Key(rawValue: kCTFontAttributeName as String): font,
            NSAttributedString.Key(rawValue: kCTKernAttributeName as String): size * spacing,
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

/// the wordmark: a clean title with a faint lit edge and a soft, low shadow.
func wordmark(_ ctx: CGContext, _ text: CGPath, light: Bool, size: CGFloat) {
    let box = text.boundingBox
    let px = size / 188
    ctx.saveGState()
    ctx.setShadow(
        offset: CGSize(width: 0, height: 3 * px), blur: 9 * px, color: rgb(light ? 0x000040 : 0x000000, light ? 0.18 : 0.4))
    ctx.addPath(text)
    ctx.setFillColor(rgb(light ? 0x1C2A9A : 0x00002A))
    ctx.fillPath()
    ctx.restoreGState()
    // a faint 1 pixel lit edge up and to the left
    var up = CGAffineTransform(translationX: -1.2 * px, y: -1.2 * px)
    ctx.addPath(text.copy(using: &up)!)
    ctx.setFillColor(rgb(0xFFFFFF, light ? 0.55 : 0.45))
    ctx.fillPath()
    let face = light
        ? gradient([(0, 0x4A5CE8), (0.5, 0x0A1A8C), (1, 0x00004A)])
        : gradient([(0, 0xFFFFFF), (0.45, 0xD2E0FF), (1, 0x6F8FE0)])
    fill(ctx, text, face, CGPoint(x: 0, y: box.minY), CGPoint(x: 0, y: box.maxY))
}

// MARK: the banner

/// a thin gold rule with a small diamond in the middle.
func goldRule(_ ctx: CGContext, _ x1: CGFloat, _ x2: CGFloat, _ y: CGFloat, _ scale: CGFloat, light: Bool) {
    let color = light ? goldDark : gold
    let mid = (x1 + x2) / 2
    ctx.setFillColor(rgb(color))
    ctx.fill(CGRect(x: x1, y: y - scale, width: (x2 - x1) / 2 - 16 * scale, height: 2 * scale))
    ctx.fill(CGRect(x: mid + 16 * scale, y: y - scale, width: (x2 - x1) / 2 - 16 * scale, height: 2 * scale))
    flat(ctx, poly([(mid, y - 7 * scale), (mid + 9 * scale, y), (mid, y + 7 * scale), (mid - 9 * scale, y)]), color)
}

func banner(_ scale: CGFloat, light: Bool) -> CGContext {
    let w = 1280 * scale
    let h = 400 * scale
    let ctx = canvas(Int(w), Int(h))
    if !light {
        navy(ctx, w, h, scale: scale, glow: CGPoint(x: 250 * scale, y: 190 * scale))
        stars(
            ctx, w, h, count: 70, scale: scale, seed: 7,
            avoid: CGRect(x: 420 * scale, y: 60 * scale, width: 800 * scale, height: 300 * scale))
    }
    shadowAlpha = light ? 0.10 : 0.28
    emblem(ctx, 52 * scale, 30 * scale, 340 * scale)
    shadowAlpha = 0.28
    let size = 176 * scale
    let word = place(wordPath("LANlord", size: size), 436 * scale, 238 * scale)
    wordmark(ctx, word, light: light, size: size)
    goldRule(ctx, 436 * scale, 1160 * scale, 76 * scale, scale, light: light)
    goldRule(ctx, 436 * scale, 1160 * scale, 268 * scale, scale, light: light)
    // the tagline in small capitals, italic and widely spaced
    let tag = place(
        wordPath("ALL YOUR LAN ARE BELONG TO YOU", size: 30 * scale, italic: true, spacing: 0.16), 446 * scale,
        326 * scale)
    ctx.addPath(tag)
    ctx.setFillColor(rgb(light ? 0x6A5210 : 0xE6D59A, 0.95))
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

let navyTile: [UInt32] = [0x00007A, 0x4B58E0, 0x000024, 0x1B28B4, 0x00004A]

func tileBase(_ p: inout Pix) {
    let n = p.w
    p.rect(0, 0, n, n, navyTile[0])
    p.rect(0, 0, n, 1, navyTile[1])
    p.rect(0, 0, 1, n, navyTile[1])
    p.rect(0, n - 1, n, 1, navyTile[2])
    p.rect(n - 1, 0, 1, n, navyTile[2])
    if n > 16 {
        p.rect(1, 1, n - 2, 1, navyTile[3])
        p.rect(1, n - 2, n - 2, 1, navyTile[4])
    }
}

let towerFront: UInt32 = 0xD7DBE8
let bay: UInt32 = 0x1F2A6B
let led: UInt32 = 0x3FD07A
let hatBlack: UInt32 = 0x262634
let hatBrim: UInt32 = 0x1A1A26

/// the 32 pixel tower: slender, under a draped robe that opens to the right, with a tipped top hat and a
/// gold monocle ring round the top light.
func pixelIcon32() -> CGContext {
    let c = burgundy
    var p = Pix(32, 32)
    tileBase(&p)
    for (x, y) in [(4, 6), (27, 9), (5, 24), (28, 21), (3, 15)] { p[x, y] = 0xFFFFFF }
    p.rect(11, 29, 12, 1, 0x00004A)
    // the staff: a thin line the height of the tower with a gold ball and a gold foot
    let left = emblemStyle == "b"
    for y in 10...28 {
        let k = (y - 10) * 4 / 18
        p[left ? 9 - k : 23 + k, y] = 0xB98A52
    }
    p.rect(left ? 9 : 22, 9, 2, 2, gold)
    p[left ? 5 : 27, 28] = gold
    p.rect(12, 11, 7, 18, towerFront)
    for (x, t, height) in [(19, 11, 18), (20, 10, 18), (21, 10, 17)] { p.rect(x, t, 1, height, c.side) }
    p.rect(19, 12, 2, 14, c.front)
    p.rect(12, 10, 9, 1, c.top)
    p.rect(14, 9, 8, 1, c.top)
    // the robe falls from the left shoulder and narrows toward the hem
    for y in 11..<29 { p.rect(12, y, 6 - (y - 11) * 4 / 17, 1, c.front) }
    p.rect(13, 12, 1, 12, c.side)
    p.rect(18, 11, 1, 17, c.front)
    p[17, 11] = gold
    p[16, 12] = gold
    // bays down the opening, the top light ringed in gold
    for y in [17, 20, 23] {
        p.rect(14, y, 4, 2, bay)
        p[14, y] = led
    }
    // a tall top hat tipped to the left
    p.rect(10, 8, 13, 1, hatBrim)
    p.rect(left ? 20 : 9, 7, 3, 1, hatBrim)
    for (y, shift) in [(1, -2), (2, -2), (3, -1), (4, -1), (5, 0), (6, 0)] {
        p.rect(12 + (left ? -shift : shift), y, 8, 1, y == 1 ? 0x383849 : hatBlack)
    }
    p.rect(11, 6, 9, 1, gold)
    for y in 2..<6 { p[(left ? 19 : 11) + (y < 3 ? -2 : (y < 5 ? -1 : 0)) * (left ? -1 : 1), y] = 0xB8C2F0 }
    return p.image()
}

func pixelIcon16() -> CGContext {
    let c = burgundy
    var p = Pix(16, 16)
    tileBase(&p)
    p.rect(5, 14, 7, 1, 0x00004A)
    // the staff is left out at this size: it only muddies the silhouette
    let left = emblemStyle == "b"
    p.rect(5, 7, 6, 7, towerFront)
    p.rect(11, 7, 2, 7, c.side)
    p.rect(5, 6, 8, 1, c.top)
    for y in 7..<14 { p.rect(5, y, 4 - (y - 7) / 3, 1, c.front) }
    p.rect(10, 7, 1, 7, c.front)
    p[9, 7] = gold
    for y in [9, 11] {
        p.rect(8, y, 2, 1, bay)
        p[8, y] = led
    }
    // the hat, tipped to the left
    p.rect(3, 5, 9, 1, hatBrim)
    p.rect(left ? 9 : 4, 4, 2, 1, hatBrim)
    p.rect(left ? 5 : 5, 1, 6, 1, 0x383849)
    p.rect(left ? 6 : 4, 2, 6, 1, hatBlack)
    p.rect(left ? 6 : 4, 3, 6, 1, gold)
    p.rect(left ? 5 : 6, 4, 4, 1, hatBlack)
    p[left ? 6 : 4, 2] = 0xB8C2F0
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

emblemStyle = ProcessInfo.processInfo.environment["EMBLEM"] ?? "b"
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
print("wrote the logo and icons to \(out)")
