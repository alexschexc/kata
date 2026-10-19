//! Persistent progress (`state.json`), format-compatible with the Zig implementation.
//!
//! Zig serialises the per-column `edition` bytes as a JSON string (`"\u0000\u0001\u0000"`);
//! this module writes exactly that and reads either that or an integer array. The JSON layout,
//! field order, two-space indentation and the absence of a trailing newline match Zig's output.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Position {
    pub row: usize,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct State {
    pub version: u8,
    pub plan_hash: u64,
    pub next_day: usize,
    pub completed_on: i32,
    pub scope_hash: u64,
    pub positions: [Position; 3],
    pub enabled: [bool; 3],
    pub linked: bool,
    pub focus: usize,
    /// Per-column edition: 0 = bundled source, 1 = ingested alternative (column 1: the
    /// Psalter, Psalms only). Missing in old files means 0.
    #[serde(with = "edition_bytes")]
    pub edition: [u8; 3],
}

impl Default for State {
    fn default() -> Self {
        State {
            version: 1,
            plan_hash: 0,
            next_day: 0,
            completed_on: 0,
            scope_hash: 0,
            positions: [Position::default(); 3],
            enabled: [true; 3],
            linked: true,
            focus: 0,
            edition: [0; 3],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StateError {
    InvalidState,
    AlreadyCompletedToday,
    ProgressOverflow,
    InvalidPlanDay,
    Io(String),
    Parse(String),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::InvalidState => write!(f, "InvalidState"),
            StateError::AlreadyCompletedToday => write!(f, "AlreadyCompletedToday"),
            StateError::ProgressOverflow => write!(f, "ProgressOverflow"),
            StateError::InvalidPlanDay => write!(f, "InvalidPlanDay"),
            StateError::Io(message) => write!(f, "{message}"),
            StateError::Parse(message) => write!(f, "invalid state file: {message}"),
        }
    }
}

impl std::error::Error for StateError {}

impl State {
    /// The plan ordinal (0-based) that is currently the day's assignment.
    pub fn assignment(&self, today: i32) -> usize {
        if self.completed_on >= today && self.next_day > 0 {
            self.next_day - 1
        } else {
            self.next_day
        }
    }

    pub fn complete(&mut self, today: i32) -> Result<(), StateError> {
        if self.completed_on >= today {
            return Err(StateError::AlreadyCompletedToday);
        }
        if self.next_day == usize::MAX {
            return Err(StateError::ProgressOverflow);
        }
        self.next_day += 1;
        self.completed_on = today;
        Ok(())
    }

    /// Reposition so plan `day` (1-based) is pending. Leaves `self` untouched on error.
    pub fn start_at_day(
        &mut self,
        day: usize,
        duration: usize,
        today: i32,
    ) -> Result<(), StateError> {
        if duration == 0 || day == 0 || day > duration {
            return Err(StateError::InvalidPlanDay);
        }
        let current = self.assignment(today);
        let cycle_start = current - current % duration;
        let target = cycle_start
            .checked_add(day - 1)
            .ok_or(StateError::ProgressOverflow)?;
        // Progress is a completed prefix, not a fabricated calendar history.
        // Repositioning makes the selected day pending, even after a completion.
        self.next_day = target;
        self.completed_on = 0;
        self.scope_hash = 0;
        self.positions = [Position::default(); 3];
        Ok(())
    }

    pub fn validate(&self) -> Result<(), StateError> {
        if self.version != 1 || self.focus >= 3 {
            return Err(StateError::InvalidState);
        }
        if !self.enabled.iter().any(|&on| on) {
            return Err(StateError::InvalidState);
        }
        if self.completed_on < 0 || self.edition.iter().any(|&e| e > 1) {
            return Err(StateError::InvalidState);
        }
        Ok(())
    }
}

mod edition_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(value: &[u8; 3], serializer: S) -> Result<S::Ok, S::Error> {
        let text: String = value.iter().map(|&b| b as char).collect();
        serializer.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 3], D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Text(String),
            List(Vec<u8>),
        }
        let bytes: Vec<u8> = match Form::deserialize(deserializer)? {
            Form::Text(text) => {
                if text.chars().any(|c| c as u32 > 0xff) {
                    return Err(D::Error::custom("edition text out of range"));
                }
                text.chars().map(|c| c as u8).collect()
            }
            Form::List(list) => list,
        };
        <[u8; 3]>::try_from(bytes).map_err(|_| D::Error::custom("edition needs exactly 3 entries"))
    }
}

/// Parse and validate state bytes. Files larger than 64 KiB are rejected like the Zig loader.
pub fn parse(bytes: &[u8]) -> Result<State, StateError> {
    if bytes.len() > 65536 {
        return Err(StateError::Parse("file too large".into()));
    }
    let state: State =
        serde_json::from_slice(bytes).map_err(|e| StateError::Parse(e.to_string()))?;
    state.validate()?;
    Ok(state)
}

/// Serialise exactly as Zig's `std.json.Stringify` with two-space indentation.
pub fn to_bytes(state: &State) -> Result<Vec<u8>, StateError> {
    state.validate()?;
    serde_json::to_vec_pretty(state).map_err(|e| StateError::Parse(e.to_string()))
}

/// Missing file means a fresh default state.
pub fn load(path: &Path) -> Result<State, StateError> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(StateError::Io(e.to_string())),
    }
}

/// Write via `<path>.tmp` then rename, creating parent directories.
pub fn save(path: &Path, state: &State) -> Result<(), StateError> {
    let bytes = to_bytes(state)?;
    let io = |e: std::io::Error| StateError::Io(e.to_string());
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, bytes).map_err(io)?;
    std::fs::rename(&temporary, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missed_calendar_days_do_not_advance_or_accumulate_assignments() {
        let mut state = State::default();
        assert_eq!(state.assignment(20261003), 0);
        state.complete(20261003).unwrap();
        assert_eq!(state.assignment(20261003), 0);
        assert_eq!(state.assignment(20261004), 1);
        assert_eq!(state.assignment(20261008), 1);
        state.complete(20261008).unwrap();
        assert_eq!(state.assignment(20261009), 2);
    }

    #[test]
    fn manual_completion_only_once_per_calendar_day() {
        let mut state = State::default();
        state.complete(20261003).unwrap();
        assert_eq!(
            state.complete(20261003),
            Err(StateError::AlreadyCompletedToday)
        );
        assert_eq!(
            state.complete(20261002),
            Err(StateError::AlreadyCompletedToday)
        );
        assert_eq!(state.next_day, 1);
    }

    #[test]
    fn invalid_persisted_pane_selections_fail_rather_than_reset() {
        let none = State {
            enabled: [false; 3],
            ..State::default()
        };
        assert_eq!(none.validate(), Err(StateError::InvalidState));
        let focus = State {
            focus: 99,
            ..State::default()
        };
        assert_eq!(focus.validate(), Err(StateError::InvalidState));
    }

    #[test]
    fn import_print_progress_leaves_the_selected_assignment_pending_today() {
        let mut state = State {
            linked: false,
            focus: 1,
            scope_hash: 123,
            positions: [
                Position { row: 8, line: 0 },
                Position::default(),
                Position::default(),
            ],
            ..State::default()
        };
        state.start_at_day(88, 89, 20261007).unwrap();
        assert_eq!(state.next_day, 87);
        assert_eq!(state.assignment(20261007), 87);
        assert_eq!(state.completed_on, 0);
        assert_eq!(state.scope_hash, 0);
        assert_eq!(state.positions[0], Position::default());
        assert!(!state.linked);
        assert_eq!(state.focus, 1);
        state.complete(20261007).unwrap();
        assert_eq!(state.next_day, 88);
        assert_eq!(state.assignment(20261008), 88);
    }

    #[test]
    fn plan_jumps_reject_invalid_day_ranges_without_modifying_state() {
        let mut state = State {
            next_day: 10,
            completed_on: 20261006,
            ..State::default()
        };
        let original = state;
        for (day, duration) in [(90, 89), (0, 89), (1, 0)] {
            assert_eq!(
                state.start_at_day(day, duration, 20261007),
                Err(StateError::InvalidPlanDay)
            );
            assert_eq!(state, original);
        }
    }

    #[test]
    fn plan_jumps_preserve_finished_cycles_and_allow_restarting_earlier_in_this_cycle() {
        let mut state = State {
            next_day: 100,
            completed_on: 20261007,
            ..State::default()
        };
        state.start_at_day(88, 89, 20261007).unwrap();
        assert_eq!(state.next_day, 176);
        state.start_at_day(1, 89, 20261007).unwrap();
        assert_eq!(state.next_day, 89);
        assert_eq!(state.completed_on, 0);
        // The last assignment is still today's visible cycle after completion.
        state = State {
            next_day: 89,
            completed_on: 20261007,
            ..State::default()
        };
        state.start_at_day(88, 89, 20261007).unwrap();
        assert_eq!(state.next_day, 87);
        // On the next date the repeat has begun, so its completed predecessor stays.
        state = State {
            next_day: 89,
            completed_on: 20261007,
            ..State::default()
        };
        state.start_at_day(88, 89, 20261008).unwrap();
        assert_eq!(state.next_day, 176);
    }

    #[test]
    fn overflowing_progress_jumps_fail_without_modifying_state() {
        let mut state = State {
            next_day: usize::MAX,
            ..State::default()
        };
        let original = state;
        assert_eq!(
            state.start_at_day(2, usize::MAX, 20261007),
            Err(StateError::ProgressOverflow)
        );
        assert_eq!(state, original);
    }

    // Files written by the Zig build (V0.3.1): `kata --start-day 5 --confirm` and `kata --complete`.
    const ZIG_START_DAY_5: &[u8] = include_bytes!("../tests/fixtures/zig-start-day-5.json");
    const ZIG_COMPLETE: &[u8] = include_bytes!("../tests/fixtures/zig-complete.json");

    #[test]
    fn loads_state_written_by_zig() {
        let start = parse(ZIG_START_DAY_5).unwrap();
        assert_eq!(
            (start.next_day, start.completed_on, start.plan_hash),
            (4, 0, 14968686216195794100)
        );
        let done = parse(ZIG_COMPLETE).unwrap();
        assert_eq!((done.next_day, done.completed_on), (1, 20261019));
        assert_eq!(done.edition, [0, 0, 0]);
    }

    #[test]
    fn rewrites_zig_files_byte_for_byte() {
        for fixture in [ZIG_START_DAY_5, ZIG_COMPLETE] {
            assert_eq!(to_bytes(&parse(fixture).unwrap()).unwrap(), fixture);
        }
    }

    #[test]
    fn reads_edition_as_string_or_array_and_defaults_missing_fields() {
        let array = parse(br#"{"edition":[0,1,0],"next_day":4}"#).unwrap();
        assert_eq!(array.edition, [0, 1, 0]);
        assert_eq!(array.next_day, 4);
        assert!(array.linked);
        let text = parse(br#"{"edition":"\u0000\u0001\u0000"}"#).unwrap();
        assert_eq!(text.edition, [0, 1, 0]);
        assert_eq!(parse(b"{}").unwrap(), State::default());
    }

    #[test]
    fn rejects_unknown_fields_bad_editions_and_oversized_files() {
        assert!(parse(br#"{"extra":1}"#).is_err());
        assert!(parse(br#"{"edition":[0,2,0]}"#).is_err());
        assert!(parse(br#"{"edition":[0,0]}"#).is_err());
        assert!(parse(b"not json").is_err());
        assert!(parse(&vec![b' '; 65537]).is_err());
    }

    #[test]
    fn save_and_load_round_trip_through_a_temporary_file() {
        let dir = std::env::temp_dir().join(format!("kata-state-test-{}", std::process::id()));
        let path = dir.join("nested/progress.json");
        let state = State {
            next_day: 87,
            edition: [0, 1, 0],
            ..State::default()
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path).unwrap(), state);
        assert!(!dir.join("nested/progress.json.tmp").exists());
        assert_eq!(load(&dir.join("missing.json")).unwrap(), State::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
