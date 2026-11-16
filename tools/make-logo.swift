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

// MARK: the emblem: flat, quiet and geometric, in the spirit of nt and backoffice branding

/// which emblem to draw: 1 is a ring of network nodes, 2 is a server tower.
var emblemStyle = 2
/// how dark the little ground shadow under the tower is; lighter on a white page.
var shadowAlpha: CGFloat = 0.28

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

/// a small flat crown whose base is centred on (cx, base) and whose width is `w`.
func crown(_ ctx: CGContext, _ cx: CGFloat, _ base: CGFloat, _ w: CGFloat) {
    let h = w * 0.62
    let l = cx - w / 2
    let r = cx + w / 2
    let body = poly([
        (l, base), (l - w * 0.02, base - h * 0.78), (cx - w * 0.2, base - h * 0.42), (cx, base - h),
        (cx + w * 0.2, base - h * 0.42), (r + w * 0.02, base - h * 0.78), (r, base),
    ])
    flat(ctx, body, 0xF0B800)
    flat(ctx, CGPath(rect: CGRect(x: l, y: base - h * 0.2, width: w, height: h * 0.2), transform: nil), 0xC48F00)
}

/// draws into a 1000 x 1000 box whose top left is (ox, oy), scaled to `size` pixels.
func emblem(_ ctx: CGContext, _ ox: CGFloat, _ oy: CGFloat, _ size: CGFloat, thick: CGFloat = 1) {
    ctx.saveGState()
    ctx.translateBy(x: ox, y: oy)
    ctx.scaleBy(x: size / 1000, y: size / 1000)
    if emblemStyle == 1 { ring(ctx) } else { tower(ctx) }
    ctx.restoreGState()
}

/// concept 1: a ring of three connected nodes under a small crown.
func ring(_ ctx: CGContext) {
    let cx: CGFloat = 500
    let cy: CGFloat = 590
    let outer: CGFloat = 340
    let inner: CGFloat = 276
    let band = CGMutablePath()
    band.addEllipse(in: CGRect(x: cx - outer, y: cy - outer, width: 2 * outer, height: 2 * outer))
    band.addEllipse(in: CGRect(x: cx - inner, y: cy - inner, width: 2 * inner, height: 2 * inner))
    ctx.saveGState()
    ctx.addPath(band)
    ctx.clip(using: .evenOdd)
    ctx.setFillColor(rgb(0x2F4BD0))
    ctx.fill(CGRect(x: 0, y: 0, width: 1000, height: 1000))
    // the half in shade: flat, cut by a straight diagonal
    ctx.setFillColor(rgb(0x1B2D98))
    ctx.move(to: CGPoint(x: 1000, y: cy - 500))
    ctx.addLine(to: CGPoint(x: 1000, y: 1000))
    ctx.addLine(to: CGPoint(x: 0, y: 1000))
    ctx.addLine(to: CGPoint(x: 0, y: cy + 500))
    ctx.closePath()
    ctx.fillPath()
    ctx.restoreGState()
    // one thin lit edge on the top left of the ring
    ctx.saveGState()
    ctx.setStrokeColor(rgb(0x8EA4FF))
    ctx.setLineWidth(7)
    ctx.setLineCap(.butt)
    for i in 0...40 {
        let a = CGFloat.pi * (1.02 + 0.46 * CGFloat(i) / 40)
        let pt = CGPoint(x: cx + (outer - 4) * cos(a), y: cy + (outer - 4) * sin(a))
        if i == 0 { ctx.move(to: pt) } else { ctx.addLine(to: pt) }
    }
    ctx.strokePath()
    ctx.restoreGState()
    // the three nodes and the links between them
    let r: CGFloat = 178
    let pts = (0..<3).map { i -> (CGFloat, CGFloat) in
        let a = -CGFloat.pi / 2 + CGFloat(i) * 2 * .pi / 3
        return (cx + r * cos(a), cy + r * sin(a) + 14)
    }
    let tri = poly(pts)
    ctx.addPath(tri)
    ctx.setStrokeColor(rgb(0xE6EBFF))
    ctx.setLineWidth(15)
    ctx.setLineJoin(.round)
    ctx.strokePath()
    for (x, y) in pts {
        flat(ctx, circle(x, y, 50), 0xFFFFFF)
        flat(ctx, circle(x, y, 22), 0x2F4BD0)
    }
    crown(ctx, cx, cy - outer + 14, 230)
}

/// concept 2: a server tower, drawn as three flat faces, with a small crown on top.
func tower(_ ctx: CGContext) {
    ctx.translateBy(x: 500, y: 540)
    ctx.scaleBy(x: 1.12, y: 1.12)
    ctx.translateBy(x: -500, y: -540)
    // a little ground shadow, flat and faint
    ctx.setFillColor(rgb(0x000000, shadowAlpha))
    ctx.fillEllipse(in: CGRect(x: 250, y: 880, width: 560, height: 70))
    // the three faces
    flat(ctx, poly([(300, 330), (560, 330), (560, 900), (300, 900)]), 0xD7DBE8)
    flat(ctx, poly([(560, 330), (700, 268), (700, 838), (560, 900)]), 0x8791B3)
    flat(ctx, poly([(300, 330), (440, 268), (700, 268), (560, 330)]), 0xF0F2FA)
    // the drive bays on the front, and a grille at the foot
    for i in 0..<4 {
        let y = 384 + CGFloat(i) * 92
        flat(ctx, CGPath(rect: CGRect(x: 338, y: y, width: 184, height: 54), transform: nil), 0x1F2A6B)
        flat(ctx, circle(362, y + 27, 9), 0x3FD07A)
        flat(ctx, CGPath(rect: CGRect(x: 386, y: y + 22, width: 116, height: 10), transform: nil), 0x39458F)
    }
    for i in 0..<3 {
        flat(ctx, CGPath(rect: CGRect(x: 338, y: 770 + CGFloat(i) * 30, width: 184, height: 9), transform: nil), 0xA9B0C8)
    }
    // vents on the side face
    for i in 0..<6 {
        let x = 580 + CGFloat(i) * 20
        let top = 330 - (x - 560) * (62.0 / 140.0) + 40
        ctx.setStrokeColor(rgb(0x6B769C))
        ctx.setLineWidth(7)
        ctx.move(to: CGPoint(x: x, y: top))
        ctx.addLine(to: CGPoint(x: x, y: top + 380))
        ctx.strokePath()
    }
    // one thin lit edge on the top left
    ctx.setStrokeColor(rgb(0xFFFFFF, 0.85))
    ctx.setLineWidth(6)
    ctx.setLineCap(.butt)
    ctx.move(to: CGPoint(x: 300, y: 330))
    ctx.addLine(to: CGPoint(x: 440, y: 268))
    ctx.strokePath()
    crown(ctx, 478, 306, 236)
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

/// the wordmark: a clean title with a faint lit edge and a soft, low shadow.
func wordmark(_ ctx: CGContext, _ text: CGPath, light: Bool, size: CGFloat, finish: Int = 0) {
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
    var face = light
        ? gradient([(0, 0x4A5CE8), (0.5, 0x0A1A8C), (1, 0x00004A)])
        : gradient([(0, 0xFFFFFF), (0.45, 0xD2E0FF), (1, 0x6F8FE0)])
    if !light && finish == 1 { face = gradient([(0, 0xFFF6B0), (0.45, 0xF4BE12), (1, 0x8A5300)]) }
    fill(ctx, text, face, CGPoint(x: 0, y: box.minY), CGPoint(x: 0, y: box.maxY))
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
    shadowAlpha = light ? 0.10 : 0.28
    emblem(ctx, 52 * scale, 30 * scale, 340 * scale)
    shadowAlpha = 0.28
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
let towerSide: UInt32 = 0x8791B3
let towerTop: UInt32 = 0xF0F2FA
let bay: UInt32 = 0x1F2A6B
let led: UInt32 = 0x3FD07A
let goldFlat: UInt32 = 0xF0B800
let goldBand: UInt32 = 0xC48F00

func pixelIcon32() -> CGContext {
    var p = Pix(32, 32)
    tileBase(&p)
    for (x, y) in [(4, 6), (27, 9), (5, 24), (28, 21), (3, 15)] { p[x, y] = 0xFFFFFF }
    // a faint ground shadow
    p.rect(9, 28, 15, 1, 0x00004A)
    // the tower: front, side and top faces
    p.rect(9, 11, 11, 17, towerFront)
    for (x, top, height) in [(20, 10, 18), (21, 9, 18), (22, 9, 17), (23, 8, 17)] {
        p.rect(x, top, 1, height, towerSide)
    }
    for (x, y, width) in [(9, 10, 12), (10, 9, 13), (12, 8, 12)] { p.rect(x, y, width, 1, towerTop) }
    // three drive bays with a light each, and a grille
    for y in [13, 17, 21] {
        p.rect(11, y, 8, 2, bay)
        p[11, y] = led
        p.rect(13, y + 1, 5, 1, 0x39458F)
    }
    p.rect(11, 25, 8, 1, 0xA9B0C8)
    p.rect(11, 26, 8, 1, 0xA9B0C8)
    p.rect(9, 10, 11, 1, 0xFFFFFF)
    // the crown on top
    for x in [11, 16, 21] { p[x, 3] = goldFlat }
    for x in [11, 12, 15, 16, 17, 20, 21] { p[x, 4] = goldFlat }
    for x in [11, 12, 13, 15, 16, 17, 19, 20, 21] { p[x, 5] = goldFlat }
    p.rect(11, 6, 11, 1, goldFlat)
    p.rect(11, 7, 11, 1, goldBand)
    return p.image()
}

func pixelIcon16() -> CGContext {
    var p = Pix(16, 16)
    tileBase(&p)
    p.rect(5, 14, 7, 1, 0x00004A)
    p.rect(5, 5, 5, 9, towerFront)
    p.rect(10, 5, 2, 9, towerSide)
    p.rect(5, 4, 7, 1, towerTop)
    for y in [6, 8, 10] {
        p.rect(6, y, 3, 1, bay)
        p[6, y] = led
    }
    p.rect(6, 12, 3, 1, 0xA9B0C8)
    for x in [6, 8, 10] { p[x, 1] = goldFlat }
    p.rect(6, 2, 5, 1, goldFlat)
    p.rect(6, 3, 5, 1, goldBand)
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

emblemStyle = Int(ProcessInfo.processInfo.environment["EMBLEM"] ?? "2") ?? 2
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
    emblemStyle = 1
    save(banner(1, light: false), "\(dir)/d-flat-1.png")
    emblemStyle = 2
    save(banner(1, light: false), "\(dir)/d-flat-2.png")
}
print("wrote the logo and icons to \(out)")
