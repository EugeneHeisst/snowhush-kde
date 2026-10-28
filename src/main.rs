//! Snowhush for KDE Plasma on Wayland: a click-through layer-shell overlay per display, drawn with OpenGL ES.
//! The snow itself is the Windows port's simulation, unchanged; KWin reports windows and the pointer over D-Bus.

mod geometry;
mod kwin;
mod layer;
mod prefs;
mod render;
mod simulation;
mod tray;

use std::collections::HashSet;
use std::ffi::c_void;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use khronos_egl as egl;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState, SimpleGlobal};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::{delegate_registry, registry_handlers};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_egl::WlEglSurface;

use geometry::{V2, smoothstep};
use kwin::{Shared, SharedState, World};
use prefs::Prefs;
use render::{Ambient, Renderer};
use simulation::{Pace, SnowSimulation, WindowId, WindowSurface};

struct Gfx {
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    renderer: Renderer,
}

impl Gfx {
    fn new(conn: &Connection) -> Result<Gfx, String> {
        let egl = egl::Instance::new(egl::Static);
        let display = unsafe { egl.get_display(conn.backend().display_ptr() as *mut c_void) }.ok_or("no EGL display")?;
        egl.initialize(display).map_err(|e| e.to_string())?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| e.to_string())?;
        let attributes = [
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::NONE,
        ];
        let config = egl.choose_first_config(display, &attributes).map_err(|e| e.to_string())?.ok_or("no EGL config with alpha")?;
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE])
            .map_err(|e| e.to_string())?;
        egl.make_current(display, None, None, Some(context)).map_err(|e| e.to_string())?;
        let gl = unsafe {
            glow::Context::from_loader_function(|name| egl.get_proc_address(name).map_or(std::ptr::null(), |p| p as *const c_void))
        };
        let renderer = Renderer::new(gl)?;
        Ok(Gfx { egl, display, config, context, renderer })
    }
}

/// The snow on one display.
struct Screen {
    output: wl_output::WlOutput,
    name: String,
    /// Logical position of the display, used until KWin reports the work area.
    position: (i32, i32),
    scale: f32,
    refresh: u32,
    layer: LayerSurface,
    viewport: Option<wp_viewport::WpViewport>,
    window: Option<(WlEglSurface, egl::Surface)>,
    /// Logical size, from the compositor's configure.
    size: (u32, u32),
    pixels: (i32, i32),
    simulation: Option<SnowSimulation>,
    surfaces: Vec<WindowSurface>,
    world_seen: u64,
    windows_changed_at: Instant,
    last_step: Option<Instant>,
    next_due: Instant,
    idle_frames: u32,
    covered: bool,
}

impl Screen {
    fn area(&self, world: Option<&World>) -> [f64; 4] {
        world
            .and_then(|world| world.screen(&self.name))
            .map(|screen| screen.area)
            .unwrap_or([self.position.0 as f64, self.position.1 as f64, self.size.0 as f64, self.size.1 as f64])
    }

    fn local(&self, area: [f64; 4], x: f64, y: f64) -> V2 {
        let sx = self.size.0 as f64 / area[2].max(1.0);
        let sy = self.size.1 as f64 / area[3].max(1.0);
        V2::new(((x - area[0]) * sx) as f32, ((area[1] + area[3] - y) * sy) as f32)
    }

    /// Hands the simulation the windows on this display when they changed.
    fn follow(&mut self, world: &World, generation: u64, now: Instant) {
        if generation == self.world_seen {
            return;
        }
        self.world_seen = generation;
        let Some(simulation) = self.simulation.as_mut() else { return };
        let area = world.screen(&self.name).map(|screen| screen.area).unwrap_or([
            self.position.0 as f64,
            self.position.1 as f64,
            self.size.0 as f64,
            self.size.1 as f64,
        ]);
        let surfaces = world.surfaces(area, (self.size.0 as f32, self.size.1 as f32));
        if surfaces == self.surfaces {
            return;
        }
        let since = (now - self.windows_changed_at).as_secs_f32();
        // A window that moves outside a drag, as when it is maximized or tiled, jumps there in one go:
        // its snow is left behind instead of being carried along.
        let interval = if world.moving || since < 0.1 { since.min(0.25) } else { 1.0 / 60.0 };
        let away: HashSet<WindowId> = world.elsewhere().collect();
        let elsewhere: HashSet<WindowId> =
            self.surfaces.iter().map(|surface| surface.id).filter(|id| !surfaces.iter().any(|s| s.id == *id) && away.contains(id)).collect();
        simulation.update_windows(&surfaces, interval, &HashSet::new(), &elsewhere, false);
        self.surfaces = surfaces;
        self.windows_changed_at = now;
    }

    fn drop_window_snow(&mut self) {
        if let Some(simulation) = self.simulation.as_mut() {
            simulation.update_windows(&[], 0.1, &HashSet::new(), &HashSet::new(), false);
        }
        self.surfaces.clear();
        self.world_seen = 0;
    }
}

fn apply(simulation: &mut SnowSimulation, prefs: &Prefs) {
    simulation.set_snowfall(prefs.snowfall);
    simulation.accumulates = prefs.accumulates;
    simulation.thaws = prefs.thaws;
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    viewporter: SimpleGlobal<wp_viewporter::WpViewporter, 1>,
    gfx: Gfx,
    screens: Vec<Screen>,
    prefs: Prefs,
}

impl App {
    fn add_screen(&mut self, qh: &QueueHandle<App>, output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&output) else { return };
        let surface = self.compositor.create_surface(qh);
        let kind = if self.prefs.behind_windows { Layer::Bottom } else { Layer::Top };
        let layer = self.layer_shell.create_layer_surface(qh, surface, kind, Some("snowhush"), Some(&output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        // Zero keeps clear of panels: the bottom one becomes the ground the snow settles on.
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        if let Ok(region) = Region::new(&self.compositor) {
            layer.wl_surface().set_input_region(Some(region.wl_region()));
        }
        let viewport = self.viewporter.get().ok().map(|viewporter| viewporter.get_viewport(layer.wl_surface(), qh, ()));
        layer.commit();
        let (scale, refresh) = output_scale(&info);
        let now = Instant::now();
        self.screens.push(Screen {
            output,
            name: info.name.clone().unwrap_or_default(),
            position: info.logical_position.unwrap_or((0, 0)),
            scale,
            refresh,
            layer,
            viewport,
            window: None,
            size: (0, 0),
            pixels: (0, 0),
            simulation: None,
            surfaces: Vec::new(),
            world_seen: 0,
            windows_changed_at: now,
            last_step: None,
            next_due: now,
            idle_frames: 0,
            covered: false,
        });
    }

    fn resize(&mut self, index: usize, width: u32, height: u32) {
        let gfx = &self.gfx;
        let screen = &mut self.screens[index];
        if width == 0 || height == 0 {
            return;
        }
        screen.size = (width, height);
        let scale = if screen.viewport.is_some() { screen.scale } else { 1.0 };
        let pixels = (((width as f32) * scale).round() as i32, ((height as f32) * scale).round() as i32);
        if let Some(viewport) = &screen.viewport {
            viewport.set_source(0.0, 0.0, pixels.0 as f64, pixels.1 as f64);
            viewport.set_destination(width as i32, height as i32);
        }
        match &screen.window {
            Some((window, _)) => window.resize(pixels.0, pixels.1, 0, 0),
            None => {
                let created = WlEglSurface::new(screen.layer.wl_surface().id(), pixels.0, pixels.1).map_err(|e| e.to_string()).and_then(|window| {
                    let surface = unsafe {
                        gfx.egl.create_window_surface(gfx.display, gfx.config, window.ptr() as egl::NativeWindowType, None)
                    }
                    .map_err(|e| e.to_string())?;
                    Ok((window, surface))
                });
                match created {
                    Ok(window) => {
                        screen.window = Some(window);
                        if gfx.egl.make_current(gfx.display, screen.window.as_ref().map(|w| w.1), screen.window.as_ref().map(|w| w.1), Some(gfx.context)).is_ok() {
                            // Frames are paced here; waiting on the compositor would stall every display behind a hidden one.
                            let _ = gfx.egl.swap_interval(gfx.display, 0);
                        }
                    }
                    Err(error) => eprintln!("snowhush: no drawing surface for {}: {error}", screen.name),
                }
            }
        }
        screen.pixels = pixels;
        match screen.simulation.as_mut() {
            Some(simulation) => simulation.resize(width as f32, height as f32),
            None => {
                let mut simulation = SnowSimulation::new(width as f32, height as f32, self.prefs.snowfall);
                apply(&mut simulation, &self.prefs);
                screen.simulation = Some(simulation);
            }
        }
        screen.world_seen = 0;
        screen.next_due = Instant::now();
    }

    fn set_prefs(&mut self, prefs: Prefs) {
        let old = std::mem::replace(&mut self.prefs, prefs);
        for screen in &mut self.screens {
            if let Some(simulation) = screen.simulation.as_mut() {
                apply(simulation, &prefs);
            }
            if old.snow_on_windows && !prefs.snow_on_windows {
                screen.drop_window_snow();
            }
            if old.behind_windows != prefs.behind_windows {
                screen.layer.set_layer(if prefs.behind_windows { Layer::Bottom } else { Layer::Top });
                screen.layer.commit();
            }
            screen.world_seen = 0;
            screen.next_due = Instant::now();
        }
    }

    /// Steps the snow on every display that is due and draws it if anything visible changed.
    fn frames(&mut self, now: Instant, world: Option<&World>, generation: u64, cursor: Option<(f64, f64)>, time: f32, light: [f32; 3]) {
        let gfx = &mut self.gfx;
        for screen in &mut self.screens {
            if screen.simulation.is_none() || screen.window.is_none() || now < screen.next_due {
                continue;
            }
            let covered = world.zip(world.and_then(|w| w.screen(&screen.name))).is_some_and(|(w, s)| w.covered(s));
            if covered != screen.covered {
                screen.covered = covered;
                screen.last_step = None;
                if covered {
                    // Leaves nothing on screen over a full-screen app and stops until it goes.
                    let surface = screen.window.as_ref().map(|w| w.1);
                    if gfx.egl.make_current(gfx.display, surface, surface, Some(gfx.context)).is_ok() {
                        unsafe {
                            use glow::HasContext;
                            gfx.renderer.gl.clear_color(0.0, 0.0, 0.0, 0.0);
                            gfx.renderer.gl.clear(glow::COLOR_BUFFER_BIT);
                        }
                        let _ = gfx.egl.swap_buffers(gfx.display, surface.unwrap());
                    }
                }
            }
            if screen.covered {
                screen.next_due = now + Duration::from_millis(250);
                continue;
            }
            let dt = screen.last_step.map_or(1.0 / 60.0, |last| (now - last).as_secs_f32().min(0.05));
            screen.last_step = Some(now);
            if self.prefs.snow_on_windows
                && let Some(world) = world
            {
                screen.follow(world, generation, now);
            }
            let area = screen.area(world);
            let point = cursor.map_or(V2::new(-10_000.0, -10_000.0), |(x, y)| screen.local(area, x, y));
            let simulation = screen.simulation.as_mut().unwrap();
            let changed = simulation.step(dt, point);

            let fastest = screen.refresh.clamp(60, 120);
            let rate = match simulation.pace() {
                Pace::Calm => 30,
                Pace::Lively => fastest.min(60),
                Pace::Interactive => fastest,
            };
            screen.next_due = now + Duration::from_secs_f64(1.0 / rate as f64);
            screen.idle_frames = if changed { 0 } else { screen.idle_frames + 1 };
            // A still pile only needs a slow refresh for its glints.
            if !(changed || (simulation.has_snow() && screen.idle_frames % 2 == 0)) {
                continue;
            }
            let surface = screen.window.as_ref().map(|w| w.1).unwrap();
            if gfx.egl.make_current(gfx.display, Some(surface), Some(surface), Some(gfx.context)).is_err() {
                continue;
            }
            let scale = screen.pixels.0 as f32 / screen.size.0.max(1) as f32;
            gfx.renderer.draw(simulation, screen.pixels, scale, Ambient { light, shadow_tint: [0.0; 4] }, time);
            if let Err(error) = gfx.egl.swap_buffers(gfx.display, surface) {
                eprintln!("snowhush: swap failed on {}: {error}", screen.name);
            }
        }
    }

    /// The EGL surface has to go before the wl_egl_window under it.
    fn remove_screens(&mut self, gone: impl Fn(&Screen) -> bool) {
        let gfx = &self.gfx;
        for screen in self.screens.iter_mut().filter(|screen| gone(screen)) {
            if let Some((_, surface)) = screen.window.take() {
                let _ = gfx.egl.make_current(gfx.display, None, None, Some(gfx.context));
                let _ = gfx.egl.destroy_surface(gfx.display, surface);
            }
        }
        self.screens.retain(|screen| !gone(screen));
    }

    fn next_due(&self) -> Option<Instant> {
        self.screens.iter().filter(|screen| screen.window.is_some()).map(|screen| screen.next_due).min()
    }
}

/// The display's scale, from its current mode against its logical size, and its refresh rate.
fn output_scale(info: &smithay_client_toolkit::output::OutputInfo) -> (f32, u32) {
    let mode = info.modes.iter().find(|mode| mode.current);
    let refresh = mode.map_or(60, |mode| ((mode.refresh_rate + 500) / 1000).max(30) as u32);
    let scale = match (mode, info.logical_size) {
        (Some(mode), Some(logical)) if logical.0 > 0 && logical.1 > 0 => {
            let physical = mode.dimensions.0.max(mode.dimensions.1) as f32;
            physical / logical.0.max(logical.1) as f32
        }
        _ => info.scale_factor.max(1) as f32,
    };
    (scale.clamp(1.0, 4.0), refresh)
}

/// The scene turns warmer at dawn and dusk, cooler and dimmer at night.
fn daylight() -> [f32; 3] {
    let hour = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut local);
        local.tm_hour as f32 + local.tm_min as f32 / 60.0
    };
    let day = smoothstep(6.0, 8.5, hour) * (1.0 - smoothstep(17.5, 20.5, hour));
    let golden = ((-((hour - 7.6) / 1.1).powi(2)).exp() + (-((hour - 18.6) / 1.2).powi(2)).exp()).min(1.0);
    let night = [0.86, 0.89, 0.97];
    let warm = [1.0, 0.965, 0.915];
    std::array::from_fn(|i| {
        let base = night[i] + (1.0 - night[i]) * day;
        base + (warm[i] - base) * golden * 0.55
    })
}

fn main() {
    let conn = Connection::connect_to_env().expect("snowhush: not running under Wayland");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("snowhush: no Wayland registry");
    let qh = queue.handle();

    let shared: SharedState = Arc::new(Mutex::new(Shared { prefs: Prefs::load(), ..Default::default() }));
    let _bus = match kwin::serve(shared.clone()) {
        Ok(bus) => bus,
        Err(error) => {
            eprintln!("snowhush: can't take {} on the session bus (already running?): {error}", kwin::SERVICE);
            std::process::exit(1);
        }
    };
    let _tray = tray::spawn(shared.clone());

    let gfx = Gfx::new(&conn).unwrap_or_else(|error| panic!("snowhush: OpenGL ES 3 unavailable: {error}"));
    let prefs = shared.lock().unwrap().prefs;
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).expect("snowhush: no wl_compositor"),
        layer_shell: LayerShell::bind(&globals, &qh).expect("snowhush: the compositor has no layer shell"),
        viewporter: SimpleGlobal::bind(&globals, &qh).expect("snowhush: no viewporter"),
        gfx,
        screens: Vec::new(),
        prefs,
    };

    let started = Instant::now();
    let mut light = daylight();
    let mut light_at = started;
    let mut prefs_seen = 0;
    let mut world: Option<World> = None;
    let mut world_generation = 0;
    loop {
        let _ = queue.flush();
        let now = Instant::now();
        let timeout = app.next_due().map_or(Duration::from_millis(100), |due| due.saturating_duration_since(now)).min(Duration::from_millis(100));
        if let Some(guard) = queue.prepare_read() {
            let mut fd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ready = unsafe { libc::poll(&mut fd, 1, timeout.as_millis() as i32) };
            if ready > 0 {
                if let Err(error) = guard.read() {
                    eprintln!("snowhush: lost the compositor: {error}");
                    break;
                }
            }
        }
        if queue.dispatch_pending(&mut app).is_err() {
            break;
        }

        let (cursor, prefs) = {
            let mut shared = shared.lock().unwrap();
            if shared.quit {
                break;
            }
            if shared.world_generation != world_generation {
                world_generation = shared.world_generation;
                world = shared.world.take();
            }
            let prefs = (shared.prefs_generation != prefs_seen).then(|| {
                prefs_seen = shared.prefs_generation;
                shared.prefs
            });
            (shared.cursor, prefs)
        };
        if let Some(prefs) = prefs {
            app.set_prefs(prefs);
        }
        let now = Instant::now();
        if (now - light_at).as_secs() >= 60 {
            light = daylight();
            light_at = now;
        }
        app.frames(now, world.as_ref(), world_generation, cursor, (now - started).as_secs_f32() % 3600.0, light);
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_screen(qh, output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&output) else { return };
        let Some(index) = self.screens.iter().position(|screen| screen.output == output) else { return };
        let (scale, refresh) = output_scale(&info);
        let screen = &mut self.screens[index];
        screen.name = info.name.clone().unwrap_or_default();
        screen.position = info.logical_position.unwrap_or((0, 0));
        screen.refresh = refresh;
        if (scale - screen.scale).abs() > 0.001 {
            screen.scale = scale;
            let (width, height) = screen.size;
            self.resize(index, width, height);
        }
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.remove_screens(|screen| screen.output == output);
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.remove_screens(|screen| &screen.layer == layer);
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let Some(index) = self.screens.iter().position(|screen| &screen.layer == layer) else { return };
        let (width, height) = configure.new_size;
        if (width, height) != self.screens[index].size || self.screens[index].window.is_none() {
            self.resize(index, width, height);
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Some(viewport) = &self.viewport {
            viewport.destroy();
        }
    }
}

impl Dispatch<wp_viewport::WpViewport, ()> for App {
    fn event(_: &mut Self, _: &wp_viewport::WpViewport, _: wp_viewport::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

wayland_client::delegate_noop!(App: wp_viewporter::WpViewporter);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
