mod ui;
mod gpu;
mod util;
mod color;
mod prompt;
mod search;

use std::sync::Arc;
use std::time::Instant;

use gpu::Gpu;
use color::Color;
use prompt::PromptState;
use search::SearchManager;
use ui::{BoxCustom, BoxRef, Size, TextInputInfo, UiState};

use winit::window::{Window, WindowId};
use winit::application::ApplicationHandler;
use winit::keyboard::{Key, KeyCode, PhysicalKey};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

const MIN_SCALE: f32 = 0.75;
const MAX_SCALE: f32 = 5.00;

const PROMPT_PREFIX: &str = "> ";

const BASE_SCALE: f32 = 1.45;

const TITLE_BASE_FONT_SIZE:  f32 = 11.0;
const PROMPT_BASE_FONT_SIZE: f32 = 13.0;
const SEARCH_BASE_FONT_SIZE: f32 = 15.0;
const RESULT_BASE_FONT_SIZE: f32 = 12.0;

const TITLE_BASE_ROW_HEIGHT:  f32 = 18.0;
const PROMPT_BASE_ROW_HEIGHT: f32 = 28.0;
const SEARCH_BASE_ROW_HEIGHT: f32 = 27.0;
const RESULT_BASE_ROW_HEIGHT: f32 = 20.0;

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

struct UserState {
    scale: f32,

    mouse_pos: [f32; 2],
    mouse_clicked: bool,

    last_keypress: Instant,

    search_btn_ref: Option<BoxRef>,

    prompt: PromptState,
}

impl UserState {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            scale: BASE_SCALE,
            mouse_clicked:  false,
            mouse_pos:      [0.0; 2],
            last_keypress:  now,
            prompt:         Default::default(),
            search_btn_ref: None
        }
    }
}

fn build_ui(
    ui: &mut UiState,
    user: &mut UserState,
    gpu: &mut Gpu,
    search: &SearchManager,
    cursor_idle_secs: f32
) {
    let scale = user.scale;
    let palette = palette();

    let title_h  = TITLE_BASE_ROW_HEIGHT  * scale;
    let prompt_h = PROMPT_BASE_ROW_HEIGHT * scale;
    let search_h = SEARCH_BASE_ROW_HEIGHT * scale;
    let result_h = RESULT_BASE_ROW_HEIGHT * scale;

    let title_font_size  = TITLE_BASE_FONT_SIZE  * scale;
    let prompt_font_size = PROMPT_BASE_FONT_SIZE * scale;
    let search_font_size = SEARCH_BASE_FONT_SIZE * scale;
    let result_font_size = RESULT_BASE_FONT_SIZE * scale;

    //
    // Measure with real glyphs so layout is accurate at any scale
    //
    let cursor_offset: f32 = user.prompt
        .iterate_chars_until_cursor()
        .filter_map(|c| gpu::get_glyph(gpu, c, prompt_font_size))
        .map(|g| g.advance)
        .sum();

    ui.row("header")
        .size(Size::fill(), Size::px(title_h))
        .bg(palette.header_bar)
        .border(palette.border)
        .build_children(|ui| {
            ui.label("header##title")
                .text("rawgrep")
                .font_size(title_font_size)
                .padding(scale * 6.0)
                .color(palette.accent)
                .build();
        });

    ui.row("prompt")
        .size(Size::fill(), Size::px(prompt_h))
        .bg(palette.prompt_box)
        .border(palette.border)
        .build_children(|ui| {
            ui.label("prompt##prefix")
                .size(Size::text(), Size::fill())
                .padding_x(8.0 * scale, scale)
                .text(PROMPT_PREFIX)
                .font_size(prompt_font_size)
                .color(palette.dim)
                .build();

            let input = ui.label("prompt##input")
                .size(Size::fill(), Size::fill())
                .padding(4.0 * scale)
                .text(user.prompt.buffer())
                .font_size(prompt_font_size)
                .build();

            ui.boxes[input].custom = BoxCustom::TextInput(TextInputInfo {
                cursor_pixel_offset: cursor_offset,
                cursor_target_pixel_offset: cursor_offset,
                cursor_idle_secs,
                cursor_char: user.prompt.char_at_cursor()
            });
        });

    ui.row("search")
        .size(Size::fill(), Size::px(search_h))
        .bg(palette.header_bar)
        .padding(scale * 15.0)
        .border(palette.border)
        .build_children(|ui| {
            user.search_btn_ref = ui.button("search##btn")
                .size(Size::px(80.0 * scale), Size::fill())
                .bg(Color::rgba(28, 40, 58, 255))
                .hover_color(Color::rgba(40, 58, 85, 255))
                .text("search")
                .font_size(search_font_size)
                .build()
                .into();
        });

    ui.scroll("results")
    .size(Size::fill(), Size::fill())
    .bg(palette.bg)
    .build_children(|ui| {
        for (i, m) in search.results.iter().enumerate() {
            let filename = String::from_utf8_lossy(&m.path);
            let line_num = m.line_num;
            let text     = String::from_utf8_lossy(&m.text);

            ui.row(&format!("result_{i}"))
                .size(Size::fill(), Size::px(result_h))
                .bg(if i % 2 == 0 { Color::rgba(15,15,15,255) } else { Color::rgba(18,18,18,255) })
                .hover_color(Color::rgba(30, 35, 45, 255))
                .build_children(|ui| {
                    ui.label(&format!("result_{i}##filename"))
                        .size(Size::px(120.0 * scale), Size::fill())
                        .font_size(result_font_size)
                        .padding(6.0 * scale)
                        .text(filename)
                        .color(palette.accent)
                        .build();

                    ui.label(&format!("result_{i}##linenum"))
                        .size(Size::px(40.0 * scale), Size::fill())
                        .font_size(result_font_size)
                        .padding(6.0 * scale)
                        .text(format!(":{line_num}"))
                        .color(palette.dim)
                        .build();

                    ui.label(&format!("result_{i}##text"))
                        .size(Size::fill(), Size::fill())
                        .font_size(result_font_size)
                        .padding(6.0 * scale)
                        .text(text)
                        .build();
                });
        }
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
    ui:     Option<UiState>,
    gpu:    Option<Gpu>,
    user:   Option<UserState>,
    search: Option<SearchManager>,
    mods:   winit::event::Modifiers,
}

impl App {
    /// Fire a search for the current prompt contents.
    fn trigger_search(user: &UserState, search: &mut SearchManager) {
        let pattern = user.prompt.buffer();
        if pattern.is_empty() { return }

        search.start(pattern, ".");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        let win: Arc<_> = el.create_window(
            Window::default_attributes()
                .with_title("rawgrep")
                // .with_transparent(true)
                .with_decorations(false)
        ).unwrap().into();

        let size = win.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        self.gpu    = Some(gpu::init(Arc::clone(&win)));
        self.ui     = Some(UiState::new(w as _, h as _));
        self.user   = Some(UserState::new());
        self.search = Some(SearchManager::spawn());
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
            Some(ui),
            Some(search),
            Some(win),
        ) = (&mut self.gpu, &mut self.user, &mut self.ui, &mut self.search, &self.window) else { return };

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

                    PhysicalKey::Code(KeyCode::Enter) => {
                        Self::trigger_search(user, search);
                    }

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
                                PhysicalKey::Code(KeyCode::KeyF) => user.prompt.move_word_forward(),  // M-f
                                PhysicalKey::Code(KeyCode::KeyB) => user.prompt.move_word_back(),     // M-b
                                _ => {}
                            }
                        } else if let Key::Character(s) = &event.logical_key {
                            user.prompt.push_str(s.as_str());
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let result_count = search.results.len();
                let result_h     = RESULT_BASE_ROW_HEIGHT * user.scale;
                let content_h    = result_count as f32 * result_h;
                let viewport_h   = gpu.win_h - (TITLE_BASE_ROW_HEIGHT + PROMPT_BASE_ROW_HEIGHT + SEARCH_BASE_ROW_HEIGHT) * user.scale;

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

                    //
                    // Clamp scroll to new content size after scale change
                    //
                    if let Some(ui) = &mut self.ui {
                        ui.clamp_scroll("results", content_h, viewport_h);
                    }
                } else {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * 40.0,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };

                    if let Some(ui) = &mut self.ui {
                        ui.scroll_by("results", -dy, content_h, viewport_h);
                    }
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

            WindowEvent::MouseInput { state: input_state, button: MouseButton::Left, .. } => {
                user.mouse_clicked = input_state == ElementState::Pressed;
            }

            WindowEvent::CursorMoved { position, .. } => {
                user.mouse_pos = [position.x as _, position.y as _];
            }

            WindowEvent::RedrawRequested => {
                search.drain();

                let cursor_idle_secs = user.last_keypress.elapsed().as_secs_f32();

                //
                // Build the box tree
                //
                ui.begin_frame(gpu.win_w, gpu.win_h);
                build_ui(ui, user, gpu, search, cursor_idle_secs);
                ui.end_frame();

                //
                // Layout
                //
                ui.layout(|text, font_size| {  // Measure text callback
                    let w = text.chars()
                        .filter_map(|c| gpu::get_glyph(gpu, c, font_size))
                        .map(|g| g.advance)
                        .sum();

                    [w, font_size]
                });

                //
                // Interaction
                //
                ui.update_interaction(user.mouse_pos, user.mouse_clicked);

                user.mouse_clicked = false;
                if user.search_btn_ref.map_or(false, |btn| ui.was_clicked(btn)) {
                    Self::trigger_search(user, search);
                }

                //
                // Tick
                //
                ui.tick();

                //
                // Render tree -> gpu draw calls
                //
                ui::render(ui, gpu);

                //
                // Submit
                //
                gpu::submit_frame(gpu).unwrap();

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
