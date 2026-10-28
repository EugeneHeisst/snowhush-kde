//! What the KWin script reports: the displays' work areas, the windows front to back and the pointer.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::geometry::Rect;
use crate::prefs::Prefs;
use crate::simulation::{WindowId, WindowSurface};

pub const SERVICE: &str = "org.snowhush.Kde";

#[derive(Deserialize, Clone, Debug, Default)]
pub struct ScreenInfo {
    pub name: String,
    /// The work area in global logical pixels: panels are left out, so snow settles on top of them.
    pub area: [f64; 4],
    pub geometry: [f64; 4],
}

#[derive(Deserialize, Clone, Debug)]
pub struct WindowInfo {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub output: String,
    /// 0: on screen, 1: on another desktop or activity, 2: minimized or hidden.
    pub state: u8,
    #[serde(default)]
    pub full: bool,
    #[serde(default)]
    pub square: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct World {
    pub screens: Vec<ScreenInfo>,
    pub windows: Vec<WindowInfo>,
    #[serde(default)]
    pub moving: bool,
}

/// Breeze rounds window corners a little; maximized and borderless windows stay square.
const CORNER_RADIUS: f32 = 5.0;

pub fn window_id(id: &str) -> WindowId {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    // Zero is the ground's id.
    (hasher.finish() as WindowId).max(1)
}

impl World {
    pub fn screen(&self, name: &str) -> Option<&ScreenInfo> {
        self.screens.iter().find(|screen| screen.name == name)
    }

    /// The windows on one display that snow can settle on, in overlay coordinates: up from the bottom left of `area`.
    pub fn surfaces(&self, area: [f64; 4], size: (f32, f32)) -> Vec<WindowSurface> {
        let (width, height) = size;
        let [ax, ay, aw, ah] = area;
        let sx = width / aw.max(1.0) as f32;
        let sy = height / ah.max(1.0) as f32;
        let mut blockers: Vec<Rect> = Vec::new();
        let mut surfaces = Vec::new();
        for window in self.windows.iter().filter(|window| window.state == 0) {
            let (left, top, right, bottom) = (window.x, window.y, window.x + window.w, window.y + window.h);
            if right <= ax || left >= ax + aw || bottom <= ay || top >= ay + ah {
                continue;
            }
            let frame = Rect {
                x: (left - ax) as f32 * sx,
                y: (ay + ah - bottom) as f32 * sy,
                width: window.w as f32 * sx,
                height: window.h as f32 * sy,
            };
            let room_above = (top - ay) as f32 * sy >= 6.0 && frame.max_y() < height;
            let on_screen = frame.max_x() > 0.0 && frame.min_x() < width && frame.max_y() > 0.0;
            let fills_screen = frame.width * frame.height >= width * height * 0.9;
            if room_above && on_screen && !fills_screen && frame.width >= 80.0 && frame.height >= 40.0 {
                let radius = if window.square { 0.0 } else { CORNER_RADIUS.min(frame.width / 2.0).min(frame.height / 2.0) };
                let region = Rect { x: frame.x, y: frame.max_y() - radius, width: frame.width, height: 40.0 + radius };
                surfaces.push(WindowSurface {
                    id: window_id(&window.id),
                    frame,
                    blockers: blockers.iter().filter(|b| intersects(b, &region)).copied().collect(),
                    corner_radius: radius,
                    corner_exponent: 2.0,
                });
            }
            blockers.push(frame);
        }
        surfaces
    }

    /// Windows that went to another desktop or activity: their snow waits for them instead of falling.
    pub fn elsewhere(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.iter().filter(|window| window.state == 1).map(|window| window_id(&window.id))
    }

    /// Whether a full-screen window is the frontmost one on this display.
    pub fn covered(&self, screen: &ScreenInfo) -> bool {
        let [gx, gy, gw, gh] = screen.geometry;
        self.windows
            .iter()
            .filter(|window| window.state == 0)
            .find(|window| window.x < gx + gw && window.x + window.w > gx && window.y < gy + gh && window.y + window.h > gy)
            .is_some_and(|window| window.full && window.x <= gx && window.y <= gy && window.x + window.w >= gx + gw && window.y + window.h >= gy + gh)
    }
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.min_x() < b.max_x() && b.min_x() < a.max_x() && a.min_y() < b.max_y() && b.min_y() < a.max_y()
}

/// State shared between the D-Bus threads and the drawing loop.
#[derive(Default)]
pub struct Shared {
    pub world: Option<World>,
    pub world_generation: u64,
    pub cursor: Option<(f64, f64)>,
    pub prefs: Prefs,
    pub prefs_generation: u64,
    pub quit: bool,
}

pub type SharedState = Arc<Mutex<Shared>>;

struct Service(SharedState);

#[zbus::interface(name = "org.snowhush.Kde")]
impl Service {
    fn update(&self, json: String) {
        match serde_json::from_str::<World>(&json) {
            Ok(world) => {
                let mut shared = self.0.lock().unwrap();
                shared.world = Some(world);
                shared.world_generation += 1;
            }
            Err(error) => eprintln!("snowhush: bad window list from KWin: {error}"),
        }
    }

    fn cursor(&self, x: f64, y: f64) {
        self.0.lock().unwrap().cursor = Some((x, y));
    }

    fn quit(&self) {
        self.0.lock().unwrap().quit = true;
    }
}

pub fn serve(shared: SharedState) -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session()?.name(SERVICE)?.serve_at("/Snowhush", Service(shared))?.build()
}
