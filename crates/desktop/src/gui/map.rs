//! World map canvas: countries (Natural Earth 110m), clickable country markers,
//! your location, the exit location and the route between them. Zoom with the
//! wheel or buttons, drag to pan.

use super::style::{alpha, ACCENT, ACCENT2, BG, LAND, MARKER, OK};
use super::Message;
use iced::mouse;
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, LineDash, Path, Stroke};
use iced::{Color, Point, Rectangle, Renderer, Theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct WorldJson {
    w: f32,
    h: f32,
    lat0: f32,
    lat1: f32,
    path: String,
    points: HashMap<String, [f32; 2]>,
}

pub struct World {
    pub w: f32,
    pub h: f32,
    lat0: f32,
    lat1: f32,
    polygons: Vec<Vec<Point>>,
    pub points: HashMap<String, Point>,
}

pub fn world() -> &'static World {
    static W: OnceLock<World> = OnceLock::new();
    W.get_or_init(|| {
        let j: WorldJson = serde_json::from_str(include_str!("../../assets/world.json")).expect("world map");
        let mut polygons = Vec::new();
        for poly in j.path.split('Z').filter(|p| !p.trim().is_empty()) {
            let pts: Vec<Point> = poly
                .trim_start_matches('M')
                .split('L')
                .filter_map(|xy| {
                    let (x, y) = xy.split_once(',')?;
                    Some(Point::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
                })
                .collect();
            if pts.len() > 2 {
                polygons.push(pts);
            }
        }
        World {
            w: j.w,
            h: j.h,
            lat0: j.lat0,
            lat1: j.lat1,
            polygons,
            points: j.points.into_iter().map(|(k, [x, y])| (k, Point::new(x, y))).collect(),
        }
    })
}

/// Map coordinates of a latitude/longitude.
pub fn lat_lon(lat: f32, lon: f32) -> Point {
    let w = world();
    Point::new((lon + 180.0) / 360.0 * w.w, (w.lat0 - lat) / (w.lat0 - w.lat1) * w.h)
}

/// Which part of the map is shown: centre (map units) and zoom (1 = whole world).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub cx: f32,
    pub cy: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        let w = world();
        Self { cx: w.w / 2.0, cy: w.h / 2.0, zoom: 1.0 }
    }
}

impl View {
    pub fn scale(&self, size: iced::Size) -> f32 {
        let w = world();
        (size.width / w.w).min(size.height / w.h) * self.zoom
    }

    /// Keep the view inside the map.
    pub fn clamped(mut self, size: iced::Size) -> Self {
        let w = world();
        self.zoom = self.zoom.clamp(1.0, 12.0);
        let s = self.scale(size);
        let (hw, hh) = (size.width / s / 2.0, size.height / s / 2.0);
        self.cx = if hw * 2.0 >= w.w { w.w / 2.0 } else { self.cx.clamp(hw, w.w - hw) };
        self.cy = if hh * 2.0 >= w.h { w.h / 2.0 } else { self.cy.clamp(hh, w.h - hh) };
        self
    }

    pub fn to_screen(self, p: Point, size: iced::Size) -> Point {
        let s = self.scale(size);
        Point::new((p.x - self.cx) * s + size.width / 2.0, (p.y - self.cy) * s + size.height / 2.0)
    }

    pub fn to_map(self, p: Point, size: iced::Size) -> Point {
        let s = self.scale(size);
        Point::new((p.x - size.width / 2.0) / s + self.cx, (p.y - size.height / 2.0) / s + self.cy)
    }

    /// View that fits the given map points with some margin (whole world if none).
    pub fn fit(points: &[Point], size: iced::Size) -> Self {
        let w = world();
        if points.is_empty() || size.width <= 0.0 {
            return Self::default();
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let base = (size.width / w.w).min(size.height / w.h);
        let span_w = (x1 - x0 + 120.0).max(w.w / 4.5);
        let span_h = y1 - y0 + 120.0;
        let zoom = (size.width / (span_w * base)).min(size.height / (span_h * base)).clamp(1.0, 6.0);
        Self { cx: (x0 + x1) / 2.0, cy: (y0 + y1) / 2.0, zoom }.clamped(size)
    }

    pub fn lerp(self, to: View, t: f32) -> View {
        View { cx: self.cx + (to.cx - self.cx) * t, cy: self.cy + (to.cy - self.cy) * t, zoom: self.zoom + (to.zoom - self.zoom) * t }
    }
}

/// Everything the map draws, taken from the app state each frame.
pub struct MapCanvas<'a> {
    pub view: View,
    /// The size the app last knew about (to keep it in sync).
    pub size: iced::Size,
    pub land: &'a canvas::Cache,
    pub markers: Vec<String>,
    pub selected: Option<String>,
    pub home: Option<Point>,
    pub exit: Option<Point>,
}

#[derive(Default)]
pub struct DragState {
    press: Option<Point>,
    dragging: bool,
}

impl MapCanvas<'_> {
    fn marker_at(&self, cursor: Point, size: iced::Size) -> Option<String> {
        let w = world();
        self.markers
            .iter()
            .filter_map(|cc| Some((cc, self.view.to_screen(*w.points.get(cc)?, size))))
            .map(|(cc, p)| (cc, p.distance(cursor)))
            .filter(|(_, d)| *d < 9.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(cc, _)| cc.clone())
    }
}

impl canvas::Program<Message> for MapCanvas<'_> {
    type State = DragState;

    fn update(&self, state: &mut DragState, event: &Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<Action<Message>> {
        let size = bounds.size();
        if size != self.size && size.width > 0.0 {
            return Some(Action::publish(Message::MapSize(size)));
        }
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let at = cursor.position_in(bounds)?;
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 60.0,
                };
                let factor = if dy > 0.0 { 1.25 } else { 0.8 };
                let anchor = self.view.to_map(at, size);
                let mut v = self.view;
                v.zoom *= factor;
                // Keep the point under the cursor fixed.
                let s = v.scale(size);
                v.cx = anchor.x - (at.x - size.width / 2.0) / s;
                v.cy = anchor.y - (at.y - size.height / 2.0) / s;
                Some(Action::publish(Message::MapView(v.clamped(size))).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.press = Some(cursor.position_in(bounds)?);
                state.dragging = false;
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let from = state.press?;
                let at = cursor.position_in(bounds).unwrap_or(from);
                let d = at - from;
                if !state.dragging && (d.x.abs() + d.y.abs()) < 4.0 {
                    return None;
                }
                state.dragging = true;
                state.press = Some(at);
                let s = self.view.scale(size);
                let v = View { cx: self.view.cx - d.x / s, cy: self.view.cy - d.y / s, ..self.view };
                Some(Action::publish(Message::MapView(v.clamped(size))).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was_drag = state.dragging;
                let pressed = state.press.take();
                state.dragging = false;
                if was_drag || pressed.is_none() {
                    return None;
                }
                let cc = self.marker_at(cursor.position_in(bounds)?, size)?;
                Some(Action::publish(Message::MapClick(cc)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, _state: &DragState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, cursor: mouse::Cursor) -> Vec<Geometry> {
        let size = bounds.size();
        let w = world();
        let view = self.view;
        let land = self.land.draw(renderer, size, |f: &mut Frame| {
            let path = Path::new(|b| {
                for poly in &w.polygons {
                    b.move_to(view.to_screen(poly[0], size));
                    for p in &poly[1..] {
                        b.line_to(view.to_screen(*p, size));
                    }
                    b.close();
                }
            });
            f.fill(&path, LAND);
            f.stroke(&path, Stroke::default().with_color(BG).with_width(0.7));
        });

        let mut f = Frame::new(renderer, size);
        let hover = cursor.position_in(bounds).and_then(|c| self.marker_at(c, size));
        for cc in &self.markers {
            let Some(p) = w.points.get(cc) else { continue };
            let p = view.to_screen(*p, size);
            let color = if self.selected.as_ref() == Some(cc) {
                ACCENT
            } else if hover.as_ref() == Some(cc) {
                ACCENT2
            } else {
                MARKER
            };
            let a = 5.0;
            let tri = Path::new(|b| {
                b.move_to(Point::new(p.x - a, p.y - a * 0.9));
                b.line_to(Point::new(p.x + a, p.y - a * 0.9));
                b.line_to(Point::new(p.x, p.y + a));
                b.close();
            });
            f.fill(&tri, color);
        }
        let home = self.home.map(|p| view.to_screen(p, size));
        let exit = self.exit.map(|p| view.to_screen(p, size));
        if let (Some(h), Some(x)) = (home, exit) {
            let mid = Point::new((h.x + x.x) / 2.0, h.y.min(x.y) - (h.x - x.x).abs() * 0.22 - 10.0);
            let route = Path::new(|b| {
                b.move_to(h);
                b.quadratic_curve_to(mid, x);
            });
            f.stroke(&route, Stroke {
                line_dash: LineDash { segments: &[6.0, 5.0], offset: 0 },
                ..Stroke::default().with_color(ACCENT2).with_width(1.8)
            });
        }
        if let Some(x) = exit {
            f.fill(&Path::circle(x, 11.0), alpha(OK, 0.22));
            f.fill(&Path::circle(x, 5.0), OK);
            f.stroke(&Path::circle(x, 5.0), Stroke::default().with_color(Color::WHITE).with_width(1.5));
        }
        if let Some(h) = home {
            f.fill(&Path::circle(h, 5.5), ACCENT);
            f.stroke(&Path::circle(h, 5.5), Stroke::default().with_color(Color::WHITE).with_width(1.5));
        }
        vec![land, f.into_geometry()]
    }

    fn mouse_interaction(&self, state: &DragState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if state.dragging {
            mouse::Interaction::Grabbing
        } else if cursor.position_in(bounds).and_then(|c| self.marker_at(c, bounds.size())).is_some() {
            mouse::Interaction::Pointer
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Session speed chart (last 60 seconds): download green, upload red.
pub struct Chart<'a> {
    pub samples: &'a std::collections::VecDeque<(f32, f32)>,
}

impl canvas::Program<Message> for Chart<'_> {
    type State = ();

    fn draw(&self, _: &(), renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let (w, h) = (bounds.width, bounds.height);
        let mut f = Frame::new(renderer, bounds.size());
        let grid = Stroke::default().with_color(super::style::LINE).with_width(1.0);
        for i in 0..=12 {
            let x = (i as f32 * w / 12.0).round() + 0.5;
            f.stroke(&Path::line(Point::new(x, 0.0), Point::new(x, h)), grid);
        }
        for i in 0..=4 {
            let y = (i as f32 * (h - 1.0) / 4.0).round() + 0.5;
            f.stroke(&Path::line(Point::new(0.0, y), Point::new(w, y)), grid);
        }
        let max = self.samples.iter().fold(1024.0f32, |m, s| m.max(s.0).max(s.1)) * 1.15;
        let n = self.samples.len();
        if n >= 2 {
            for (k, color) in [(0usize, OK), (1usize, super::style::BAD)] {
                let path = Path::new(|b| {
                    for (i, s) in self.samples.iter().enumerate() {
                        let v = if k == 0 { s.0 } else { s.1 };
                        let p = Point::new(w - (n - 1 - i) as f32 * w / 59.0, h - 2.0 - v / max * (h - 6.0));
                        if i == 0 {
                            b.move_to(p);
                        } else {
                            b.line_to(p);
                        }
                    }
                });
                f.stroke(&path, Stroke::default().with_color(color).with_width(1.8));
            }
        }
        vec![f.into_geometry()]
    }
}

pub fn chart_max(samples: &std::collections::VecDeque<(f32, f32)>) -> f32 {
    samples.iter().fold(1024.0f32, |m, s| m.max(s.0).max(s.1)) * 1.15
}
