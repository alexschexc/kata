//! Reading plans: JSON config, validation and per-day chapter assignments. Ported from
//! `src/plan.zig`; validation rules and error behaviour are identical.

use serde::{Deserialize, Serialize};

/// `stream` indexes `Config::streams`; for library streams `chapter` is the 1-based reading
/// unit, not a Bible chapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub book: String,
    pub chapter: u16,
    pub stream: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Book {
    pub name: String,
    pub chapters: u16,
}

/// A library stream reads one ingested book (`library` = its file in the library folder) in
/// `unit`s ("chapter" or "section"); its single book holds a display name and the unit count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub books: Vec<Book>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub days: u16,
    pub rates: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub name: String,
    pub repeat: bool,
    pub streams: Vec<Stream>,
    pub phases: Vec<Phase>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    InvalidPlan,
    DayOutOfRange,
    Parse(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::InvalidPlan => write!(f, "InvalidPlan"),
            PlanError::DayOutOfRange => write!(f, "DayOutOfRange"),
            PlanError::Parse(message) => write!(f, "invalid plan JSON: {message}"),
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Debug, Clone)]
pub struct Plan {
    config: Config,
}

impl Plan {
    pub fn new(bytes: &[u8]) -> Result<Plan, PlanError> {
        let config: Config =
            serde_json::from_slice(bytes).map_err(|e| PlanError::Parse(e.to_string()))?;
        validate(&config)?;
        Ok(Plan { config })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn total_days(&self) -> usize {
        self.config.phases.iter().map(|p| p.days as usize).sum()
    }

    /// Chapters for a zero-based day ordinal. Nonrepeating plans return `DayOutOfRange` after
    /// the end.
    pub fn assignments(&self, ordinal: usize) -> Result<Vec<Chapter>, PlanError> {
        let duration = self.total_days();
        if !self.config.repeat && ordinal >= duration {
            return Err(PlanError::DayOutOfRange);
        }
        let day = if self.config.repeat {
            ordinal % duration
        } else {
            ordinal
        };
        let mut result = Vec::new();
        for (index, stream) in self.config.streams.iter().enumerate() {
            let mut remaining = day;
            let mut offset = 0usize;
            let mut rate = 0usize;
            for phase in &self.config.phases {
                let phase_rate = phase.rates[index] as usize;
                if remaining < phase.days as usize {
                    offset += remaining * phase_rate;
                    rate = phase_rate;
                    break;
                }
                offset += phase.days as usize * phase_rate;
                remaining -= phase.days as usize;
            }
            for book in &stream.books {
                let chapters = book.chapters as usize;
                if offset >= chapters {
                    offset -= chapters;
                    continue;
                }
                let count = rate.min(chapters - offset);
                for i in 0..count {
                    result.push(Chapter {
                        book: book.name.clone(),
                        chapter: (offset + i + 1) as u16,
                        stream: index,
                    });
                }
                rate -= count;
                offset = 0;
                if rate == 0 {
                    break;
                }
            }
        }
        Ok(result)
    }
}

fn validate(config: &Config) -> Result<(), PlanError> {
    let bad = Err(PlanError::InvalidPlan);
    if config.name.trim_matches([' ', '\t', '\r', '\n']).is_empty()
        || config.streams.is_empty()
        || config.phases.is_empty()
    {
        return bad;
    }
    for phase in &config.phases {
        if phase.days == 0
            || phase.rates.len() != config.streams.len()
            || phase.rates.iter().all(|&r| r == 0)
        {
            return bad;
        }
    }
    for (index, stream) in config.streams.iter().enumerate() {
        if stream.books.is_empty() {
            return bad;
        }
        if let Some(file) = &stream.library {
            if stream.books.len() != 1 || file.is_empty() || file.contains(['/', '\\']) {
                return bad;
            }
            match stream.unit.as_deref() {
                Some("chapter") | Some("section") => {}
                _ => return bad,
            }
        }
        let mut chapters = 0usize;
        for book in &stream.books {
            if book.chapters == 0 || book.name.trim_matches(' ').is_empty() {
                return bad;
            }
            // Only plain English letters, digits and spaces are safe identifiers. Canonical
            // Bible-name mapping belongs to the source adapter.
            if stream.library.is_none()
                && !book
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b' ')
            {
                return bad;
            }
            chapters += book.chapters as usize;
        }
        let mut assigned = 0usize;
        for phase in &config.phases {
            assigned += phase.days as usize * phase.rates[index] as usize;
            if assigned > chapters {
                return bad;
            }
        }
        if assigned != chapters {
            return bad;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = r#"{"name":"Simple","repeat":false,"streams":[{"books":[{"name":"1 John","chapters":2}]}],"phases":[{"days":2,"rates":[1]}]}"#;
    pub const OPTINA: &str = include_str!("../../../../config/optina.json");

    fn plan(json: &str) -> Plan {
        Plan::new(json.as_bytes()).unwrap()
    }
    fn at(chapters: &[Chapter], i: usize) -> (&str, u16) {
        (chapters[i].book.as_str(), chapters[i].chapter)
    }
    fn invalid(json: &str) {
        assert_eq!(
            Plan::new(json.as_bytes()).unwrap_err(),
            PlanError::InvalidPlan,
            "{json}"
        );
    }

    #[test]
    fn parse_config_and_expose_name_and_duration() {
        let p = plan(SIMPLE);
        assert_eq!(p.name(), "Simple");
        assert_eq!(p.total_days(), 2);
    }

    #[test]
    fn irregular_phases_preserve_stream_and_book_order_and_allow_paused_streams() {
        let p = plan(
            r#"{"name":"Irregular","repeat":true,"streams":[{"books":[{"name":"Matthew","chapters":1},{"name":"Mark","chapters":2}]},{"books":[{"name":"Acts","chapters":2}]}],"phases":[{"days":1,"rates":[2,0]},{"days":1,"rates":[0,2]},{"days":1,"rates":[1,0]}]}"#,
        );
        let first = p.assignments(0).unwrap();
        assert_eq!(
            (first.len(), at(&first, 0), at(&first, 1)),
            (2, ("Matthew", 1), ("Mark", 1))
        );
        let second = p.assignments(1).unwrap();
        assert_eq!(
            (second.len(), at(&second, 0), at(&second, 1)),
            (2, ("Acts", 1), ("Acts", 2))
        );
        let last = p.assignments(2).unwrap();
        assert_eq!((last.len(), at(&last, 0)), (1, ("Mark", 2)));
        assert_eq!(p.assignments(3).unwrap(), first);
    }

    #[test]
    fn nonrepeating_plans_reject_out_of_range_ordinals() {
        let p = plan(SIMPLE);
        assert_eq!(p.assignments(2), Err(PlanError::DayOutOfRange));
        assert_eq!(p.assignments(usize::MAX), Err(PlanError::DayOutOfRange));
    }

    #[test]
    fn reject_empty_configuration_and_empty_collections() {
        for json in [
            r#"{"name":"","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}"#,
            r#"{"name":"  ","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[],"phases":[{"days":1,"rates":[]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[]}],"phases":[{"days":1,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[]}"#,
        ] {
            invalid(json);
        }
    }

    #[test]
    fn reject_zero_chapters_days_progress_and_mismatched_rates() {
        for json in [
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":0}]}],"phases":[{"days":1,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":0,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[0]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1,1]}]}"#,
        ] {
            invalid(json);
        }
    }

    #[test]
    fn reject_undershoot_and_overshoot_of_each_stream() {
        for json in [
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":3}]}],"phases":[{"days":2,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":2,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":2}]},{"books":[{"name":"Acts","chapters":1}]}],"phases":[{"days":1,"rates":[1,2]}]}"#,
        ] {
            invalid(json);
        }
    }

    #[test]
    fn reject_unsafe_book_names_while_accepting_full_english_names() {
        for name in [
            "",
            " ",
            "../John",
            "John/Acts",
            "John\\\\Acts",
            "John:1",
            "John;cmd",
            "John<Acts",
            "John\\n",
            "John\\t",
            "John\\u007f",
            "John\\u0000",
        ] {
            invalid(&format!(
                r#"{{"name":"X","repeat":true,"streams":[{{"books":[{{"name":"{name}","chapters":1}}]}}],"phases":[{{"days":1,"rates":[1]}}]}}"#
            ));
        }
    }

    #[test]
    fn optina_boundaries_days_1_82_83_89_and_90() {
        let p = plan(OPTINA);
        assert_eq!((p.name(), p.total_days()), ("Optina", 89));
        // (ordinal, gospel chapter, first epistle-stream chapter, count)
        for (ordinal, gospel, first, count) in [
            (0, 1, 1, 3),
            (81, 14, 14, 3),
            (82, 15, 16, 2),
            (88, 21, 22, 2),
            (89, 1, 1, 3),
        ] {
            let c = p.assignments(ordinal).unwrap();
            assert_eq!(c.len(), count);
            let wrap = ordinal == 0 || ordinal == 89;
            assert_eq!(at(&c, 0), (if wrap { "Matthew" } else { "John" }, gospel));
            assert_eq!(at(&c, 1), (if wrap { "Acts" } else { "Revelation" }, first));
            if count == 3 {
                assert_eq!(at(&c, 2), (c[1].book.as_str(), first + 1));
            }
        }
    }

    #[test]
    fn optina_covers_every_chapter_exactly_once_in_ordered_streams() {
        let p = plan(OPTINA);
        let (mut book_index, mut next, mut counts) = ([0usize; 2], [1u16; 2], [0usize; 2]);
        for day in 0..p.total_days() {
            let c = p.assignments(day).unwrap();
            let rates = if day < 82 { [1, 2] } else { [1, 1] };
            let mut cursor = 0;
            for (stream, rate) in rates.into_iter().enumerate() {
                for _ in 0..rate {
                    let book = &p.config().streams[stream].books[book_index[stream]];
                    assert_eq!(at(&c, cursor), (book.name.as_str(), next[stream]));
                    cursor += 1;
                    counts[stream] += 1;
                    if next[stream] == book.chapters {
                        book_index[stream] += 1;
                        next[stream] = 1;
                    } else {
                        next[stream] += 1;
                    }
                }
            }
            assert_eq!(cursor, c.len());
        }
        assert_eq!(counts, [89, 171]);
        for (stream, index) in book_index.into_iter().enumerate() {
            assert_eq!(index, p.config().streams[stream].books.len());
        }
        assert_eq!(
            p.assignments(usize::MAX).unwrap(),
            p.assignments(usize::MAX % 89).unwrap()
        );
    }

    #[test]
    fn json_parsing_rejects_malformed_missing_unknown_and_out_of_u16_bounds_fields() {
        for json in [
            "",
            "{}",
            "not JSON",
            r#"{"name":"X","repeat":true,"extra":1,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":65536}]}],"phases":[{"days":1,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":65536,"rates":[1]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[65536]}]}"#,
            r#"{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":-1}]}],"phases":[{"days":1,"rates":[1]}]}"#,
        ] {
            assert!(Plan::new(json.as_bytes()).is_err(), "{json}");
        }
    }

    #[test]
    fn u16_chapter_limits_and_durations_beyond_u16_are_safe() {
        let p = plan(
            r#"{"name":"Long","repeat":true,"streams":[{"books":[{"name":"First","chapters":65535},{"name":"Second","chapters":65535}]}],"phases":[{"days":65535,"rates":[1]},{"days":65535,"rates":[1]}]}"#,
        );
        assert_eq!(p.total_days(), 131070);
        for (index, ordinal) in [65534usize, 65535, 131069, 131070].into_iter().enumerate() {
            let c = p.assignments(ordinal).unwrap();
            assert_eq!(c.len(), 1);
            let book = if index == 0 || index == 3 {
                "First"
            } else {
                "Second"
            };
            let chapter = if index == 0 || index == 2 { 65535 } else { 1 };
            assert_eq!(at(&c, 0), (book, chapter));
        }
    }

    #[test]
    fn library_streams_carry_their_file_and_unit_and_number_units_per_stream() {
        let p = plan(
            r#"{"name":"Mixed","repeat":false,"streams":[{"books":[{"name":"John","chapters":2}]},{"books":[{"name":"Building Microservices","chapters":4}],"library":"buildingmicroservices2ndedition","unit":"section"}],"phases":[{"days":2,"rates":[1,2]}]}"#,
        );
        let day = p.assignments(1).unwrap();
        assert_eq!(day.len(), 3);
        assert_eq!((day[1].stream, day[1].chapter), (1, 3));
        assert_eq!(
            p.config().streams[day[1].stream].library.as_deref(),
            Some("buildingmicroservices2ndedition")
        );
        invalid(
            r#"{"name":"Bad","repeat":false,"streams":[{"books":[{"name":"X","chapters":1}],"library":"x","unit":"page"}],"phases":[{"days":1,"rates":[1]}]}"#,
        );
    }

    #[test]
    fn optina_hash_matches_the_value_zig_stores_in_state_files() {
        // plan_hash in the Zig-written fixtures is Wyhash(0, plan bytes) of the built-in Optina plan.
        let fixture =
            crate::state::parse(include_bytes!("../tests/fixtures/zig-complete.json")).unwrap();
        assert_eq!(crate::wyhash::hash(0, OPTINA.as_bytes()), fixture.plan_hash);
    }
}
