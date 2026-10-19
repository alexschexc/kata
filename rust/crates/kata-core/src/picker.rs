//! Plan, day and chapter picker state machine, ported from `src/picker.zig`.

use crate::keys::recognized;

pub use crate::library::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ChoosePlan(usize),
    StartDay(usize),
    Home,
    OpenPlace,
    NextChapter,
    PreviousChapter,
    OpenLocation(Location),
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Closed,
    Plans,
    Days,
    ConfirmDay,
}

const MAX_DIGITS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub kind: Kind,
    pub selected: usize,
    digits: [u8; MAX_DIGITS],
    pub digit_count: usize,
    pub invalid_input: bool,
    pub numeric: bool,
    pub number_base: usize,
}

impl Default for Picker {
    fn default() -> Self {
        Picker {
            kind: Kind::Closed,
            selected: 0,
            digits: [0; MAX_DIGITS],
            digit_count: 0,
            invalid_input: false,
            numeric: false,
            number_base: 1,
        }
    }
}

impl Picker {
    pub fn new(kind: Kind) -> Picker {
        Picker {
            kind,
            ..Picker::default()
        }
    }

    /// A picker of `kind` with `selected` highlighted.
    pub fn at(kind: Kind, selected: usize) -> Picker {
        Picker {
            kind,
            selected,
            ..Picker::default()
        }
    }

    /// A numbered menu: typed digits select a row, counted from `number_base`.
    pub fn numbered(selected: usize, number_base: usize) -> Picker {
        Picker {
            kind: Kind::Plans,
            selected,
            numeric: true,
            number_base,
            ..Picker::default()
        }
    }

    /// The digits typed so far.
    pub fn digits(&self) -> &[u8] {
        &self.digits[..self.digit_count]
    }

    /// Feed one key with `count` available choices; returns an action when one is chosen.
    pub fn handle(&mut self, key: u8, count: usize) -> Option<Action> {
        if !recognized(key) {
            return None;
        }
        if key == 27 || key == b'q' {
            self.kind = Kind::Closed;
            self.digit_count = 0;
            return None;
        }
        if count == 0 || self.kind == Kind::Closed {
            return None;
        }
        if self.selected >= count {
            self.selected = count - 1;
            if self.kind == Kind::ConfirmDay {
                self.kind = Kind::Days;
                return None;
            }
        }
        if self.kind == Kind::ConfirmDay {
            self.kind = Kind::Days;
            return (key == b'y').then_some(Action::StartDay(self.selected + 1));
        }
        if (self.kind == Kind::Days || self.numeric) && key.is_ascii_digit() {
            if self.digit_count < MAX_DIGITS {
                self.digits[self.digit_count] = key;
                self.digit_count += 1;
                self.invalid_input = false;
            } else {
                self.invalid_input = true;
            }
            return None;
        }
        if key == 127 || key == 8 {
            self.digit_count = self.digit_count.saturating_sub(1);
            self.invalid_input = false;
            return None;
        }
        match key {
            b'j' => self.selected = (count - 1).min(self.selected.saturating_add(1)),
            b'k' => self.selected = self.selected.saturating_sub(1),
            4 => self.selected = (count - 1).min(self.selected.saturating_add(10)),
            21 => self.selected = self.selected.saturating_sub(10),
            b'g' => self.selected = 0,
            b'G' => self.selected = count - 1,
            10 | 13 => {
                if self.digit_count > 0 {
                    // Digits are ASCII, so this parse only fails on usize overflow.
                    let typed = std::str::from_utf8(self.digits())
                        .ok()
                        .and_then(|s| s.parse::<usize>().ok());
                    let Some(day) = typed else {
                        self.invalid_input = true;
                        return None;
                    };
                    let base = if self.kind == Kind::Days {
                        1
                    } else {
                        self.number_base
                    };
                    if self.invalid_input || day < base || day - base >= count {
                        self.invalid_input = true;
                        return None;
                    }
                    self.selected = day - base;
                    self.digit_count = 0;
                }
                if self.kind == Kind::Plans {
                    return Some(Action::ChoosePlan(self.selected));
                }
                self.kind = Kind::ConfirmDay;
                return None;
            }
            _ => return None,
        }
        self.digit_count = 0;
        self.invalid_input = false;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(picker: &mut Picker, keys: &str, count: usize) {
        for key in keys.bytes() {
            picker.handle(key, count);
        }
    }

    #[test]
    fn overflowing_and_out_of_range_numbers_never_select_or_confirm() {
        for digits in [
            "0",
            "6",
            "99999999999999999999",
            "999999999999999999999999999999999",
        ] {
            let mut picker = Picker {
                selected: 1,
                ..Picker::new(Kind::Days)
            };
            press(&mut picker, digits, 5);
            assert_eq!(picker.handle(13, 5), None);
            assert_eq!(picker.kind, Kind::Days);
            assert_eq!(picker.selected, 1);
        }
    }

    #[test]
    fn shrinking_and_empty_menus_keep_selections_in_bounds() {
        let mut picker = Picker {
            selected: usize::MAX,
            ..Picker::new(Kind::Plans)
        };
        assert_eq!(picker.handle(13, 0), None);
        picker.handle(b'k', 2);
        assert!(picker.selected < 2);
        match picker.handle(13, 2) {
            Some(Action::ChoosePlan(n)) => assert!(n < 2),
            other => panic!("expected a plan choice, got {other:?}"),
        }
    }

    #[test]
    fn unknown_bytes_leave_a_pending_confirmation_unchanged() {
        let mut picker = Picker {
            selected: 1,
            ..Picker::new(Kind::ConfirmDay)
        };
        for byte in [b'@', 0, 255] {
            assert_eq!(picker.handle(byte, 5), None);
            assert_eq!(picker.kind, Kind::ConfirmDay);
        }
    }

    #[test]
    fn stale_confirmation_selection_cannot_overflow_or_emit_an_invalid_day() {
        let mut picker = Picker {
            selected: usize::MAX,
            ..Picker::new(Kind::ConfirmDay)
        };
        assert_eq!(picker.handle(b'y', 2), None);
    }

    #[test]
    fn plan_picker_navigates_and_chooses_only_on_enter() {
        let mut picker = Picker::new(Kind::Plans);
        assert_eq!(picker.handle(b'j', 2), None);
        assert_eq!(picker.handle(13, 2), Some(Action::ChoosePlan(1)));
    }

    #[test]
    fn day_number_entry_previews_then_needs_separate_confirmation() {
        let mut picker = Picker::new(Kind::Days);
        press(&mut picker, "88", 89);
        assert_eq!(picker.handle(13, 89), None);
        assert_eq!(picker.kind, Kind::ConfirmDay);
        assert_eq!(picker.handle(b'y', 89), Some(Action::StartDay(88)));
    }

    #[test]
    fn invalid_days_and_cancelled_dialogs_never_emit_progress_changes() {
        let mut picker = Picker::new(Kind::Days);
        press(&mut picker, "90", 89);
        assert_eq!(picker.handle(13, 89), None);
        assert!(picker.invalid_input);
        assert_eq!(picker.kind, Kind::Days);
        picker.handle(27, 89);
        assert_eq!(picker.kind, Kind::Closed);
        picker = Picker {
            selected: 87,
            ..Picker::new(Kind::ConfirmDay)
        };
        assert_eq!(picker.handle(b'n', 89), None);
        assert_eq!(picker.kind, Kind::Days);
    }

    #[test]
    fn numbered_choices_select_a_chapter_directly_without_a_progress_confirmation() {
        let mut picker = Picker {
            numeric: true,
            ..Picker::new(Kind::Plans)
        };
        press(&mut picker, "20", 21);
        assert_eq!(picker.handle(13, 21), Some(Action::ChoosePlan(19)));
    }
}
