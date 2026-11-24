//! Session-only overrides of the avatar's first sparse stats map.
use crate::protocol::messages::OwnHomeData;
use anyhow::{ensure, Result};
use std::{collections::BTreeMap, sync::OnceLock};

pub const EXP_LEVEL: u32 = 1_900_002;
pub const EXP_POINTS: u32 = 1_900_003;

/// Read fields.csv named rows from the same bundled inputs as other unlocks.
pub fn fields_unlocked(level: u32) -> u32 {
    crate::game::starter::fields_unlocked(level)
}

pub fn tutorial_words(rows: &[u32]) -> Result<[u32; 6]> {
    let mut words = [0u32; 6];
    for &row in rows {
        ensure!(row < 174, "tutorial row must be 0..173 (got {row})");
        words[(row >> 5) as usize] |= 1 << (row & 31);
    }
    Ok(words)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Overrides {
    pub level: Option<u32>,
    pub xp: Option<u32>,
    pub tutorial_words: Option<[u32; 6]>,
}

// XP thresholds transcribed from the target APK's CSV tables.
// Read the reduced table at runtime so it stays out of the executable.
fn requirements() -> &'static [u32] {
    static TABLE: OnceLock<Vec<u32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let csv = crate::data::resource_data::load("exp-levels.csv");
        csv.lines()
            .skip(1)
            .enumerate()
            .map(|(index, line)| {
                let (level, xp) = line.split_once(',').expect("bundled level row");
                assert_eq!(level.parse::<usize>().unwrap(), index + 1);
                xp.parse().expect("bundled XP requirement")
            })
            .collect()
    })
}

pub fn validate_stats(stats: &BTreeMap<u32, u32>) -> Result<()> {
    ensure!(
        stats
            .iter()
            .all(|(&id, &value)| id <= i32::MAX as u32 && value <= i32::MAX as u32),
        "avatar stat quantities must fit a nonnegative int32"
    );
    let level = stats.get(&EXP_LEVEL).copied().unwrap_or(1);
    let xp = stats.get(&EXP_POINTS).copied().unwrap_or(0);
    ensure!(
        (1..=requirements().len() as u32).contains(&level),
        "level must be in 1..={} for build 1598 (got {level})",
        requirements().len()
    );
    let required = requirements()[level as usize - 1];
    ensure!(
        xp < required,
        "within-level XP must be below {required} at level {level} (got {xp}); no automatic level-up is performed"
    );
    Ok(())
}

impl Overrides {
    pub fn is_empty(self) -> bool {
        self.level.is_none() && self.xp.is_none() && self.tutorial_words.is_none()
    }

    pub fn check_after_ready(self, specs: &[String]) -> Result<()> {
        ensure!(
            self.is_empty() || specs.is_empty(),
            "--xp/--level (including TOML defaults) cannot be combined with explicit --after-ready; omit --after-ready to use generated account homes"
        );
        Ok(())
    }

    pub fn apply(self, mut home: OwnHomeData) -> Result<OwnHomeData> {
        for (id, value) in [(EXP_LEVEL, self.level), (EXP_POINTS, self.xp)] {
            if let Some(value) = value {
                home.avatar_stats.insert(id, value);
            }
        }
        if let Some(words) = self.tutorial_words {
            home.tutorial_words = words;
        }
        let level = home.avatar_stats.get(&EXP_LEVEL).copied().unwrap_or(1);
        home.fields_unlocked = fields_unlocked(level);
        // Type 33 is generated-order history, not a level unlock. Recomputing
        // shop unlocks must preserve it and the other independent bitsets.
        let independent = |kind: &u32| [16, 33, 36, 51, 80, 115, 120, 133, 148].contains(kind);
        if home.unlocked.keys().any(|kind| !independent(kind)) {
            let saved: BTreeMap<_, _> = home
                .unlocked
                .iter()
                .filter(|(kind, _)| independent(kind))
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            home.unlocked = crate::game::starter::unlocks(level);
            home.unlocked.extend(saved);
        }
        crate::game::starter::complete_tutorial_farm(&mut home.blobs[2], home.tutorial_words);
        validate_stats(&home.avatar_stats)?;
        Ok(home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_override_preserves_predefined_order_bits() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.predefined_orders = 7;
        let message = Overrides {
            level: Some(5),
            ..Default::default()
        }
        .apply(home.message())
        .unwrap();
        assert_eq!(message.unlocked[&33], [7]);
        let mut raw = OwnHomeData::default();
        raw.unlocked.insert(33, vec![3]);
        assert_eq!(
            Overrides::default().apply(raw).unwrap().unlocked,
            BTreeMap::from([(33, vec![3])])
        );
    }
    #[test]
    fn tutorial_bitmap_uses_rows_as_type36_bits() {
        assert_eq!(
            tutorial_words(&[0, 31, 32, 173]).unwrap(),
            [0x80000001, 1, 0, 0, 0, 1 << 13]
        );
        assert_eq!(
            tutorial_words(&(0..174).collect::<Vec<_>>()).unwrap(),
            [u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0x00003fff]
        );
    }

    #[test]
    fn fields_unlock_by_explicit_unlock_level() {
        assert_eq!(fields_unlocked(1), 0x00000003);
        assert_eq!(fields_unlocked(2), 0x00000007);
        assert_eq!(fields_unlocked(5), 0x0000000f);
    }

    #[test]
    fn bundled_requirements_and_invalid_progress() {
        assert_eq!(requirements().len(), 700);
        assert_eq!(&requirements()[..5], &[27, 7, 14, 30, 50]);
        assert_eq!(requirements()[699], 9_107_000);
        for (level, xp) in [
            (0, 0),
            (701, 0),
            (1, 27),
            (2, 7),
            (5, 50),
            (700, 9_107_000),
            (1, u32::MAX),
        ] {
            assert!(Overrides {
                level: Some(level),
                xp: Some(xp),
                tutorial_words: None,
            }
            .apply(OwnHomeData::default())
            .is_err());
        }
        for (level, xp) in [(1, 0), (1, 26), (5, 30), (700, 9_106_999)] {
            Overrides {
                level: Some(level),
                xp: Some(xp),
                tutorial_words: None,
            }
            .apply(OwnHomeData::default())
            .unwrap();
        }
    }

    #[test]
    fn merge_preserves_unspecified_and_unrelated_stats() {
        let mut home = OwnHomeData {
            avatar_stats: BTreeMap::from([(1_900_000, 123), (EXP_LEVEL, 5), (EXP_POINTS, 7)]),
            ..OwnHomeData::default()
        };
        home.fields_unlocked = fields_unlocked(5);
        assert_eq!(Overrides::default().apply(home.clone()).unwrap(), home);
        let xp = Overrides {
            xp: Some(0),
            ..Overrides::default()
        }
        .apply(home.clone())
        .unwrap();
        assert_eq!(
            xp.avatar_stats,
            BTreeMap::from([(1_900_000, 123), (EXP_LEVEL, 5), (EXP_POINTS, 0)])
        );
        let level = Overrides {
            level: Some(4),
            ..Overrides::default()
        }
        .apply(home.clone())
        .unwrap();
        assert_eq!(level.avatar_stats[&EXP_POINTS], 7);
        assert_eq!(level.avatar_stats[&EXP_LEVEL], 4);
        // A lower level with incompatible retained XP fails; XP is not reset.
        assert!(Overrides {
            level: Some(2),
            ..Overrides::default()
        }
        .apply(home)
        .is_err());
    }
}
