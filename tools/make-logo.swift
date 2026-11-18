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
/// draw the figure for the app icon: bent proportions (a shorter, sturdier tower, a bigger hat, a staff leaning in closer).
var iconMode = false

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
    let top: (CGFloat, CGFloat) = iconMode ? (258, 548) : (emblemStyle == "b" ? (326, 376) : (606, 376))
    let foot: (CGFloat, CGFloat) = iconMode ? (222, 926) : (emblemStyle == "b" ? (268, 926) : (676, 926))
    ctx.setFillColor(rgb(0x000000, shadowAlpha))
    ctx.fillEllipse(in: CGRect(x: foot.0 - 28, y: 920, width: 56, height: 14))
    let dx = foot.0 - top.0
    let dy = foot.1 - top.1
    let len = (dx * dx + dy * dy).squareRoot()
    let ux = dx / len
    let uy = dy / len
    stroke(ctx, segment(top.0 + ux * 14, top.1 + uy * 14, foot.0 - ux * 24, foot.1 - uy * 24), (iconMode ? 0x7A5632 : 0xB98A52), iconMode ? 13 : 10)
    stroke(ctx, segment(top.0 + ux * 14 - 3, top.1 + uy * 14, foot.0 - ux * 24 - 3, foot.1 - uy * 24), 0xFFFFFF, 2, alpha: 0.35)
    stroke(ctx, segment(foot.0 - ux * 26, foot.1 - uy * 26, foot.0, foot.1), gold, 10)
    flat(ctx, circle(top.0, top.1, iconMode ? 25 : 20), gold)
    if iconMode { stroke(ctx, circle(top.0, top.1, 25), goldDark, 5) }
    flat(ctx, circle(top.0 - 7, top.1 - 8, iconMode ? 7 : 6), 0xF4DC86)
}

/// a tall top hat tipped a little to one side: a wide brim, a satin band and a lit left edge.
func topHat(_ ctx: CGContext) {
    ctx.saveGState()
    if iconMode {
        ctx.translateBy(x: 470, y: 528)
        ctx.scaleBy(x: 1.5, y: 1.5)
        ctx.translateBy(x: -470, y: -346)
    }
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
    if iconMode {
        ctx.saveGState()
        ctx.translateBy(x: 470, y: 920)
        ctx.scaleBy(x: 1.55, y: 0.68)
        ctx.translateBy(x: -470, y: -920)
    }
    if shadow {
        ctx.setFillColor(rgb(0x000000, shadowAlpha))
        ctx.fillEllipse(in: CGRect(x: 335, y: 902, width: 290, height: 44))
    }
    flat(ctx, poly([(355, 360), (515, 360), (515, 920), (355, 920)]), 0xD7DBE8)
    flat(ctx, poly([(515, 360), (585, 324), (585, 884), (515, 920)]), robe.side)
    flat(ctx, poly([(355, 360), (425, 324), (585, 324), (515, 360)]), robe.top)
    // the bays down the open middle, where the robe falls away; the short figure shows more of the front,
    // with four broad bays and big lights so it reads as a compact server
    if iconMode {
        for i in 0..<4 {
            let y = 540 + CGFloat(i) * 92
            flat(ctx, box(396, y, 96, 76), 0x1F2A6B)
            stroke(ctx, box(396, y, 96, 76), 0x0E1540, 4)
            flat(ctx, circle(424, y + 38, 18), 0x6BF0A0)
            flat(ctx, circle(424, y + 38, 12), 0x3FD07A)
            flat(ctx, box(450, y + 28, 32, 20), 0x39458F)
        }
    } else {
        for i in 0..<3 {
            let y = 600 + CGFloat(i) * 52
            flat(ctx, box(426, y, 62, 34), 0x1F2A6B)
            flat(ctx, circle(438, y + 17, 6), 0x3FD07A)
            flat(ctx, box(450, y + 13, 30, 8), 0x39458F)
        }
    }
    // the side face: one broad fold of cloth
    flat(ctx, poly([(515, 372), (548, 356), (578, 640), (556, 886), (515, 912)]), robe.front)
    // the robe: a large fold falling from the left shoulder, and a narrow one at the right edge
    if iconMode {
        flat(ctx, poly([(346, 352), (440, 350), (410, 470), (384, 640), (370, 790), (362, 916), (346, 916)]), robe.front)
        flat(ctx, poly([(362, 364), (386, 362), (374, 560), (366, 912), (352, 912)]), robe.side)
    } else {
        flat(ctx, poly([(346, 352), (474, 350), (446, 470), (420, 640), (398, 790), (382, 916), (346, 916)]), robe.front)
        flat(ctx, poly([(366, 364), (398, 362), (390, 560), (376, 912), (356, 912)]), robe.side)
    }
    flat(ctx, poly([(515, 358), (490, 356), (494, 560), (500, 720), (498, 916), (515, 916)]), robe.front)
    // gold on the collar edge only
    stroke(ctx, iconMode ? segment(440, 350, 410, 470) : segment(474, 350, 446, 470), gold, 5)
    // one lit edge
    stroke(ctx, segment(346, 352, 425, 324), 0xFFFFFF, 5, alpha: 0.8)
    if iconMode { ctx.restoreGState() }
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

/// the icon backdrop: a calm flat colour with a very subtle vignette. "teal" is a muted slate teal near the desktop
/// teal but desaturated, "cream" is a warm parchment.
var backdropStyle = "teal"

func backdropColours() -> (centre: UInt32, edge: UInt32) {
    backdropStyle == "cream" ? (0xF4EAD2, 0xE2D4B4) : (0x5E8E92, 0x436C71)
}

/// draws the icon figure, bent to fill a square, with its middle at `centre` and its height `height`.
func iconArt(_ ctx: CGContext, centre: CGPoint, height: CGFloat) {
    iconMode = true
    emblemStyle = "b"
    ctx.saveGState()
    ctx.translateBy(x: centre.x, y: centre.y)
    let k = height / 709
    ctx.scaleBy(x: k, y: k)
    ctx.translateBy(x: -445, y: -591)
    gentleman(ctx, burgundy, shadow: true)
    ctx.restoreGState()
    iconMode = false
}

/// a flat vignette backdrop over `rect`.
func paintBackdrop(_ ctx: CGContext, _ rect: CGRect) {
    let c = backdropColours()
    ctx.setFillColor(rgb(c.centre))
    ctx.fill(rect)
    let g = CGGradient(colorsSpace: space, colors: [rgb(c.centre, 0), rgb(c.edge, 1)] as CFArray, locations: [0, 1])!
    let mid = CGPoint(x: rect.midX, y: rect.midY)
    ctx.drawRadialGradient(
        g, startCenter: mid, startRadius: rect.width * 0.30, endCenter: mid, endRadius: rect.width * 0.78,
        options: [.drawsAfterEndLocation])
}

// MARK: the app icon tile

func tile(_ size: CGFloat, margin: CGFloat, thick: CGFloat = 1) -> CGContext {
    let ctx = canvas(Int(size), Int(size))
    let m = size * margin
    let box = CGRect(x: m, y: m, width: size - 2 * m, height: size - 2 * m)
    let shape = round(box, box.width * 0.215)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: size * 0.012), blur: size * 0.03, color: rgb(0x000000, margin > 0.06 ? 0.45 : 0.3))
    ctx.addPath(shape)
    ctx.setFillColor(rgb(backdropColours().centre))
    ctx.fillPath()
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    paintBackdrop(ctx, box)
    ctx.restoreGState()
    iconArt(ctx, centre: CGPoint(x: box.midX, y: box.midY + box.height * 0.01), height: box.height * 0.88)
    return ctx
}

// MARK: the social avatar

/// the same art on the same backdrop, full bleed, with everything inside the inscribed circle.
func social(_ size: CGFloat) -> CGContext {
    let ctx = canvas(Int(size), Int(size))
    paintBackdrop(ctx, CGRect(x: 0, y: 0, width: size, height: size))
    iconArt(ctx, centre: CGPoint(x: size / 2, y: size / 2), height: size * 0.74)
    return ctx
}

// MARK: a preview of the icon at every size, on light gray and on dark

/// draws `src` at (x, y), scaled, upright; `smooth` is off for the pixel icons.
func blit(_ dst: CGContext, _ src: CGContext, _ x: CGFloat, _ y: CGFloat, scale: CGFloat = 1, smooth: Bool = true) {
    let w = CGFloat(src.width) * scale
    let h = CGFloat(src.height) * scale
    dst.saveGState()
    dst.interpolationQuality = smooth ? .high : .none
    dst.translateBy(x: x, y: y + h)
    dst.scaleBy(x: 1, y: -1)
    dst.draw(src.makeImage()!, in: CGRect(x: 0, y: 0, width: w, height: h))
    dst.restoreGState()
}

func iconStrip() -> CGContext {
    let ctx = canvas(1600, 560)
    ctx.setFillColor(rgb(0xC0C0C0))
    ctx.fill(CGRect(x: 0, y: 0, width: 1600, height: 560))
    ctx.setFillColor(rgb(0x14141C))
    ctx.fill(CGRect(x: 560, y: 280, width: 1040, height: 280))
    blit(ctx, tile(512, margin: 0.098), 24, 24)
    for row in [0, 1] as [CGFloat] {
        let y = row * 280
        blit(ctx, tile(256, margin: 0.098), 590, y + 12)
        blit(ctx, tile(64, margin: 0.04), 890, y + 100)
        blit(ctx, pixelIcon32(), 980, y + 116, smooth: false)
        blit(ctx, pixelIcon16(), 1040, y + 124, smooth: false)
        blit(ctx, pixelIcon32(), 1100, y + 92, scale: 3, smooth: false)
        blit(ctx, pixelIcon16(), 1230, y + 108, scale: 4, smooth: false)
        blit(ctx, tile(48, margin: 0.03, thick: 2.2), 1330, y + 116)
        blit(ctx, tile(48, margin: 0.03, thick: 2.2), 1400, y + 92, scale: 2)
    }
    return ctx
}

// MARK: the installer's wallpaper: a true 16-colour ordered dither, black on blue like the nt backdrop

/// the only two colours the wallpaper uses, both from the standard 16-colour palette: black and dark blue.
let dark: [UInt8] = [0, 0, 0]
let blue: [UInt8] = [0, 0, 128]
let wallN = 384
/// the ink and the ground of the pattern mask (the mask is drawn white on black, then dithered).
var wallInk: UInt32 = 0xFFFFFF
var wallBase: UInt32 = 0x000000

/// the 8 x 8 ordered-dither (bayer) matrix.
let bayer8: [[Int]] = [
    [0, 32, 8, 40, 2, 34, 10, 42], [48, 16, 56, 24, 50, 18, 58, 26], [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22], [3, 35, 11, 43, 1, 33, 9, 41], [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37], [63, 31, 55, 23, 61, 29, 53, 21],
]

/// draws `motif` at each spot and at the neighbouring copies, so the tile wraps without a seam.
func repeatMotif(_ ctx: CGContext, _ spots: [(CGFloat, CGFloat)], _ motif: (CGContext) -> Void) {
    let n = CGFloat(wallN)
    for (x, y) in spots {
        for dx in [-n, 0, n] {
            for dy in [-n, 0, n] {
                ctx.saveGState()
                ctx.translateBy(x: x + dx, y: y + dy)
                motif(ctx)
                ctx.restoreGState()
            }
        }
    }
}

func smallCrown() -> CGPath {
    poly([(-24, -128), (-28, -156), (-11, -142), (0, -162), (11, -142), (28, -156), (24, -128)])
}

/// pattern c: a classic damask motif: an ogee frame round a small flower, four leaves and a crown above.
func damask(_ ctx: CGContext) {
    ctx.scaleBy(x: 0.8, y: 0.8)
    let ogee = CGMutablePath()
    ogee.move(to: CGPoint(x: 0, y: -122))
    ogee.addCurve(to: CGPoint(x: 64, y: 0), control1: CGPoint(x: 40, y: -94), control2: CGPoint(x: 64, y: -48))
    ogee.addCurve(to: CGPoint(x: 0, y: 122), control1: CGPoint(x: 64, y: 48), control2: CGPoint(x: 38, y: 92))
    ogee.addCurve(to: CGPoint(x: -64, y: 0), control1: CGPoint(x: -38, y: 92), control2: CGPoint(x: -64, y: 48))
    ogee.addCurve(to: CGPoint(x: 0, y: -122), control1: CGPoint(x: -64, y: -48), control2: CGPoint(x: -40, y: -94))
    ogee.closeSubpath()
    flat(ctx, ogee, wallInk)
    var inner = CGAffineTransform(scaleX: 0.62, y: 0.62)
    flat(ctx, ogee.copy(using: &inner)!, wallBase)
    flat(ctx, circle(0, 0, 14), wallInk)
    for a in 0..<4 {
        let angle = CGFloat(a) * .pi / 2
        flat(ctx, ellipse(cos(angle) * 26, sin(angle) * 26, a % 2 == 0 ? 12 : 7, a % 2 == 0 ? 7 : 12), wallInk)
    }
    for m in [1, -1] as [CGFloat] {
        for v in [1, -1] as [CGFloat] {
            let leaf = CGMutablePath()
            leaf.move(to: CGPoint(x: 58 * m, y: 22 * v))
            leaf.addCurve(to: CGPoint(x: 88 * m, y: 128 * v), control1: CGPoint(x: 98 * m, y: 34 * v), control2: CGPoint(x: 106 * m, y: 88 * v))
            leaf.addCurve(to: CGPoint(x: 58 * m, y: 22 * v), control1: CGPoint(x: 70 * m, y: 90 * v), control2: CGPoint(x: 54 * m, y: 64 * v))
            leaf.closeSubpath()
            flat(ctx, leaf, wallInk)
        }
    }
    flat(ctx, smallCrown(), wallInk)
    flat(ctx, poly([(0, 128), (14, 142), (0, 156), (-14, 142)]), wallInk)
}

/// pattern d: a fleur-de-lis: a pointed centre petal, two curling side petals, a band and a lower petal.
func fleur(_ ctx: CGContext) {
    ctx.scaleBy(x: 0.7, y: 0.7)
    let petal = CGMutablePath()
    petal.move(to: CGPoint(x: 0, y: -124))
    petal.addCurve(to: CGPoint(x: 0, y: 14), control1: CGPoint(x: 36, y: -84), control2: CGPoint(x: 34, y: -30))
    petal.addCurve(to: CGPoint(x: 0, y: -124), control1: CGPoint(x: -34, y: -30), control2: CGPoint(x: -36, y: -84))
    flat(ctx, petal, wallInk)
    flat(ctx, ellipse(0, -60, 5, 34), wallBase)
    for m in [1, -1] as [CGFloat] {
        let side = CGMutablePath()
        side.move(to: CGPoint(x: 8 * m, y: -10))
        side.addCurve(to: CGPoint(x: 92 * m, y: -56), control1: CGPoint(x: 34 * m, y: -64), control2: CGPoint(x: 76 * m, y: -86))
        side.addCurve(to: CGPoint(x: 78 * m, y: 8), control1: CGPoint(x: 112 * m, y: -30), control2: CGPoint(x: 104 * m, y: 0))
        side.addCurve(to: CGPoint(x: 12 * m, y: 22), control1: CGPoint(x: 56 * m, y: 22), control2: CGPoint(x: 30 * m, y: 16))
        side.closeSubpath()
        flat(ctx, side, wallInk)
    }
    flat(ctx, box(-40, 16, 80, 16), wallInk)
    let low = CGMutablePath()
    low.move(to: CGPoint(x: 0, y: 38))
    low.addCurve(to: CGPoint(x: 0, y: 108), control1: CGPoint(x: 18, y: 56), control2: CGPoint(x: 16, y: 80))
    low.addCurve(to: CGPoint(x: 0, y: 38), control1: CGPoint(x: -16, y: 80), control2: CGPoint(x: -18, y: 56))
    flat(ctx, low, wallInk)
}

/// a pair of mirrored scrolls, the scrollwork between the fleurs.
func scrolls(_ ctx: CGContext) {
    ctx.scaleBy(x: 0.7, y: 0.7)
    for m in [1, -1] as [CGFloat] {
        let spiral = CGMutablePath()
        var t: CGFloat = 0
        var first = true
        while t < 3.2 * .pi {
            let r = 24 * (1 - t / (3.6 * .pi)) + 3
            let pt = CGPoint(x: m * (22 + r * cos(t)), y: r * sin(t))
            if first { spiral.move(to: pt); first = false } else { spiral.addLine(to: pt) }
            t += 0.12
        }
        ctx.addPath(spiral)
        ctx.setStrokeColor(rgb(wallInk))
        ctx.setLineWidth(7)
        ctx.setLineCap(.round)
        ctx.strokePath()
    }
}

func diamond(_ ctx: CGContext) {
    flat(ctx, poly([(0, -16), (11, 0), (0, 16), (-11, 0)]), wallInk)
}

/// the pattern as an 8 bit mask, one tile, drawn with coregraphics and wrapping without a seam.
func wallMask(_ style: String) -> [UInt8] {
    let ctx = canvas(wallN, wallN)
    ctx.setFillColor(rgb(wallBase))
    ctx.fill(CGRect(x: 0, y: 0, width: wallN, height: wallN))
    if style == "d" {
        repeatMotif(ctx, [(96, 96), (288, 96), (96, 288), (288, 288)], fleur)
        repeatMotif(ctx, [(192, 96), (192, 288), (0, 96), (0, 288)], scrolls)
        repeatMotif(ctx, [(192, 192), (0, 192), (192, 0), (0, 0)], diamond)
    } else {
        repeatMotif(ctx, [(96, 192), (288, 0)], damask)
        repeatMotif(ctx, [(96, 0), (288, 192)], diamond)
    }
    let bytes = ctx.data!.assumingMemoryBound(to: UInt8.self)
    var out = [UInt8](repeating: 0, count: wallN * wallN)
    for y in 0..<wallN {
        for x in 0..<wallN { out[y * wallN + x] = bytes[y * ctx.bytesPerRow + x * 4] }
    }
    return out
}

/// ordered-dithers a brightness field in 0...1 to black and dark blue, in 2 x 2 pixel cells.
func dithered(_ w: Int, _ h: Int, _ light: (Int, Int) -> Double) -> [UInt8] {
    var px = [UInt8](repeating: 255, count: w * h * 4)
    for y in 0..<h {
        for x in 0..<w {
            let threshold = (Double(bayer8[(y / 2) & 7][(x / 2) & 7]) + 0.5) / 64
            let c = light(x, y) > threshold ? blue : dark
            let o = (y * w + x) * 4
            px[o] = c[0]
            px[o + 1] = c[1]
            px[o + 2] = c[2]
        }
    }
    return px
}

/// writes raw pixels as a png without touching them: no scaling, no blending, no anti-aliasing.
func savePixels(_ px: [UInt8], _ w: Int, _ h: Int, _ path: String) {
    let provider = CGDataProvider(data: Data(px) as CFData)!
    let image = CGImage(
        width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4, space: space,
        bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue), provider: provider, decode: nil,
        shouldInterpolate: false, intent: .defaultIntent)!
    let url = URL(fileURLWithPath: path)
    try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, image, nil)
    CGImageDestinationFinalize(dest)
}

/// one seamless tile: an even dither with the pattern as denser islands.
func wallTilePixels(_ style: String) -> [UInt8] {
    let mask = wallMask(style)
    return dithered(wallN, wallN) { x, y in 0.36 + 0.52 * Double(mask[y * wallN + x]) / 255 }
}

/// the full 1920 x 1080 wallpaper: near solid black at the edges and corners, a denser blue dither in the middle,
/// and the pattern showing up as higher-density islands inside that gradient.
func wallpaperPixels(_ style: String) -> [UInt8] {
    let mask = wallMask(style)
    return dithered(1920, 1080) { x, y in
        let dx = (Double(x) - 960) / 960
        let dy = (Double(y) - 540) / 540
        let r = min(1, (dx * dx * 0.55 + dy * dy * 0.9).squareRoot())
        let fall = pow(1 - r, 1.05)
        let m = Double(mask[(y % wallN) * wallN + (x % wallN)]) / 255
        return 0.74 * fall + m * 0.46 * (0.12 + 0.88 * fall)
    }
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

/// the flat backdrop of a pixel icon, with a one pixel darker edge.
func tileBase(_ p: inout Pix) {
    let c = backdropColours()
    let n = p.w
    p.rect(0, 0, n, n, c.edge)
    p.rect(1, 1, n - 2, n - 2, c.centre)
}

let towerFront: UInt32 = 0xD7DBE8
let bay: UInt32 = 0x1F2A6B
let led: UInt32 = 0x3FD07A
let hatBlack: UInt32 = 0x262634
let hatBrim: UInt32 = 0x1A1A26
let shaft: UInt32 = 0x7A5632

/// the 32 pixel icon: a big tilted top hat on a short sturdy tower in the burgundy robe, with the staff leaning on the left.
func pixelIcon32() -> CGContext {
    let c = burgundy
    var p = Pix(32, 32)
    tileBase(&p)
    p.rect(8, 29, 17, 1, backdropColours().edge)
    // the tower: front, side and top, under the robe
    p.rect(10, 16, 10, 13, towerFront)
    for (x, t) in [(20, 16), (21, 15), (22, 15), (23, 14)] { p.rect(x, t, 1, 29 - t - (x - 20) / 2, c.side) }
    p.rect(10, 15, 14, 1, c.top)
    for y in 16..<29 { p.rect(10, y, 4 - (y - 16) / 5, 1, c.front) }
    p.rect(11, 17, 1, 9, c.side)
    p.rect(19, 16, 1, 13, c.front)
    p[15, 16] = gold
    for y in [17, 21, 25] {
        p.rect(14, y, 5, 3, bay)
        p.rect(14, y, 2, 2, led)
    }
    // the staff, leaning on the left, with a gold ball
    for y in 17...28 { p[7 - (y - 17) / 5, y] = shaft }
    p.rect(6, 15, 2, 2, gold)
    p[5, 28] = gold
    // a big top hat, tipped to the right
    p.rect(7, 13, 20, 1, hatBrim)
    p.rect(9, 14, 3, 1, hatBrim)
    for (y, shift) in [(2, 2), (3, 2), (4, 2), (5, 1), (6, 1), (7, 1), (8, 0), (9, 0), (10, 0), (11, 0), (12, 0)] {
        p.rect(11 + shift, y, 11, 1, y == 2 ? 0x383849 : hatBlack)
    }
    p.rect(11, 10, 12, 1, gold)
    p.rect(11, 11, 12, 1, gold)
    for y in 3..<10 { p[11 + (y < 5 ? 2 : (y < 8 ? 1 : 0)), y] = 0xB8C2F0 }
    return p.image()
}

func pixelIcon16() -> CGContext {
    let c = burgundy
    var p = Pix(16, 16)
    tileBase(&p)
    p.rect(4, 8, 6, 6, towerFront)
    p.rect(10, 8, 3, 6, c.side)
    p.rect(4, 7, 9, 1, c.top)
    p.rect(4, 8, 2, 6, c.front)
    p.rect(9, 8, 1, 6, c.front)
    for y in [9, 12] {
        p.rect(6, y, 3, 2, bay)
        p[6, y] = led
    }
    // the hat, tipped to the right
    p.rect(2, 6, 12, 1, hatBrim)
    p.rect(4, 5, 2, 1, hatBrim)
    for (y, shift) in [(2, 1), (3, 1), (4, 0), (5, 0)] {
        p.rect(4 + shift, y, 7, 1, y == 2 ? 0x383849 : hatBlack)
    }
    p.rect(4, 4, 8, 1, gold)
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
backdropStyle = ProcessInfo.processInfo.environment["BACKDROP"] ?? "cream"
let wallStyle = ProcessInfo.processInfo.environment["BACKGROUND"] ?? "d"
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "assets"
let ui = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "crates/ui/assets"

save(setupPanel(1), "\(out)/setup-panel.png")
save(setupPanel(2), "\(out)/setup-panel@2x.png")
save(setupPanel(2), "\(ui)/setup-panel.png")

// the installer's wallpaper: a two-colour ordered dither, as a seamless tile and as a full screen
savePixels(wallTilePixels(wallStyle), wallN, wallN, "\(out)/setup-tile.png")
savePixels(wallTilePixels(wallStyle), wallN, wallN, "\(ui)/setup-tile.png")
savePixels(wallpaperPixels(wallStyle), 1920, 1080, "\(out)/setup-background.png")

// the macos icon has a margin around the tile; the windows and window icons nearly fill it
save(tile(512, margin: 0.098), "\(out)/icon-512.png")
save(tile(512, margin: 0.04), "\(ui)/icon.png")
save(pixelIcon32(), "\(out)/icon-32.png")

// the same art as a social avatar: square, with everything inside the inscribed circle
save(social(1024), "\(out)/social.png")
save(social(400), "\(out)/social-400.png")

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

// optional: a folder to leave previews in (the icon at every size on both backdrops, both wallpaper patterns)
if CommandLine.arguments.count > 3 {
    let prev = CommandLine.arguments[3]
    save(scaled(pixelIcon16(), to: 256), "\(prev)/zoom-16.png")
    save(scaled(pixelIcon32(), to: 256), "\(prev)/zoom-32.png")
    let keep = backdropStyle
    for style in ["teal", "cream"] {
        backdropStyle = style
        save(iconStrip(), "\(prev)/icon-bg-\(style).png")
    }
    backdropStyle = keep
    savePixels(wallpaperPixels("c"), 1920, 1080, "\(prev)/setup-bg-c.png")
    savePixels(wallpaperPixels("d"), 1920, 1080, "\(prev)/setup-bg-d.png")
}
print("wrote the icons and the setup artwork to \(out)")
