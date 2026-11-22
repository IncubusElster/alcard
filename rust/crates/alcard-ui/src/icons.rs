//! Native Alcard pixel icons. Geometry is authored independently for 16 and
//! 24 pixels; tiny-skia performs all rasterization and alpha composition.
//! Empty areas stay transparent, including the holes of Outline icons.
use crate::theme::{IconStyle, Theme};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

type Contour = [(i32, i32)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconKind {
    Terminal,
    Files,
    Settings,
    Web,
    Network,
    Music,
    Games,
    Calendar,
    Notes,
    Image,
    Generic,
    Info,
    Warning,
    Error,
    Notifications,
}

impl IconKind {
    /// Exact, bounded semantic aliases; never guess a category from a hash.
    /// Absolute paths are left to the caller's local image resolver.
    pub fn from_name(value: &str) -> Option<Self> {
        if value.len() > 256 || value.contains(['/', '\\']) {
            return None;
        }
        let value = value.to_ascii_lowercase();
        let value = value
            .strip_suffix(".desktop")
            .or_else(|| value.strip_suffix(".png"))
            .unwrap_or(&value);
        Some(match value {
            "utilities-terminal" | "terminal" | "terminal-emulator" | "org.gnome.terminal"
            | "foot" | "alacritty" | "kitty" | "konsole" | "xterm" | "alcard-terminal" => {
                Self::Terminal
            }
            "system-file-manager"
            | "folder"
            | "inode-directory"
            | "file-manager"
            | "org.gnome.nautilus"
            | "nautilus"
            | "thunar"
            | "pcmanfm"
            | "org.kde.dolphin"
            | "dolphin"
            | "alcard-files" => Self::Files,
            "preferences-system"
            | "preferences-desktop"
            | "settings"
            | "systemsettings"
            | "org.gnome.settings"
            | "org.kde.systemsettings"
            | "alcard-settings" => Self::Settings,
            "web-browser"
            | "internet-web-browser"
            | "firefox"
            | "org.mozilla.firefox"
            | "chromium"
            | "chromium-browser"
            | "google-chrome"
            | "org.gnome.epiphany"
            | "alcard-browser" => Self::Web,
            "network-workgroup"
            | "network-wired"
            | "network-wireless"
            | "network-transmit-receive"
            | "network-manager"
            | "nm-connection-editor"
            | "alcard-network" => Self::Network,
            "multimedia-player" | "audio-x-generic" | "audio-player" | "music" | "rhythmbox"
            | "org.gnome.music" | "vlc" | "mpv" | "alcard-music" => Self::Music,
            "applications-games" | "input-gaming" | "games" | "alcard-games" => Self::Games,
            "x-office-calendar" | "office-calendar" | "calendar" | "org.gnome.calendar"
            | "alcard-calendar" => Self::Calendar,
            "accessories-text-editor"
            | "text-editor"
            | "text-x-generic"
            | "notes"
            | "org.gnome.texteditor"
            | "org.gnome.gedit"
            | "mousepad"
            | "leafpad"
            | "alcard-notes" => Self::Notes,
            "image-x-generic"
            | "image-viewer"
            | "applications-graphics"
            | "org.gnome.loupe"
            | "org.gnome.eog"
            | "ristretto"
            | "alcard-image" => Self::Image,
            "dialog-information" | "information" | "info" => Self::Info,
            "dialog-warning" | "warning" => Self::Warning,
            "dialog-error" | "error" => Self::Error,
            "preferences-system-notifications" | "notification" | "notifications" => {
                Self::Notifications
            }
            "application-x-executable"
            | "application-default-icon"
            | "application-x-generic"
            | "alcard-shell" => Self::Generic,
            _ => return None,
        })
    }
}

fn blend(a: u32, b: u32, amount: u32) -> u32 {
    let channel = |shift: u32| -> u32 {
        let a = (a >> shift) & 255_u32;
        let b = (b >> shift) & 255_u32;
        (a * (100 - amount) + b * amount + 50) / 100
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

struct Palette {
    main: u32,
    light: u32,
    shade: u32,
    edge: u32,
    ink: u32,
    other: u32,
}
impl Palette {
    fn new(kind: IconKind, theme: &Theme) -> Self {
        let (main, other) = match kind {
            IconKind::Terminal => (0x509bc9, 0xc2edf2),
            IconKind::Files => (0xe5b75c, 0xf4d78e),
            IconKind::Settings => (0x8eacc2, 0xd9e8f0),
            IconKind::Web => (0x59a7d4, 0x8ed3c2),
            IconKind::Network => (0x6bb8bb, 0xc2e7da),
            IconKind::Music => (0xb28bd5, 0xe2bee6),
            IconKind::Games => (0xd78293, 0xf1ce79),
            IconKind::Calendar => (0xd9e4e9, 0xd9777d),
            IconKind::Notes => (0xe4d49c, 0x779dc6),
            IconKind::Image => (0x89bad3, 0x82b88d),
            IconKind::Generic => (0x9fb8d2, 0xdbe9f1),
            IconKind::Info => (theme.accent, theme.text),
            IconKind::Warning => (theme.warning, 0x644a25),
            IconKind::Error => (theme.error, theme.text),
            IconKind::Notifications => (0xe5bd68, 0xf7df9c),
        };
        let main = blend(main, theme.accent, 8);
        Self {
            main,
            light: blend(main, 0xffffff, 42),
            shade: blend(main, 0x243a4c, 30),
            edge: blend(main, 0x172a3b, 55),
            ink: 0xf2f7fa,
            other,
        }
    }
}

struct Icon<'a> {
    canvas: &'a mut Pixmap,
    transform: Transform,
    small: bool,
    solid: bool,
    palette: Palette,
}
impl Icon<'_> {
    fn color(value: u32) -> Paint<'static> {
        let mut paint = Paint {
            anti_alias: false,
            ..Paint::default()
        };
        paint.set_color_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255);
        paint
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
            self.canvas
                .fill_rect(rect, &Self::color(color), self.transform, None);
        }
    }
    fn path(points: &[(i32, i32)], builder: &mut PathBuilder) {
        if let Some(&(x, y)) = points.first() {
            builder.move_to(x as f32, y as f32);
            for &(x, y) in &points[1..] {
                builder.line_to(x as f32, y as f32);
            }
            builder.close();
        }
    }
    fn polygon(&mut self, points: &[(i32, i32)], color: u32) {
        let mut path = PathBuilder::new();
        Self::path(points, &mut path);
        if let Some(path) = path.finish() {
            self.canvas.fill_path(
                &path,
                &Self::color(color),
                FillRule::Winding,
                self.transform,
                None,
            );
        }
    }
    fn line(&mut self, points: &[(i32, i32)], width: f32, color: u32) {
        let mut path = PathBuilder::new();
        if let Some(&(x, y)) = points.first() {
            path.move_to(x as f32 + 0.5, y as f32 + 0.5);
            for &(x, y) in &points[1..] {
                path.line_to(x as f32 + 0.5, y as f32 + 0.5);
            }
        }
        if let Some(path) = path.finish() {
            self.canvas.stroke_path(
                &path,
                &Self::color(color),
                &Stroke {
                    width,
                    ..Stroke::default()
                },
                self.transform,
                None,
            );
        }
    }
    fn chamfer(x: i32, y: i32, w: i32, h: i32, corner: i32) -> [(i32, i32); 8] {
        [
            (x + corner, y),
            (x + w - corner, y),
            (x + w, y + corner),
            (x + w, y + h - corner),
            (x + w - corner, y + h),
            (x + corner, y + h),
            (x, y + h - corner),
            (x, y + corner),
        ]
    }
    fn panel(&mut self, x: i32, y: i32, w: i32, h: i32, corner: i32, color: u32) {
        let outer = Self::chamfer(x, y, w, h, corner);
        if self.solid {
            self.polygon(&outer, color);
        }
        let mut ring = PathBuilder::new();
        Self::path(&outer, &mut ring);
        Self::path(
            &Self::chamfer(x + 1, y + 1, w - 2, h - 2, (corner - 1).max(0)),
            &mut ring,
        );
        if let Some(path) = ring.finish() {
            let edge = if self.solid {
                self.palette.edge
            } else {
                self.palette.main
            };
            self.canvas.fill_path(
                &path,
                &Self::color(edge),
                FillRule::EvenOdd,
                self.transform,
                None,
            );
        }
        if self.solid && w >= 8 && h >= 6 {
            self.rect(
                x + corner.max(2),
                y + 1,
                w - corner.max(2) * 2,
                1,
                self.palette.light,
            );
            self.rect(
                x + corner.max(2),
                y + h - 2,
                w - corner.max(2) * 2,
                1,
                self.palette.shade,
            );
        }
    }
    fn body(&mut self, points: &[(i32, i32)], color: u32) {
        if self.solid {
            self.polygon(points, color);
        }
        let mut edge = points.to_vec();
        if let Some(point) = points.first() {
            edge.push(*point);
        }
        self.line(
            &edge,
            1.0,
            if self.solid {
                self.palette.edge
            } else {
                self.palette.main
            },
        );
    }
    fn ink(&self) -> u32 {
        if self.solid {
            self.palette.ink
        } else {
            self.palette.light
        }
    }

    fn terminal(&mut self) {
        let (x, y, w, h, prompt, baseline, bar) = if self.small {
            (1, 3, 14, 10, 4, 7, 9)
        } else {
            (2, 4, 20, 15, 6, 10, 13)
        };
        self.panel(x, y, w, h, 2, self.palette.shade);
        self.line(
            &[
                (prompt, baseline - 1),
                (prompt + 2, baseline + 1),
                (prompt, baseline + 3),
            ],
            1.0,
            self.ink(),
        );
        self.rect(
            bar,
            baseline + 3,
            if self.small { 3 } else { 5 },
            1,
            self.ink(),
        );
        if self.small {
            self.rect(7, 13, 2, 1, self.palette.main);
            self.rect(4, 14, 8, 1, self.palette.main);
        } else {
            self.rect(11, 19, 2, 2, self.palette.main);
            self.rect(7, 21, 10, 1, self.palette.main);
        }
    }
    fn files(&mut self) {
        if self.small {
            self.body(
                &[
                    (2, 3),
                    (6, 3),
                    (8, 5),
                    (13, 5),
                    (14, 6),
                    (14, 13),
                    (2, 13),
                    (1, 12),
                    (1, 4),
                ],
                self.palette.shade,
            );
            self.panel(1, 6, 14, 8, 1, self.palette.main);
        } else {
            self.body(
                &[
                    (3, 4),
                    (9, 4),
                    (11, 7),
                    (20, 7),
                    (21, 8),
                    (21, 19),
                    (3, 19),
                    (2, 18),
                    (2, 5),
                ],
                self.palette.shade,
            );
            self.panel(2, 9, 20, 12, 2, self.palette.main);
        }
    }
    fn settings(&mut self) {
        let (outer, hole): (&Contour, &Contour) = if self.small {
            (
                &[
                    (6, 1),
                    (10, 1),
                    (10, 3),
                    (12, 4),
                    (14, 3),
                    (15, 6),
                    (13, 7),
                    (13, 9),
                    (15, 10),
                    (14, 13),
                    (12, 12),
                    (10, 13),
                    (10, 15),
                    (6, 15),
                    (6, 13),
                    (4, 12),
                    (2, 13),
                    (1, 10),
                    (3, 9),
                    (3, 7),
                    (1, 6),
                    (2, 3),
                    (4, 4),
                    (6, 3),
                ],
                &[
                    (7, 5),
                    (9, 5),
                    (11, 7),
                    (11, 9),
                    (9, 11),
                    (7, 11),
                    (5, 9),
                    (5, 7),
                ],
            )
        } else {
            (
                &[
                    (9, 2),
                    (15, 2),
                    (15, 5),
                    (18, 6),
                    (20, 4),
                    (23, 9),
                    (20, 10),
                    (20, 14),
                    (23, 15),
                    (20, 20),
                    (18, 18),
                    (15, 19),
                    (15, 22),
                    (9, 22),
                    (9, 19),
                    (6, 18),
                    (4, 20),
                    (1, 15),
                    (4, 14),
                    (4, 10),
                    (1, 9),
                    (4, 4),
                    (6, 6),
                    (9, 5),
                ],
                &[
                    (10, 8),
                    (14, 8),
                    (16, 10),
                    (16, 14),
                    (14, 16),
                    (10, 16),
                    (8, 14),
                    (8, 10),
                ],
            )
        };
        if self.solid {
            let mut body = PathBuilder::new();
            Self::path(outer, &mut body);
            Self::path(hole, &mut body);
            if let Some(path) = body.finish() {
                self.canvas.fill_path(
                    &path,
                    &Self::color(self.palette.main),
                    FillRule::EvenOdd,
                    self.transform,
                    None,
                );
            }
        }
        for points in [outer, hole] {
            let mut contour = points.to_vec();
            contour.push(points[0]);
            self.line(
                &contour,
                1.0,
                if self.solid {
                    self.palette.edge
                } else {
                    self.palette.main
                },
            );
        }
        let (x, y, w) = if self.small { (7, 2, 2) } else { (10, 3, 4) };
        self.rect(x, y, w, 1, self.palette.light);
    }
    fn web(&mut self) {
        if self.small {
            self.panel(1, 1, 14, 14, 4, self.palette.main);
            self.line(&[(5, 2), (4, 5), (4, 10), (6, 13)], 1.0, self.ink());
            self.line(&[(10, 2), (11, 5), (11, 10), (9, 13)], 1.0, self.ink());
            self.rect(2, 7, 12, 1, self.ink());
            self.rect(4, 4, 8, 1, self.palette.other);
            self.rect(4, 11, 8, 1, self.palette.other);
        } else {
            self.panel(2, 2, 20, 20, 6, self.palette.main);
            self.line(&[(9, 3), (7, 7), (7, 16), (10, 20)], 1.0, self.ink());
            self.line(&[(14, 3), (16, 7), (16, 16), (13, 20)], 1.0, self.ink());
            self.rect(3, 11, 18, 1, self.ink());
            self.rect(5, 7, 14, 1, self.palette.other);
            self.rect(5, 16, 14, 1, self.palette.other);
        }
    }
    fn network(&mut self) {
        if self.small {
            self.rect(7, 5, 1, 4, self.palette.light);
            self.rect(3, 8, 10, 1, self.palette.light);
            self.rect(3, 9, 1, 2, self.palette.light);
            self.rect(12, 9, 1, 2, self.palette.light);
            for (x, y) in [(5, 1), (1, 11), (10, 11)] {
                self.panel(x, y, 5, 4, 1, self.palette.main);
            }
        } else {
            self.rect(11, 8, 2, 6, self.palette.light);
            self.rect(5, 13, 14, 1, self.palette.light);
            self.rect(5, 14, 1, 3, self.palette.light);
            self.rect(18, 14, 1, 3, self.palette.light);
            for (x, y) in [(8, 2), (2, 17), (15, 17)] {
                self.panel(x, y, 7, 5, 1, self.palette.main);
            }
        }
    }
    fn music(&mut self) {
        if self.small {
            self.rect(6, 3, 2, 10, self.palette.main);
            self.rect(13, 2, 2, 10, self.palette.main);
            self.polygon(&[(6, 3), (15, 1), (15, 4), (6, 6)], self.palette.main);
            self.panel(2, 11, 6, 4, 1, self.palette.main);
            self.panel(9, 10, 6, 4, 1, self.palette.main);
        } else {
            self.rect(9, 5, 2, 14, self.palette.main);
            self.rect(19, 3, 2, 14, self.palette.main);
            self.polygon(&[(9, 5), (21, 2), (21, 6), (9, 9)], self.palette.main);
            self.rect(10, 5, 9, 1, self.palette.light);
            self.panel(3, 16, 8, 6, 2, self.palette.main);
            self.panel(13, 14, 8, 6, 2, self.palette.main);
        }
    }
    fn games(&mut self) {
        if self.small {
            self.panel(1, 4, 14, 9, 2, self.palette.main);
            self.rect(4, 6, 1, 5, self.ink());
            self.rect(2, 8, 5, 1, self.ink());
            self.rect(10, 6, 2, 2, self.palette.other);
            self.rect(12, 9, 2, 2, self.palette.other);
        } else {
            self.panel(2, 6, 20, 13, 3, self.palette.main);
            self.rect(6, 9, 2, 7, self.ink());
            self.rect(4, 11, 6, 2, self.ink());
            self.rect(15, 9, 3, 3, self.palette.other);
            self.rect(18, 13, 3, 3, self.palette.other);
            self.rect(11, 15, 2, 1, self.palette.shade);
        }
    }
    fn calendar(&mut self) {
        let (x, y, w, h, bind, grid, step) = if self.small {
            (2, 3, 12, 12, 5, 8, 3)
        } else {
            (3, 4, 18, 18, 7, 12, 5)
        };
        self.panel(x, y, w, h, 1, self.palette.main);
        self.rect(
            x + 1,
            y + 1,
            w - 2,
            if self.small { 3 } else { 4 },
            self.palette.other,
        );
        self.rect(bind, y - 2, 1, 4, self.palette.light);
        self.rect(
            bind + if self.small { 5 } else { 8 },
            y - 2,
            1,
            4,
            self.palette.light,
        );
        for row in 0..2 {
            for col in 0..3 {
                self.rect(
                    x + 2 + col * step,
                    grid + row * step,
                    2,
                    2,
                    if row == 0 && col == 1 {
                        self.palette.other
                    } else if self.solid {
                        self.palette.edge
                    } else {
                        self.palette.main
                    },
                );
            }
        }
    }
    fn notes(&mut self) {
        if self.small {
            self.panel(2, 1, 11, 14, 1, self.palette.main);
            for y in [4, 7, 10] {
                self.rect(
                    4,
                    y,
                    5,
                    1,
                    if self.solid {
                        self.palette.shade
                    } else {
                        self.palette.light
                    },
                );
            }
            self.line(&[(8, 13), (14, 7)], 2.0, self.palette.other);
            self.rect(7, 13, 2, 2, self.palette.edge);
        } else {
            self.panel(4, 2, 15, 20, 1, self.palette.main);
            for y in [6, 10, 14] {
                self.rect(
                    7,
                    y,
                    8,
                    1,
                    if self.solid {
                        self.palette.shade
                    } else {
                        self.palette.light
                    },
                );
            }
            self.line(&[(13, 20), (21, 12)], 3.0, self.palette.other);
            self.line(&[(14, 18), (20, 12)], 1.0, self.palette.light);
            self.polygon(&[(11, 22), (12, 18), (15, 21)], self.palette.edge);
        }
    }
    fn image(&mut self) {
        if self.small {
            self.panel(1, 3, 14, 11, 1, self.palette.main);
            self.rect(10, 5, 2, 2, 0xf4d484);
            let hills = [(3, 12), (6, 7), (9, 10), (11, 8), (13, 12)];
            if self.solid {
                self.polygon(&hills, self.palette.other);
            } else {
                self.line(&hills, 1.0, self.palette.other);
            }
        } else {
            self.panel(2, 4, 20, 16, 2, self.palette.main);
            self.panel(15, 7, 4, 4, 1, 0xf4d484);
            let hills = [(4, 17), (9, 10), (13, 14), (16, 11), (20, 17)];
            if self.solid {
                self.polygon(&hills, self.palette.other);
            } else {
                self.line(&hills, 1.0, self.palette.other);
            }
        }
    }
    fn generic(&mut self) {
        if self.small {
            self.body(
                &[(3, 1), (9, 1), (13, 5), (13, 14), (3, 14)],
                self.palette.main,
            );
            self.line(&[(9, 1), (9, 5), (13, 5)], 1.0, self.palette.light);
            for y in [7, 10] {
                self.rect(5, y, 6, 1, self.ink());
            }
        } else {
            self.body(
                &[(5, 2), (14, 2), (19, 7), (19, 21), (5, 21)],
                self.palette.main,
            );
            self.line(&[(14, 2), (14, 7), (19, 7)], 1.0, self.palette.light);
            for y in [10, 14, 18] {
                self.rect(8, y, 8, 1, self.ink());
            }
        }
    }
    fn info(&mut self, error: bool) {
        let (x, n, corner, start, end) = if self.small {
            (1, 14, 4, 5, 10)
        } else {
            (2, 20, 6, 8, 15)
        };
        self.panel(x, x, n, n, corner, self.palette.main);
        if error {
            self.line(
                &[(start, start), (end, end)],
                if self.small { 1.0 } else { 2.0 },
                self.ink(),
            );
            self.line(
                &[(end, start), (start, end)],
                if self.small { 1.0 } else { 2.0 },
                self.ink(),
            );
        } else if self.small {
            self.rect(7, 4, 2, 2, self.ink());
            self.rect(7, 7, 2, 5, self.ink());
        } else {
            self.rect(11, 6, 2, 2, self.ink());
            self.rect(11, 10, 2, 7, self.ink());
            self.rect(9, 17, 6, 1, self.ink());
        }
    }
    fn warning(&mut self) {
        let ink = if self.solid {
            self.palette.other
        } else {
            self.palette.light
        };
        if self.small {
            self.body(&[(8, 1), (15, 14), (1, 14)], self.palette.main);
            self.rect(7, 6, 2, 4, ink);
            self.rect(7, 11, 2, 2, ink);
        } else {
            self.body(&[(12, 2), (23, 22), (1, 22)], self.palette.main);
            self.rect(11, 8, 2, 7, ink);
            self.rect(11, 17, 2, 2, ink);
        }
    }
    fn notifications(&mut self) {
        if self.small {
            self.rect(7, 1, 2, 2, self.palette.main);
            self.body(
                &[
                    (5, 3),
                    (10, 3),
                    (12, 5),
                    (12, 10),
                    (14, 12),
                    (2, 12),
                    (4, 10),
                    (4, 5),
                ],
                self.palette.main,
            );
            self.rect(2, 12, 12, 1, self.palette.light);
            self.rect(7, 14, 2, 1, self.palette.main);
        } else {
            self.rect(10, 2, 4, 2, self.palette.main);
            self.body(
                &[
                    (9, 5),
                    (15, 5),
                    (17, 7),
                    (18, 15),
                    (20, 17),
                    (4, 17),
                    (6, 15),
                    (7, 7),
                ],
                self.palette.main,
            );
            if self.solid {
                self.rect(8, 8, 1, 6, self.palette.light);
            }
            self.rect(4, 17, 16, 2, self.palette.light);
            self.rect(10, 20, 4, 2, self.palette.main);
        }
    }
}

/// Draw one icon, source-over, without painting a surrounding background.
/// 16 and 24 have native layouts. Other bounded sizes use the nearer layout;
/// zero or sizes above 64 are ignored rather than allocating arbitrary images.
pub fn draw_icon(canvas: &mut Pixmap, x: i32, y: i32, size: u32, kind: IconKind, theme: &Theme) {
    if size == 0 || size > 64 {
        return;
    }
    let small = size < 20;
    let native = if small { 16.0 } else { 24.0 };
    let scale = size as f32 / native;
    let mut icon = Icon {
        canvas,
        transform: Transform::from_row(scale, 0.0, 0.0, scale, x as f32, y as f32),
        small,
        solid: theme.icon_style == IconStyle::Solid,
        palette: Palette::new(kind, theme),
    };
    match kind {
        IconKind::Terminal => icon.terminal(),
        IconKind::Files => icon.files(),
        IconKind::Settings => icon.settings(),
        IconKind::Web => icon.web(),
        IconKind::Network => icon.network(),
        IconKind::Music => icon.music(),
        IconKind::Games => icon.games(),
        IconKind::Calendar => icon.calendar(),
        IconKind::Notes => icon.notes(),
        IconKind::Image => icon.image(),
        IconKind::Generic => icon.generic(),
        IconKind::Info => icon.info(false),
        IconKind::Warning => icon.warning(),
        IconKind::Error => icon.info(true),
        IconKind::Notifications => icon.notifications(),
    }
}
