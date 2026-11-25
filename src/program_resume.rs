//! Metadata from Rex's Program Resume read and change event. Never retains launch arguments.
use crate::connection::activity::time;
use icu_properties::{
    CodePointMapData, CodePointSetData,
    props::{DefaultIgnorableCodePoint, GeneralCategory},
};
use serde::{Deserialize, de::SeqAccess};
use serde_json::value::RawValue;
use std::{collections::VecDeque, fmt, time::SystemTime};

pub const MIN_API_REVISION: u32 = 24;
pub const MAX_RECORDS_PER_BLOCK: usize = 16;
pub const MAX_RECORDS_PER_HOST: usize = 1024;
const MAX_NAME: usize = 128;
const MAX_CWD: usize = 1024;
const MAX_KEY_OWNER_NAME: usize = 256;
const MAX_KEY_REPORTED_AT: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResumeSnapshot {
    pub records: Vec<ResumeSummary>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeSummary {
    pub key: RecordKey,
    pub program: String,
    pub owner_name: String,
    pub cwd: Option<String>,
    pub reported_at: Option<SystemTime>,
    pub chosen: bool,
    pub trust: Trust,
}

/// Opaque row identity. Raw name and timestamp distinguish wire records, not display labels.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RecordKey {
    pub pid: u64,
    owner_name: String,
    reported_at: String,
}

impl fmt::Debug for RecordKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordKey")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted { rule: String },
    Untrusted(Reason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Wrapped,
    NoRule,
    ProgramDiffers,
    FlagNotListed,
    FlagValue,
    FlagsDiffer,
    DirNotAllowed,
    LaunchUnreadable,
    ConfigIncomplete,
    Unstated,
    #[serde(other)]
    Unrecognised,
}

impl Reason {
    pub fn explain(self) -> &'static str {
        match self {
            Self::Wrapped => {
                "Runs inside another program, such as a sandbox, that the command alone would leave out."
            }
            Self::NoRule => "No trust rule names this command.",
            Self::ProgramDiffers => "The running program is not the one this command starts.",
            Self::FlagNotListed => "Started with a flag its trust rule does not list.",
            Self::FlagValue => "A flag's value does not fit its trust rule.",
            Self::FlagsDiffer => {
                "The command's flags differ from the ones the program was started with."
            }
            Self::DirNotAllowed => "Would start outside the folders its trust rule allows.",
            Self::LaunchUnreadable => "Rex could not read how the program was started.",
            Self::ConfigIncomplete => {
                "The host's init.lua did not finish loading, so nothing is trusted."
            }
            Self::Unrecognised => "Not trusted. This version of Rex Rune does not know the reason.",
            Self::Unstated => "Not trusted.",
        }
    }
}

// The same metadata schema handles events and detached replies. Replies never go
// through a Value or a flatten/untagged map that could retain launch arguments.
#[derive(Deserialize)]
pub(crate) struct WireSnapshot {
    #[serde(deserialize_with = "read_records")]
    records: Records,
    #[serde(default)]
    truncated: bool,
}

struct Records {
    items: Vec<WireRecord>,
    capped: bool,
}

#[derive(Deserialize)]
struct WireRecord {
    owner: WireOwner,
    program: String,
    reported_at: String,
    cwd: Option<String>,
    #[serde(default)]
    chosen: bool,
    #[serde(default, deserialize_with = "read_optional")]
    trusted: Option<bool>,
    #[serde(default, deserialize_with = "read_optional")]
    rule: Option<String>,
    #[serde(default, deserialize_with = "read_reason")]
    untrusted_reason: Option<Reason>,
}

#[derive(Deserialize)]
struct WireOwner {
    pid: u64,
    #[serde(default)]
    name: String,
}

fn read_optional<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let raw = <&RawValue>::deserialize(d)?;
    Ok(serde_json::from_str(raw.get()).ok())
}

fn read_reason<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Reason>, D::Error> {
    let raw = <&RawValue>::deserialize(d)?;
    let reason = serde_json::from_str::<Reason>(raw.get()).unwrap_or(Reason::Unrecognised);
    // Only absence is Unstated. A present null, malformed value, or unknown name fails closed.
    Ok(Some(if reason == Reason::Unstated {
        Reason::Unrecognised
    } else {
        reason
    }))
}

fn read_records<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Records, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Records;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list of Program Resume records")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Records, A::Error> {
            let mut items = Vec::new();
            let mut capped = false;
            while let Some(raw) = seq.next_element::<&RawValue>()? {
                let Ok(record) = serde_json::from_str::<WireRecord>(raw.get()) else {
                    continue;
                };
                // Keep the identity raw, but do not keep unbounded text in a key.
                if record.owner.name.len() > MAX_KEY_OWNER_NAME
                    || record.reported_at.len() > MAX_KEY_REPORTED_AT
                {
                    capped = true;
                    continue;
                }
                if items.len() == MAX_RECORDS_PER_BLOCK {
                    while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                    return Ok(Records {
                        items,
                        capped: true,
                    });
                }
                items.push(record);
            }
            Ok(Records { items, capped })
        }
    }

    d.deserialize_seq(Visitor)
}

impl WireSnapshot {
    pub(crate) fn into_snapshot(self) -> ResumeSnapshot {
        let mut has_chosen = false;
        let records = self
            .records
            .items
            .into_iter()
            .map(|record| {
                let chosen = record.chosen && !has_chosen;
                has_chosen |= chosen;
                let trust = match (record.trusted, record.rule, record.untrusted_reason) {
                    (Some(true), Some(rule), None) if !rule.is_empty() => Trust::Trusted {
                        rule: escape(&rule, MAX_NAME),
                    },
                    (_, _, reason) => Trust::Untrusted(reason.unwrap_or(Reason::Unstated)),
                };
                ResumeSummary {
                    reported_at: time::timestamp(&record.reported_at),
                    owner_name: escape(&record.owner.name, MAX_NAME),
                    key: RecordKey {
                        pid: record.owner.pid,
                        owner_name: record.owner.name,
                        reported_at: record.reported_at,
                    },
                    program: escape(&record.program, MAX_NAME),
                    cwd: record.cwd.map(|cwd| escape(&cwd, MAX_CWD)),
                    chosen,
                    trust,
                }
            })
            .collect();
        ResumeSnapshot {
            records,
            truncated: self.truncated || self.records.capped,
        }
    }
}

impl ResumeSnapshot {
    pub(crate) fn apply_host_budget(&mut self, remaining: usize) {
        if self.records.len() > remaining {
            self.records.truncate(remaining);
            self.truncated = true;
        }
    }
}

/// Expose invisible or misleading code points. Limit the escaped text, keeping both ends
/// and never cutting a visible escape in half. The limit counts Unicode scalars, not bytes.
pub fn escape(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let units = || {
        text.chars().scan(true, |after_escape, c| {
            use GeneralCategory as Cat;
            let category = CodePointMapData::<Cat>::new().get(c);
            let mark = matches!(
                category,
                Cat::NonspacingMark | Cat::SpacingMark | Cat::EnclosingMark
            );
            let unit = if c == '\\' {
                Escape::Backslash
            } else if c.is_control()
                || CodePointSetData::new::<DefaultIgnorableCodePoint>().contains(c)
                || matches!(
                    category,
                    Cat::Format
                        | Cat::LineSeparator
                        | Cat::ParagraphSeparator
                        | Cat::PrivateUse
                        | Cat::Unassigned
                )
                || (c != ' ' && c.is_whitespace())
                || (*after_escape && mark)
                || c == '\u{2800}'
            // Braille blank is assigned and visibly empty.
            {
                Escape::Unicode(c)
            } else if mark {
                Escape::Mark(c)
            } else {
                Escape::Plain(c)
            };
            *after_escape = matches!(unit, Escape::Unicode(_) | Escape::Backslash);
            Some(unit)
        })
    };
    let prefix_limit = max / 2;
    let mut prefix_end = 0;
    let mut used = 0;
    let mut out = String::new();
    for unit in units() {
        used += unit.len();
        if used > max {
            out.truncate(prefix_end);
            out.push('…');
            push_tail(&mut out, units(), max - prefix_limit - 1);
            break;
        }
        unit.push(&mut out);
        if used <= prefix_limit {
            prefix_end = out.len();
        }
    }
    out
}

fn push_tail(out: &mut String, units: impl Iterator<Item = Escape>, limit: usize) {
    let mut tail: VecDeque<Escape> = VecDeque::new();
    let mut used = 0;
    for unit in units {
        used += unit.len();
        tail.push_back(unit);
        while used > limit {
            used -= tail.pop_front().expect("tail exceeds limit").len();
        }
    }

    // A cut can expose marks that previously followed a plain character. Escape
    // that leading run so it cannot attach to the ellipsis or to a visible escape.
    let extra_for_mark = |unit: &Escape| match unit {
        Escape::Mark(c) => Some(Escape::Unicode(*c).len() - 1),
        _ => None,
    };
    let mut extra: usize = tail.iter().map_while(extra_for_mark).sum();
    while used + extra > limit {
        let mark = tail.pop_front().expect("tail starts with a mark");
        extra -= extra_for_mark(&mark).expect("tail starts with a mark");
        used -= mark.len();
    }
    let mut leading = true;
    for unit in tail {
        if let (true, Escape::Mark(c)) = (leading, &unit) {
            Escape::Unicode(*c).push(out);
        } else {
            leading = false;
            unit.push(out);
        }
    }
}

enum Escape {
    Plain(char),
    Mark(char),
    Unicode(char),
    Backslash,
}

impl Escape {
    fn len(&self) -> usize {
        match self {
            Self::Plain(_) | Self::Mark(_) => 1,
            Self::Backslash => 2,
            Self::Unicode(c) => match *c as u32 {
                0..=0xffff => 8,
                0x10000..=0xfffff => 9,
                _ => 10,
            },
        }
    }

    fn push(self, out: &mut String) {
        use fmt::Write;
        match self {
            Self::Plain(c) | Self::Mark(c) => out.push(c),
            Self::Backslash => out.push_str("\\\\"),
            Self::Unicode(c) => write!(out, "\\u{{{:04X}}}", c as u32).expect("write to String"),
        }
    }
}

#[cfg(test)]
mod tests;
