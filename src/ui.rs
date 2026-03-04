#![allow(unused)]

use std::collections::HashMap;

use cranelift_entity::PrimaryMap;
use smallvec::SmallVec;

use crate::{color::Color, gpu::{FONT_SIZE, Gpu}};

#[derive(Eq, PartialEq, PartialOrd, Copy, Clone, Debug)]
pub struct BoxRef(pub u32);

cranelift_entity::entity_impl!(BoxRef);

bitflags::bitflags! {
    #[derive(Clone, Copy, Default)]
    pub struct BoxFlags: u32 {
        const DRAW_BG         = 0x0;
        const DRAW_BORDER     = 0x1;
        const DRAW_TEXT       = 0x2;
        const CLICKABLE       = 0x4;
        const HOVERABLE       = 0x8;
        const CLIP_CHILDREN   = 0x10;
        const SCROLL_CHILDREN = 0x20;
    }
}

pub struct TextInputInfo {
    pub cursor_idle_secs:    f32,
    pub cursor_pixel_offset: f32,
    pub cursor_target_pixel_offset: f32,
    pub cursor_char: Option<char>,
}

#[derive(Default)]
pub enum BoxCustom {
    #[default]
    None,

    TextInput(TextInputInfo),
}

impl BoxCustom {
    #[inline]
    pub fn text_input(&self) -> Option<&TextInputInfo> {
        match self {
            Self::TextInput(i) => Some(i),
            _ => None
        }
    }
}

pub struct Box {
    pub key: u64,   // string hash - links to BoxPersist

    pub children: SmallVec<[BoxRef; 4]>,
    pub parent: Option<BoxRef>,

    // appearance
    pub flags:        BoxFlags,
    pub bg_color:     Color,
    pub hover_color:  Color,
    pub border_color: Color,
    pub text:         String, // @Memory
    pub text_color:   Color,

    // layout INPUT
    pub pref_size:  [Size; 2],
    pub child_axis: Axis,
    pub padding:    f32,

    // layout OUTPUT
    pub rect: [f32; 4], // x0, y0, x1, y1
    pub computed_size: [f32; 2], // w, h

    pub custom: BoxCustom,
}

impl Default for Box {
    fn default() -> Self {
        Self {
            key:           0,
            parent:        None,
            children:      SmallVec::default(),
            pref_size:     [Size::fill(), Size::fill()],
            child_axis:    Axis::X,
            padding:       4.0,
            flags:         BoxFlags::empty(),
            bg_color:      Color::default(),
            hover_color:   Color::default(),
            border_color:  Color::default(),
            text:          String::new(),
            text_color:    Color::rgba(255, 255, 255, 255),
            computed_size: [0.0; 2],
            rect:          [0.0; 4],
            custom:        Default::default(),
        }
    }
}

impl Box {
    #[inline]
    pub fn text_input(&self) -> Option<&TextInputInfo> {
        self.custom.text_input()
    }
}

#[derive(Default)]
pub struct BoxPersist {
    pub hot_t:              f32,   // hover  [0..1]
    pub active_t:           f32,   // click  [0..1]
    pub scroll_off:         f32,   // scroll offset in pixels
    pub last_frame_touched: u64,
    pub cursor_visual_x:    f32,
}

#[derive(Clone, Copy)]
pub enum SizeKind {
    Pixels(f32),
    TextContent,
    ParentPct(f32),
    ChildrenSum,
}

#[derive(Clone, Copy)]
pub struct Size {
    pub kind: SizeKind,
    pub strictness: f32, // 0.0 = flexible, 1.0 = rigid
}

impl Size {
    pub fn px(v: f32)  -> Self { Self { kind: SizeKind::Pixels(v),       strictness: 1.0 } }
    pub fn fill()      -> Self { Self { kind: SizeKind::ParentPct(1.0),  strictness: 0.0 } }
    pub fn text()      -> Self { Self { kind: SizeKind::TextContent,     strictness: 1.0 } }
    pub fn pct(v: f32) -> Self { Self { kind: SizeKind::ParentPct(v),    strictness: 0.0 } }
    pub fn children()  -> Self { Self { kind: SizeKind::ChildrenSum,     strictness: 1.0 } }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Axis { #[default] X, Y }

// These let callers push/pop style without passing params everywhere
#[derive(Default)]
struct Stacks {
    size_x:     Vec<Size>,
    size_y:     Vec<Size>,
    bg_color:   Vec<Color>,
    text_color: Vec<Color>,
    child_axis: Vec<Axis>,
    padding:    Vec<f32>,
}

impl Stacks {
    fn top_size_x(&self)     -> Size   { self.size_x.last().copied().unwrap_or(Size::fill()) }
    fn top_size_y(&self)     -> Size   { self.size_y.last().copied().unwrap_or(Size::fill()) }
    fn top_bg(&self)         -> Color  { self.bg_color.last().copied().unwrap_or(Color::default()) }
    fn top_text_color(&self) -> Color  { self.text_color.last().copied().unwrap_or(Color::rgba(255,255,255,255)) }
    fn top_axis(&self)       -> Axis   { self.child_axis.last().copied().unwrap_or(Axis::X) }
    fn top_padding(&self)    -> f32    { self.padding.last().copied().unwrap_or(4.0) }
}

pub struct UiState {
    // box storage - rebuilt every frame
    pub boxes:      PrimaryMap<BoxRef, Box>,
    pub root:       Option<BoxRef>,

    // parent stack for building the tree
    parent_stack:   Vec<BoxRef>,

    // context stacks
    stacks:         Stacks,

    // persistent state - survives across frames
    pub persist:    HashMap<u64, BoxPersist>,

    // interaction
    pub hot_key:    u64,    // currently hovered
    pub active_key: u64,    // currently pressed

    // frame counter - used to reap dead persist entries
    pub frame:      u64,

    // window size - needed for root box and ParentPct
    pub win_w:      f32,
    pub win_h:      f32,
}

impl UiState {
    #[inline]
    pub fn new(win_w: f32, win_h: f32) -> Self {
        Self {
            boxes:        PrimaryMap::default(),
            root:         None,
            parent_stack: Vec::new(),
            stacks:       Stacks::default(),
            persist:      HashMap::new(),
            hot_key:      0,
            active_key:   0,
            frame:        0,
            win_w,
            win_h,
        }
    }

    #[inline]
    pub fn begin_frame(&mut self, win_w: f32, win_h: f32) {
        self.win_w = win_w;
        self.win_h = win_h;
        self.frame += 1;
        self.boxes.clear();
        self.parent_stack.clear();

        // Make root box
        let root = self.boxes.push(Box {
            pref_size:  [Size::px(win_w), Size::px(win_h)],
            child_axis: Axis::Y,
            computed_size: [win_w, win_h],
            ..Default::default()
        });
        self.root = Some(root);
        self.parent_stack.push(root);
    }

    #[inline]
    pub fn end_frame(&mut self) {
        // Remove persist entries not touched this frame
        self.persist.retain(|_, p| p.last_frame_touched == self.frame);
    }

    #[inline]
    pub fn was_clicked(&self, key: BoxRef) -> bool {
        let string_key = self.boxes[key].key;
        string_key == self.active_key
    }

    /// Push a new box as child of current parent, return its key
    #[inline]
    pub fn push_box(&mut self, string_key: u64, flags: BoxFlags) -> BoxRef {
        let persist = self.persist.entry(string_key).or_default();
        persist.last_frame_touched = self.frame;

        let parent = self.parent_stack.last().copied();

        let id = self.boxes.push(Box {
            key:        string_key,
            parent,
            pref_size:  [self.stacks.top_size_x(), self.stacks.top_size_y()],
            child_axis: self.stacks.top_axis(),
            padding:    self.stacks.top_padding(),
            flags,
            bg_color:   self.stacks.top_bg(),
            text_color: self.stacks.top_text_color(),
            ..Default::default()
        });

        // Link into parent
        if let Some(pk) = parent {
            self.boxes[pk].children.push(id);
        }

        id
    }

    #[inline]
    pub fn push_parent(&mut self, key: BoxRef) {
        self.parent_stack.push(key);
    }

    #[inline]
    pub fn pop_parent(&mut self) {
        self.parent_stack.pop();
    }

    #[inline]
    pub fn update_interaction(&mut self, mouse: [f32; 2], clicked: bool) {
        self.hot_key = 0;

        //
        // Walk all boxes, find topmost hovered + hoverable
        //
        for b in self.boxes.values() {
            if !b.flags.contains(BoxFlags::HOVERABLE) { continue }

            let [x0, y0, x1, y1] = b.rect;
            if mouse[0] >= x0 && mouse[0] <= x1
            && mouse[1] >= y0 && mouse[1] <= y1 {
                self.hot_key = b.key;
            }
        }

        if clicked {
            self.active_key = self.hot_key;
        } else if self.active_key != 0 {
            self.active_key = 0;
        }
    }

    #[inline]
    pub fn tick_animations(&mut self) {
        //
        // Collect targets first to avoid borrow issues
        //
        let targets = self.boxes.values()
            .filter_map(|b| b.text_input().map(|t| (b.key, t.cursor_target_pixel_offset)))
            .collect::<Vec<_>>(); // @SmallVecCandidate

        for (key, p) in &mut self.persist {
            let hot_target    = if *key == self.hot_key    { 1.0 } else { 0.0 };
            let active_target = if *key == self.active_key { 1.0 } else { 0.0 };
            p.hot_t    += (hot_target - p.hot_t) * 0.15;
            p.active_t *= 0.75;
            if active_target == 1.0 { p.active_t = 1.0; }

            // Smooth cursor
            if let Some(&(_, target)) = targets.iter().find(|(k, _)| k == key) {
                p.cursor_visual_x += (target - p.cursor_visual_x) * 0.25;
            }
        }
    }

    fn pass1_standalone(
        &mut self,
        key: BoxRef,
        measure_callback: &mut impl FnMut(&str) -> [f32; 2]
    ) {
        let children = self.boxes[key].children.clone();
        for axis in [Axis::X, Axis::Y] {
            let axis = axis as usize;

            let kind = self.boxes[key].pref_size[axis].kind;
            match kind {
                SizeKind::Pixels(v) => self.boxes[key].computed_size[axis] = v,

                SizeKind::TextContent => {
                    let text = &self.boxes[key].text;
                    let measured = measure_callback(text);
                    self.boxes[key].computed_size[axis] = measured[axis];
                }

                _ => {}
            }
        }

        for c in children {
            self.pass1_standalone(c, measure_callback);
        }
    }

    fn pass2a_parent_pct(&mut self, key: BoxRef, parent_size: [f32; 2]) {
        let children = self.boxes[key].children.clone();
        for axis in [Axis::X, Axis::Y] {
            let axis = axis as usize;

            let kind = self.boxes[key].pref_size[axis].kind;
            if let SizeKind::ParentPct(pct) = kind {
                self.boxes[key].computed_size[axis] = parent_size[axis] * pct;
            }
        }

        let my_size = self.boxes[key].computed_size;
        for c in children {
            self.pass2a_parent_pct(c, my_size);
        }
    }

    fn pass2b_children_sum(&mut self, key: BoxRef) {
        let children = self.boxes[key].children.clone();
        for c in &children {
            self.pass2b_children_sum(*c);
        }

        let child_axis = self.boxes[key].child_axis as usize;
        for axis in [Axis::X, Axis::Y] {
            let axis = axis as usize;

            let kind = self.boxes[key].pref_size[axis].kind;
            if !matches!(kind, SizeKind::ChildrenSum) { continue }

            let v = if axis == child_axis {
                children.iter().map(|ck| self.boxes[*ck].computed_size[axis]).sum()
            } else {
                children.iter().map(|ck| self.boxes[*ck].computed_size[axis]).fold(0.0_f32, f32::max)
            };

            self.boxes[key].computed_size[axis] = v;
        }
    }

    fn resolve_overflow(&mut self, key: BoxRef) {
        let children = self.boxes[key].children.clone();

        let axis = self.boxes[key].child_axis as usize;
        let parent_size = self.boxes[key].computed_size[axis];
        let total: f32 = children.iter().map(|ck| self.boxes[*ck].computed_size[axis]).sum();
        let overflow = (total - parent_size).max(0.0);

        if overflow > 0.0 {
            let total_give: f32 = children.iter().map(|ck| {
                let b = &self.boxes[*ck];
                b.computed_size[axis] * (1.0 - b.pref_size[axis].strictness)
            }).sum();

            if total_give > 0.0 {
                for c in &children {
                    let b = &self.boxes[*c];
                    let size = b.computed_size[axis];
                    let give = size * (1.0 - b.pref_size[axis].strictness);
                    let shrink = overflow * (give / total_give);
                    self.boxes[*c].computed_size[axis] = (size - shrink).max(0.0);
                }
            }
        }

        for c in children {
            self.resolve_overflow(c);
        }
    }

    fn pass3_place(&mut self, key: BoxRef, origin: [f32; 2]) {
        let children = self.boxes[key].children.clone();

        let axis  = self.boxes[key].child_axis as usize;
        let cross = 1 - axis;
        let size  = self.boxes[key].computed_size;

        self.boxes[key].rect = [
            origin[0], origin[1],
            origin[0] + size[0], origin[1] + size[1],
        ];

        let mut cursor = origin[axis];
        for c in children {
            let child_size = self.boxes[c].computed_size;
            let mut child_origin = origin;
            child_origin[axis]  = cursor;
            child_origin[cross] = origin[cross];
            self.pass3_place(c, child_origin);
            cursor += child_size[axis];
        }
    }

    pub fn push_size(&mut self, x: Size, y: Size) { self.stacks.size_x.push(x); self.stacks.size_y.push(y); }
    pub fn pop_size(&mut self)                    { self.stacks.size_x.pop(); self.stacks.size_y.pop(); }
    pub fn push_bg(&mut self, c: Color)           { self.stacks.bg_color.push(c); }
    pub fn pop_bg(&mut self)                      { self.stacks.bg_color.pop(); }
    pub fn push_axis(&mut self, a: Axis)          { self.stacks.child_axis.push(a); }
    pub fn pop_axis(&mut self)                    { self.stacks.child_axis.pop(); }
}

//
// Render pass
//
// Walks the finished laid-out box tree and emits gpu draw calls.
//

#[inline]
pub fn render(ui: &UiState, gpu: &mut Gpu) {
    if let Some(root) = ui.root {
        render_box(ui, gpu, root);
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [lerp(a[0],b[0],t), lerp(a[1],b[1],t), lerp(a[2],b[2],t), lerp(a[3],b[3],t)]
}

fn render_box(ui: &UiState, gpu: &mut Gpu, key: BoxRef) {
    let b = &ui.boxes[key];
    let [x0, y0, x1, y1] = b.rect;
    let (x, y, w, h) = (x0, y0, x1 - x0, y1 - y0);

    let persist  = ui.persist.get(&b.key);
    let hot_t    = persist.map(|p| p.hot_t).unwrap_or(0.0);
    let active_t = persist.map(|p| p.active_t).unwrap_or(0.0);

    if b.flags.contains(BoxFlags::DRAW_BG) {
        let c = lerp_color(b.bg_color.into(), b.hover_color.into(), hot_t);
        let flash = active_t * 0.15;
        let c = [
            (c[0] + flash).min(1.0),
            (c[1] + flash).min(1.0),
            (c[2] + flash).min(1.0),
            (c[3])
        ].into();
        crate::gpu::draw_rect(gpu, x, y, w, h, c);
    }

    //
    // 1px border on bottom edge only
    //
    if b.flags.contains(BoxFlags::DRAW_BORDER) {
        // bottom border
        crate::gpu::draw_rect(gpu, x, y + h - 1.0, w, 1.0, b.border_color);
        // left border
        crate::gpu::draw_rect(gpu, x, y, 1.0, h, b.border_color);
    }

    if b.flags.contains(BoxFlags::DRAW_TEXT) {
        let pad = b.padding;
        let text_y = y + h * 0.5 + gpu.font_scale * 5.0;

        if !b.text.is_empty() {
            crate::gpu::draw_text(gpu, &b.text, x + pad, text_y, b.text_color);
        }

        if let BoxCustom::TextInput(TextInputInfo {
            cursor_pixel_offset,
            cursor_target_pixel_offset,
            cursor_idle_secs,
            cursor_char
        }) = b.custom {
            // Use smoothed position from persist, fall back to raw offset
            let smooth_x = persist
                .map(|p| p.cursor_visual_x)
                .unwrap_or(cursor_pixel_offset);

            let cursor_w = 8.0  * gpu.font_scale;
            let cursor_h = 14.0 * gpu.font_scale;
            let cursor_x = x + pad + smooth_x;
            let cursor_y = y + (h - cursor_h) * 0.5;

            let alpha = if cursor_idle_secs < 0.5 {
                1.0
            } else if ((cursor_idle_secs - 0.5) % 1.0) < 0.5 {
                1.0
            } else {
                0.0
            };

            if alpha > 0.0 {
                crate::gpu::draw_rect(
                    gpu,
                    cursor_x,
                    cursor_y,
                    cursor_w,
                    cursor_h,
                    Color::rgba(150, 190, 220, 255)
                );

                if let Some(ch) = cursor_char {
                    crate::gpu::draw_text(
                        gpu,
                        &ch.to_string(),
                        cursor_x,
                        text_y,
                        Color::rgba(8, 8, 10, 255)
                    );
                }
            }
        }
    }

    for &child in &b.children {
        render_box(ui, gpu, child);
    }
}


pub struct BoxBuilder<'a> {
    ui:           &'a mut UiState,

    key:          u64,

    flags:        BoxFlags,
    size_x:       Option<Size>,
    size_y:       Option<Size>,
    bg:           Option<Color>,
    color:        Option<Color>,
    hover_color:  Option<Color>,
    border_color: Option<Color>,
    axis:         Option<Axis>,
    text:         Option<String>,
    padding:      Option<f32>,
}

impl<'a> BoxBuilder<'a> {
    #[inline]
    fn new(ui: &'a mut UiState, key: &str, flags: BoxFlags) -> Self {
        Self {
            ui,
            key: hash_str(key),
            flags,
            padding: None,
            size_x: None, size_y: None,
            bg: None, color: None, hover_color: None, border_color: None, axis: None,
            text: None,
        }
    }

    #[inline]
    pub fn size(mut self, x: Size, y: Size) -> Self {
        self.size_x = Some(x);
        self.size_y = Some(y);
        self
    }

    #[inline]
    pub fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }

    #[inline]
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    #[inline]
    pub fn border(mut self, c: Color) -> Self {
        self.border_color = Some(c);
        self
    }

    #[inline]
    pub fn hover_color(mut self, c: Color) -> Self {
        self.hover_color = Some(c);
        self
    }

    #[inline]
    pub fn padding(mut self, p: f32) -> Self {
        self.padding = Some(p);
        self
    }

    #[inline]
    pub fn text(mut self, t: impl Into<String>) -> Self {
        self.text = Some(t.into());
        self
    }

    // terminal, no children
    #[inline]
    pub fn build(self) -> BoxRef {
        self.build_inner(None::<fn(&mut UiState)>)
    }

    // container, has children via closure
    #[inline]
    pub fn build_children(self, f: impl FnOnce(&mut UiState)) -> BoxRef {
        self.build_inner(Some(f))
    }

    fn build_inner(self, f: Option<impl FnOnce(&mut UiState)>) -> BoxRef {
        let ui = self.ui;

        // Push stacks
        if let Some(x) = self.size_x { ui.stacks.size_x.push(x) }
        if let Some(y) = self.size_y { ui.stacks.size_y.push(y) }
        if let Some(c) = self.bg     { ui.stacks.bg_color.push(c) }
        if let Some(c) = self.color  { ui.stacks.text_color.push(c) }
        if let Some(a) = self.axis   { ui.stacks.child_axis.push(a) }
        if let Some(p) = self.padding { ui.stacks.padding.push(p); }

        let id = ui.push_box(self.key, self.flags);

        // hover_color set directly since it's not a stack
        if let Some(c) = self.hover_color { ui.boxes[id].hover_color = c }
        if let Some(c) = self.border_color {
            ui.boxes[id].border_color = c;
            ui.boxes[id].flags |= BoxFlags::DRAW_BORDER;
        }

        // Set text directly on the box after creation
        if let Some(t) = self.text {
            ui.boxes[id].text = t;
            ui.boxes[id].flags |= BoxFlags::DRAW_TEXT;
        }

        if let Some(f) = f {
            ui.push_parent(id);
            f(ui);
            ui.pop_parent();
        }

        // Pop stacks
        if self.size_x.is_some() { ui.stacks.size_x.pop(); }
        if self.size_y.is_some() { ui.stacks.size_y.pop(); }
        if self.bg.is_some()     { ui.stacks.bg_color.pop(); }
        if self.color.is_some()  { ui.stacks.text_color.pop(); }
        if self.axis.is_some()   { ui.stacks.child_axis.pop(); }
        if self.padding.is_some() { ui.stacks.padding.pop(); }

        id
    }
}

//
// UiState builder methods
//

impl UiState {
    /// Horizontal container
    #[inline]
    pub fn row<'a>(&'a mut self, key: &str) -> BoxBuilder<'a> {
        let mut b = BoxBuilder::new(self, key, BoxFlags::DRAW_BG);
        b.axis = Some(Axis::X);
        b
    }

    /// Vertical container
    #[inline]
    pub fn col<'a>(&'a mut self, key: &str) -> BoxBuilder<'a> {
        let mut b = BoxBuilder::new(self, key, BoxFlags::DRAW_BG);
        b.axis = Some(Axis::Y);
        b
    }

    /// Text label, no background
    #[inline]
    pub fn label<'a>(&'a mut self, key: &str) -> BoxBuilder<'a> {
        BoxBuilder::new(self, key, BoxFlags::DRAW_TEXT)
    }

    /// Scrollable vertical container
    #[inline]
    pub fn scroll<'a>(&'a mut self, key: &str) -> BoxBuilder<'a> {
        let mut b = BoxBuilder::new(
            self, key,
            BoxFlags::DRAW_BG | BoxFlags::SCROLL_CHILDREN | BoxFlags::CLIP_CHILDREN
        );
        b.axis = Some(Axis::Y);
        b
    }

    /// Clickable row, hoverable + clickable flags
    #[inline]
    pub fn button<'a>(&'a mut self, key: &str) -> BoxBuilder<'a> {
        BoxBuilder::new(
            self, key,
            BoxFlags::DRAW_BG | BoxFlags::HOVERABLE | BoxFlags::CLICKABLE
        )
    }

    /// Fixed-size empty spacer, for virtual list padding
    #[inline]
    pub fn spacer(&mut self, key: &str, axis: Axis, size: f32) {
        let sz = Size::px(size);
        let (sx, sy) = match axis {
            Axis::X => (sz, Size::px(0.0)),   // sy is 0, not fill
            Axis::Y => (Size::fill(), sz),
        };
        self.push_size(sx, sy);
        self.push_box(hash_str(key), BoxFlags::empty());
        self.pop_size();
    }

    // layout - runs all three passes from the root
    #[inline]
    pub fn layout(&mut self, mut measure_callback: impl FnMut(&str) -> [f32; 2]) {
        if let Some(root) = self.root {
            let win = [self.win_w, self.win_h];
            self.pass1_standalone(root, &mut measure_callback);
            self.pass2a_parent_pct(root, win);
            self.pass2b_children_sum(root);
            self.resolve_overflow(root);
            self.pass3_place(root, [0.0, 0.0]);
        }
    }

    #[inline]
    pub fn tick(&mut self) {
        self.tick_animations();
    }
}

pub fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher, DefaultHasher};
    let mut h = DefaultHasher::new();
    let key_part = s.find("##").map(|i| &s[..i]).unwrap_or(s);
    key_part.hash(&mut h);
    h.finish()
}
