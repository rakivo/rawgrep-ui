mod ui;
mod gpu;
mod util;
mod color;
mod prompt;
mod search;
mod highlight;

use std::sync::Arc;
use std::time::Instant;

use gpu::Gpu;
use color::Color;
use highlight::TokenKind;
use prompt::PromptState;
use search::{SearchManager, SearchStatus};
use ui::{Axis, BoxCustom, BoxRef, MatchInfo, Size, TextInputInfo, UiState};

use util::{display_path, lerp_color};
use winit::window::{Window, WindowId};
use winit::application::ApplicationHandler;
use winit::keyboard::{Key, KeyCode, PhysicalKey};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

const MIN_SCALE: f32 = 0.75;
const MAX_SCALE: f32 = 5.00;
const BASE_SCALE: f32 = 1.45;

const PROMPT_PREFIX: &str = "> ";

const SCROLL_BAR_HOVER_EXPANSION_FACTOR: f32 = 3.5;

const TITLE_BASE_FONT_SIZE:  f32 = 15.0;
const PROMPT_BASE_FONT_SIZE: f32 = 13.0;
const SEARCH_BASE_FONT_SIZE: f32 = 14.0;
const RESULT_BASE_FONT_SIZE: f32 = 12.0;

const TITLE_BASE_ROW_HEIGHT:  f32 = 25.0;
const PROMPT_BASE_ROW_HEIGHT: f32 = 28.0;
const SEARCH_BASE_ROW_HEIGHT: f32 = 26.0;
const RESULT_BASE_ROW_HEIGHT: f32 = 20.0;

const fn top_bars_h(scale: f32) -> f32 {
    TITLE_BASE_ROW_HEIGHT + (PROMPT_BASE_ROW_HEIGHT + SEARCH_BASE_ROW_HEIGHT) * scale
}

const fn results_viewport_h(gpu: &Gpu, scale: f32) -> f32 {
    gpu.win_h - top_bars_h(scale)
}

const fn result_h(scale: f32) -> f32 {
    RESULT_BASE_ROW_HEIGHT * scale
}

const fn results_content_h(result_count: usize, scale: f32) -> f32 {
    result_count as f32 * result_h(scale)
}

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
    prompt_input_ref: Option<BoxRef>,

    frame_search_results: Vec<BoxRef>, // Rebuilt every frame

    scrollbar_drag: Option<f32>, // Some(offset_within_thumb) when dragging
    scrollbar_hot_t: f32,

    prompt: PromptState,
}

impl UserState {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            scale: BASE_SCALE,
            mouse_pos:      [0.0; 2],
            last_keypress:  now,

            mouse_clicked:        Default::default(),
            prompt:               Default::default(),
            prompt_input_ref:     Default::default(),
            frame_search_results: Default::default(),
            scrollbar_drag:       Default::default(),
            search_btn_ref:       Default::default(),
            scrollbar_hot_t:      Default::default(),
        }
    }
}

struct ScrollbarGeometry {
    bar_x:      f32,
    bar_w:      f32,
    track_y:    f32,
    track_h:    f32,
    thumb_y:    f32,
    thumb_h:    f32,
    max_scroll: f32,
}

impl ScrollbarGeometry {
    fn compute(
        gpu: &Gpu,
        ui: &UiState,
        result_count: usize,
        scale: f32,
    ) -> Option<ScrollbarGeometry> {
        let top_bars_h = top_bars_h(scale);
        let viewport_h = results_viewport_h(gpu, scale);
        let content_h  = results_content_h(result_count, scale);

        if content_h <= viewport_h { return None; }

        let scroll_visual = ui.get_scroll("results");
        let max_scroll    = (content_h - viewport_h).max(1.0);
        let bar_w         = 3.0_f32.max(scale * 2.0);
        let bar_x         = gpu.win_w - bar_w - 2.0;
        let thumb_pct     = (viewport_h / content_h).min(1.0);
        let thumb_h       = (thumb_pct * viewport_h).max(20.0);
        let scroll_pct    = (scroll_visual / max_scroll).clamp(0.0, 1.0);
        let thumb_y       = top_bars_h + scroll_pct * (viewport_h - thumb_h);

        Some(ScrollbarGeometry {
            bar_x, bar_w,
            track_y: top_bars_h, track_h: viewport_h,
            thumb_y, thumb_h,
            max_scroll
        })
    }
}

#[inline]
fn draw_error(
    ui: &UiState,
    user: &UserState,
    gpu: &mut Gpu,
    search: &SearchManager
) {
    let SearchStatus::Error(msg) = &search.status else {
        return;
    };

    if let Some(input) = user.prompt_input_ref {
        let b = &ui.boxes[input];
        let [x0, y0, _, y1] = b.rect;

        let w = gpu.win_w - x0; // Stretch to window edge
        let h = y1 - y0;

        let text_x = x0 + 4.0 * user.scale;
        let text_y = y0 + h * 0.5 + PROMPT_BASE_FONT_SIZE * user.scale * 0.35;

        //
        // Dim overlay user you can still see the input text underneath
        //
        gpu::draw_rect(gpu, x0, y0, w, h, Color::rgba(40, 10, 10, 210));
        gpu::draw_text(
            gpu,
            msg,
            text_x, text_y,
            PROMPT_BASE_FONT_SIZE * user.scale * 0.85,
            Color::rgba(180, 80, 80, 255)
        );
    }
}

#[inline]
fn draw_scrollbar(gpu: &mut Gpu, geom: &ScrollbarGeometry, hot_t: f32) {
    let bar_w_hot = geom.bar_w * SCROLL_BAR_HOVER_EXPANSION_FACTOR;
    let bar_w     = geom.bar_w + (bar_w_hot - geom.bar_w) * hot_t;
    let bar_x     = geom.bar_x + geom.bar_w - bar_w;

    let thumb_color = lerp_color(
        Color::rgba(100, 100, 100, 50).into(),
        Color::rgba(150, 190, 220, 90).into(),
        hot_t,
    );

    gpu::draw_rect(gpu, bar_x, geom.track_y, bar_w, geom.track_h, Color::rgba(255, 255, 255, 8));
    gpu::draw_rect(gpu, bar_x, geom.thumb_y, bar_w, geom.thumb_h, thumb_color.into());
}

fn interact(
    ui: &mut UiState,
    user: &mut UserState,
    _gpu: &mut Gpu,
    search: &mut SearchManager,
    scrollbar: Option<&ScrollbarGeometry>
) {
    let thumb_hovered = scrollbar.map_or(false, |geom| {
        let [mx, my] = user.mouse_pos;

        let hit_x = geom.bar_x + geom.bar_w - geom.bar_w * SCROLL_BAR_HOVER_EXPANSION_FACTOR;

           mx >= hit_x
        && my >= geom.thumb_y
        && my <= geom.thumb_y + geom.thumb_h
    });
    let hot_target = if thumb_hovered || user.scrollbar_drag.is_some() {
        1.0
    } else {
        0.0
    };
    user.scrollbar_hot_t += (hot_target - user.scrollbar_hot_t) * 0.15;

    if !user.mouse_clicked {
        return;
    }

    if thumb_hovered && let Some(s) = scrollbar {
        user.scrollbar_drag = Some(user.mouse_pos[1] - s.thumb_y);
        return;
    }

    //
    // Only check UI clicks if not dragging scrollbar
    //

    let search_clicked = user.search_btn_ref.map_or(false, |btn| ui.was_clicked(btn));
    let result_index   = user.frame_search_results.iter().position(|&b| ui.was_clicked(b));

    if search_clicked {
        App::trigger_search(user, search);
    }

    if let Some(index) = result_index {
        // @Cutnpaste from build_ui
        let result_h      = result_h(user.scale);
        let scroll_off    = ui.get_scroll("results");
        let visible_start = (scroll_off / result_h).floor() as usize;

        let index = visible_start + index;
        let result = &search.results[index];
        if let Ok(path) = std::str::from_utf8(&result.path) {
            crate::util::open_in_emacs(path, result.line_num);
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

    let title_h  = TITLE_BASE_ROW_HEIGHT;
    let prompt_h = PROMPT_BASE_ROW_HEIGHT * scale;
    let search_h = SEARCH_BASE_ROW_HEIGHT * scale;
    let result_h = RESULT_BASE_ROW_HEIGHT * scale;

    let title_font_size  = TITLE_BASE_FONT_SIZE;
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
                .padding(6.0)
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

            user.prompt_input_ref = Some(input);

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
        .padding(scale * 10.0)
        .border(palette.border)
        .build_children(|ui| {
            user.search_btn_ref = ui.button("search##btn")
                .size(Size::text(), Size::fill())
                .bg(Color::rgba(28, 40, 58, 255))
                .hover_color(Color::rgba(40, 58, 85, 255))
                .text("search")
                .font_size(search_font_size)
                .build()
                .into();
        });

    let results = &search.results;

    let viewport_h = results_viewport_h(gpu, scale);

    let scroll_off    = ui.get_scroll("results");
    let visible_start = (scroll_off / result_h).floor() as usize;
    let visible_count = (viewport_h / result_h).ceil() as usize + 2; // +2 for the partially visible bottom and the top rows
    let visible_end   = (visible_start + visible_count).min(results.len());

    let top_space    = visible_start as f32 * result_h;
    let bottom_space = (results.len().saturating_sub(visible_end)) as f32 * result_h;

    ui.scroll("results")
    .size(Size::fill(), Size::fill())
    .bg(palette.bg)
    .build_children(|ui| {
        // Phantom space above visible rows
        if top_space > 0.0 {
            ui.spacer("results##top", Axis::Y, top_space);
        }

        let max_line_num = search.results.iter().map(|m| m.line_num).max().unwrap_or(1);
        let digits_count   = max_line_num.ilog10() + 1;
        let digit_w = gpu::get_glyph(gpu, '0', result_font_size).map(|g| g.advance)
            .unwrap_or(result_font_size * 0.6);
        let linenum_w = (digits_count as f32 + 4.5) * digit_w ;

        user.frame_search_results.clear();

        for (index, m) in results.get(visible_start..visible_end).unwrap_or_default().iter().enumerate() {
            let index = index + visible_start;

            let Ok(text_raw)     = std::str::from_utf8(&m.text) else { continue };
            let text = text_raw.trim_start();
            let trim_offset = (text_raw.len() - text.len()) as u32;
            let ranges = m.ranges.iter().filter_map(|(s, e)| {    // adjust ranges
                if *e <= trim_offset { None }       // range entirely in trimmed part
                else { Some((s.saturating_sub(trim_offset), *e - trim_offset)) }
            }).collect();

            let Ok(filename) = std::str::from_utf8(&m.path) else { continue };
            let filename     = display_path(&filename, 16); // @Constant @Tune
            let line_num     = m.line_num;

            let result_ref = ui.row(&format!("result_{index}"))
                .size(Size::fill(), Size::px(result_h))
                .bg(if index % 2 == 0 { Color::rgba(15,15,15,255) } else { Color::rgba(18,18,18,255) })
                .hover_color(Color::rgba(30, 35, 45, 255))
                .build_children(|ui| {
                    ui.label(&format!("result_{index}##filename"))
                        .size(Size::px(140.0 * scale), Size::fill())
                        .font_size(result_font_size)
                        .padding(6.0 * scale)
                        .text(filename.clone()) // @Clone
                        .color(palette.accent)
                        .build();

                    ui.label(&format!("result_{index}##linenum"))
                        .size(Size::px(linenum_w), Size::fill())
                        .font_size(result_font_size)
                        .padding(6.0 * scale)
                        .text(format!(":{line_num}"))
                        .color(palette.dim)
                        .build();

                    let text_ref = ui.label(&format!("result_{index}##text"))
                        .size(Size::fill(), Size::fill())
                        .font_size(result_font_size)
                        .padding(6.0 * scale)
                        .text(text)
                        .build();

                    let mut byte_kinds = vec![TokenKind::Normal; text.len() + 1];  // @Memory
                    for t in highlight::tokenize(text) {
                        for k in &mut byte_kinds[t.start as usize..t.end as usize] {
                            *k = t.kind;
                        }
                    }

                    ui.boxes[text_ref].custom = BoxCustom::Match(MatchInfo {
                        match_ranges: ranges,
                        byte_kinds: byte_kinds.into()
                    });
                });

            user.frame_search_results.push(result_ref);
        }

        // Phantom space below visible rows
        if bottom_space > 0.0 {
            ui.spacer("results##bottom", Axis::Y, bottom_space);
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
    ui:     Option<UiState>,
    gpu:    Option<Gpu>,
    user:   Option<UserState>,
    search: Option<SearchManager>,

    window: Option<Arc<Window>>,
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

                let old_prompt_len = user.prompt.buffer().len();
                let old_cursor_pos = user.prompt.cursor();

                let mut stop_drawing_error = false;

                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => el.exit(),

                    PhysicalKey::Code(KeyCode::Enter) => {
                        Self::trigger_search(user, search);
                        stop_drawing_error = true;
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
                                PhysicalKey::Code(KeyCode::KeyD) => user.prompt.kill_word_forward(),  // M-d
                                _ => {}
                            }
                        } else if let Key::Character(s) = &event.logical_key {
                            user.prompt.push_str(s.as_str());
                        }
                    }
                }

                stop_drawing_error |= user.prompt.buffer().len() != old_prompt_len;
                stop_drawing_error |= user.prompt.cursor() != old_cursor_pos;

                if stop_drawing_error && matches!(search.status, SearchStatus::Error(_)) {
                    //
                    // Clear the error on keypress!
                    //
                    search.status = SearchStatus::Idle;
                    search.pending.lock().status = SearchStatus::Idle;
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let content_h  = results_content_h(search.results.len(), user.scale);
                let viewport_h = results_viewport_h(gpu, user.scale);

                if self.mods.state().control_key() {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 * 0.01,
                    };

                    user.scale = (user.scale + dy * 0.1).clamp(MIN_SCALE, MAX_SCALE);
                    ui.clamp_scroll("results", content_h, viewport_h);

                    // @Speed: We recompute glyphs on each scale change from scratch...
                    gpu.glyphs.clear();
                    gpu.atlas_cur_x = 1;
                    gpu.atlas_cur_y = 1;
                    gpu.atlas_row_h = 0;

                    return;
                }

                //
                // Update possible result scroll
                //
                // @Incomplete
                //
                // Though, we probably wanna detect that `mouse_pos` is inside the results section..
                //

                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 40.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if let Some(ui) = &mut self.ui {
                    ui.scroll_by("results", -dy, content_h, viewport_h);
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
                if input_state == ElementState::Released {
                    user.scrollbar_drag = None;
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                user.mouse_pos = [position.x as _, position.y as _];

                let Some(drag_off) = user.scrollbar_drag else {
                    return;
                };
                let Some(scrollbar) = ScrollbarGeometry::compute(
                    gpu,
                    ui,
                    search.results.len(),
                    user.scale
                ) else {
                    return;
                };
                let thumb_top  = user.mouse_pos[1] - drag_off - scrollbar.track_y;
                let scroll_pct = (thumb_top / (scrollbar.track_h - scrollbar.thumb_h)).clamp(0.0, 1.0);
                let new_scroll = scroll_pct * scrollbar.max_scroll;
                let k = ui.hash_str("results");
                if let Some(p) = ui.persist.get_mut(&k) {
                    p.scroll_target = new_scroll;
                    p.scroll_offset = new_scroll;
                    p.scroll_visual = new_scroll;
                }
            }

            WindowEvent::RedrawRequested => {
                //
                // Drain potential search results to our local storage
                //
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
                let scrollbar = ScrollbarGeometry::compute(gpu, ui, search.results.len(), user.scale);
                {
                    interact(ui, user, gpu, search, scrollbar.as_ref());
                    user.mouse_clicked = false;

                    // If scrollbar captured the click, clear UI active state so no flash on results
                    if user.scrollbar_drag.is_some() {
                        ui.clear_active();
                    }
                }

                //
                // Tick
                //
                ui.tick();

                //
                // Render tree -> gpu draw calls
                //
                ui::render(ui, gpu);
                if let Some(ref geom) = scrollbar {
                    draw_scrollbar(gpu, geom, user.scrollbar_hot_t);
                }
                draw_error(ui, user, gpu, search);

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
    rawgrep::util::init_logging();

    let el = EventLoop::new().unwrap();
    //
    // Poll so the rainbow animates smoothly without input events
    //
    el.set_control_flow(ControlFlow::Poll);
    el.run_app(&mut App::default()).unwrap();
}
