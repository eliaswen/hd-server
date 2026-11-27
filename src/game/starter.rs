//! Build-1598 table inputs, reduced from APK CSVs.
//! Farm generation and unlock rules are implemented here; no prebuilt farm or wire blob.
use crate::game::home::HomeData;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Deserialize)]
pub struct Table {
    kind: u32,
    rows: Vec<BTreeMap<String, String>>,
}
/// Parse the cached tables now (see `Server::start`).
pub(crate) fn preload() {
    tables();
    layout();
}

pub fn tables() -> &'static Vec<Table> {
    static DATA: OnceLock<Vec<Table>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("starter/tables.json")).unwrap()
    })
}
/// Type, table, named rows, physical rows, word count. Positions are cumulative
/// within the encoded bitset layout, relative to the avatar's fixed prefix.
pub type LayoutRow = (u32, String, usize, usize, usize);

pub fn layout() -> &'static Vec<LayoutRow> {
    static DATA: OnceLock<Vec<LayoutRow>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("starter/layout.json")).unwrap()
    })
}
pub fn unlocks(level: u32) -> BTreeMap<u32, Vec<u32>> {
    let mut result = BTreeMap::new();
    for (kind, _, _, _, count) in layout() {
        if [16, 33, 36, 51, 80, 115, 120, 133, 148].contains(kind) {
            continue;
        }
        let table = tables().iter().find(|t| t.kind == *kind).unwrap();
        if !table
            .rows
            .first()
            .is_some_and(|r| r.contains_key("UnlockLevel"))
        {
            continue;
        }
        let words = table_words(table, *count, level);
        if *kind != 4 && words.iter().any(|w| *w != 0) {
            result.insert(*kind, words);
        }
    }
    result
}
fn table_words(table: &Table, count: usize, level: u32) -> Vec<u32> {
    let mut words = vec![0; count];
    for (row, data) in table.rows.iter().enumerate() {
        if (table.kind == 4 && row == 0)
            || data["UnlockLevel"]
                .trim()
                .parse::<i32>()
                .is_ok_and(|n| i64::from(n) <= i64::from(level))
        {
            words[row / 32] |= 1 << (row % 32);
        }
    }
    words
}
pub fn fields_unlocked(level: u32) -> u32 {
    let table = tables().iter().find(|t| t.kind == 4).unwrap();
    table_words(table, 1, level)[0]
}
pub(crate) fn resolve(name: &str) -> Result<(u32, &BTreeMap<String, String>)> {
    let mut matches = tables().iter().flat_map(|t| {
        t.rows.iter().enumerate().filter_map(move |(row, r)| {
            (r["Name"].trim() == name).then_some((t.kind * 100000 + row as u32, r))
        })
    });
    let found = matches
        .next()
        .with_context(|| format!("unresolved starter name {name}"))?;
    ensure!(matches.next().is_none(), "ambiguous starter name {name}");
    Ok(found)
}
pub fn new_home() -> Result<HomeData> {
    let records: Vec<Vec<String>> =
        serde_json::from_str(&crate::data::resource_data::load("starter/level_0.json"))?;
    let mut active = false;
    let mut group = "";
    let mut header: Option<&Vec<String>> = None;
    let mut types: Option<&Vec<String>> = None;
    let mut objects = Vec::new();
    for r in &records {
        let cell = |i: usize| r.get(i).map(String::as_str).unwrap_or("");
        if !cell(0).is_empty() {
            active = ["Objects", "Objects1", "Objects2"].contains(&cell(0));
            group = "";
            header = None;
            types = None;
            continue;
        }
        if !active {
            continue;
        }
        if !cell(1).is_empty() {
            group = cell(1);
            header = None;
            types = None;
            continue;
        }
        let Some(h) = header else {
            header = Some(r);
            continue;
        };
        let Some(ty) = types else {
            types = Some(r);
            continue;
        };
        if !r.iter().skip(2).any(|c| !c.trim().is_empty()) {
            continue;
        }
        let (id, definition) = resolve(group)?;
        let mut obj = Map::new();
        obj.insert("ID".into(), json!(id));
        for (i, key) in h.iter().enumerate().skip(2) {
            if key.is_empty() || cell(i).is_empty() {
                continue;
            }
            ensure!(
                ["X", "Y", "Mirrored"].contains(&key.as_str()),
                "unsupported starter column {key}"
            );
            let value = match ty.get(i).map(String::as_str) {
                Some("int") => json!(cell(i).parse::<i32>()?),
                Some("boolean") => json!(cell(i).eq_ignore_ascii_case("true")),
                _ => anyhow::bail!("unsupported starter column type"),
            };
            if key != "Mirrored" || value == true {
                obj.insert(key.clone(), value);
            }
        }
        if [1000000, 1400000, 1800000].contains(&id) {
            let count: usize = definition["BrokenPartCount"].parse()?;
            obj.insert("BrokenParts".into(), json!(vec![true; count]));
        }
        if id == 2100003 {
            obj.insert("TargetData".into(), json!(1100000));
        }
        objects.push(Value::Object(obj));
    }
    ensure!(objects.len() == 516, "starter must contain 516 objects");
    let mut home = HomeData::from_blobs([
        json!({}),
        json!({}),
        json!({"TileMapWidth":85,"TileMapHeight":71,"ObjectVersion":2,"Objects":objects}),
    ]);
    home.table_unlocks = true;
    home.command_replay = true;
    let config: Vec<Vec<String>> =
        serde_json::from_str(&crate::data::resource_data::load("starter/game_config.json"))?;
    let mut initial = false;
    for row in config {
        if !row[0].is_empty() {
            initial = row[0] == "InitialAvatar";
        }
        if initial {
            let (id, _) = resolve(&row[2])?;
            home.avatar_stats.insert(id, row[1].parse()?);
        }
    }
    // A missing/zero raw level merely displays as 1 through the client getter's
    // clamp. Initialize the stat itself for the first in-session level-up.
    home.avatar_stats.insert(crate::game::progression::EXP_LEVEL, 1);
    home.avatar_stats.insert(crate::game::progression::EXP_POINTS, 0);
    crate::game::home::merge_free_balances(&mut home.avatar_stats);
    home.validate()?;
    Ok(home)
}
/// Called only on a message copy, or while explicitly initializing a new home.
pub fn complete_tutorial_farm(c: &mut Value, words: [u32; 6]) {
    if words != [u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0x3fff] {
        return;
    }
    if let Some(objects) = c.get_mut("Objects").and_then(Value::as_array_mut) {
        for obj in objects {
            if obj["ID"] == 2100003 {
                obj["ID"] = json!(1100000);
                obj.as_object_mut().unwrap().remove("TargetData");
                obj.as_object_mut().unwrap().remove("ConstructionTimer");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game::progression::Overrides,
        protocol::{messages::OwnHomeData, Message},
    };
    #[test]
    fn starter_sends_explicit_level_before_first_rollover() {
        let mut home = new_home().unwrap();
        let message = OwnHomeData::from_bytes(&home.message().to_bytes()).unwrap();
        assert_eq!(message.avatar_stats[&1900002], 1);
        assert_eq!(message.avatar_stats[&1900003], 0);
        crate::replay::award_xp(&mut home.avatar_stats, 27).unwrap();
        let message = OwnHomeData::from_bytes(&home.message().to_bytes()).unwrap();
        assert_eq!(message.avatar_stats[&1900002], 2);
        assert_eq!(message.avatar_stats[&1900003], 0);
        assert_eq!(message.avatar_stats[&1900000], 435);
        assert_eq!(message.avatar_stats[&400002], 4);
    }
    #[test]
    fn starter_farm_initial_inventory() {
        let home = new_home().unwrap();
        assert_eq!(home.c["Objects"].as_array().unwrap().len(), 516);
        assert_eq!(
            home.avatar_stats,
            BTreeMap::from([
                (1600000, 9),
                (1900002, 1),
                (1900003, 0),
                (1900001, 30),
                (1900000, 350)
            ])
        );
        assert_eq!(
            OwnHomeData::from_bytes(&home.message().to_bytes()).unwrap(),
            home.message()
        );
    }
    #[test]
    fn layout_and_unlock_rules() {
        assert_eq!(layout().len(), 63);
        assert_eq!(layout().iter().map(|x| x.4 * 4).sum::<usize>(), 340);
        let home = new_home().unwrap();
        assert_eq!(home.message().unlocked[&13], vec![1]);
        for kind in [16, 33, 36, 51, 80, 115, 120, 133, 148] {
            assert!(!unlocks(700).contains_key(&kind));
        }
        for level in [1, 2, 4, 9] {
            let msg = Overrides {
                level: Some(level),
                ..Overrides::default()
            }
            .apply(home.message())
            .unwrap();
            assert_eq!(msg.unlocked, unlocks(level));
            assert_eq!(OwnHomeData::from_bytes(&msg.to_bytes()).unwrap(), msg);
        }
        let mut complete = home;
        complete.tutorial_words =
            crate::game::progression::tutorial_words(&(0..174).collect::<Vec<_>>()).unwrap();
        complete_tutorial_farm(&mut complete.c, complete.tutorial_words);
        assert!(!complete.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["ID"] == 2100003));
        assert!(complete.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["ID"] == 1100000 && o["X"] == 13 && o["Y"] == 21));
    }
}
