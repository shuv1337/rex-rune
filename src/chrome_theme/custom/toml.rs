//! Quoted strings, one `[terminal]` section, string arrays and dotted palette keys.
use super::{Map, Value};

pub(super) fn parse(mut text: &str) -> Option<Value> {
    let mut chrome = Map::new();
    let mut terminal = Map::new();
    let mut palette = Map::new();
    let mut in_terminal = false;
    loop {
        trivia(&mut text);
        if text.is_empty() {
            break;
        }
        if let Some(rest) = text.strip_prefix("[terminal]").filter(|_| !in_terminal) {
            in_terminal = true;
            text = rest;
        } else {
            let end = text.find(|c: char| !(c.is_ascii_alphanumeric() || "_.-".contains(c)))?;
            let key = &text[..end];
            text = horizontal(&text[end..]).strip_prefix('=')?;
            text = horizontal(text);
            let value = if key == "ansi" && in_terminal {
                array(&mut text)?
            } else {
                Value::String(quoted(&mut text)?)
            };
            let (table, key) = if !in_terminal {
                (&mut chrome, key)
            } else if let Some(index) = key.strip_prefix("palette.") {
                index.parse::<u8>().ok()?;
                (&mut palette, index)
            } else {
                (&mut terminal, key)
            };
            if key.is_empty() || table.insert(key.to_owned(), value).is_some() {
                return None;
            }
        }
        text = horizontal(text);
        if !text.is_empty() && !text.starts_with(['\r', '\n', '#']) {
            return None;
        }
    }
    if !palette.is_empty()
        && terminal
            .insert("palette".into(), Value::Object(palette))
            .is_some()
    {
        return None;
    }
    if in_terminal
        && chrome
            .insert("terminal".into(), Value::Object(terminal))
            .is_some()
    {
        return None;
    }
    Some(Value::Object(chrome))
}

fn horizontal(text: &str) -> &str {
    text.trim_start_matches([' ', '\t'])
}

fn trivia(text: &mut &str) {
    loop {
        *text = text.trim_start();
        if !text.starts_with('#') {
            return;
        }
        *text = text.split_once('\n').map_or("", |(_, rest)| rest);
    }
}

fn array(text: &mut &str) -> Option<Value> {
    *text = text.strip_prefix('[')?;
    let mut values = Vec::new();
    loop {
        trivia(text);
        if let Some(rest) = text.strip_prefix(']') {
            *text = rest;
            return Some(Value::Array(values));
        }
        values.push(Value::String(quoted(text)?));
        if values.len() > 256 {
            return None;
        }
        trivia(text);
        if !text.starts_with(']') {
            *text = text.strip_prefix(',')?;
        }
    }
}

fn quoted(text: &mut &str) -> Option<String> {
    let quote = *text.as_bytes().first()?;
    if !matches!(quote, b'"' | b'\'') {
        return None;
    }
    let mut escaped = false;
    for (i, byte) in text.bytes().enumerate().skip(1) {
        if matches!(byte, b'\r' | b'\n') {
            return None;
        }
        if !escaped && byte == quote {
            let value = if quote == b'"' {
                serde_json::from_str(&text[..=i]).ok()?
            } else {
                text[1..i].to_owned()
            };
            *text = &text[i + 1..];
            return Some(value);
        }
        escaped = !escaped && quote == b'"' && byte == b'\\';
    }
    None
}
