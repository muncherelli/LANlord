//! the installer's wallpaper is a true 16-colour image: only black and dark blue, fully opaque, no
//! anti-aliasing or blending. these tests fail if a regenerated image picks up any other colour.

const TILE: &[u8] = include_bytes!("../assets/setup-tile.png");
const BACKGROUND: &[u8] = include_bytes!("../../../assets/setup-background.png");

fn check(bytes: &[u8], width: usize, height: usize) {
    let image = eframe::icon_data::from_png_bytes(bytes).expect("a png");
    assert_eq!(
        (image.width as usize, image.height as usize),
        (width, height)
    );
    let (mut black, mut blue) = (0usize, 0usize);
    let (pixels, rest) = image.rgba.as_chunks::<4>();
    assert!(rest.is_empty());
    for px in pixels {
        match (px[0], px[1], px[2], px[3]) {
            (0, 0, 0, 255) => black += 1,
            (0, 0, 128, 255) => blue += 1,
            other => panic!("{other:?} is not a palette colour"),
        }
    }
    assert!(black > 0 && blue > 0, "both colours are used");
}

#[test]
fn the_tile_uses_only_the_palette() {
    check(TILE, 384, 384);
}

#[test]
fn the_background_uses_only_the_palette() {
    check(BACKGROUND, 1920, 1080);
}
