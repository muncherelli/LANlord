//! the toolkit-agnostic appearance model: which experience draws the desktop and which
//! named colour scheme recolours it. no front end types live here; the gui and the tui
//! each convert these roles into their own colours.

/// an 8-bit rgb colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// the colour as an `(r, g, b)` tuple.
    pub const fn tuple(self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }
}

/// the look a colour scheme belongs to; a scheme is only valid for its own experience.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperienceId {
    Nt4,
}

/// a colour token, named after the win32 system colour it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Desktop,
    AppWorkspace,
    ActiveCaption,
    ActiveCaptionEnd,
    InactiveCaption,
    InactiveCaptionEnd,
    CaptionText,
    InactiveCaptionText,
    ActiveBorder,
    InactiveBorder,
    WindowFrame,
    Face,
    Light,
    Highlight3d,
    Shadow,
    DarkShadow,
    ButtonText,
    GrayText,
    Window,
    WindowText,
    Selection,
    SelectionText,
    Menu,
    MenuText,
    MenuHot,
    MenuBar,
    ScrollTrack,
    InfoBack,
    InfoText,
    HotLight,
}

impl Role {
    /// how many roles there are; the length of a scheme's table.
    pub const COUNT: usize = Role::HotLight as usize + 1;
}

/// a named table of role colours that one experience accepts.
#[derive(Clone, Copy, Debug)]
pub struct ColorScheme {
    pub name: &'static str,
    pub experience: ExperienceId,
    /// `None` means the role is derived or inherited by the experience.
    pub roles: [Option<Rgb>; Role::COUNT],
}

impl ColorScheme {
    /// the colour for `role`, when the scheme sets one.
    pub const fn get(&self, role: Role) -> Option<Rgb> {
        self.roles[role as usize]
    }
}

const fn c(r: u8, g: u8, b: u8) -> Option<Rgb> {
    Some(Rgb::new(r, g, b))
}

/// the nt4 vga "windows standard" scheme. `Light` is 223, the value the gui and the tui
/// already draw the inner highlight of a raised edge with.
pub const WINDOWS_STANDARD: ColorScheme = ColorScheme {
    name: "Windows Standard",
    experience: ExperienceId::Nt4,
    roles: [
        c(0, 128, 128),   // Desktop
        c(128, 128, 128), // AppWorkspace
        c(0, 0, 128),     // ActiveCaption
        c(0, 0, 128),     // ActiveCaptionEnd
        c(128, 128, 128), // InactiveCaption
        c(128, 128, 128), // InactiveCaptionEnd
        c(255, 255, 255), // CaptionText
        c(192, 192, 192), // InactiveCaptionText
        c(192, 192, 192), // ActiveBorder
        c(192, 192, 192), // InactiveBorder
        c(0, 0, 0),       // WindowFrame
        c(192, 192, 192), // Face
        c(223, 223, 223), // Light
        c(255, 255, 255), // Highlight3d
        c(128, 128, 128), // Shadow
        c(0, 0, 0),       // DarkShadow
        c(0, 0, 0),       // ButtonText
        c(128, 128, 128), // GrayText
        c(255, 255, 255), // Window
        c(0, 0, 0),       // WindowText
        c(0, 0, 128),     // Selection
        c(255, 255, 255), // SelectionText
        c(192, 192, 192), // Menu
        c(0, 0, 0),       // MenuText
        c(0, 0, 128),     // MenuHot
        c(192, 192, 192), // MenuBar
        c(192, 192, 192), // ScrollTrack
        c(255, 255, 225), // InfoBack
        c(0, 0, 0),       // InfoText
        c(0, 0, 255),     // HotLight
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_standard_sets_every_role() {
        assert_eq!(WINDOWS_STANDARD.experience, ExperienceId::Nt4);
        assert!(WINDOWS_STANDARD.roles.iter().all(Option::is_some));
    }

    #[test]
    fn windows_standard_matches_the_nt4_values() {
        let get = |role| WINDOWS_STANDARD.get(role).map(Rgb::tuple);
        assert_eq!(get(Role::Desktop), Some((0, 128, 128)));
        assert_eq!(get(Role::Face), Some((192, 192, 192)));
        assert_eq!(get(Role::ActiveCaption), Some((0, 0, 128)));
        assert_eq!(get(Role::InfoBack), Some((255, 255, 225)));
    }
}
