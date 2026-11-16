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

/// which emblem to draw: "crest" is the midnight jacket in a heraldic shield, "crimson" is the burgundy one alone.
var emblemStyle = "crest"
/// draw only the figure, without the shield; the small icons and the app tile use it.
var plainEmblem = false
/// how dark the little ground shadow under the tower is; lighter on a white page.
var shadowAlpha: CGFloat = 0.28

let gold: UInt32 = 0xD4AF37
let goldDark: UInt32 = 0xA9841A
let facePlate: UInt32 = 0xB3B9CF
let ink: UInt32 = 0x23232F

struct Cloth {
    let front: UInt32
    let side: UInt32
    let top: UInt32
}

let burgundy = Cloth(front: 0x7A1630, side: 0x570F22, top: 0x9A2744)
let midnight = Cloth(front: 0x2B46B0, side: 0x1A2D78, top: 0x4863CC)

func cloth() -> Cloth { emblemStyle == "crimson" ? burgundy : midnight }

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
        ctx.translateBy(x: 500, y: 526)
        ctx.scaleBy(x: 0.92, y: 0.92)
        ctx.translateBy(x: -498, y: -548)
        gentleman(ctx, cloth(), shadow: false)
    } else {
        ctx.translateBy(x: 500, y: 536)
        ctx.scaleBy(x: 1.17, y: 1.17)
        ctx.translateBy(x: -498, y: -548)
        gentleman(ctx, cloth(), shadow: true)
    }
    ctx.restoreGState()
}

/// the heraldic frame: a flat shield with a thin gold border, an inner rule and a small monogram.
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
    flat(ctx, circle(500, 96, 44), gold)
    flat(ctx, circle(500, 96, 34), 0x070C55)
    flat(ctx, place(wordPath("L", size: 52), 485, 114), gold)
    stroke(ctx, segment(380, 96, 444, 96), gold, 6)
    stroke(ctx, segment(556, 96, 620, 96), gold, 6)
}

/// calm eyes: small, level, close together and half lidded, each with a thin brow above.
func eyes(_ ctx: CGContext) {
    flat(ctx, CGPath(roundedRect: CGRect(x: 394, y: 396, width: 82, height: 48), cornerWidth: 10, cornerHeight: 10, transform: nil), facePlate)
    for x in [417, 453] as [CGFloat] {
        flat(ctx, circle(x, 421, 15), 0xFFFFFF)
        flat(ctx, circle(x, 424, 7), ink)
        // the upper lid: plate colour over the top of the eye, with a fine dark edge
        ctx.saveGState()
        ctx.addPath(circle(x, 421, 15))
        ctx.clip()
        flat(ctx, box(x - 16, 404, 32, 12), facePlate)
        stroke(ctx, segment(x - 16, 416, x + 16, 416), ink, 3)
        ctx.restoreGState()
        stroke(ctx, segment(x - 17, 390, x + 17, 390), ink, 4)
    }
}

/// a crisp gold monocle over the left eye, with a short neat chain.
func monocle(_ ctx: CGContext) {
    stroke(ctx, CGPath(ellipseIn: CGRect(x: 417 - 25, y: 421 - 25, width: 50, height: 50), transform: nil), gold, 6)
    let chain = CGMutablePath()
    chain.move(to: CGPoint(x: 435, y: 439))
    chain.addQuadCurve(to: CGPoint(x: 446, y: 482), control: CGPoint(x: 449, y: 454))
    stroke(ctx, chain, gold, 3)
    flat(ctx, circle(446, 485, 5), gold)
}

/// a watch chain hung neatly between the panels, with the watch at its lowest point.
func watchChain(_ ctx: CGContext) {
    let chain = CGMutablePath()
    chain.move(to: CGPoint(x: 383, y: 508))
    chain.addQuadCurve(to: CGPoint(x: 487, y: 508), control: CGPoint(x: 435, y: 580))
    stroke(ctx, chain, gold, 3)
    flat(ctx, circle(435, 544, 12), gold)
    flat(ctx, circle(435, 544, 5), goldDark)
}

/// a tall top hat: a crown about two thirds as wide as the top of the tower, a wide brim that
/// overhangs both sides, a satin band and a lit left edge.
func topHat(_ ctx: CGContext) {
    flat(ctx, ellipse(470, 340, 134, 24), 0x1A1A26)
    flat(ctx, poly([(393, 342), (399, 148), (541, 148), (547, 342)]), 0x262634)
    flat(ctx, ellipse(470, 148, 71, 13), 0x383849)
    flat(ctx, poly([(394, 316), (546, 316), (547, 342), (393, 342)]), gold)
    stroke(ctx, segment(398, 156, 394, 316), 0xFFFFFF, 5, alpha: 0.6)
}

/// a walking cane leaning beside the tower, with a gold ball handle.
func cane(_ ctx: CGContext) {
    stroke(ctx, segment(642, 452, 620, 930), ink, 11)
    stroke(ctx, segment(642, 452, 620, 930), 0xFFFFFF, 2, alpha: 0.3)
    flat(ctx, circle(643, 430, 22), gold)
    flat(ctx, circle(637, 423, 6), 0xF4DC86)
}

/// a slender tower in a neat velvet jacket: an open shawl collar piped in gold, the bays down the
/// middle, a gold cord at the waist, calm eyes with a monocle, a watch chain and a tall top hat.
func gentleman(_ ctx: CGContext, _ jacket: Cloth, shadow: Bool) {
    if shadow {
        ctx.setFillColor(rgb(0x000000, shadowAlpha))
        ctx.fillEllipse(in: CGRect(x: 345, y: 902, width: 290, height: 44))
    }
    flat(ctx, poly([(355, 360), (515, 360), (515, 920), (355, 920)]), 0xD7DBE8)
    flat(ctx, poly([(515, 360), (585, 324), (585, 884), (515, 920)]), jacket.side)
    flat(ctx, poly([(355, 360), (425, 324), (585, 324), (515, 360)]), jacket.top)
    for x in [538, 556, 572] as [CGFloat] {
        let top = 360 - (x - 515) * (36.0 / 70.0)
        stroke(ctx, segment(x, top + 70, x, top + 470), jacket.front, 6, alpha: 0.4)
    }
    // the bays down the middle and a small grille at the foot
    for i in 0..<3 {
        let y = 496 + CGFloat(i) * 54
        flat(ctx, box(396, y, 78, 38), 0x1F2A6B)
        flat(ctx, circle(409, y + 19, 6.5), 0x3FD07A)
        flat(ctx, box(422, y + 15, 42, 8), 0x39458F)
    }
    for i in 0..<3 {
        flat(ctx, box(396, 780 + CGFloat(i) * 20, 78, 6), 0xA9B0C8)
    }
    // the jacket: a panel down each side of the front with a level hem, piped in gold
    flat(ctx, poly([(355, 360), (378, 360), (381, 520), (383, 912), (355, 912)]), jacket.front)
    flat(ctx, poly([(515, 360), (492, 360), (489, 520), (487, 912), (515, 912)]), jacket.front)
    stroke(ctx, segment(378, 360, 383, 912), gold, 4)
    stroke(ctx, segment(492, 360, 487, 912), gold, 4)
    stroke(ctx, segment(355, 910, 383, 910), gold, 4)
    stroke(ctx, segment(487, 910, 515, 910), gold, 4)
    // the shawl collar hangs open, piped in gold
    flat(ctx, poly([(355, 360), (404, 360), (393, 470), (355, 488)]), jacket.top)
    flat(ctx, poly([(515, 360), (466, 360), (477, 470), (515, 488)]), jacket.top)
    stroke(ctx, segment(404, 360, 393, 470), gold, 4)
    stroke(ctx, segment(393, 470, 355, 488), gold, 4)
    stroke(ctx, segment(466, 360, 477, 470), gold, 4)
    stroke(ctx, segment(477, 470, 515, 488), gold, 4)
    eyes(ctx)
    // a neat gold cord at the waist, tied at one side with two short tassels
    let cord = CGMutablePath()
    cord.move(to: CGPoint(x: 383, y: 678))
    cord.addQuadCurve(to: CGPoint(x: 487, y: 678), control: CGPoint(x: 435, y: 692))
    stroke(ctx, cord, gold, 6)
    flat(ctx, circle(466, 686, 10), gold)
    flat(ctx, circle(466, 686, 4), goldDark)
    stroke(ctx, segment(466, 694, 474, 742), gold, 4)
    stroke(ctx, segment(466, 694, 456, 736), gold, 4)
    flat(ctx, poly([(470, 738), (479, 739), (482, 760), (468, 760)]), gold)
    flat(ctx, poly([(451, 732), (460, 733), (461, 754), (449, 753)]), gold)
    watchChain(ctx)
    monocle(ctx)
    stroke(ctx, segment(355, 360, 425, 324), 0xFFFFFF, 5, alpha: 0.85)
    topHat(ctx)
    cane(ctx)
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
let bay: UInt32 = 0x1F2A6B
let led: UInt32 = 0x3FD07A
let hatBlack: UInt32 = 0x262634
let hatBrim: UInt32 = 0x1A1A26
let pupil: UInt32 = 0x23232F

/// the 32 pixel tower: slender, in a jacket colour block with a gold collar edge, two calm dark eyes,
/// a gold monocle ring, and a tall top hat with a wide brim.
func pixelIcon32() -> CGContext {
    let c = cloth()
    var p = Pix(32, 32)
    tileBase(&p)
    for (x, y) in [(4, 6), (27, 9), (5, 24), (28, 21), (3, 15)] { p[x, y] = 0xFFFFFF }
    p.rect(11, 29, 12, 1, 0x00004A)
    p.rect(12, 11, 7, 18, towerFront)
    for (x, t, height) in [(19, 11, 18), (20, 10, 18), (21, 10, 17)] { p.rect(x, t, 1, height, c.side) }
    p.rect(12, 10, 9, 1, c.top)
    p.rect(14, 9, 8, 1, c.top)
    // the jacket panels down both sides of the front
    p.rect(12, 11, 2, 17, c.front)
    p.rect(17, 11, 2, 17, c.front)
    // the open collar, edged in gold
    p.rect(12, 11, 3, 1, c.top)
    p.rect(16, 11, 3, 1, c.top)
    p[14, 12] = gold
    p[16, 12] = gold
    // two calm eyes on a plate, and a monocle round the left one
    p.rect(14, 13, 3, 2, 0xB3B9CF)
    p[14, 14] = pupil
    p[16, 14] = pupil
    for (x, y) in [(13, 13), (14, 13), (15, 13), (13, 14), (15, 14), (13, 15), (14, 15), (15, 15)] { p[x, y] = gold }
    for y in [18, 21, 24] {
        p.rect(14, y, 3, 2, bay)
        p[14, y] = led
    }
    p.rect(14, 27, 3, 1, gold)
    // a tall top hat with a wide brim, sitting low
    p.rect(9, 8, 15, 1, hatBrim)
    p.rect(12, 1, 9, 7, hatBlack)
    p.rect(12, 1, 9, 1, 0x383849)
    p.rect(12, 6, 9, 1, gold)
    for y in 2..<7 { p[12, y] = 0xB8C2F0 }
    return p.image()
}

func pixelIcon16() -> CGContext {
    let c = cloth()
    var p = Pix(16, 16)
    tileBase(&p)
    p.rect(5, 14, 7, 1, 0x00004A)
    p.rect(5, 7, 6, 7, towerFront)
    p.rect(11, 7, 2, 7, c.side)
    p.rect(5, 6, 8, 1, c.top)
    p.rect(5, 7, 1, 7, c.front)
    p.rect(10, 7, 1, 7, c.front)
    p[6, 7] = gold
    p[9, 7] = gold
    p[6, 8] = pupil
    p[9, 8] = pupil
    for y in [10, 12] {
        p.rect(7, y, 2, 1, bay)
        p[7, y] = led
    }
    p.rect(3, 5, 10, 1, hatBrim)
    p.rect(5, 1, 6, 4, hatBlack)
    p.rect(5, 3, 6, 1, gold)
    p[5, 1] = 0xB8C2F0
    p[5, 2] = 0xB8C2F0
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
    for (style, name) in [("crest", "h-dignified-crest"), ("crimson", "h-dignified-crimson")] {
        emblemStyle = style
        save(banner(1, light: false), "\(dir)/\(name).png")
    }
}
print("wrote the logo and icons to \(out)")
