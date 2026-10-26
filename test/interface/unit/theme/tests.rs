use super::Theme;

#[test]
fn theme_wire_values_are_closed_and_round_trip() {
    for theme in [Theme::Light, Theme::Dark] {
        assert!(Theme::parse(theme.as_str()) == Some(theme));
    }
    for value in ["", "auto", "DARK", "unknown"] {
        assert!(Theme::parse(value).is_none());
    }
}
