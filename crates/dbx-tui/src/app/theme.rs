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
    SynKeyword,
    SynType,
    SynString,
    SynNumber,
    SynComment,
    SynFunction,
    SynIdentifier,
    SynPunct,
    SynBad,
    EditorBg,
    CurrentLine,
    LineNumber,
    LineNumberActive,
    MatchedBrace,
    RangeToExecute,
    /// The completion popup's fill.
    PopupBg,
}

impl Role {
    pub const ALL: [Role; 35] = [
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
        Role::SynKeyword,
        Role::SynType,
        Role::SynString,
        Role::SynNumber,
        Role::SynComment,
        Role::SynFunction,
        Role::SynIdentifier,
        Role::SynPunct,
        Role::SynBad,
        Role::EditorBg,
        Role::CurrentLine,
        Role::LineNumber,
        Role::LineNumberActive,
        Role::MatchedBrace,
        Role::RangeToExecute,
        Role::PopupBg,
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
    syn_keyword: Color,
    syn_type: Color,
    syn_string: Color,
    syn_number: Color,
    syn_comment: Color,
    syn_function: Color,
    syn_identifier: Color,
    syn_punct: Color,
    syn_bad: Color,
    editor_bg: Color,
    current_line: Color,
    line_number: Color,
    line_number_active: Color,
    matched_brace: Color,
    range_to_execute: Color,
    popup_bg: Color,
    /// A selection is drawn reversed: for a theme whose selection colour is
    /// the terminal's own background, where no colour would show.
    selection_reversed: bool,
}

impl Palette {
    pub const NAMES: [&'static str; 3] = ["catppuccin", "terminal", "intellij-dark"];

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
            Role::SynKeyword => self.syn_keyword,
            Role::SynType => self.syn_type,
            Role::SynString => self.syn_string,
            Role::SynNumber => self.syn_number,
            Role::SynComment => self.syn_comment,
            Role::SynFunction => self.syn_function,
            Role::SynIdentifier => self.syn_identifier,
            Role::SynPunct => self.syn_punct,
            Role::SynBad => self.syn_bad,
            Role::EditorBg => self.editor_bg,
            Role::CurrentLine => self.current_line,
            Role::LineNumber => self.line_number,
            Role::LineNumberActive => self.line_number_active,
            Role::MatchedBrace => self.matched_brace,
            Role::RangeToExecute => self.range_to_execute,
            Role::PopupBg => self.popup_bg,
        }
    }

    /// What a role adds to its colour: function calls are italic, a matched
    /// brace bold (as in IntelliJ), a selection reversed in a theme that has no
    /// colour to give it.
    pub fn modifier(&self, role: Role) -> ratatui::style::Modifier {
        use ratatui::style::Modifier;
        match role {
            Role::SynFunction => Modifier::ITALIC,
            Role::MatchedBrace => Modifier::BOLD,
            Role::SelectionBg if self.selection_reversed => Modifier::REVERSED,
            _ => Modifier::empty(),
        }
    }

    /// The theme `DBX_TUI_THEME` names (a stand-in for a config file), or
    /// herdr's default when it is unset or names none we have.
    pub fn from_setting(value: Option<&str>) -> Palette {
        value.and_then(|v| Self::by_name(v.trim())).unwrap_or_else(Self::default_theme)
    }

    pub fn by_name(name: &str) -> Option<Palette> {
        match name {
            "catppuccin" => Some(Self::default_theme()),
            "terminal" => Some(Self::terminal()),
            "intellij-dark" => Some(Self::intellij_dark()),
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
            syn_keyword: Color::Rgb(250, 179, 135),
            syn_type: Color::Rgb(250, 179, 135),
            syn_string: Color::Rgb(166, 227, 161),
            syn_number: Color::Rgb(148, 226, 213),
            syn_comment: Color::Rgb(108, 112, 134),
            syn_function: Color::Rgb(137, 180, 250),
            syn_identifier: Color::Rgb(205, 214, 244),
            syn_punct: Color::Rgb(205, 214, 244),
            syn_bad: Color::Rgb(243, 139, 168),
            editor_bg: Color::Reset,
            current_line: Color::Rgb(30, 30, 46),
            line_number: Color::Rgb(108, 112, 134),
            line_number_active: Color::Rgb(166, 173, 200),
            matched_brace: Color::Rgb(69, 71, 90),
            range_to_execute: Color::Rgb(166, 227, 161),
            popup_bg: Color::Rgb(24, 24, 37),
            selection_reversed: false,
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
            syn_keyword: Color::Yellow,
            syn_type: Color::Yellow,
            syn_string: Color::Green,
            syn_number: Color::Cyan,
            syn_comment: Color::Gray,
            syn_function: Color::Blue,
            syn_identifier: Color::Reset,
            syn_punct: Color::Reset,
            syn_bad: Color::LightRed,
            editor_bg: Color::Reset,
            current_line: Color::DarkGray,
            line_number: Color::DarkGray,
            line_number_active: Color::Gray,
            matched_brace: Color::Magenta,
            range_to_execute: Color::Green,
            popup_bg: Color::DarkGray,
            selection_reversed: true,
        }
    }


    /// IntelliJ's Dark scheme. The syntax colours, editor background, current
    /// line, selection, line numbers, matched brace and range-to-execute are
    /// the values in the research notes (`intellij-editor.md`, section 3); the
    /// roles the notes do not cover (accent, panels, surfaces) are our own
    /// choice from the same family of greys and blues.
    pub fn intellij_dark() -> Self {
        let rgb = |hex: u32| Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        Self {
            accent: rgb(0x548AF7),
            panel_bg: rgb(0x2B2D30),
            sidebar_bg: rgb(0x2B2D30),
            active_row_bg: rgb(0x2E436E),
            selection_bg: rgb(0x214283),
            surface0: rgb(0x393B40),
            surface1: rgb(0x43454A),
            surface_dim: rgb(0x393B40),
            overlay0: rgb(0x6F737A),
            overlay1: rgb(0x868A91),
            text: rgb(0xBCBEC4),
            subtext0: rgb(0xA1A3AB),
            mauve: rgb(0xC77DBB),
            green: rgb(0x6AAB73),
            yellow: rgb(0xF2C55C),
            red: rgb(0xF75464),
            blue: rgb(0x57AAF7),
            teal: rgb(0x2AACB8),
            peach: rgb(0xCF8E6D),
            syn_keyword: rgb(0xCF8E6D),
            syn_type: rgb(0xCF8E6D),
            syn_string: rgb(0x6AAB73),
            syn_number: rgb(0x2AACB8),
            syn_comment: rgb(0x7A7E85),
            syn_function: rgb(0x57AAF7),
            syn_identifier: rgb(0xBCBEC4),
            syn_punct: rgb(0xBCBEC4),
            syn_bad: rgb(0xF75464),
            editor_bg: rgb(0x1E1F22),
            current_line: rgb(0x26282E),
            line_number: rgb(0x4B5059),
            line_number_active: rgb(0xA1A3AB),
            matched_brace: rgb(0x43454A),
            range_to_execute: rgb(0x3D7A49),
            // IntelliJ's popups and lookup lists: #2B2D30.
            popup_bg: rgb(0x2B2D30),
            selection_reversed: false,
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
            syn_keyword: c(19),
            syn_type: c(20),
            syn_string: c(21),
            syn_number: c(22),
            syn_comment: c(23),
            syn_function: c(24),
            syn_identifier: c(25),
            syn_punct: c(26),
            syn_bad: c(27),
            editor_bg: c(28),
            current_line: c(29),
            line_number: c(30),
            line_number_active: c(31),
            matched_brace: c(32),
            range_to_execute: c(33),
            popup_bg: c(34),
            selection_reversed: false,
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

    #[test]
    fn in_herdrs_themes_syntax_roles_borrow_the_palette_roles_the_slice_names() {
        for p in [Palette::default_theme(), Palette::terminal()] {
            assert_eq!(p.color(Role::SynKeyword), p.color(Role::Peach));
            assert_eq!(p.color(Role::SynType), p.color(Role::Peach));
            assert_eq!(p.color(Role::SynString), p.color(Role::Green));
            assert_eq!(p.color(Role::SynNumber), p.color(Role::Teal));
            assert_eq!(p.color(Role::SynComment), p.color(Role::Overlay0));
            assert_eq!(p.color(Role::SynFunction), p.color(Role::Blue));
            assert_eq!(p.color(Role::SynIdentifier), p.color(Role::Text));
            assert_eq!(p.color(Role::SynPunct), p.color(Role::Text));
            assert_eq!(p.color(Role::SynBad), p.color(Role::Red));
        }
        assert!(Palette::default_theme().modifier(Role::SynFunction).contains(ratatui::style::Modifier::ITALIC));
        assert!(!Palette::default_theme().modifier(Role::SynKeyword).contains(ratatui::style::Modifier::ITALIC));
    }

    #[test]
    fn the_caret_rows_line_number_is_brighter_than_the_others_in_every_theme() {
        for name in Palette::NAMES {
            let p = Palette::by_name(name).expect(name);
            assert_ne!(p.color(Role::LineNumber), p.color(Role::LineNumberActive), "{name}");
            assert_ne!(p.color(Role::SynKeyword), p.color(Role::SynIdentifier), "{name}: keywords must differ from identifiers");
            assert_ne!(p.color(Role::SynComment), p.color(Role::SynIdentifier), "{name}");
        }
    }

    #[test]
    fn intellij_dark_has_intellijs_exact_dark_values() {
        let p = Palette::by_name("intellij-dark").expect("the theme exists");
        let rgb = |r, g, b| Color::Rgb(r, g, b);
        assert_eq!(p.color(Role::SynKeyword), rgb(0xCF, 0x8E, 0x6D));
        assert_eq!(p.color(Role::SynType), rgb(0xCF, 0x8E, 0x6D));
        assert_eq!(p.color(Role::SynString), rgb(0x6A, 0xAB, 0x73));
        assert_eq!(p.color(Role::SynNumber), rgb(0x2A, 0xAC, 0xB8));
        assert_eq!(p.color(Role::SynComment), rgb(0x7A, 0x7E, 0x85));
        assert_eq!(p.color(Role::SynFunction), rgb(0x57, 0xAA, 0xF7));
        assert_eq!(p.color(Role::SynIdentifier), rgb(0xBC, 0xBE, 0xC4));
        assert_eq!(p.color(Role::SynPunct), rgb(0xBC, 0xBE, 0xC4));
        assert_eq!(p.color(Role::SynBad), rgb(0xF7, 0x54, 0x64));
        assert_eq!(p.color(Role::Text), rgb(0xBC, 0xBE, 0xC4));
        assert_eq!(p.color(Role::EditorBg), rgb(0x1E, 0x1F, 0x22));
        assert_eq!(p.color(Role::CurrentLine), rgb(0x26, 0x28, 0x2E));
        assert_eq!(p.color(Role::SelectionBg), rgb(0x21, 0x42, 0x83));
        assert_eq!(p.color(Role::LineNumber), rgb(0x4B, 0x50, 0x59));
        assert_eq!(p.color(Role::LineNumberActive), rgb(0xA1, 0xA3, 0xAB));
        assert_eq!(p.color(Role::MatchedBrace), rgb(0x43, 0x45, 0x4A));
        assert_eq!(p.color(Role::RangeToExecute), rgb(0x3D, 0x7A, 0x49));
        assert_eq!(p.color(Role::PopupBg), rgb(0x2B, 0x2D, 0x30));
        assert!(p.modifier(Role::SynFunction).contains(ratatui::style::Modifier::ITALIC));
    }

    #[test]
    fn the_theme_setting_picks_a_theme_by_name_and_falls_back_to_the_default() {
        assert_eq!(Palette::from_setting(None), Palette::default_theme());
        assert_eq!(Palette::from_setting(Some("intellij-dark")), Palette::intellij_dark());
        assert_eq!(Palette::from_setting(Some(" terminal\n")), Palette::terminal());
        assert_eq!(Palette::from_setting(Some("nonsense")), Palette::default_theme());
    }

    #[test]
    fn in_every_theme_a_selection_shows_and_a_matched_brace_is_not_the_current_line() {
        let look = |p: &Palette, role| (p.color(role), p.modifier(role));
        for name in Palette::NAMES {
            let p = Palette::by_name(name).expect(name);
            assert_ne!(look(&p, Role::SelectionBg), look(&p, Role::EditorBg), "{name}: a selection must show on the editor background");
            assert_ne!(look(&p, Role::SelectionBg), look(&p, Role::CurrentLine), "{name}: a selection must show on the caret row");
            assert_ne!(look(&p, Role::MatchedBrace), look(&p, Role::CurrentLine), "{name}: a matched brace must show on the caret row");
        }
    }

    #[test]
    fn in_every_theme_the_popup_is_told_from_the_editor_and_its_selected_row_shows_on_it() {
        let look = |p: &Palette, role| (p.color(role), p.modifier(role));
        for name in Palette::NAMES {
            let p = Palette::by_name(name).expect(name);
            assert_ne!(look(&p, Role::PopupBg), look(&p, Role::EditorBg), "{name}: the popup must stand out from the editor");
            assert_ne!(look(&p, Role::SelectionBg), look(&p, Role::PopupBg), "{name}: the selected row must show on the popup");
        }
    }

    #[test]
    fn every_listed_theme_name_is_found() {
        assert!(Palette::NAMES.contains(&"intellij-dark"));
        for name in Palette::NAMES {
            assert!(Palette::by_name(name).is_some(), "{name}");
        }
    }
}
