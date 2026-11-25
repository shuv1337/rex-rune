//! Links in a rendered frame: OSC 8 hyperlinks, and URLs written out as plain text.
use crate::terminal::{Cell, Frame};
use std::ops::Range;

/// Plain-text URL prefixes, lowercase.
const PREFIXES: [&str; 4] = ["https://", "http://", "ftp://", "mailto:"];
/// Schemes a link may open. Others (`file:` among them) would name something on this
/// machine while the output may come from a remote host, or run an arbitrary handler.
const SCHEMES: [&str; 4] = ["https", "http", "ftp", "mailto"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub uri: String,
    /// First and last cell it covers, as `row * cols + col` frame indices.
    pub start: usize,
    pub end: usize,
}

impl Frame {
    /// The link drawn over cell `index` (`row * cols + col`), if it may be opened. An OSC 8
    /// hyperlink wins over text; a URL in text may continue across soft-wrapped rows.
    pub fn link_at(&self, index: usize) -> Option<Link> {
        let cols = self.cols as usize;
        let row = index.checked_div(cols)?;
        self.cells.get(row)?.get(index % cols)?;
        let wrapped = |r: usize| self.wrapped.get(r).copied().unwrap_or(false);
        let mut first = row;
        while first > 0 && wrapped(first - 1) {
            first -= 1;
        }
        let mut last = row;
        while last + 1 < self.cells.len() && wrapped(last) {
            last += 1;
        }
        // The logical line, one entry per character: the second half of a wide one is skipped.
        let cells: Vec<(usize, &Cell)> = (first..=last)
            .flat_map(|r| {
                self.cells[r]
                    .iter()
                    .enumerate()
                    .map(move |(c, cell)| (r * cols + c, cell))
            })
            .filter(|(_, cell)| !cell.spacer_tail)
            .collect();
        let at = cells.iter().rposition(|&(i, _)| i <= index)?;
        // The last index a character covers, its wide tail included.
        let end_of = |k: usize| cells.get(k + 1).map_or(last * cols + cols - 1, |c| c.0 - 1);

        if let Some(target) = &cells[at].1.link {
            let uri = openable(target)?;
            let same = |(_, c): &(usize, &Cell)| c.link.as_ref() == Some(target);
            let start = cells[..at]
                .iter()
                .rposition(|c| !same(c))
                .map_or(0, |k| k + 1);
            let end = cells[at..]
                .iter()
                .position(|c| !same(c))
                .map_or(cells.len(), |k| at + k);
            return Some(Link {
                uri,
                start: cells[start].0,
                end: end_of(end - 1),
            });
        }

        let mut text = String::new();
        let mut offsets = Vec::with_capacity(cells.len());
        for (_, cell) in &cells {
            offsets.push(text.len());
            text.push_str(if cell.text.is_empty() {
                " "
            } else {
                &cell.text
            });
        }
        let byte = offsets[at];
        let found = find_urls(&text).into_iter().find(|r| r.contains(&byte))?;
        let start = offsets.partition_point(|&o| o < found.start);
        let end = offsets.partition_point(|&o| o < found.end);
        Some(Link {
            uri: openable(&text[found])?,
            start: cells[start].0,
            end: end_of(end - 1),
        })
    }
}

/// `uri` normalised, if it parses and its scheme may be opened.
fn openable(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    SCHEMES
        .contains(&url.scheme())
        .then(|| url.as_str().to_owned())
}

/// Byte ranges of the URLs written in `text`.
pub(crate) fn find_urls(text: &str) -> Vec<Range<usize>> {
    // ASCII lowercasing keeps every byte offset.
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some((start, prefix)) = PREFIXES
        .iter()
        .filter_map(|p| lower[from..].find(p).map(|i| (from + i, *p)))
        .min_by_key(|&(i, _)| i)
    {
        let body = start + prefix.len();
        let end = trim(
            text,
            start,
            text[body..].find(stops).map_or(text.len(), |i| body + i),
        );
        let word_start = !text[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        if word_start && end > body {
            found.push(start..end);
        }
        from = end.max(body);
    }
    found
}

/// Characters that end a URL in text: spacing, quotes and angle brackets that commonly
/// enclose one, and box drawing that frames one in a TUI.
fn stops(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '<' | '>' | '"' | '\'' | '`')
        || ('\u{2500}'..='\u{259f}').contains(&c)
}

/// The end of `text[start..end]` without sentence punctuation or an unmatched closing
/// bracket after it, so `(see https://a.example/x_(y)).` keeps only the URL.
fn trim(text: &str, start: usize, mut end: usize) -> usize {
    while let Some(c) = text[start..end].chars().next_back() {
        let open = match c {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            '.' | ',' | ';' | ':' | '!' | '?' => {
                end -= 1;
                continue;
            }
            _ => break,
        };
        let url = &text[start..end];
        if url.matches(c).count() <= url.matches(open).count() {
            break;
        }
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Renderer;
    use ghostty_vt::{Terminal, TerminalOptions};

    fn frame(cols: u16, rows: u16, output: &str) -> Frame {
        let mut terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 0,
        })
        .unwrap();
        terminal.vt_write(output.as_bytes());
        Renderer::new().unwrap().frame(&terminal).unwrap()
    }

    fn uri_at(frame: &Frame, row: usize, col: usize) -> Option<String> {
        frame
            .link_at(row * frame.cols as usize + col)
            .map(|l| l.uri)
    }

    #[test]
    fn text_urls_end_before_trailing_punctuation_and_unmatched_brackets() {
        let f = frame(
            80,
            3,
            "see (https://en.example.org/wiki/A_(b)). Then\r\n\
             [doc](http://a.example/x?y=1&z=2), \"ftp://f.example/p\"\r\n\
             xhttps://no.example mailto:me@example.org!",
        );
        let wiki = Some("https://en.example.org/wiki/A_(b)".to_owned());
        assert_eq!(uri_at(&f, 0, 5), wiki);
        assert_eq!(uri_at(&f, 0, 37), wiki, "last character of the URL");
        assert_eq!(uri_at(&f, 0, 38), None, "the closing bracket after it");
        assert_eq!(uri_at(&f, 0, 3), None);
        assert_eq!(uri_at(&f, 1, 6), Some("http://a.example/x?y=1&z=2".into()));
        assert_eq!(uri_at(&f, 1, 33), None, "the comma after it");
        assert_eq!(uri_at(&f, 1, 37), Some("ftp://f.example/p".into()));
        assert_eq!(uri_at(&f, 2, 3), None, "a scheme inside a word");
        assert_eq!(uri_at(&f, 2, 25), Some("mailto:me@example.org".into()));
    }

    #[test]
    fn a_text_url_continues_across_soft_wrapped_rows_only() {
        let f = frame(10, 6, "go https://a.example/long\r\nhttps://b.example/");
        let link = f.link_at(10 + 4).expect("link on the wrapped row");
        assert_eq!(link.uri, "https://a.example/long");
        assert_eq!((link.start, link.end), (3, 2 * 10 + 4));
        let link = f.link_at(3 * 10).expect("the next line's own URL");
        assert_eq!(link.uri, "https://b.example/");
        assert_eq!((link.start, link.end), (3 * 10, 4 * 10 + 7));
    }

    #[test]
    fn hyperlinks_cover_their_cells_and_only_open_web_schemes() {
        let f = frame(
            40,
            2,
            "a \x1b]8;;https://h.example/x\x1b\\here\x1b]8;;\x1b\\ b\r\n\
             \x1b]8;;file:///etc/passwd\x1b\\file\x1b]8;;\x1b\\",
        );
        let link = f.link_at(3).expect("hyperlink");
        assert_eq!(link.uri, "https://h.example/x");
        assert_eq!((link.start, link.end), (2, 5));
        assert_eq!(f.link_at(6), None);
        assert_eq!(uri_at(&f, 1, 1), None);
    }
}
