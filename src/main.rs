mod ui;
mod gpu;
mod util;
mod color;
mod prompt;

use std::{sync::Arc, time::Instant};

use color::Color;
use prompt::PromptState;
use ui::{BoxCustom, Size, TextInputInfo, UiState};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

const MIN_SCALE: f32 = 0.75;
const MAX_SCALE: f32 = 5.00;

const PROMPT_PREFIX: &str = "> ";

pub struct Palette {
    pub bg:         Color,
    pub header_bar: Color,
    pub prompt_box: Color,

    pub accent:     Color,   // highlighted text, filenames
    pub dim:        Color,   // muted text, prompt prefix
    pub hover:      Color,   // hover overlay for clickable rows
    pub border:     Color,
}

#[inline]
pub fn palette() -> Palette {
    Palette {
        bg:          const { Color::rgba(13,  13,  13,  255) }, // near black
        header_bar:  const { Color::rgba(20,  20,  20,  255) }, // barely lighter
        prompt_box:  const { Color::rgba(18,  18,  18,  255) }, // inset feel
        accent:      const { Color::rgba(150, 190, 220, 255) }, // steel blue, muted
        dim:         const { Color::rgba(80,  85,  95,  255) }, // very muted
        hover:       const { Color::rgba(30,  30,  30,  255) }, // subtle
        border:      const { Color::rgba(38,  38,  42,  255) }, // more subtle
    }
}

struct User {
    scale: f32,

    mouse_pos: [f32; 2],
    mouse_clicked: bool,

    last_keypress: Instant,
    start: Instant,
    last_second: u64,

    frame_counter: usize,
    fps_history: Vec<usize>,

    prompt: PromptState,
}

impl User {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            scale: 1.0,
            mouse_clicked:  false,
            mouse_pos:      [0.0; 2],
            last_keypress:  now,
            start:          now,
            last_second:  0,
            frame_counter:  0,
            fps_history: Vec::new(),
            prompt:         Default::default(),
        }
    }
}

fn build_ui(ui: &mut UiState, user: &User, gpu: &mut gpu::Gpu, cursor_idle_secs: f32) {
    let scale = user.scale;
    let row_h = 24.0 * scale;
    let palette = palette();

    //
    // Measure both with real glyphs so layout is accurate at any scale
    //
    let prefix_width: f32 = PROMPT_PREFIX
        .chars()
        .filter_map(|c| gpu::get_glyph(gpu, c))
        .map(|g| g.advance)
        .sum();
    let cursor_offset: f32 = user.prompt
        .iterate_chars_until_cursor()
        .filter_map(|c| gpu::get_glyph(gpu, c))
        .map(|g| g.advance)
        .sum();

    ui.row("header")
        .size(Size::fill(), Size::px(row_h))
        .bg(palette.header_bar)
        .border(palette.border)
        .build_children(|ui| {
            ui.label("header##title")
                .text("rawgrep")
                .padding(scale * 8.0)
                .color(palette.accent)
                .build();
        });

    ui.row("prompt")
        .size(Size::fill(), Size::px(row_h))
        .bg(palette.prompt_box)
        .border(palette.border)
        .build_children(|ui| {
            ui.label("prompt##prefix")
                .size(Size::px(prefix_width + 8.0 * scale), Size::fill())
                .padding(8.0 * scale)
                .text(PROMPT_PREFIX)
                .color(palette.dim)
                .build();

            let input = ui.label("prompt##input")
                .size(Size::fill(), Size::fill())
                .padding(4.0 * scale)
                .text(user.prompt.buffer())
                .build();

            ui.boxes[input].custom = BoxCustom::TextInput(TextInputInfo {
                cursor_pixel_offset: cursor_offset,
                cursor_target_pixel_offset: cursor_offset,
                cursor_idle_secs,
                cursor_char: user.prompt.char_at_cursor()
            });
        });

    ui.row("btnrow")
        .size(Size::fill(), Size::px(row_h))
        .bg(palette.header_bar)
        .padding(scale * 15.0)
        .border(palette.border)
        .build_children(|ui| {
            ui.button("btn##search")
                .size(Size::px(80.0 * scale), Size::fill())
                .bg(Color::rgba(28, 40, 58, 255))
                .hover_color(Color::rgba(40, 58, 85, 255))
                .text("search")
                .build();
        });
}

//
//
// Winit plumbing
//
//

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    ui:     Option<ui::UiState>,
    gpu:    Option<gpu::Gpu>,
    user:   Option<User>,
    mods:   winit::event::Modifiers,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let win: Arc<_> = el.create_window(
            Window::default_attributes().with_title("rawgrep-ui")
        ).unwrap().into();

        let size = win.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        self.gpu    = Some(gpu::init(Arc::clone(&win)));
        self.ui     = Some(UiState::new(w as _, h as _));
        self.user   = Some(User::new());
        self.window = Some(win);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let WindowEvent::ModifiersChanged(m) = &event {
            self.mods = *m;
            return;
        }

        let (
            Some(gpu),
            Some(user),
            Some(win)
        ) = (&mut self.gpu, &mut self.user, &self.window) else { return };

        match event {
            WindowEvent::CloseRequested => el.exit(),

            WindowEvent::ModifiersChanged(m) => self.mods = m,

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed { return; }

                user.last_keypress = Instant::now();

                let ctrl = self.mods.state().control_key();
                let alt = self.mods.state().alt_key();

                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => el.exit(),

                    PhysicalKey::Code(KeyCode::Space)      => user.prompt.push_char(' '),
                    PhysicalKey::Code(KeyCode::Backspace)  => if ctrl {
                        user.prompt.kill_word_back()                                                  // C-w
                    } else {
                        _ = user.prompt.pop_char()
                    }

                    PhysicalKey::Code(KeyCode::ArrowLeft)  => user.prompt.move_cursor_left(),
                    PhysicalKey::Code(KeyCode::ArrowRight) => user.prompt.move_cursor_right(),

                    _ => {
                        //
                        // EMACS bindings
                        //

                        if ctrl {
                            match event.physical_key {
                                PhysicalKey::Code(KeyCode::KeyB) => user.prompt.move_cursor_left(),   // C-b
                                PhysicalKey::Code(KeyCode::KeyF) => user.prompt.move_cursor_right(),  // C-f
                                PhysicalKey::Code(KeyCode::KeyA) => user.prompt.move_cursor_start(),  // C-a
                                PhysicalKey::Code(KeyCode::KeyE) => user.prompt.move_cursor_end(),    // C-e
                                PhysicalKey::Code(KeyCode::KeyD) => _ = user.prompt.delete_forward(), // C-d
                                PhysicalKey::Code(KeyCode::KeyK) => user.prompt.kill_line(),          // C-k
                                _ => {}
                            }
                        } else if alt {
                            match event.physical_key {
                                PhysicalKey::Code(KeyCode::KeyF) => user.prompt.move_word_forward(), // M-f
                                PhysicalKey::Code(KeyCode::KeyB) => user.prompt.move_word_back(),    // M-b
                                _ => {}
                            }
                        } else if let Key::Character(s) = &event.logical_key {
                            user.prompt.push_str(s.as_str());
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                if self.mods.state().control_key() {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.01,
                    };
                    user.scale = (user.scale + dy * 0.1).clamp(MIN_SCALE, MAX_SCALE);

                    // @Speed: We recompute glyphs on each scale change from scratch...
                    gpu.glyphs.clear();
                    gpu.atlas_cur_x = 1;
                    gpu.atlas_cur_y = 1;
                    gpu.atlas_row_h = 0;
                    gpu.font_scale = user.scale;
                }
            }

            WindowEvent::Resized(sz) => {
                if sz.width > 0 && sz.height > 0 {
                    gpu.win_w = sz.width  as _;
                    gpu.win_h = sz.height as _;
                    gpu.surface_config.width  = sz.width;
                    gpu.surface_config.height = sz.height;
                    gpu.surface.configure(&gpu.device, &gpu.surface_config);
                }
            }

            WindowEvent::MouseInput { state: state_, button: MouseButton::Left, .. } => {
                user.mouse_clicked = state_ == ElementState::Pressed;
            }

            WindowEvent::CursorMoved { position, .. } => {
                user.mouse_pos = [position.x as _, position.y as _];
            }

            WindowEvent::RedrawRequested => {
                let ui   = self.ui.as_mut().unwrap();
                let gpu  = self.gpu.as_mut().unwrap();
                let user = self.user.as_mut().unwrap();

                user.frame_counter += 1;
                let secs = user.start.elapsed().as_secs();
                if secs != user.last_second {
                    let fps = user.frame_counter;
                    user.fps_history.push(fps);
                    user.frame_counter = 0;
                    user.last_second = secs;
                }

                let cursor_idle_secs = user.last_keypress.elapsed().as_secs_f32();

                //
                // Build the box tree
                //
                ui.begin_frame(gpu.win_w, gpu.win_h);

                let fps = if user.fps_history.is_empty() {
                    271
                } else {
                    user.fps_history.iter().sum::<usize>() / user.fps_history.len()
                };
                let row_h = 24.0 * user.scale;
                ui.row("FPS")
                    .size(Size::fill(), Size::px(row_h))
                    .build_children(|ui| {
                        ui.label("header##title")
                            .text(format!("FPS: {fps}"))
                            .padding(user.scale * 8.0)
                            .color(palette().accent)
                            .build();
                    });

                build_ui(ui, user, gpu, cursor_idle_secs);
                ui.end_frame();

                //
                // Layout
                //
                ui.layout(|text| {  // Measure text callback
                    let w = text.chars()
                        .filter_map(|c| gpu::get_glyph(gpu, c))
                        .map(|g| g.advance)
                        .sum();

                    [w, gpu.scaled_font_size()]
                });

                //
                // Interaction
                //
                ui.update_interaction(user.mouse_pos, user.mouse_clicked);

                //
                // Tick animations
                //
                ui.tick();

                //
                // Render tree -> gpu draw calls
                //
                ui::render(ui, gpu);

                //
                // Submit
                //
                gpu::submit(gpu).unwrap();

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
