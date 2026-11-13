//! Colours and widget styles (dark, ProtonVPN-like).

use iced::border::Radius;
use iced::widget::{button, container, text_input};
use iced::{gradient, Background, Border, Color, Degrees, Font, Shadow, Theme, Vector};

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

pub const BG: Color = rgb(0x16171b);
pub const SIDE: Color = rgb(0x1d1e23);
pub const CARD: Color = rgb(0x24252b);
pub const RAISE: Color = rgb(0x2c2d34);
pub const LINE: Color = rgb(0x33343c);
pub const TEXT: Color = rgb(0xececf1);
pub const MUTED: Color = rgb(0x9b9caa);
pub const FAINT: Color = rgb(0x6c6d7a);
pub const ACCENT: Color = rgb(0x7a5cff);
pub const ACCENT2: Color = rgb(0x9b85ff);
pub const OK: Color = rgb(0x2fd08f);
pub const WARN: Color = rgb(0xf5b13d);
pub const BAD: Color = rgb(0xf2546f);
pub const LAND: Color = rgb(0x2a2b33);
pub const MARKER: Color = rgb(0x5d5e6c);

pub const BOLD: Font = Font { weight: iced::font::Weight::Bold, ..Font::DEFAULT };
pub const SEMIBOLD: Font = Font { weight: iced::font::Weight::Semibold, ..Font::DEFAULT };

pub fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

pub fn theme() -> Theme {
    Theme::custom(
        "Geo Unlocker",
        iced::theme::Palette { background: BG, text: TEXT, primary: ACCENT, success: OK, warning: WARN, danger: BAD },
    )
}

fn border(color: Color, width: f32, radius: f32) -> Border {
    Border { color, width, radius: Radius::from(radius) }
}

/// Flat panel with optional border.
pub fn panel(bg: Color, radius: f32, edge: Option<Color>) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(bg.into()),
        border: border(edge.unwrap_or(Color::TRANSPARENT), if edge.is_some() { 1.0 } else { 0.0 }, radius),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

/// Floating card (drawers, modals, toasts).
pub fn floating(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SIDE.into()),
        border: border(LINE, 1.0, 14.0),
        shadow: Shadow { color: alpha(Color::BLACK, 0.45), offset: Vector::new(0.0, 10.0), blur_radius: 40.0 },
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn scrim(_: &Theme) -> container::Style {
    container::Style { background: Some(alpha(Color::BLACK, 0.45).into()), ..Default::default() }
}

/// The connection card's background tint: red (off), amber (connecting), purple (on).
pub fn conn_card(state: &'static str) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let (a, b) = match state {
            "on" => (alpha(ACCENT, 0.35), alpha(OK, 0.06)),
            "wait" => (alpha(WARN, 0.22), alpha(SIDE, 0.0)),
            _ => (alpha(BAD, 0.20), alpha(SIDE, 0.0)),
        };
        container::Style {
            background: Some(Background::Gradient(
                gradient::Linear::new(Degrees(135.0)).add_stop(0.0, a).add_stop(0.75, b).into(),
            )),
            text_color: Some(TEXT),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    Primary,
    Ghost,
    Outline,
    Danger,
    Tab(bool),
    Icon,
    Chip,
    Connect(bool),
}

pub fn btn(kind: Btn) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        let (bg, fg, edge, radius) = match kind {
            Btn::Primary => (if hover { ACCENT2 } else { ACCENT }, Color::WHITE, None, 10.0),
            Btn::Ghost => (if hover { LINE } else { RAISE }, TEXT, None, 9.0),
            Btn::Outline => (if hover { alpha(Color::WHITE, 0.07) } else { Color::TRANSPARENT }, TEXT, Some(alpha(Color::WHITE, 0.55)), 10.0),
            Btn::Danger => (if hover { alpha(BAD, 0.85) } else { BAD }, Color::WHITE, None, 9.0),
            Btn::Tab(on) => (if on { RAISE } else if hover { alpha(RAISE, 0.5) } else { Color::TRANSPARENT }, if on { TEXT } else { MUTED }, None, 8.0),
            Btn::Icon => (if hover { RAISE } else { Color::TRANSPARENT }, if hover { TEXT } else { MUTED }, None, 9.0),
            Btn::Chip => (if hover { RAISE } else { CARD }, TEXT, Some(LINE), 999.0),
            Btn::Connect(active) => (
                if hover { alpha(ACCENT, 0.18) } else { Color::TRANSPARENT },
                if active { OK } else if hover { TEXT } else { MUTED },
                Some(if active { OK } else if hover { ACCENT } else { LINE }),
                999.0,
            ),
        };
        button::Style {
            background: Some(if disabled { alpha(bg, 0.5) } else { bg }.into()),
            text_color: if disabled { alpha(fg, 0.5) } else { fg },
            border: border(edge.unwrap_or(Color::TRANSPARENT), if edge.is_some() { 1.5 } else { 0.0 }, radius),
            ..Default::default()
        }
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: BG.into(),
        border: border(if focused { ACCENT } else { LINE }, 1.0, 9.0),
        icon: FAINT,
        placeholder: FAINT,
        value: TEXT,
        selection: alpha(ACCENT, 0.4),
    }
}

/// A list row; the connected one is tinted.
pub fn row_style(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: if active { Some(alpha(ACCENT, 0.14).into()) } else { None },
        border: border(Color::TRANSPARENT, 0.0, 9.0),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn pill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(color, 0.15).into()),
        border: border(Color::TRANSPARENT, 0.0, 999.0),
        text_color: Some(color),
        ..Default::default()
    }
}
