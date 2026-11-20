//! the gui's colours for the terminal: the same rgb values as crates/ui/src/theme.rs, written as truecolor when the terminal says it can, or as the
//! nearest 256-colour or 16-colour entry when it cannot.

use lanlord_app::appearance::{Role, WINDOWS_STANDARD};
use ratatui::style::Color;

/// an rgb colour from the gui.
pub type Rgb = (u8, u8, u8);

/// the colour of `role` in the nt4 "windows standard" scheme the terminal draws with.
const fn role(role: Role) -> Rgb {
    WINDOWS_STANDARD
        .get(role)
        .expect("windows standard sets every role")
        .tuple()
}

/// the desktop.
pub const TEAL: Rgb = role(Role::Desktop);
/// windows, the taskbar and buttons.
pub const GRAY: Rgb = role(Role::Face);
/// the inner highlight of a raised edge.
pub const LIGHT: Rgb = role(Role::Light);
/// the inner shadow of a raised edge, and disabled text.
pub const SHADOW: Rgb = role(Role::Shadow);
/// the active title bar and the selection.
pub const NAVY: Rgb = role(Role::ActiveCaption);
/// the outer highlight, field backgrounds and text on navy.
pub const WHITE: Rgb = role(Role::Window);
/// text and the outer shadow.
pub const BLACK: Rgb = role(Role::WindowText);

/// how many colours the terminal can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Indexed256,
    Basic16,
}

impl Depth {
    /// guess from the environment: `NO_COLOR` keeps to the 16 basic colours; truecolor when
    /// `COLORTERM=truecolor|24bit`, a `TERM` ending in `-direct` or `truecolor`, or windows
    /// terminal (`WT_SESSION`); 256 colours for a `TERM` ending in `256color`; otherwise 16.
    pub fn detect() -> Depth {
        let var = |name: &str| std::env::var(name).ok();
        Depth::from_env(&Env {
            colorterm: var("COLORTERM").as_deref(),
            term: var("TERM").as_deref(),
            no_color: var("NO_COLOR").is_some_and(|v| !v.is_empty()),
            windows_terminal: var("WT_SESSION").is_some(),
        })
    }

    fn from_env(env: &Env) -> Depth {
        let colorterm = env.colorterm.unwrap_or_default().to_ascii_lowercase();
        let term = env.term.unwrap_or_default().to_ascii_lowercase();
        if env.no_color {
            Depth::Basic16
        } else if colorterm == "truecolor"
            || colorterm == "24bit"
            || term.ends_with("-direct")
            || term.ends_with("truecolor")
            || env.windows_terminal
        {
            Depth::TrueColor
        } else if term.ends_with("256color") {
            Depth::Indexed256
        } else {
            Depth::Basic16
        }
    }
}

/// the environment variables the colour depth is guessed from.
#[derive(Default)]
struct Env<'a> {
    colorterm: Option<&'a str>,
    term: Option<&'a str>,
    no_color: bool,
    windows_terminal: bool,
}

/// turns gui colours into terminal colours for one colour depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub depth: Depth,
}

impl Palette {
    pub fn new(depth: Depth) -> Palette {
        Palette { depth }
    }

    pub fn detect() -> Palette {
        Palette::new(Depth::detect())
    }

    /// the terminal colour for `rgb` at this depth.
    pub fn color(&self, rgb: Rgb) -> Color {
        match self.depth {
            Depth::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
            Depth::Indexed256 => Color::Indexed(nearest_256(rgb)),
            Depth::Basic16 => nearest_16(rgb),
        }
    }
}

impl Default for Palette {
    fn default() -> Palette {
        Palette::new(Depth::TrueColor)
    }
}

/// the six levels of the xterm colour cube.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn distance(a: Rgb, b: Rgb) -> u32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).unsigned_abs();
    d(a.0, b.0).pow(2) + d(a.1, b.1).pow(2) + d(a.2, b.2).pow(2)
}

/// the closest entry of the xterm 256-colour table, from the colour cube or the gray ramp.
fn nearest_256(rgb: Rgb) -> u8 {
    let level = |v: u8| {
        CUBE.iter()
            .enumerate()
            .min_by_key(|(_, c)| (i32::from(**c) - i32::from(v)).unsigned_abs())
            .map_or(0, |(i, _)| i as u8)
    };
    let (r, g, b) = (level(rgb.0), level(rgb.1), level(rgb.2));
    let cube = (CUBE[r as usize], CUBE[g as usize], CUBE[b as usize]);
    let cube_index = 16 + 36 * r + 6 * g + b;
    let average = ((u16::from(rgb.0) + u16::from(rgb.1) + u16::from(rgb.2)) / 3) as u8;
    // the ramp runs 8, 18 .. 238: round to the nearest step
    let step = (average.saturating_sub(3) / 10).min(23);
    let gray = 8 + 10 * step;
    if distance(rgb, (gray, gray, gray)) < distance(rgb, cube) {
        232 + step
    } else {
        cube_index
    }
}

/// the closest of the 16 basic terminal colours, using the vga values the gui palette is
/// built from.
fn nearest_16(rgb: Rgb) -> Color {
    const BASIC: [(Rgb, Color); 16] = [
        ((0, 0, 0), Color::Black),
        ((128, 0, 0), Color::Red),
        ((0, 128, 0), Color::Green),
        ((128, 128, 0), Color::Yellow),
        ((0, 0, 128), Color::Blue),
        ((128, 0, 128), Color::Magenta),
        ((0, 128, 128), Color::Cyan),
        ((192, 192, 192), Color::Gray),
        ((128, 128, 128), Color::DarkGray),
        ((255, 0, 0), Color::LightRed),
        ((0, 255, 0), Color::LightGreen),
        ((255, 255, 0), Color::LightYellow),
        ((0, 0, 255), Color::LightBlue),
        ((255, 0, 255), Color::LightMagenta),
        ((0, 255, 255), Color::LightCyan),
        ((255, 255, 255), Color::White),
    ];
    BASIC
        .iter()
        .min_by_key(|(c, _)| distance(*c, rgb))
        .map_or(Color::Reset, |(_, color)| *color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_depth_comes_from_the_environment() {
        let depth = |colorterm, term, no_color, windows_terminal| {
            Depth::from_env(&Env {
                colorterm,
                term,
                no_color,
                windows_terminal,
            })
        };
        assert_eq!(
            depth(Some("truecolor"), Some("xterm"), false, false),
            Depth::TrueColor
        );
        assert_eq!(depth(Some("24bit"), None, false, false), Depth::TrueColor);
        assert_eq!(
            depth(None, Some("xterm-direct"), false, false),
            Depth::TrueColor
        );
        assert_eq!(
            depth(None, Some("konsole-truecolor"), false, false),
            Depth::TrueColor
        );
        assert_eq!(
            depth(None, None, false, true),
            Depth::TrueColor,
            "windows terminal"
        );
        assert_eq!(
            depth(None, Some("xterm-256color"), false, false),
            Depth::Indexed256
        );
        assert_eq!(depth(None, Some("linux"), false, false), Depth::Basic16);
        assert_eq!(depth(None, None, false, false), Depth::Basic16);
        // no_color wins over everything
        assert_eq!(
            depth(Some("truecolor"), Some("xterm-256color"), true, true),
            Depth::Basic16
        );
    }

    #[test]
    fn the_constants_read_the_windows_standard_scheme() {
        assert_eq!(role(Role::Desktop), (0, 128, 128));
        assert_eq!(role(Role::Light), (223, 223, 223));
        assert_eq!(role(Role::ActiveCaption), NAVY);
    }

    #[test]
    fn truecolor_keeps_the_gui_values() {
        let palette = Palette::new(Depth::TrueColor);
        assert_eq!(palette.color(TEAL), Color::Rgb(0, 128, 128));
        assert_eq!(palette.color(GRAY), Color::Rgb(192, 192, 192));
    }

    #[test]
    fn the_theme_maps_to_the_matching_basic_colours() {
        let palette = Palette::new(Depth::Basic16);
        assert_eq!(palette.color(TEAL), Color::Cyan);
        assert_eq!(palette.color(GRAY), Color::Gray);
        assert_eq!(palette.color(SHADOW), Color::DarkGray);
        assert_eq!(palette.color(NAVY), Color::Blue);
        assert_eq!(palette.color(WHITE), Color::White);
        assert_eq!(palette.color(BLACK), Color::Black);
    }

    #[test]
    fn the_256_table_uses_the_cube_and_the_gray_ramp() {
        let palette = Palette::new(Depth::Indexed256);
        assert_eq!(palette.color(BLACK), Color::Indexed(16));
        assert_eq!(palette.color(WHITE), Color::Indexed(231));
        assert_eq!(palette.color((0, 0, 255)), Color::Indexed(21));
        // 192 gray sits closer to the ramp's 188 than to the cube's 175
        assert_eq!(palette.color(GRAY), Color::Indexed(250));
        // the ramp rounds: 14 is nearer the ramp's 18 than its 8
        assert_eq!(palette.color((14, 14, 14)), Color::Indexed(233));
    }
}
