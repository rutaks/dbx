//! Palette roles and themes. The only module that may name a colour.
//!
//! Derived from herdr (https://github.com/herdrdev/herdr), Apache-2.0,
//! v0.9.5, commit ee5bfb5: the role names come from `Palette` in
//! `src/app/state.rs`, and the values of the default theme (herdr's
//! "catppuccin", Catppuccin Mocha) and of the `terminal` theme are copied from
//! it. dbx-tui is also Apache-2.0. Only these two of herdr's 18 themes are
//! ported so far.
//!
//! View code names a `Role`; a theme maps roles to colours. No colour literal
//! appears anywhere else in the crate (a test greps for it).

use ratatui::style::Color;

/// What a colour is for. These are herdr's role names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Highlight, focused titles, key names in hints.
    Accent,
    /// Floating panels and overlays.
    PanelBg,
    /// The sidebar's fill. `Reset` keeps the terminal background.
    SidebarBg,
    /// The focused or active row.
    ActiveRowBg,
    /// The cursor row.
    SelectionBg,
    Surface0,
    Surface1,
    /// Separators and dividers.
    SurfaceDim,
    /// Muted text: unfocused titles, hint descriptions, section titles.
    Overlay0,
    Overlay1,
    Text,
    Subtext0,
    Mauve,
    Green,
    Yellow,
    Red,
    Blue,
    Teal,
    Peach,
}

impl Role {
    pub const ALL: [Role; 19] = [
        Role::Accent,
        Role::PanelBg,
        Role::SidebarBg,
        Role::ActiveRowBg,
        Role::SelectionBg,
        Role::Surface0,
        Role::Surface1,
        Role::SurfaceDim,
        Role::Overlay0,
        Role::Overlay1,
        Role::Text,
        Role::Subtext0,
        Role::Mauve,
        Role::Green,
        Role::Yellow,
        Role::Red,
        Role::Blue,
        Role::Teal,
        Role::Peach,
    ];
}

/// A theme: one colour per role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    accent: Color,
    panel_bg: Color,
    sidebar_bg: Color,
    active_row_bg: Color,
    selection_bg: Color,
    surface0: Color,
    surface1: Color,
    surface_dim: Color,
    overlay0: Color,
    overlay1: Color,
    text: Color,
    subtext0: Color,
    mauve: Color,
    green: Color,
    yellow: Color,
    red: Color,
    blue: Color,
    teal: Color,
    peach: Color,
}

impl Palette {
    pub const NAMES: [&'static str; 2] = ["catppuccin", "terminal"];

    pub fn color(&self, role: Role) -> Color {
        match role {
            Role::Accent => self.accent,
            Role::PanelBg => self.panel_bg,
            Role::SidebarBg => self.sidebar_bg,
            Role::ActiveRowBg => self.active_row_bg,
            Role::SelectionBg => self.selection_bg,
            Role::Surface0 => self.surface0,
            Role::Surface1 => self.surface1,
            Role::SurfaceDim => self.surface_dim,
            Role::Overlay0 => self.overlay0,
            Role::Overlay1 => self.overlay1,
            Role::Text => self.text,
            Role::Subtext0 => self.subtext0,
            Role::Mauve => self.mauve,
            Role::Green => self.green,
            Role::Yellow => self.yellow,
            Role::Red => self.red,
            Role::Blue => self.blue,
            Role::Teal => self.teal,
            Role::Peach => self.peach,
        }
    }

    pub fn by_name(name: &str) -> Option<Palette> {
        match name {
            "catppuccin" => Some(Self::default_theme()),
            "terminal" => Some(Self::terminal()),
            _ => None,
        }
    }

    /// herdr's default theme (Catppuccin Mocha).
    pub fn default_theme() -> Self {
        Self {
            accent: Color::Rgb(137, 180, 250),
            panel_bg: Color::Rgb(24, 24, 37),
            sidebar_bg: Color::Reset,
            active_row_bg: Color::Rgb(30, 30, 46),
            selection_bg: Color::Rgb(49, 50, 68),
            surface0: Color::Rgb(49, 50, 68),
            surface1: Color::Rgb(69, 71, 90),
            surface_dim: Color::Rgb(30, 30, 46),
            overlay0: Color::Rgb(108, 112, 134),
            overlay1: Color::Rgb(127, 132, 156),
            text: Color::Rgb(205, 214, 244),
            subtext0: Color::Rgb(166, 173, 200),
            mauve: Color::Rgb(203, 166, 247),
            green: Color::Rgb(166, 227, 161),
            yellow: Color::Rgb(249, 226, 175),
            red: Color::Rgb(243, 139, 168),
            blue: Color::Rgb(137, 180, 250),
            teal: Color::Rgb(148, 226, 213),
            peach: Color::Rgb(250, 179, 135),
        }
    }

    /// The terminal's own 16 ANSI colours, for terminals without truecolour.
    pub fn terminal() -> Self {
        Self {
            accent: Color::Blue,
            panel_bg: Color::Reset,
            sidebar_bg: Color::Reset,
            active_row_bg: Color::DarkGray,
            selection_bg: Color::Reset,
            surface0: Color::Reset,
            surface1: Color::DarkGray,
            surface_dim: Color::DarkGray,
            overlay0: Color::Gray,
            overlay1: Color::White,
            text: Color::Reset,
            subtext0: Color::Gray,
            mauve: Color::Gray,
            green: Color::Green,
            yellow: Color::Yellow,
            red: Color::LightRed,
            blue: Color::Blue,
            teal: Color::Cyan,
            peach: Color::Yellow,
        }
    }

    /// Test palette in which every role has its own colour, so a rendered
    /// cell can be traced back to the role that drew it.
    #[cfg(test)]
    pub fn probe() -> Self {
        let c = |i: u8| Color::Rgb(100 + i, 0, 0);
        Self {
            accent: c(0),
            panel_bg: c(1),
            sidebar_bg: c(2),
            active_row_bg: c(3),
            selection_bg: c(4),
            surface0: c(5),
            surface1: c(6),
            surface_dim: c(7),
            overlay0: c(8),
            overlay1: c(9),
            text: c(10),
            subtext0: c(11),
            mauve: c(12),
            green: c(13),
            yellow: c(14),
            red: c(15),
            blue: c(16),
            teal: c(17),
            peach: c(18),
        }
    }

    /// The role a colour belongs to in this palette, if it is unambiguous.
    #[cfg(test)]
    pub fn role_of(&self, color: Color) -> Option<Role> {
        let mut found = Role::ALL.iter().filter(|r| self.color(**r) == color);
        match (found.next(), found.next()) {
            (Some(r), None) => Some(*r),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    #[test]
    fn herdr_default_theme_has_herdrs_catppuccin_values() {
        let p = Palette::default_theme();
        assert_eq!(p.color(Role::Accent), Color::Rgb(137, 180, 250));
        assert_eq!(p.color(Role::SurfaceDim), Color::Rgb(30, 30, 46));
        assert_eq!(p.color(Role::Overlay0), Color::Rgb(108, 112, 134));
        assert_eq!(p.color(Role::SidebarBg), Color::Reset);
    }

    #[test]
    fn terminal_theme_uses_only_the_ansi_colours() {
        let p = Palette::terminal();
        for role in Role::ALL {
            assert!(!matches!(p.color(role), Color::Rgb(..) | Color::Indexed(_)), "{role:?}");
        }
        assert_eq!(p.color(Role::Accent), Color::Blue);
    }

    #[test]
    fn themes_are_found_by_name() {
        assert_eq!(Palette::by_name("catppuccin"), Some(Palette::default_theme()));
        assert_eq!(Palette::by_name("terminal"), Some(Palette::terminal()));
        assert_eq!(Palette::by_name("no-such-theme"), None);
    }

    #[test]
    fn the_probe_palette_gives_every_role_its_own_colour() {
        let p = Palette::probe();
        for a in Role::ALL {
            assert_eq!(p.role_of(p.color(a)), Some(a));
        }
        assert_eq!(p.role_of(Color::Reset), None);
    }
}
