//! The weekly start policy for queued transfers (XDL-E6-S2).
//!
//! This is deliberately a policy over *starting* work. A running transfer is
//! allowed to finish its current operation when the hour changes; pausing it
//! in the middle would turn a calendar preference into an unexpected cancel.
//!
//! The provider does not expose a rate limiter yet (XD-E7-S1), so the policy
//! has two states: [`ScheduleMode::FullSpeed`] and
//! [`ScheduleMode::Paused`]. There is no throttled state that could claim to
//! limit bytes without an engine primitive to enforce it.
//!
//! The caller supplies a local [`ScheduleSlot`]. Keeping timezone conversion
//! outside the engine avoids silently treating UTC as the user's wall clock;
//! the same slot is also convenient for a keyboard accessible 7×24 editor.

use serde::{Deserialize, Serialize};

/// Number of days represented by a weekly schedule.
pub const DAYS_PER_WEEK: usize = 7;
/// Number of hourly slots in one day.
pub const HOURS_PER_DAY: usize = 24;
/// Number of hourly slots in one week.
pub const HOURS_PER_WEEK: usize = DAYS_PER_WEEK * HOURS_PER_DAY;

/// A day in the schedule, ordered from Monday so its index is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// Zero-based index in a [`WeeklySchedule`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }

    /// The day for a zero-based index.
    #[must_use]
    pub const fn from_index(index: usize) -> Option<Self> {
        Some(match index {
            0 => Self::Monday,
            1 => Self::Tuesday,
            2 => Self::Wednesday,
            3 => Self::Thursday,
            4 => Self::Friday,
            5 => Self::Saturday,
            6 => Self::Sunday,
            _ => return None,
        })
    }
}

/// One valid local hour in a weekly schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScheduleSlot {
    /// The local day. The caller is responsible for resolving timezone and
    /// daylight-saving transitions before constructing this value.
    pub weekday: Weekday,
    /// The hour at the start of the slot, in the range `0..24`.
    pub hour: u8,
}

impl<'de> Deserialize<'de> for ScheduleSlot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSlot {
            weekday: Weekday,
            hour: u8,
        }

        let raw = RawSlot::deserialize(deserializer)?;
        Self::new(raw.weekday, raw.hour).ok_or_else(|| {
            serde::de::Error::custom(format!("schedule hour must be below {HOURS_PER_DAY}"))
        })
    }
}

impl ScheduleSlot {
    /// Construct a slot, rejecting the boundary value 24.
    #[must_use]
    pub const fn new(weekday: Weekday, hour: u8) -> Option<Self> {
        if hour < 24 {
            Some(Self { weekday, hour })
        } else {
            None
        }
    }

    /// Zero-based index in a [`WeeklySchedule`].
    #[must_use]
    pub const fn index(self) -> usize {
        self.weekday.index() * HOURS_PER_DAY + self.hour as usize
    }
}

/// What a new queued transfer may do during one hourly slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleMode {
    /// New work may claim a worker permit.
    FullSpeed,
    /// New work waits for a later open slot.
    Paused,
}

impl ScheduleMode {
    /// Whether a queued transfer may start in this mode.
    #[must_use]
    pub const fn allows_start(self) -> bool {
        matches!(self, Self::FullSpeed)
    }
}

/// A seven by twenty-four weekly policy.
///
/// Slots are stored Monday 00:00 through Sunday 23:00. The vector is kept
/// private so every instance has exactly [`HOURS_PER_WEEK`] entries; use
/// [`Self::from_slots`] when restoring a serialized value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WeeklySchedule {
    slots: Vec<ScheduleMode>,
}

impl<'de> Deserialize<'de> for WeeklySchedule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSchedule {
            slots: Vec<ScheduleMode>,
        }

        let raw = RawSchedule::deserialize(deserializer)?;
        Self::from_slots(raw.slots).map_err(serde::de::Error::custom)
    }
}

impl WeeklySchedule {
    /// A policy that permits new work in every slot.
    #[must_use]
    pub fn full_speed() -> Self {
        Self {
            slots: vec![ScheduleMode::FullSpeed; HOURS_PER_WEEK],
        }
    }

    /// A policy that pauses new work in every slot.
    #[must_use]
    pub fn paused() -> Self {
        Self {
            slots: vec![ScheduleMode::Paused; HOURS_PER_WEEK],
        }
    }

    /// Build a schedule from its Monday-first slot list.
    ///
    /// Rejecting malformed lengths matters because silently padding or
    /// truncating a persisted policy would move a user's pause to another
    /// hour. A missing setting should use [`Self::default`] instead.
    ///
    /// # Errors
    /// Returns [`ScheduleError::WrongSlotCount`] unless exactly 168 slots are supplied.
    pub fn from_slots(slots: Vec<ScheduleMode>) -> Result<Self, ScheduleError> {
        if slots.len() != HOURS_PER_WEEK {
            return Err(ScheduleError::WrongSlotCount {
                expected: HOURS_PER_WEEK,
                actual: slots.len(),
            });
        }
        Ok(Self { slots })
    }

    /// The schedule's complete Monday-first slot list.
    #[must_use]
    pub fn slots(&self) -> &[ScheduleMode] {
        &self.slots
    }

    /// Read the policy for a local slot.
    #[must_use]
    pub fn mode_at(&self, slot: ScheduleSlot) -> ScheduleMode {
        // The constructor guarantees the vector's length, and ScheduleSlot
        // guarantees its hour. Keeping this indexing infallible makes a bad
        // schedule impossible to smuggle into a scheduling decision.
        self.slots[slot.index()]
    }

    /// Whether a queued transfer may start now.
    #[must_use]
    pub fn allows_start(&self, slot: ScheduleSlot) -> bool {
        self.mode_at(slot).allows_start()
    }

    /// Change one hourly slot.
    pub fn set_mode(&mut self, slot: ScheduleSlot, mode: ScheduleMode) {
        self.slots[slot.index()] = mode;
    }

    /// Count open slots, useful for summaries without interpreting the grid.
    #[must_use]
    pub fn open_slots(&self) -> usize {
        self.slots.iter().filter(|mode| mode.allows_start()).count()
    }
}

impl Default for WeeklySchedule {
    fn default() -> Self {
        Self::full_speed()
    }
}

/// Why a serialized schedule could not be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// The policy did not contain exactly seven days of hourly slots.
    #[error("weekly schedule needs {expected} hourly slots, got {actual}")]
    WrongSlotCount { expected: usize, actual: usize },
}

#[cfg(test)]
mod tests {
    use super::{ScheduleMode, ScheduleSlot, Weekday, WeeklySchedule, HOURS_PER_WEEK};

    #[test]
    fn default_schedule_allows_every_slot() {
        let schedule = WeeklySchedule::default();
        assert_eq!(schedule.open_slots(), HOURS_PER_WEEK);
        assert!(schedule.allows_start(ScheduleSlot::new(Weekday::Sunday, 23).unwrap()));
    }

    #[test]
    fn slots_are_monday_first_and_hour_addressable() {
        let mut schedule = WeeklySchedule::full_speed();
        let monday_midnight = ScheduleSlot::new(Weekday::Monday, 0).unwrap();
        let sunday_evening = ScheduleSlot::new(Weekday::Sunday, 23).unwrap();
        schedule.set_mode(monday_midnight, ScheduleMode::Paused);
        schedule.set_mode(sunday_evening, ScheduleMode::Paused);

        assert_eq!(schedule.mode_at(monday_midnight), ScheduleMode::Paused);
        assert_eq!(schedule.mode_at(sunday_evening), ScheduleMode::Paused);
        assert!(schedule.allows_start(ScheduleSlot::new(Weekday::Monday, 1).unwrap()));
        assert_eq!(schedule.open_slots(), HOURS_PER_WEEK - 2);
    }

    #[test]
    fn slot_constructor_rejects_the_next_day_boundary() {
        assert!(ScheduleSlot::new(Weekday::Monday, 24).is_none());
    }

    #[test]
    fn malformed_serialized_slot_rejects_the_next_day_boundary() {
        let error = serde_json::from_str::<ScheduleSlot>(r#"{"weekday":"monday","hour":24}"#)
            .expect_err("a persisted slot at hour 24 must not wrap to tomorrow");
        assert!(error.to_string().contains("below 24"));
    }

    #[test]
    fn malformed_slot_lists_are_rejected_without_padding() {
        let error = WeeklySchedule::from_slots(vec![ScheduleMode::Paused; HOURS_PER_WEEK - 1])
            .expect_err("a short policy must not be silently padded");
        assert_eq!(
            error.to_string(),
            "weekly schedule needs 168 hourly slots, got 167"
        );
    }

    #[test]
    fn malformed_serialized_slot_lists_are_rejected_without_padding() {
        let slots = serde_json::to_string(&vec![ScheduleMode::Paused; HOURS_PER_WEEK - 1])
            .expect("serialize short slots");
        let json = format!(r#"{{"slots":{slots}}}"#);
        let error = serde_json::from_str::<WeeklySchedule>(&json)
            .expect_err("a short persisted policy must not be accepted");
        assert!(error.to_string().contains("168"));
    }

    #[test]
    fn schedule_round_trips_as_a_stable_grid() {
        let mut schedule = WeeklySchedule::paused();
        schedule.set_mode(
            ScheduleSlot::new(Weekday::Wednesday, 9).unwrap(),
            ScheduleMode::FullSpeed,
        );
        let json = serde_json::to_string(&schedule).expect("serialize schedule");
        let restored: WeeklySchedule = serde_json::from_str(&json).expect("deserialize schedule");
        assert_eq!(restored, schedule);
    }

    #[test]
    fn every_weekday_has_a_stable_index() {
        for index in 0..7 {
            assert_eq!(Weekday::from_index(index).unwrap().index(), index);
        }
        assert_eq!(Weekday::from_index(7), None);
    }
}
