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

/// which emblem to draw: "purple" and "crimson" are the tower in a robe, "crest" is the purple robe in a heraldic shield.
var emblemStyle = "crest"
/// draw only the robed tower, without the shield; the small icons and the app tile use it.
var plainEmblem = false
/// how dark the little ground shadow under the tower is; lighter on a white page.
var shadowAlpha: CGFloat = 0.28

let gold: UInt32 = 0xD4AF37
let goldDark: UInt32 = 0xA9841A

struct Robe {
    let front: UInt32
    let side: UInt32
    let top: UInt32
}

let purple = Robe(front: 0x5E2FA3, side: 0x3F1D70, top: 0x7440BE)
let crimson = Robe(front: 0xA81E38, side: 0x7C1228, top: 0xBF2A47)

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
    if emblemStyle == "crest" && !plainEmblem {
        shield(ctx)
        ctx.translateBy(x: 500, y: 520)
        ctx.scaleBy(x: 0.84, y: 0.84)
        ctx.translateBy(x: -500, y: -545)
        dress(ctx, shadow: false)
    } else {
        ctx.translateBy(x: 500, y: 560)
        ctx.scaleBy(x: 1.08, y: 1.08)
        ctx.translateBy(x: -500, y: -560)
        dress(ctx, shadow: true)
    }
    ctx.restoreGState()
}

/// the heraldic frame: a flat shield with a thin gold border, an inner rule and a small flourish.
func shield(_ ctx: CGContext) {
    let outline = CGMutablePath()
    outline.move(to: CGPoint(x: 170, y: 120))
    outline.addLine(to: CGPoint(x: 830, y: 120))
    outline.addLine(to: CGPoint(x: 830, y: 620))
    outline.addCurve(to: CGPoint(x: 500, y: 985), control1: CGPoint(x: 830, y: 800), control2: CGPoint(x: 660, y: 890))
    outline.addCurve(to: CGPoint(x: 170, y: 620), control1: CGPoint(x: 340, y: 890), control2: CGPoint(x: 170, y: 800))
    outline.closeSubpath()
    flat(ctx, outline, 0x070C55)
    stroke(ctx, outline, gold, 20)
    var inset = CGAffineTransform(translationX: 500, y: 540).scaledBy(x: 0.92, y: 0.92).translatedBy(x: -500, y: -540)
    stroke(ctx, outline.copy(using: &inset)!, goldDark, 6)
    // a tiny gold roundel with an L on the crown of the shield, between two short rules
    flat(ctx, circle(500, 96, 44), gold)
    flat(ctx, circle(500, 96, 34), 0x070C55)
    let mono = place(wordPath("L", size: 52), 485, 114)
    flat(ctx, mono, gold)
    stroke(ctx, segment(380, 96, 444, 96), gold, 6)
    stroke(ctx, segment(556, 96, 620, 96), gold, 6)
}

func ground(_ ctx: CGContext) {
    ctx.setFillColor(rgb(0x000000, shadowAlpha))
    ctx.fillEllipse(in: CGRect(x: 250, y: 880, width: 560, height: 70))
}

/// the front bays: dark slots with a light and a short label bar.
func bays(_ ctx: CGContext, from top: CGFloat, step: CGFloat, count: Int = 4) {
    for i in 0..<count {
        let y = top + CGFloat(i) * step
        flat(ctx, box(350, y, 160, 54), 0x1F2A6B)
        flat(ctx, circle(374, y + 27, 9), 0x3FD07A)
        flat(ctx, box(398, y + 22, 96, 10), 0x39458F)
    }
    for i in 0..<3 {
        flat(ctx, box(350, 770 + CGFloat(i) * 30, 160, 9), 0xA9B0C8)
    }
}

/// the tower in a robe: the mantle over the top and down both sides with gold edge trim, a white
/// ermine collar with a few dark spots, and a gold clasp. the bays show between the robe edges.
func robed(_ ctx: CGContext, _ robe: Robe, shadow: Bool) {
    if shadow { ground(ctx) }
    flat(ctx, poly([(300, 330), (560, 330), (560, 900), (300, 900)]), 0xD7DBE8)
    flat(ctx, poly([(560, 330), (700, 268), (700, 838), (560, 900)]), robe.side)
    flat(ctx, poly([(300, 330), (440, 268), (700, 268), (560, 330)]), robe.top)
    // the mantle edges down the front, trimmed in gold, and folds on the side
    flat(ctx, box(300, 330, 50, 570), robe.front)
    flat(ctx, box(510, 330, 50, 570), robe.front)
    flat(ctx, box(346, 404, 6, 496), gold)
    flat(ctx, box(508, 404, 6, 496), gold)
    for x in [600, 640, 676] as [CGFloat] {
        let top = 330 - (x - 560) * (62.0 / 140.0)
        stroke(ctx, segment(x, top + 70, x, top + 520), robe.top, 7, alpha: 0.55)
    }
    bays(ctx, from: 428, step: 84)
    // the ermine collar across the front and round the side, edged in gold
    flat(ctx, poly([(300, 330), (560, 330), (560, 404), (300, 404)]), 0xF6F6FA)
    flat(ctx, poly([(560, 330), (700, 268), (700, 338), (560, 404)]), 0xD6D6E6)
    flat(ctx, box(300, 400, 260, 8), gold)
    flat(ctx, poly([(560, 400), (700, 334), (700, 342), (560, 408)]), gold)
    for x in [334, 392, 450, 508] as [CGFloat] {
        flat(ctx, ellipse(x, 366, 9, 15), 0x1A1A26)
        flat(ctx, box(x - 2, 378, 4, 14), 0x1A1A26)
    }
    for (x, y) in [(594, 358), (640, 336)] as [(CGFloat, CGFloat)] {
        flat(ctx, ellipse(x, y, 8, 12), 0x1A1A26)
    }
    // the gold clasp
    flat(ctx, circle(430, 404, 24), gold)
    flat(ctx, circle(430, 404, 11), goldDark)
    // one thin lit edge on the top left
    stroke(ctx, segment(300, 330, 440, 268), 0xFFFFFF, 6, alpha: 0.85)
}

let burgundy = Robe(front: 0x7A1630, side: 0x570F22, top: 0x9A2744)
let midnight = Robe(front: 0x2B46B0, side: 0x1A2D78, top: 0x4863CC)

/// the tall black top hat with a thin gold band, lit on its left edge.
func topHat(_ ctx: CGContext) {
    flat(ctx, ellipse(498, 300, 138, 30), 0x1A1A26)
    flat(ctx, box(424, 150, 148, 150), 0x262634)
    flat(ctx, ellipse(498, 150, 74, 17), 0x383849)
    flat(ctx, box(424, 244, 148, 20), gold)
    stroke(ctx, segment(424, 156, 424, 300), 0xFFFFFF, 6, alpha: 0.7)
}

/// a thin gold monocle round a light on the front, with a short chain and a bead.
func monocle(_ ctx: CGContext, _ x: CGFloat, _ y: CGFloat, chain length: CGFloat = 1) {
    stroke(ctx, CGPath(ellipseIn: CGRect(x: x - 46, y: y - 46, width: 92, height: 92), transform: nil), gold, 11)
    let chain = CGMutablePath()
    chain.move(to: CGPoint(x: x + 33, y: y + 33))
    chain.addQuadCurve(to: CGPoint(x: x + 33 + 37 * length, y: y + 33 + 67 * length), control: CGPoint(x: x + 33 + 49 * length, y: y + 33 + 19 * length))
    stroke(ctx, chain, gold, 6)
    flat(ctx, circle(x + 33 + 37 * length, y + 33 + 72 * length, 9), gold)
}

/// a gold watch chain draped across the front, with the watch hanging at the lowest point.
func watchChain(_ ctx: CGContext) {
    let chain = CGMutablePath()
    chain.move(to: CGPoint(x: 362, y: 556))
    chain.addQuadCurve(to: CGPoint(x: 500, y: 556), control: CGPoint(x: 430, y: 672))
    stroke(ctx, chain, gold, 6)
    flat(ctx, circle(430, 618, 20), gold)
    flat(ctx, circle(430, 618, 9), goldDark)
    stroke(ctx, segment(430, 600, 430, 610), gold, 5)
}

/// a walking cane leaning beside the tower, with a gold ball handle.
func cane(_ ctx: CGContext) {
    stroke(ctx, segment(786, 420, 748, 930), 0x22222E, 15)
    stroke(ctx, segment(786, 420, 748, 930), 0xFFFFFF, 3, alpha: 0.35)
    flat(ctx, circle(788, 394, 30), gold)
    flat(ctx, circle(780, 386, 9), 0xF4DC86)
}

/// two flat, deadpan eyes on a recessed plate on the upper front, one eyebrow a little raised.
func eyes(_ ctx: CGContext) {
    flat(ctx, CGPath(roundedRect: CGRect(x: 368, y: 384, width: 124, height: 76), cornerWidth: 14, cornerHeight: 14, transform: nil), 0xB3B9CF)
    for x in [396, 464] as [CGFloat] {
        flat(ctx, circle(x, 422, 25), 0xFFFFFF)
        flat(ctx, circle(x + 6, 426, 11), 0x1A1A26)
    }
    stroke(ctx, segment(438, 372, 492, 358), 0x2A2A3C, 8)
}

/// the tower in a loose velvet smoking jacket: it hangs open down the middle with a shawl collar
/// piped in gold, uneven hems, one side slipping off the shoulder, and a gold cord with tassels.
func jacketed(_ ctx: CGContext, _ jacket: Robe, shadow: Bool) {
    if shadow { ground(ctx) }
    flat(ctx, poly([(300, 330), (560, 330), (560, 900), (300, 900)]), 0xD7DBE8)
    flat(ctx, poly([(560, 330), (700, 268), (700, 838), (560, 900)]), jacket.side)
    flat(ctx, poly([(300, 330), (440, 268), (700, 268), (560, 330)]), jacket.top)
    for x in [600, 640, 676] as [CGFloat] {
        let top = 330 - (x - 560) * (62.0 / 140.0)
        stroke(ctx, segment(x, top + 340, x, top + 560), jacket.front, 7, alpha: 0.45)
    }
    bays(ctx, from: 520, step: 66, count: 3)
    // the right side has slipped off the shoulder and hangs over the side face
    flat(ctx, poly([(560, 338), (652, 304), (672, 410), (640, 520), (560, 566)]), jacket.front)
    stroke(ctx, segment(560, 566, 640, 520), gold, 5)
    stroke(ctx, segment(640, 520, 672, 410), gold, 5)
    // the jacket hangs open: a panel down each side of the front with an uneven hem
    flat(ctx, poly([(300, 330), (352, 330), (358, 520), (374, 700), (378, 884), (322, 898), (300, 888)]), jacket.front)
    flat(ctx, poly([(560, 336), (508, 330), (504, 520), (490, 700), (486, 862), (540, 876), (560, 866)]), jacket.front)
    stroke(ctx, segment(352, 330, 358, 520), gold, 6)
    stroke(ctx, segment(358, 520, 378, 884), gold, 6)
    stroke(ctx, segment(508, 330, 504, 520), gold, 6)
    stroke(ctx, segment(504, 520, 486, 862), gold, 6)
    stroke(ctx, segment(300, 890, 378, 886), gold, 6)
    stroke(ctx, segment(486, 864, 560, 868), gold, 6)
    // the shawl collar hangs open at the top, piped in gold
    flat(ctx, poly([(300, 330), (374, 330), (360, 498), (300, 520)]), jacket.top)
    flat(ctx, poly([(560, 330), (486, 330), (500, 498), (560, 520)]), jacket.top)
    stroke(ctx, segment(374, 330, 360, 498), gold, 7)
    stroke(ctx, segment(486, 330, 500, 498), gold, 7)
    stroke(ctx, segment(360, 498, 300, 520), gold, 6)
    stroke(ctx, segment(500, 498, 560, 520), gold, 6)
    eyes(ctx)
    // a loosely knotted gold cord with tassels, hanging to one side
    let cord = CGMutablePath()
    cord.move(to: CGPoint(x: 376, y: 704))
    cord.addQuadCurve(to: CGPoint(x: 490, y: 710), control: CGPoint(x: 430, y: 734))
    stroke(ctx, cord, gold, 9)
    flat(ctx, circle(470, 718, 16), gold)
    flat(ctx, circle(470, 718, 6), goldDark)
    stroke(ctx, segment(470, 730, 482, 804), gold, 7)
    stroke(ctx, segment(470, 730, 450, 794), gold, 7)
    flat(ctx, poly([(476, 800), (490, 802), (496, 834), (472, 832)]), gold)
    flat(ctx, poly([(443, 790), (457, 792), (460, 824), (438, 820)]), gold)
    watchChain(ctx)
    monocle(ctx, 396, 422, chain: 0.5)
    stroke(ctx, segment(300, 330, 440, 268), 0xFFFFFF, 6, alpha: 0.85)
    topHat(ctx)
    cane(ctx)
}

/// the tower as drawn for the current style.
func dress(_ ctx: CGContext, shadow: Bool) {
    switch emblemStyle {
    case "purple":
        robed(ctx, purple, shadow: shadow)
        monocle(ctx, 374, 539)
    case "crimson":
        jacketed(ctx, burgundy, shadow: shadow)
    default:
        jacketed(ctx, midnight, shadow: shadow)
    }
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
    ctx.setFillColor(rgb(light ? 0x6A5210 : (emblemStyle == "purple" ? 0xA8BCFF : 0xE6D59A), 0.95))
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
    plainEmblem = true
    emblem(ctx, box.midX - e / 2, box.minY + box.height * 0.1, e, thick: thick)
    plainEmblem = false
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
let facePlate: UInt32 = 0xB3B9CF
let bay: UInt32 = 0x1F2A6B
let led: UInt32 = 0x3FD07A
let ermine: UInt32 = 0xF6F6FA
let spot: UInt32 = 0x1A1A26
let hatBlack: UInt32 = 0x262634
let hatBrim: UInt32 = 0x1A1A26
let pupil: UInt32 = 0x1A1A26

/// what the current style wears: the cloth, whether it is an ermine robe, and whether it has a hat and eyes.
func look() -> (cloth: Robe, ermine: Bool, hat: Bool) {
    switch emblemStyle {
    case "purple": return (purple, true, false)
    case "crimson": return (burgundy, false, true)
    default: return (midnight, false, true)
    }
}

/// the 32 pixel tower: the cloth colour down both edges with gold trim, deadpan eyes, a monocle and a hat.
func pixelIcon32() -> CGContext {
    let w = look()
    var p = Pix(32, 32)
    tileBase(&p)
    for (x, y) in [(4, 6), (27, 9), (5, 24), (28, 21), (3, 15)] { p[x, y] = 0xFFFFFF }
    p.rect(9, 28, 15, 1, 0x00004A)
    p.rect(9, 11, 11, 17, towerFront)
    for (x, t, height) in [(20, 10, 18), (21, 9, 18), (22, 9, 17), (23, 8, 17)] { p.rect(x, t, 1, height, w.cloth.side) }
    for (x, y, width) in [(9, 10, 12), (10, 9, 13), (12, 8, 12)] { p.rect(x, y, width, 1, w.cloth.top) }
    p.rect(9, 11, 2, 17, w.cloth.front)
    p.rect(18, 11, 2, 17, w.cloth.front)
    if w.ermine {
        p.rect(11, 14, 1, 14, gold)
        p.rect(18, 14, 1, 14, gold)
        for y in [18, 21, 24] {
            p.rect(12, y, 6, 2, bay)
            p[12, y] = led
        }
        p.rect(9, 11, 11, 3, ermine)
        p.rect(20, 10, 4, 3, 0xD6D6E6)
        for x in [11, 14, 17] { p[x, 12] = spot }
        p.rect(9, 13, 11, 1, gold)
        p.rect(14, 12, 2, 3, gold)
        p[9, 10] = 0xFFFFFF
        return p.image()
    }
    // the collar hangs open: a short band of cloth at each top corner with a gold edge
    p.rect(9, 11, 3, 1, w.cloth.top)
    p.rect(9, 12, 2, 1, w.cloth.top)
    p.rect(17, 11, 3, 1, w.cloth.top)
    p.rect(18, 12, 2, 1, w.cloth.top)
    p.rect(12, 11, 1, 3, gold)
    p.rect(17, 11, 1, 3, gold)
    // a recessed plate with two deadpan eyes: white with a dark pupil, both looking a little right
    p.rect(12, 12, 6, 4, facePlate)
    for x in [12, 15] {
        p.rect(x, 13, 2, 2, 0xFFFFFF)
        p[x + 1, 14] = pupil
    }
    // the monocle round the left eye
    for (x, y) in [(11, 12), (12, 12), (13, 12), (14, 12), (11, 13), (14, 13), (11, 14), (14, 14), (11, 15), (12, 15), (13, 15), (14, 15)] {
        p[x, y] = gold
    }
    for y in [18, 21, 24] {
        p.rect(12, y, 6, 2, bay)
        p[12, y] = led
    }
    // a gold cord with a tassel hanging to one side
    p.rect(12, 26, 6, 1, gold)
    p[17, 27] = gold
    if w.hat {
        p.rect(10, 7, 12, 1, hatBrim)
        p.rect(12, 2, 8, 5, hatBlack)
        p.rect(12, 2, 8, 1, 0x383849)
        p.rect(12, 5, 8, 1, gold)
        for y in 2..<7 { p[12, y] = 0xB8C2F0 }
    }
    return p.image()
}

func pixelIcon16() -> CGContext {
    let w = look()
    var p = Pix(16, 16)
    tileBase(&p)
    p.rect(5, 14, 7, 1, 0x00004A)
    p.rect(5, 5, 5, 9, towerFront)
    p.rect(10, 5, 2, 9, w.cloth.side)
    p.rect(5, 4, 7, 1, w.cloth.top)
    p.rect(5, 5, 1, 9, w.cloth.front)
    p.rect(9, 5, 1, 9, w.cloth.front)
    if w.ermine {
        p.rect(5, 5, 5, 2, ermine)
        p[7, 5] = spot
        p.rect(5, 7, 5, 1, gold)
        p[7, 6] = gold
        for y in [8, 10, 12] {
            p.rect(6, y, 3, 1, bay)
            p[6, y] = led
        }
        for (x, y) in [(5, 9), (6, 9), (7, 9), (5, 10), (7, 10), (5, 11), (6, 11), (7, 11)] { p[x, y] = gold }
        return p.image()
    }
    // two dark eye pixels on the light face, the hat above, the jacket as a colour block with gold trim
    p[6, 6] = pupil
    p[8, 6] = pupil
    p[7, 5] = w.cloth.top
    for y in [8, 10, 12] {
        p.rect(6, y, 3, 1, bay)
        p[6, y] = led
    }
    p[5, 8] = gold
    p[9, 8] = gold
    p.rect(5, 3, 7, 1, hatBrim)
    p.rect(6, 1, 5, 2, hatBlack)
    p[6, 1] = 0xB8C2F0
    p[6, 2] = 0xB8C2F0
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

emblemStyle = ProcessInfo.processInfo.environment["EMBLEM"] ?? "crest"
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
    for (style, name) in [("crimson", "g-eyes-crimson"), ("crest", "g-eyes-crest")] {
        emblemStyle = style
        save(banner(1, light: false), "\(dir)/\(name).png")
    }
}
print("wrote the logo and icons to \(out)")
