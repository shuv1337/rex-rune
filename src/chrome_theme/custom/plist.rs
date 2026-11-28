//! The XML plist exported by iTerm2. Values outside the colour keys are skipped;
//! declarations are never fetched or expanded.
use crate::terminal_theme::TerminalTheme;

pub(super) fn parse(text: &str) -> Option<TerminalTheme> {
    let mut xml = Xml(text);
    match xml.next()? {
        Token::Open("plist") => {}
        _ => return None,
    }
    xml.expect(Token::Open("dict"))?;
    let mut term = TerminalTheme::default();
    while let Some(key) = xml.key()? {
        let slot = match key {
            "Background Color" => &mut term.background,
            "Foreground Color" => &mut term.foreground,
            "Cursor Color" => &mut term.cursor,
            "Cursor Text Color" => &mut term.cursor_text,
            "Selection Color" => &mut term.selection_background,
            "Selected Text Color" => &mut term.selection_foreground,
            _ => {
                if let Some(index) = key
                    .strip_prefix("Ansi ")
                    .and_then(|key| key.strip_suffix(" Color"))
                    .and_then(|i| i.parse::<u8>().ok())
                    .filter(|&i| i < 16)
                {
                    let color = xml.rgb()?;
                    term.palette.retain(|&(i, _)| i != index);
                    term.palette.push((index, color));
                } else {
                    xml.skip_value()?;
                }
                continue;
            }
        };
        *slot = Some(xml.rgb()?);
    }
    xml.expect(Token::Close("plist"))?;
    if xml.skip_trivia()?.is_empty() && term != TerminalTheme::default() {
        term.palette.sort_unstable_by_key(|&(i, _)| i);
        Some(term)
    } else {
        None
    }
}

struct Xml<'a>(&'a str);

#[derive(Copy, Clone, PartialEq, Eq)]
enum Token<'a> {
    Open(&'a str),
    Close(&'a str),
    Empty(&'a str),
    Text(&'a str),
}

impl<'a> Xml<'a> {
    fn skip_trivia(&mut self) -> Option<&'a str> {
        loop {
            self.0 = self.0.trim_start();
            if let Some(comment) = self.0.strip_prefix("<!--") {
                self.0 = comment.split_once("-->")?.1;
            } else {
                return Some(self.0);
            }
        }
    }

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let input = self.skip_trivia()?;
            if let Some(decl) = input.strip_prefix("<?xml ") {
                self.0 = decl.split_once("?>")?.1;
            } else if let Some(decl) = input.strip_prefix("<!DOCTYPE plist") {
                let (doctype, rest) = decl.split_once('>')?;
                if doctype.contains(['[', '<']) {
                    return None;
                }
                self.0 = rest;
            } else if let Some(tag) = input.strip_prefix('<') {
                let (tag, rest) = tag.split_once('>')?;
                self.0 = rest;
                if tag.contains('<') {
                    return None;
                }
                let is_name = |name: &str| {
                    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
                };
                return if let Some(name) = tag.strip_prefix('/') {
                    is_name(name).then_some(Token::Close(name))
                } else if let Some(name) = tag.strip_suffix('/') {
                    let name = name.trim_end();
                    is_name(name).then_some(Token::Empty(name))
                } else if tag.starts_with("plist ") {
                    Some(Token::Open("plist"))
                } else {
                    is_name(tag).then_some(Token::Open(tag))
                };
            } else {
                let (text, rest) = input.split_at(input.find('<')?);
                self.0 = rest;
                return Some(Token::Text(text.trim_end()));
            }
        }
    }

    fn expect(&mut self, token: Token<'_>) -> Option<()> {
        (self.next()? == token).then_some(())
    }

    // None inside Some means end of dictionary, not malformed input.
    fn key(&mut self) -> Option<Option<&'a str>> {
        match self.next()? {
            Token::Open("key") => Some(Some(self.text("key")?)),
            Token::Close("dict") => Some(None),
            _ => None,
        }
    }

    fn text(&mut self, tag: &str) -> Option<&'a str> {
        let Token::Text(text) = self.next()? else {
            return None;
        };
        self.expect(Token::Close(tag))?;
        Some(text)
    }

    fn rgb(&mut self) -> Option<u32> {
        self.expect(Token::Open("dict"))?;
        let mut components = [None; 3];
        while let Some(key) = self.key()? {
            let index = match key {
                "Red Component" => 0,
                "Green Component" => 1,
                "Blue Component" => 2,
                _ => {
                    self.skip_value()?;
                    continue;
                }
            };
            let Token::Open(tag @ ("real" | "integer")) = self.next()? else {
                return None;
            };
            let component: f64 = self.text(tag)?.parse().ok()?;
            if !component.is_finite() {
                return None;
            }
            components[index] = Some((component.clamp(0., 1.) * 255.).round() as u32);
        }
        let [Some(r), Some(g), Some(b)] = components else {
            return None;
        };
        Some(r << 16 | g << 8 | b)
    }

    fn skip_value(&mut self) -> Option<()> {
        let mut open = Vec::new();
        match self.next()? {
            Token::Empty(_) => return Some(()),
            Token::Open(tag) => open.push(tag),
            _ => return None,
        }
        while !open.is_empty() {
            match self.next()? {
                Token::Open(tag) if open.len() < 32 => open.push(tag),
                Token::Close(tag) if open.pop()? == tag => {}
                Token::Text(_) | Token::Empty(_) => {}
                _ => return None,
            }
        }
        Some(())
    }
}
