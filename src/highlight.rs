#[derive(Clone, Copy, PartialEq)]
pub enum TokenKind {
    Normal,
    Keyword,
    String,
    Comment,
    Number,
    Note,
    Type,   // UppercaseIdent
    Macro,  // ident!
}

pub struct Token {
    pub start: u32,
    pub end:   u32,
    pub kind:  TokenKind,
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl",
    "trait", "where", "if", "else", "match", "for", "while", "loop",
    "return", "break", "continue", "in", "as", "ref", "type", "const",
    "static", "unsafe", "async", "await", "move", "dyn", "box", "self",
    "Self", "super", "crate", "true", "false", "Some", "None", "Ok", "Err",
];

//
// Just some very basic tokenization for `BoxCustom::Match` highlighting.
//

pub fn tokenize(src: &str) -> Vec<Token> {
    #[inline]
    const fn tk(s: usize, e: usize, k: TokenKind) -> Token {
        Token { start: s as _, end: e as _, kind: k }
    }

    let bytes = src.as_bytes();
    let len   = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < len {
        // Line comment - scan for @Notes inside
        if bytes[i] == b'/' && i + 1 < len && bytes[i+1] == b'/' {
            let mut j = i;
            let mut seg_start = i;
            while j < len {
                if bytes[j] == b'@' && j + 1 < len && bytes[j + 1].is_ascii_uppercase() {
                    if j > seg_start { tokens.push(tk(seg_start, j, TokenKind::Comment)); }

                    let note_start = j;
                    j += 1;
                    while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                        j += 1;
                    }

                    tokens.push(tk(note_start, j, TokenKind::Note));
                    seg_start = j;
                }

                j += 1;
            }

            if seg_start < len { tokens.push(tk(seg_start, len, TokenKind::Comment)); }
            break;
        }

        // @Note annotation
        if bytes[i] == b'@' {
            if i + 1 < len && bytes[i+1].is_ascii_uppercase() {
                let start = i;
                i += 1;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                tokens.push(tk(start, i, TokenKind::Note));
                continue;
            }

            i += 1;
            continue;
        }

        // String literal
        if bytes[i] == b'"' {
            let start = i;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' { i += 2; continue; }
                if bytes[i] == b'"'  { i += 1; break; }
                i += 1;
            }

            tokens.push(tk(start, i, TokenKind::String));
            continue;
        }

        // Char literal or Lifetime
        if bytes[i] == b'\'' {
            let start = i;
            i += 1;
            let mut j = i;
            while j < len {
                if bytes[j] == b'\\' { j += 2; continue; }
                if bytes[j] == b'\'' {
                    i = j + 1;
                    tokens.push(tk(start, i, TokenKind::String));
                    break;
                }
                if bytes[j].is_ascii_whitespace() || bytes[j] == b'>' || bytes[j] == b',' {
                    i = start + 1;
                    break;
                }
                j += 1;
            }

            if j >= len { i = start + 1; }
            continue;
        }

        // Number
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.') {
                i += 1;
            }

            tokens.push(tk(start, i, TokenKind::Number));
            continue;
        }

        // Identifier, Keyword, Type, or Macro
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];

            // Check for macro call: ident!
            if i < len && bytes[i] == b'!' {
                i += 1;
                tokens.push(tk(start, i, TokenKind::Macro));
                continue;
            }

            let kind = if KEYWORDS.contains(&word) {
                TokenKind::Keyword
            } else if bytes[start].is_ascii_uppercase() {
                TokenKind::Type
            } else {
                TokenKind::Normal
            };

            tokens.push(tk(start, i, kind));
            continue;
        }

        i += 1;
    }

    tokens
}
