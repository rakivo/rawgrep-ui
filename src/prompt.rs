use smallstr::SmallString;

pub struct PromptState {
    buffer: SmallString<[u8; 256]>,
    cursor: u32
}

impl Default for PromptState {
    fn default() -> Self {
        Self {
            buffer: SmallString::new(),
            cursor: 0
        }
    }
}

impl PromptState {
    #[inline]
    pub fn push_str(&mut self, str: &str) {
        self.buffer.insert_str(self.cursor as _, str);
        self.cursor += str.len() as u32;
    }

    #[inline]
    pub fn push_char(&mut self, char: char) {
        self.buffer.insert(self.cursor as _, char);
        self.cursor += char.len_utf8() as u32;
    }

    #[inline]
    pub fn pop_char(&mut self) -> Option<char> {
        if self.cursor == 0 { return None }

        let prev = self.prev_char_boundary();
        let c = self.buffer.remove(prev);
        self.cursor = prev as u32;

        Some(c)
    }

    #[inline]
    pub fn iterate_chars_until_cursor(&self) -> impl Iterator<Item = char> {
        self.buffer.char_indices().filter_map(|(index, char)| {
            if index < self.cursor as usize {
                Some(char)
            } else {
                None
            }
        })
    }

    #[inline]
    pub fn buffer(&self) -> &str {
        self.buffer.as_str()
    }

    #[inline]
    pub fn cursor(&self) -> u32 {
        self.cursor
    }

    #[inline]
    pub fn char_at_cursor(&self) -> Option<char> {
        self.buffer[self.cursor as usize..].chars().next()
    }

    #[inline]
    pub fn prev_char_boundary(&self) -> usize {
        let mut i = self.cursor as usize;
        if i == 0 { return i }
        loop { i -= 1; if self.buffer.is_char_boundary(i) { return i } }
    }

    #[inline]
    pub fn next_char_boundary(&self) -> usize {
        let mut i = self.cursor as usize + 1;
        while !self.buffer.is_char_boundary(i) { i += 1; }
        i.min(self.buffer.len())
    }

    #[inline]
    pub fn move_cursor_left_by(&mut self, by: u32) {
        for _ in 0..by {
            if self.cursor == 0 { break }
            self.cursor = self.prev_char_boundary() as u32;
        }
    }

    #[inline]
    pub fn move_cursor_left(&mut self) {
        self.move_cursor_left_by(1)
    }

    #[inline]
    pub fn move_cursor_right_by(&mut self, by: u32) {
        for _ in 0..by {
            if self.cursor as usize >= self.buffer.len() { break; }
            self.cursor = self.next_char_boundary() as u32;
        }
    }

    #[inline]
    pub fn move_cursor_right(&mut self) {
        self.move_cursor_right_by(1)
    }

    #[inline]
    pub fn move_cursor_start(&mut self) {
        self.cursor = 0
    }

    #[inline]
    pub fn move_cursor_end(&mut self) {
        self.cursor = self.buffer.len() as u32
    }

    #[inline]
    pub fn move_word_forward(&mut self) {
        let s = &self.buffer[self.cursor as usize..];
        let mut n = 0usize;
        let mut chars = s.chars();
        // skip non-word chars (whitespace, punctuation)
        for c in &mut chars {
            if c.is_alphanumeric() { n += c.len_utf8(); break; }
            n += c.len_utf8();
        }
        // skip word chars
        for c in &mut chars {
            if !c.is_alphanumeric() { break; }
            n += c.len_utf8();
        }
        self.cursor = (self.cursor as usize + n).min(self.buffer.len()) as u32;
    }

    #[inline]
    pub fn move_word_back(&mut self) {
        let s = &self.buffer[..self.cursor as usize];
        let mut n = 0usize;
        let mut chars = s.chars().rev();
        // skip non-word chars
        for c in &mut chars {
            if c.is_alphanumeric() { n += c.len_utf8(); break; }
            n += c.len_utf8();
        }
        // skip word chars
        for c in &mut chars {
            if !c.is_alphanumeric() { break; }
            n += c.len_utf8();
        }
        self.cursor = self.cursor.saturating_sub(n as u32);
    }

    #[inline]
    pub fn delete_forward(&mut self) -> Option<char> {
        if self.cursor as usize >= self.buffer.len() { return None }

        let c = self.buffer.remove(self.cursor as usize);
        Some(c)
    }

    #[inline]
    pub fn kill_line(&mut self) {
        self.buffer.truncate(self.cursor as usize);
    }

    #[inline]
    pub fn kill_word_back(&mut self) {
        let old = self.cursor;
        self.move_word_back();
        unsafe { self.buffer.as_mut_vec() }.drain(self.cursor as usize..old as usize);
    }

    #[inline]
    pub fn kill_word_forward(&mut self) {
        let old = self.cursor;
        self.move_word_forward();
        unsafe { self.buffer.as_mut_vec() }.drain(old as usize..self.cursor as usize);
        self.cursor = old;
    }
}
