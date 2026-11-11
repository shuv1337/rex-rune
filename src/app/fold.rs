//! Searching names without their case or accents: `rose` finds `Rosé Pine`, and the part of
//! the name that matched can be marked.
use std::ops::Range;

/// The plain letters a Latin letter with a mark, or a ligature, is searched as. `c` is in
/// lower case.
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ð' | 'ď' | 'đ' => "d",
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĳ' => "ij",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// A mark written as its own character after the letter it sits on (`e` then U+0301).
fn combining(c: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&c)
}

/// `text` as it is searched, and for each byte of that the bytes of `text` it came from.
fn folded(text: &str) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut from = Vec::with_capacity(text.len());
    for (start, c) in text.char_indices() {
        if combining(c) {
            continue;
        }
        let source = start..start + c.len_utf8();
        for lower in c.to_lowercase() {
            let before = out.len();
            match plain(lower) {
                Some(letters) => out.push_str(letters),
                None => out.push(lower),
            }
            from.extend(std::iter::repeat_n(source.clone(), out.len() - before));
        }
    }
    (out, from)
}

/// `text` without case or accents: `Rosé Pine` is `rose pine`.
pub fn fold(text: &str) -> String {
    folded(text).0
}

/// A name as it is searched, kept beside the name so its matches can be marked in it.
pub struct Name<'a> {
    text: &'a str,
    folded: String,
    from: Vec<Range<usize>>,
}

impl<'a> Name<'a> {
    pub fn new(text: &'a str) -> Self {
        let (folded, from) = folded(text);
        Self { text, folded, from }
    }

    /// The bytes of the name that hold bytes `found` of its folded form, whole characters.
    fn source(&self, found: Range<usize>) -> Range<usize> {
        let start = self.from[found.start].start;
        let end = self.from[found.end - 1].end;
        // A mark after the last letter is drawn with it.
        let marks: usize = self.text[end..]
            .chars()
            .take_while(|c| combining(*c))
            .map(char::len_utf8)
            .sum();
        start..end + marks
    }

    /// Where each word of `query` (already folded) is in the name, as byte ranges of the name
    /// in order and not overlapping; none unless every word is there. With `word_starts` a
    /// word counts only where a word of the name begins. An empty query matches, marking
    /// nothing.
    pub fn find(&self, query: &str, word_starts: bool) -> Option<Vec<Range<usize>>> {
        let mut marks: Vec<Range<usize>> = Vec::new();
        for word in query.split_whitespace() {
            let found = self
                .folded
                .match_indices(word)
                .map(|(at, _)| at)
                .find(|at| {
                    !word_starts
                        || self.folded[..*at]
                            .chars()
                            .next_back()
                            .is_none_or(|before| !before.is_alphanumeric())
                })?;
            marks.push(self.source(found..found + word.len()));
        }
        marks.sort_by_key(|mark| (mark.start, mark.end));
        // Words that matched the same letters ("ro rose") mark them once.
        let mut merged: Vec<Range<usize>> = Vec::new();
        for mark in marks {
            match merged.last_mut() {
                Some(last) if mark.start <= last.end => last.end = last.end.max(mark.end),
                _ => merged.push(mark),
            }
        }
        Some(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(text: &str, query: &str) -> Option<Vec<String>> {
        let marks = Name::new(text).find(&fold(query), false)?;
        Some(marks.into_iter().map(|m| text[m].to_owned()).collect())
    }

    #[test]
    fn case_and_accents_are_folded_both_ways() {
        assert_eq!(fold("Rosé Pine Moon"), "rose pine moon");
        assert_eq!(fold("ÅNGSTRÖM"), "angstrom");
        assert_eq!(fold("Straße Æsir Œuvre"), "strasse aesir oeuvre");
        // A decomposed accent is the letter alone.
        assert_eq!(fold("Rose\u{301} Pine"), "rose pine");
        assert_eq!(marked("Rosé Pine", "rose"), Some(vec!["Rosé".into()]));
        assert_eq!(marked("Rose Pine", "rosé"), Some(vec!["Rose".into()]));
        assert_eq!(marked("Gruvbox Dark", "BOX"), Some(vec!["box".into()]));
        assert_eq!(marked("Gruvbox Dark", "boxy"), None);
    }

    #[test]
    fn a_mark_covers_whole_characters_of_the_name() {
        // Half of what a ligature folds to still marks the ligature.
        assert_eq!(marked("Straße", "stras"), Some(vec!["Straß".into()]));
        assert_eq!(marked("Æsir", "a"), Some(vec!["Æ".into()]));
        // A combining mark stays with its letter, in the middle and at the end.
        assert_eq!(
            marked("Rose\u{301} Pine", "rose"),
            Some(vec!["Rose\u{301}".into()])
        );
        assert_eq!(
            marked("Rose\u{301}s", "ose"),
            Some(vec!["ose\u{301}".into()])
        );
    }

    #[test]
    fn every_word_must_be_there_and_each_is_marked_once() {
        assert_eq!(
            marked("Rosé Pine Moon", "moon rose"),
            Some(vec!["Rosé".into(), "Moon".into()])
        );
        assert_eq!(marked("Rosé Pine Moon", "rose dawn"), None);
        assert_eq!(marked("Rosé Pine", "ro rose"), Some(vec!["Rosé".into()]));
        assert_eq!(marked("Rosé Pine", ""), Some(vec![]));
        assert_eq!(marked("Rosé Pine", "   "), Some(vec![]));
    }

    #[test]
    fn word_starts_leave_out_the_middle_of_a_word() {
        let find = |text: &str, query: &str| Name::new(text).find(query, true).is_some();
        assert!(find("Rosé Pine", "pi"));
        assert!(find("Rosé Pine", "ro pi"));
        assert!(!find("Rosé Pine", "ine"));
        assert!(find("Gruvbox-Dark", "dark"), "after a hyphen");
        assert!(find("Catppuccin Mocha", "mo"));
        assert!(!find("Catppuccin Mocha", "cha"));
        // The first place a word of the name starts with it, not the first place at all.
        let marks = Name::new("Aurora Rose").find("ro", true).unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!((marks[0].start, marks[0].end), (7, 9));
    }
}
