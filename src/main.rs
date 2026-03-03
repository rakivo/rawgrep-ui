mod gpu;
mod util;

use std::sync::Arc;
use std::time::Instant;
use ecow::EcoString;
use gpu::GpuColor;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Debug, Copy, Clone)]
struct Color {
    pub r: u8, pub g: u8, pub b: u8, pub a: u8
}

impl From<GpuColor> for Color {
    fn from(GpuColor([r, g, b, a]): GpuColor) -> Self {
        Self {
            r: (r * 255.0) as _,
            g: (g * 255.0) as _,
            b: (b * 255.0) as _,
            a: (a * 255.0) as _,
        }
    }
}

impl Color {
    #[inline]
    pub fn hsv(h: f32, s: f32, v: f32) -> Self {
        GpuColor::hsv(h, s, v).into()
    }

    #[inline]
    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a}
    }
}

struct State {
    prompt_buffer: EcoString,
    start: Instant,
}

impl State {
    fn new() -> Self {
        Self {
            prompt_buffer: Default::default(),
            start: Instant::now()
        }
    }
}

fn render(gpu: &mut gpu::Gpu, state: &State) -> Result<(), wgpu::SurfaceError> {
    let t = state.start.elapsed().as_secs_f32();

    // header bar
    gpu::draw_rect(gpu, 10.0, 10.0, 780.0, 40.0, Color::rgba(30, 30, 40, 255));

    // rainbow title, each char gets a hue offset by its index
    gpu::draw_text_colored(gpu, "rawgrep-ui", 20.0, 38.0, |i| {
        // hue cycles over time + offset per char for wave effect
        let hue = (t * 0.5 + i as f32 * 0.15) % 1.0;
        Color::hsv(hue, 0.8, 1.0)
    });

    // prompt box
    gpu::draw_rect(gpu, 10.0, 60.0, 780.0, 40.0, Color::rgba(20, 20, 28, 255));
    gpu::draw_text(gpu, "> ", 20.0, 90.0, Color::rgba(100, 100, 120, 255));
    gpu::draw_text(gpu, &state.prompt_buffer, 44.0, 90.0, Color::rgba(220, 220, 220, 255));

    // blinking cursor
    let cursor_x = 44.0 + state.prompt_buffer.chars()
        .filter_map(|c| gpu.glyphs.get(&c))
        .map(|g| g.advance)
        .sum::<f32>();

    if (t * 2.0) as u32 % 2 == 0 {
        gpu::draw_rect(gpu, cursor_x + 3.5, 72.5, 11.0, 24.0, Color::rgba(10, 200, 10, 255));
    }

    gpu::submit(gpu)
}

//
//
// Winit plumbing
//
//

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    gpu:    Option<gpu::Gpu>,
    state:  Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let win = Arc::new(el.create_window(Window::default_attributes().with_title("rawgrep-ui")).unwrap());
        self.gpu    = Some(gpu::init(Arc::clone(&win)));
        self.state  = Some(State::new());
        self.window = Some(win);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let (
            Some(gpu),
            Some(state),
            Some(win)
        ) = (&mut self.gpu, &mut self.state, &self.window) else { return };

        match event {
            WindowEvent::CloseRequested => el.exit(),

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed { return; }
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape)     => el.exit(),
                    PhysicalKey::Code(KeyCode::Space)      => { state.prompt_buffer.push(' '); }
                    PhysicalKey::Code(KeyCode::Backspace)  => { state.prompt_buffer.pop(); }

                    _ => {
                        if let Key::Character(s) = &event.logical_key {
                            state.prompt_buffer.push_str(s.as_str());
                        }
                    }
                }
            }

            WindowEvent::Resized(sz) => {
                if sz.width > 0 && sz.height > 0 {
                    gpu.win_w = sz.width  as f32;
                    gpu.win_h = sz.height as f32;
                    gpu.surface_config.width  = sz.width;
                    gpu.surface_config.height = sz.height;
                    gpu.surface.configure(&gpu.device, &gpu.surface_config);
                }
            }

            WindowEvent::RedrawRequested => {
                match render(gpu, state) {
                    Ok(_) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        gpu.surface.configure(&gpu.device, &gpu.surface_config);
                    }
                    Err(e) => eprintln!("{e:?}"),
                }
                win.request_redraw();
            }

            _ => {}
        }
    }
}

fn main() {
    env_logger::init();
    let el = EventLoop::new().unwrap();
    //
    // Poll so the rainbow animates smoothly without input events
    //
    el.set_control_flow(ControlFlow::Poll);
    el.run_app(&mut App::default()).unwrap();
}
