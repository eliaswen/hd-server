//! Persistent JSON representation; wire bytes stay in protocol::messages.
use crate::protocol::{messages::OwnHomeData, Message};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeData {
    /// Requested farm name. Its location in the outgoing avatar is unconfirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm_name: Option<String>,
    /// Server replay metadata for documented effects whose client JSON keys are unknown.
    #[serde(
        default = "empty_replay_state",
        skip_serializing_if = "replay_state_empty"
    )]
    pub replay_state: Value,
    /// Command-33 acknowledgement. Server metadata: no client JSON state key is known.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub order_board_unlocked: bool,
    /// Type-33 predefined-order completion bits (not generic level unlocks).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub predefined_orders: u32,
    /// Server-only classification needed for active-special-order limits.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub special_order_slots: std::collections::BTreeSet<usize>,
    /// Server-only timer snapshot clock; omitted on legacy/non-timed homes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timer_saved_at_ms: Option<u64>,
    /// Legacy overrides retained for file compatibility; currently ignored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub construction_speed_up_costs: BTreeMap<u32, u32>,
    /// New homes replay supported gameplay commands; old homes explicitly opt in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub command_replay: bool,
    /// Opt-in table unlocks; absent on legacy homes to preserve their wire state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub table_unlocks: bool,
    /// Optional server metadata for binary avatar stats; never a blob JSON key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub avatar_stats: BTreeMap<u32, u32>,
    /// Tutorial data type 36 completion bitmap; stored as six BE32 words.
    #[serde(default, skip_serializing_if = "tutorial_words_are_empty")]
    pub tutorial_words: [u32; 6],
    pub a: Value,
    pub b: Value,
    pub c: Value,
}
fn empty_replay_state() -> Value {
    serde_json::json!({})
}
fn replay_state_empty(v: &Value) -> bool {
    v.as_object().is_none_or(|m| m.is_empty())
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}
fn tutorial_words_are_empty(words: &[u32; 6]) -> bool {
    words == &[0; 6]
}
impl Default for HomeData {
    fn default() -> Self {
        Self::from_blobs(OwnHomeData::default().blobs)
    }
}
/// Plain cash: the only gold balance the client HUD displays.
pub const CASH: u32 = 1900000;
/// InitialAvatar's FreeCash counter. The client ignores it for display, so the
/// server keeps all gold in [`CASH`] and folds this counter into it.
pub const FREE_CASH: u32 = 1900131;
/// Plain diamonds: the diamond balance the client HUD displays.
pub const DIAMONDS: u32 = 1900001;
/// FreeDiamonds counter. Like FreeCash, the server folds it into [`DIAMONDS`]
/// so there is a single diamond balance (paid/free split is irrelevant here).
pub const FREE_DIAMONDS: u32 = 1900015;
/// Map a "free" currency counter to the single balance it is stored in.
pub fn balance_id(id: u32) -> u32 {
    match id {
        FREE_CASH => CASH,
        FREE_DIAMONDS => DIAMONDS,
        other => other,
    }
}
/// Move FreeCash/FreeDiamonds into plain cash/diamonds (saturating at the VInt range).
pub fn merge_free_balances(stats: &mut BTreeMap<u32, u32>) {
    for (free_id, plain_id) in [(FREE_CASH, CASH), (FREE_DIAMONDS, DIAMONDS)] {
        if let Some(free) = stats.remove(&free_id) {
            let plain = stats.entry(plain_id).or_insert(0);
            *plain = (u64::from(*plain) + u64::from(free)).min(i32::MAX as u64) as u32;
        }
    }
}
impl HomeData {
    pub fn from_blobs([a, b, c]: [Value; 3]) -> Self {
        Self {
            farm_name: None,
            replay_state: empty_replay_state(),
            order_board_unlocked: false,
            predefined_orders: 0,
            special_order_slots: Default::default(),
            timer_saved_at_ms: None,
            construction_speed_up_costs: BTreeMap::new(),
            command_replay: false,
            table_unlocks: false,
            avatar_stats: BTreeMap::new(),
            tutorial_words: [0; 6],
            a,
            b,
            c,
        }
    }
    pub fn blobs(&self) -> [Value; 3] {
        [self.a.clone(), self.b.clone(), self.c.clone()]
    }
    pub fn load(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading home {}", path.display()))?;
        let mut home: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid home JSON {} (requires a, b, c)", path.display()))?;
        // Migrates older saves; persisted by the next write of this home.
        merge_free_balances(&mut home.avatar_stats);
        home.validate()?;
        Ok(home)
    }
    pub fn validate(&self) -> Result<()> {
        crate::game::progression::validate_stats(&self.avatar_stats)?;
        ensure!(
            self.construction_speed_up_costs
                .iter()
                .all(|(&id, &cost)| id <= i32::MAX as u32 && cost <= i32::MAX as u32),
            "construction speed-up costs must fit nonnegative int32"
        );
        ensure!(
            self.a.is_object() && self.b.is_object() && self.c.is_object(),
            "home a, b and c must be JSON objects"
        );
        for v in [&self.a, &self.b, &self.c] {
            ensure!(
                serde_json::to_vec(v)?.len() <= 1 << 20,
                "home JSON blob exceeds 1 MiB"
            );
        }
        let body = self.message().to_bytes();
        ensure!(
            body.len() <= crate::net::connection::MAX_PAYLOAD,
            "encoded home exceeds frame limit"
        );
        ensure!(
            OwnHomeData::from_bytes(&body)? == self.message(),
            "home encoding did not round-trip"
        );
        Ok(())
    }
    pub fn message(&self) -> OwnHomeData {
        let level = self
            .avatar_stats
            .get(&crate::game::progression::EXP_LEVEL)
            .copied()
            .unwrap_or(1);
        let mut message = OwnHomeData {
            unlocked: if self.table_unlocks {
                crate::game::starter::unlocks(level)
            } else {
                BTreeMap::new()
            },
            avatar_stats: self.avatar_stats.clone(),
            fields_unlocked: crate::game::progression::fields_unlocked(level),
            tutorial_words: self.tutorial_words,
            unknown_avatar_vector_count: 0,
            blobs: self.blobs(),
        };
        if self.predefined_orders != 0 {
            message.unlocked.insert(33, vec![self.predefined_orders]);
        }
        if let Some(bits) = self.replay_state["Bits"].as_object() {
            for (kind, words) in bits {
                if let (Ok(kind), Ok(words)) = (
                    kind.parse::<u32>(),
                    serde_json::from_value::<Vec<u32>>(words.clone()),
                ) {
                    if words.iter().any(|w| *w != 0) {
                        message.unlocked.insert(kind, words);
                    } else {
                        message.unlocked.remove(&kind);
                    }
                }
            }
        }
        message
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_home_round_trip() {
        let home = HomeData::default();
        home.validate().unwrap();
        assert_eq!(
            OwnHomeData::from_bytes(&home.message().to_bytes()).unwrap(),
            home.message()
        );
        let json = serde_json::to_vec(&home).unwrap();
        assert_eq!(serde_json::from_slice::<HomeData>(&json).unwrap(), home);
        assert!(!serde_json::to_value(&home)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("avatar_stats"));
    }

    #[test]
    fn stored_avatar_stats_round_trip_without_changing_blobs() {
        let home = HomeData {
            avatar_stats: BTreeMap::from([(1_900_000, 100), (1_900_002, 5), (1_900_003, 30)]),
            ..HomeData::default()
        };
        home.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<HomeData>(serde_json::to_value(&home).unwrap()).unwrap(),
            home
        );
        assert_eq!(
            OwnHomeData::from_bytes(&home.message().to_bytes()).unwrap(),
            home.message()
        );
        let overridden = crate::game::progression::Overrides {
            level: None,
            xp: Some(7),
            tutorial_words: None,
        }
        .apply(home.message())
        .unwrap();
        assert_eq!(overridden.blobs, home.blobs());
        assert_eq!(overridden.avatar_stats[&1_900_000], 100);
        assert_eq!(overridden.avatar_stats[&1_900_002], 5);
        assert_eq!(home.avatar_stats[&1_900_003], 30);
    }
}
