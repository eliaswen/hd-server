//! Development replay: skip unsupported/failed effects and keep replaying.
//! Instance serials belong to a loaded simulation, never to the account on disk.
use crate::{
    game::home::HomeData,
    protocol::messages::{EndClientTurn, TurnCommand},
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};
mod extended;
pub(crate) mod orders;
mod production;
mod timers;
mod visitors;

type Row = BTreeMap<String, String>;
fn tables() -> &'static BTreeMap<String, Vec<Row>> {
    static DATA: OnceLock<BTreeMap<String, Vec<Row>>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("replay-tables.json")).unwrap()
    })
}
fn number(row: &Row, key: &str) -> Result<u32> {
    match row.get(key).map(|v| v.trim()).filter(|s| !s.is_empty()) {
        Some(s) => s.parse().with_context(|| format!("invalid {key}: {s}")),
        None => Ok(0),
    }
}
fn type_tables() -> &'static BTreeMap<u32, String> {
    static DATA: OnceLock<BTreeMap<u32, String>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("replay-type-tables.json")).unwrap()
    })
}
fn replay_config() -> &'static BTreeMap<String, u32> {
    static CONFIG: OnceLock<BTreeMap<String, u32>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("replay-config.json")).unwrap()
    })
}
fn duration(row: &Row) -> Result<u32> {
    number(row, "TimeMin")?
        .checked_mul(60)
        .and_then(|s| s.checked_add(number(row, "TimeSec").ok()?))
        .context("production duration overflow")
}
fn remove(stats: &mut BTreeMap<u32, u32>, id: u32, amount: u32) -> Result<()> {
    let held = stats.get(&id).copied().unwrap_or(0);
    ensure!(
        held >= amount,
        "insufficient inventory for data ID {id}: need {amount}, have {held}"
    );
    stats.insert(id, held - amount);
    Ok(())
}
fn spend_diamonds(stats: &mut BTreeMap<u32, u32>, amount: u32) -> Result<()> {
    use crate::game::home::{merge_free_balances, DIAMONDS};
    merge_free_balances(stats);
    let held = stats.get(&DIAMONDS).copied().unwrap_or(0);
    ensure!(held >= amount, "insufficient diamonds");
    stats.insert(DIAMONDS, held - amount);
    Ok(())
}
fn instant_cost(home: &HomeData, good: u32, timer: &Value) -> Result<u32> {
    let config = replay_config();
    if home.avatar_stats.get(&1900002).copied().unwrap_or(1)
        < config["FREE_DIAMOND_SPEED_UP_BEFORE_LEVEL"]
    {
        return Ok(0);
    }
    let row = definition(good)?;
    let total = u64::from(duration(row)?.max(1));
    let remaining = timers::remaining(timer) * 66 / 1000;
    let cost = (u64::from(number(row, "InstantComplete")?) * remaining)
        .div_ceil(total)
        .max(1);
    u32::try_from(cost).context("diamond cost overflow")
}
fn definition(id: u32) -> Result<&'static Row> {
    let table = type_tables()
        .get(&(id / 100000))
        .context("no replay definition for data type")?;
    tables()[table]
        .iter()
        .filter(|r| r.get("Name").is_some_and(|s| !s.is_empty()))
        .nth((id % 100000) as usize)
        .context("data row out of range")
}
fn level_row(level: u32) -> Result<&'static Row> {
    tables()["exp_levels"]
        .iter()
        .find(|r| r["Level"].parse::<u32>().ok() == Some(level))
        .context("level missing from table")
}
// Dedicated rows override the smallest fitting generic footprint.
fn construction_site(building: &Row) -> Result<u32> {
    let width = number(building, "TileWidth")?;
    let height = number(building, "TileHeight")?;
    let mut best = None;
    for (index, row) in tables()["construction_buildings"]
        .iter()
        .filter(|r| r.get("Name").is_some_and(|s| !s.is_empty()))
        .enumerate()
    {
        let id = 2100000 + index as u32;
        if let Some(target) = row.get("OnlyForBuilding").filter(|s| !s.is_empty()) {
            if building.get("Name") == Some(target) {
                return Ok(id);
            }
            continue;
        }
        let w = number(row, "TileWidth")?;
        let h = number(row, "TileHeight")?;
        if w >= width && h >= height {
            let spare = u64::from(w) * u64::from(h) - u64::from(width) * u64::from(height);
            if best.is_none_or(|(area, _)| spare < area) {
                best = Some((spare, id));
            }
        }
    }
    best.map(|(_, id)| id)
        .context("no construction site fits building")
}
fn add(stats: &mut BTreeMap<u32, u32>, id: u32, amount: u32) -> Result<()> {
    // Gold and diamonds each live in one plain balance; see merge_free_balances.
    let id = crate::game::home::balance_id(id);
    let next = stats
        .get(&id)
        .copied()
        .unwrap_or(0)
        .checked_add(amount)
        .context("stat overflow")?;
    ensure!(next <= i32::MAX as u32, "stat overflow");
    stats.insert(id, next);
    Ok(())
}
fn grant(stats: &mut BTreeMap<u32, u32>, id: u32, amount: u32) -> Result<()> {
    add(stats, id, amount)?;
    Ok(())
}
fn spend(stats: &mut BTreeMap<u32, u32>, amount: u32) -> Result<()> {
    use crate::game::home::{merge_free_balances, CASH};
    merge_free_balances(stats);
    let cash = stats.get(&CASH).copied().unwrap_or(0);
    ensure!(cash >= amount, "insufficient cash");
    stats.insert(CASH, cash - amount);
    Ok(())
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
/// Prepare a copy at delivery time. Stored homes are not silently rewritten
/// on login; the next replayed turn saves the newly advanced timer snapshot.
pub fn elapsed_home(mut home: HomeData, now: u64) -> HomeData {
    if let Some(saved) = home.timer_saved_at_ms {
        let ticks = now.saturating_sub(saved) / 66;
        if let Some(objects) = home.c.get_mut("Objects").and_then(Value::as_array_mut) {
            for object in objects {
                timers::advance_object(object, ticks);
            }
        }
        if let Some(states) = home.replay_state["ObjectStates"].as_array_mut() {
            for state in states {
                timers::advance_state(state, ticks);
            }
        }
        home.timer_saved_at_ms = Some(saved.saturating_add(ticks * 66));
    }
    home
}

/// Base XP rollover and level rewards. Threshold bonuses remain unmodeled
/// but must not prevent the documented base XP or its ordinary rewards.
pub fn award_xp(stats: &mut BTreeMap<u32, u32>, amount: u32) -> Result<()> {
    let mut next = stats.clone();
    let mut level = next.get(&1900002).copied().unwrap_or(1);
    let mut xp = next
        .get(&1900003)
        .copied()
        .unwrap_or(0)
        .checked_add(amount)
        .context("XP overflow")?;
    if amount == 0 {
        return Ok(());
    }
    loop {
        let row = level_row(level)?;
        if number(row, "LevelThresholds")? != 0 {
            tracing::debug!(
                level,
                "applying base XP; threshold reward bonuses remain unmodeled"
            );
        }
        let requirement = number(row, "ExpToNextLevel")?;
        ensure!(requirement > 0, "invalid XP requirement");
        if xp < requirement {
            break;
        }
        ensure!(level < 700, "maximum level reached");
        xp -= requirement;
        level += 1;
        let rows = &tables()["exp_levels"];
        let start = rows
            .iter()
            .position(|r| r["Level"].parse::<u32>().ok() == Some(level))
            .context("missing next level")?;
        for (i, reward) in rows[start..].iter().enumerate() {
            if i > 0 && !reward["Level"].is_empty() {
                break;
            }
            if !reward["GoodReward"].is_empty() {
                let (id, _) = crate::game::starter::resolve(&reward["GoodReward"])?;
                if [17, 185].contains(&(id / 100000)) {
                    tracing::warn!(
                        level,
                        id,
                        "special level reward storage unknown; base XP and level rollover continue"
                    );
                    continue;
                }
                grant(&mut next, id, number(reward, "GoodAmount")?)?;
            }
        }
    }
    next.insert(1900002, level);
    next.insert(1900003, xp);
    crate::game::progression::validate_stats(&next)?;
    *stats = next;
    Ok(())
}
fn stat_changes(before: &BTreeMap<u32, u32>, after: &BTreeMap<u32, u32>) -> Vec<(u32, u32, u32)> {
    before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|id| {
            let old = before.get(&id).copied().unwrap_or(0);
            let new = after.get(&id).copied().unwrap_or(0);
            (old != new).then_some((id, old, new))
        })
        .collect()
}
#[derive(Clone, Debug)]
struct Object {
    instance: u32,
    value: Value,
    harvested: bool,
    state: Value,
}
#[derive(Clone, Debug)]
pub struct Replay {
    visitors: visitors::Visitors,
    extra: Value,
    warnings: Vec<String>,
    order_board_unlocked: bool,
    predefined_orders: u32,
    special_order_slots: BTreeSet<usize>,
    objects: Vec<Object>,
    next: BTreeMap<u32, u32>,
    seen: BTreeSet<Vec<u8>>,
    last_turn: Option<u32>,
    clock_tick: u32,
    clock_fraction: u64,
}
impl Replay {
    pub fn new(home: &HomeData) -> Result<Self> {
        let values = home
            .c
            .get("Objects")
            .and_then(Value::as_array)
            .context("home has no Objects array")?;
        let mut replay = Self {
            visitors: visitors::Visitors::load(home),
            extra: home.replay_state.clone(),
            warnings: Vec::new(),
            order_board_unlocked: home.order_board_unlocked,
            predefined_orders: home.predefined_orders,
            special_order_slots: home.special_order_slots.clone(),
            objects: Vec::new(),
            next: BTreeMap::new(),
            seen: BTreeSet::new(),
            last_turn: None,
            clock_tick: 0,
            clock_fraction: 0,
        };
        for (index, value) in values.iter().enumerate() {
            replay.create(value.clone())?;
            if let Some(state) = home.replay_state["ObjectStates"].get(index) {
                replay.objects.last_mut().unwrap().state = state.clone();
            }
        }
        Ok(replay)
    }
    fn advance_to(&mut self, tick: u32) {
        if tick <= self.clock_tick {
            return;
        }
        // ticks * 1000/(30*66); carry the fractional 66-ms unit between calls.
        let numerator = u64::from(tick - self.clock_tick) * 1000 + self.clock_fraction;
        let elapsed = numerator / (30 * 66);
        self.clock_fraction = numerator % (30 * 66);
        self.clock_tick = tick;
        for object in &mut self.objects {
            timers::advance_object(&mut object.value, elapsed);
            timers::advance_state(&mut object.state, elapsed);
        }
    }
    fn create(&mut self, value: Value) -> Result<u32> {
        let id = value["ID"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .context("invalid object data ID")?;
        let class = id / 100000;
        let serial = self.next.entry(class).or_default();
        ensure!(*serial < 100000, "instance serial exhausted");
        let instance = class
            .checked_mul(100000)
            .and_then(|n| n.checked_add(*serial))
            .context("instance overflow")?;
        *serial += 1;
        tracing::trace!(
            instance_id = instance,
            data_id = id,
            next_serial = *serial,
            "allocated object instance in candidate simulation"
        );
        self.objects.push(Object {
            instance,
            value,
            harvested: false,
            state: json!({}),
        });
        Ok(instance)
    }
    fn position(&self, id: i32) -> Result<usize> {
        self.objects
            .iter()
            .position(|o| o.instance == id as u32)
            .with_context(|| format!("instance {id} not found"))
    }
    fn replace(&mut self, idx: usize, id: u32) -> Result<()> {
        let mut value = self.objects.remove(idx).value;
        let obj = value.as_object_mut().context("invalid farm object")?;
        obj.retain(|k, _| ["X", "Y", "Mirrored"].contains(&k.as_str()));
        obj.insert("ID".into(), json!(id));
        self.create(value)?;
        Ok(())
    }
    fn save_objects(&self, home: &mut HomeData) {
        home.replay_state = self.extra.clone();
        if self
            .objects
            .iter()
            .any(|o| o.state.as_object().is_some_and(|m| !m.is_empty()))
        {
            home.replay_state["ObjectStates"] =
                json!(self.objects.iter().map(|o| &o.state).collect::<Vec<_>>());
        } else if let Some(m) = home.replay_state.as_object_mut() {
            m.remove("ObjectStates");
        }
        home.c["Objects"] = json!(self
            .objects
            .iter()
            .map(|o| {
                let mut value = o.value.clone();
                // A reconnect between harvest and clear must not award the crop
                // twice. Snapshot a credited crop as an empty field; keep its old
                // instance live until command 23 performs the allocation.
                if o.harvested {
                    value["ID"] = json!(400000);
                }
                value
            })
            .collect::<Vec<_>>());
    }
    /// Work on candidates; the account store commits this session state only
    /// after its matching home replacement is durable.
    pub fn apply(&mut self, home: &mut HomeData, turn: &EndClientTurn, body: &[u8]) -> Vec<String> {
        if self.seen.contains(body) {
            tracing::debug!(
                turn_tick = turn.a,
                "duplicate turn ignored; no commands applied twice"
            );
            return vec!["duplicate turn ignored".into()];
        }
        let mut notes = Vec::new();
        // Development policy: arrival order wins. No divergence or tick check
        // is allowed to disable this simulation's replay context.
        if self.last_turn.is_some_and(|last| turn.a <= last) {
            tracing::debug!(turn_tick=turn.a,last_tick=?self.last_turn,"non-increasing turn tick; continuing replay in arrival order");
        }
        for (command_index, command) in turn.decoded.iter().enumerate() {
            if let Some(tick) = command.tick() {
                self.advance_to(tick);
            }
            let mut candidate = self.clone();
            let mut next_home = home.clone();
            match candidate.execute(&mut next_home, command) {
                Ok(()) => {
                    for warning in candidate.warnings.drain(..) {
                        notes.push(format!("command_index={command_index} type={} ({}): {warning}; known effects applied, replay continues",command.command_type(),command.meaning()));
                    }
                    tracing::debug!(turn_tick=turn.a,command_index,command_type=command.command_type(),meaning=command.meaning(),
                        stats_changes=?stat_changes(&home.avatar_stats,&next_home.avatar_stats),
                        objects_before=self.objects.len(),objects_after=candidate.objects.len(),
                        "replayed command into candidate home; account store will commit the snapshot");
                    *self = candidate;
                    *home = next_home;
                }
                Err(error) => {
                    let reason = format!(
                        "command_index={command_index} type={} ({}): {error:#}",
                        command.command_type(),
                        command.meaning()
                    );
                    tracing::debug!(turn_tick=turn.a,command_index,command_type=command.command_type(),%reason,"command effects rolled back; skipping only this command and continuing replay");
                    notes.push(reason);
                }
            }
        }
        if turn.decoded.len() != turn.n as usize || !turn.raw_remainder.is_empty() {
            notes.push("unknown/truncated command or optional tail retained raw; replay continues on the next packet".into());
        }
        self.advance_to(turn.a);
        // A timer-only turn can make an empty order slot ready too.
        let mut board_candidate = self.clone();
        let mut board_home = home.clone();
        match board_candidate
            .fill_orders(&mut board_home)
            .and_then(|_| board_candidate.tick_visitors(&board_home))
        {
            Ok(()) => {
                notes.append(&mut board_candidate.warnings);
                *self = board_candidate;
                *home = board_home;
            }
            Err(error) => notes.push(format!("order board generation skipped: {error:#}")),
        }
        self.save_objects(home);
        if home.timer_saved_at_ms.is_some()
            || self
                .objects
                .iter()
                .any(|o| timers::has_running(&o.value) || timers::remaining(&o.state["Timer"]) > 0)
        {
            home.timer_saved_at_ms = Some(now_ms());
        }
        self.seen.insert(body.to_vec());
        if self.seen.len() > 256 {
            self.seen.pop_first();
        }
        self.last_turn = Some(turn.a);
        notes
    }
    fn execute(&mut self, home: &mut HomeData, command: &TurnCommand) -> Result<()> {
        match command {
            TurnCommand::Harvest { object_id, .. } => {
                let i = self.position(*object_id)?;
                let o = &self.objects[i];
                ensure!(o.instance / 100000 == 4, "harvest requires a field");
                if o.harvested {
                    return Ok(());
                }
                let id = o.value["ID"].as_u64().unwrap() as u32;
                ensure!(id != 400000, "cannot harvest empty field");
                let row = definition(id)?;
                add(&mut home.avatar_stats, id, number(row, "Harvest")?)?;
                award_xp(&mut home.avatar_stats, number(row, "ExpCollect")?)?;
                self.objects[i].harvested = true;
            }
            TurnCommand::FollowUp22 { .. } => {}
            TurnCommand::FollowUp23 { object_id, .. } => {
                let i = self.position(*object_id)?;
                ensure!(
                    self.objects[i].instance / 100000 == 4 && self.objects[i].harvested,
                    "clear requires credited harvest"
                );
                self.replace(i, 400000)?;
            }
            TurnCommand::Plant {
                object_id, data_id, ..
            } => {
                let i = self.position(*object_id)?;
                ensure!(
                    self.objects[i].value["ID"] == 400000,
                    "plant requires EmptyField"
                );
                let id = u32::try_from(*data_id)?;
                ensure!(id / 100000 == 4 && id != 400000, "invalid crop");
                let row = definition(id)?;
                let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
                ensure!(
                    number(row, "UnlockLevel")? <= level,
                    "crop locked at current level"
                );
                let quantity = home.avatar_stats.get(&id).copied().unwrap_or(0);
                ensure!(quantity > 0, "no crop in silo for planting");
                home.avatar_stats.insert(id, quantity - 1);
                self.replace(i, id)?;
                // The client permits writing crops back ripe until E4.
            }
            TurnCommand::Schema {
                command_type,
                fields,
                ..
            } => {
                let int = |i: usize| -> Result<i32> {
                    fields
                        .get(i)
                        .and_then(Value::as_i64)
                        .and_then(|n| i32::try_from(n).ok())
                        .context("expected command integer")
                };
                match command_type {
                    27 => self.trash_order(home, int(0)?)?,
                    28 => self.complete_order(home, int(0)?)?,
                    29 => self.speed_up_order(home, int(0)?)?,
                    33 => self.acknowledge_order_unlock(home, int(0)?)?,
                    40 => self.complete_visitor(home, int(0)?)?,
                    // : pay and clear the persisted truck reward stash once.
                    41 => self.collect_order_reward(home)?,
                    53 => {
                        let i = self.position(int(0)?)?;
                        let object = self.objects[i]
                            .value
                            .as_object_mut()
                            .context("invalid farm object")?;
                        match object.get("Mirrored") {
                            None | Some(Value::Bool(false)) => {
                                object.insert("Mirrored".into(), json!(true));
                            }
                            Some(Value::Bool(true)) => {
                                object.remove("Mirrored");
                            }
                            _ => anyhow::bail!("invalid object mirrored flag"),
                        }
                    }
                    3 => {
                        // Captured move confirmed by the player: instance,
                        // fixed-point delta X/Y, flag, old X/Y, data ID, tick.
                        // The flag's meaning is unconfirmed; retain orientation.
                        if fields.get(3) == Some(&json!(true)) {
                            self.unknown("move from saved layout/storage recognized; layout-object lookup is untraced, ordinary move attempted only if instance exists");
                            if self.position(int(0)?).is_err() {
                                return Ok(());
                            }
                        }
                        let i = self.position(int(0)?)?;
                        let anchored = self.objects[i].value["X"].as_i64().unwrap_or(0)
                            == i64::from(int(4)?)
                            && self.objects[i].value["Y"].as_i64().unwrap_or(0)
                                == i64::from(int(5)?);
                        if !anchored {
                            self.unknown("move anchor differs from stored position; using  absolute fixed-point coordinates");
                        }
                        for (axis, field) in [("X", 1), ("Y", 2)] {
                            let delta = int(field)?;
                            ensure!(delta % 512 == 0, "move is not tile-aligned");
                            let old = self.objects[i].value[axis].as_i64().unwrap_or(0);
                            self.objects[i].value[axis] = json!(if anchored {
                                old + i64::from(delta / 512)
                            } else {
                                i64::from(delta.div_euclid(512))
                            });
                        }
                    }
                    9 => self.feed_animal(home, int(0)?)?,
                    10 => self.start_production(home, int(0)?, int(1)?)?,
                    12 => self.instant_production(home, int(0)?, int(1)?)?,
                    13 => self.collect_production(home, int(0)?)?,
                    14 => self.collect_animal(home, int(0)?)?,
                    15 => self.instant_animal(home, int(0)?)?,
                    16 => self.instant_construction(home, int(0)?)?,
                    21 | 24 => {}
                    25 => {
                        let i = self.position(int(0)?)?;
                        let id = u32::try_from(
                            self.objects[i].value["ID"]
                                .as_u64()
                                .context("paint object ID missing")?,
                        )?;
                        let row = definition(id)?;
                        // Client 0x1c4a6f -> 0x4bd5c6 reads BuildingFixExp;
                        // base loader 0x2fb813..0x2fb85a defaults to game_config.
                        let xp = if row.contains_key("BuildingFixExp") {
                            number(row, "BuildingFixExp")?
                        } else {
                            replay_config()["BuildingFixExp"]
                        };
                        let part = usize::try_from(int(1)?)?;
                        let parts = self.objects[i]
                            .value
                            .get_mut("BrokenParts")
                            .and_then(Value::as_array_mut)
                            .context("building has no broken parts")?;
                        let value = parts.get_mut(part).context("paint part out of range")?;
                        if value == &json!(false) {
                            return Ok(());
                        }
                        ensure!(value == &json!(true), "invalid broken part state");
                        *value = json!(false);
                        award_xp(&mut home.avatar_stats, xp)?;
                    }
                    17 => {
                        let i = self.position(int(0)?)?;
                        ensure!(
                            self.objects[i].instance / 100000 == 21,
                            "claim requires construction site"
                        );
                        ensure!(
                            timers::remaining(&self.objects[i].value["ConstructionTimer"]) == 0,
                            "construction is not finished"
                        );
                        let target = self.objects[i].value["TargetData"]
                            .as_u64()
                            .context("construction has no target")?;
                        let target = u32::try_from(target)?;
                        // : construction XP belongs to claim, not purchase
                        // or speed-up. ExpToBuild column mapping is static/inferred.
                        let xp = if target / 100000 == 6 {
                            number(definition(target)?, "ExpToBuild")?
                        } else {
                            0
                        };
                        self.replace(i, target)?;
                        award_xp(&mut home.avatar_stats, xp)?;
                    }
                    4 | 5 => {
                        let id = u32::try_from(int(0)?)?;
                        let row = definition(id)?;
                        let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
                        ensure!(
                            number(row, "UnlockLevel")? <= level,
                            "purchase locked at current level"
                        );
                        let count = self
                            .objects
                            .iter()
                            .filter(|o| o.value["ID"] == id || o.value["TargetData"] == id)
                            .count() as u32;
                        let every = number(row, "DoPriceIncreaseEveryNObject")?;
                        let mut obj = json!({"ID":id});
                        if *command_type == 4 {
                            ensure!(int(1)? == -1, "alternate placement mode unsupported");
                            ensure!(
                                int(2)? % 512 == 0 && int(3)? % 512 == 0,
                                "object position is not tile-aligned"
                            );
                            ensure!(
                                [3, 4, 6, 11].contains(&(id / 100000)),
                                "object purchase category unsupported"
                            );
                            if let Some(column) =
                                row.get("MaxHabitatsColumnName").filter(|s| !s.is_empty())
                            {
                                ensure!(
                                    count < number(level_row(level)?, column)?,
                                    "building limit reached"
                                );
                            }
                            if id == 400000 {
                                let count = self
                                    .objects
                                    .iter()
                                    .filter(|o| o.instance / 100000 == 4)
                                    .count();
                                ensure!(
                                    count < number(level_row(level)?, "MaxFields")? as usize,
                                    "field limit reached"
                                );
                            }
                            ensure!(
                                (0..85 * 512).contains(&int(2)?)
                                    && (0..71 * 512).contains(&int(3)?),
                                "placement outside starter map"
                            );
                            obj["X"] = json!(int(2)? / 512);
                            obj["Y"] = json!(int(3)? / 512);
                            // Command 4's first packed boolean is the object's
                            // mirrored orientation (confirmed by client research).
                            let mirrored = fields
                                .get(4)
                                .and_then(Value::as_bool)
                                .context("placement mirrored flag missing or invalid")?;
                            if mirrored {
                                obj["Mirrored"] = json!(true);
                            }
                            ensure!(
                                fields.get(5) == Some(&json!(false)),
                                "alternate purchase flag unsupported"
                            );
                            // Duration from the building CSV is an  hypothesis.
                            let seconds = duration(row)?;
                            if seconds > 0 {
                                obj["ID"] = json!(construction_site(row)?);
                                obj["TargetData"] = json!(id);
                                obj["ConstructionTimer"] = timers::new(seconds);
                            }
                        } else {
                            ensure!(id / 100000 == 13, "command 5 requires animal data");
                            let habitat = self.position(int(3)?)?;
                            ensure!(
                                self.objects[habitat].instance / 100000 == 11,
                                "animal habitat not found"
                            );
                            ensure!(
                                fields.get(4) == Some(&json!(false)),
                                "animal purchase flag unsupported"
                            );
                            let habitat_data = definition(
                                self.objects[habitat].value["ID"].as_u64().unwrap() as u32,
                            )?;
                            ensure!(
                                habitat_data["Animal"] == row["Name"],
                                "animal does not match habitat"
                            );
                            let habitat_index = self.objects[..habitat]
                                .iter()
                                .filter(|o| o.instance / 100000 == 11)
                                .count();
                            // Live world-position reads confirm saved animal
                            // tiles are offsets: the client adds the habitat origin.
                            for (axis, field, size) in
                                [("X", 1, "TileWidth"), ("Y", 2, "TileHeight")]
                            {
                                let origin =
                                    self.objects[habitat].value[axis].as_i64().unwrap_or(0);
                                let extent = i64::from(number(habitat_data, size)?);
                                ensure!(extent > 0, "animal habitat has no footprint");
                                let tile = i64::from(int(field)?.div_euclid(512));
                                obj[axis] = json!((tile - origin).clamp(0, extent - 1));
                            }
                            let residents = self
                                .objects
                                .iter()
                                .filter(|o| {
                                    o.instance / 100000 == 13
                                        && o.value["AnimalHabitatIndex"] == habitat_index
                                })
                                .count();
                            ensure!(
                                residents < number(habitat_data, "Capacity")? as usize,
                                "habitat full"
                            );
                            obj["AnimalHabitatIndex"] = json!(habitat_index);
                            obj["Fed"] = json!(false);
                        }
                        let price = number(row, "Price")?
                            .checked_add(match count.checked_div(every) {
                                None => 0,
                                Some(tiers) => tiers
                                    .checked_mul(number(row, "PriceIncrease")?)
                                    .context("tiered price overflow")?,
                            })
                            .context("price overflow")?;
                        spend(&mut home.avatar_stats, price)?;
                        self.create(obj)?;
                        // : only decorations grant BuyExp on purchase;
                        // processing buildings grant ExpToBuild at claim instead.
                        if *command_type == 4 && id / 100000 == 3 && level < 700 {
                            award_xp(&mut home.avatar_stats, number(row, "BuyExp")?)?;
                        }
                    }
                    _ => {
                        if !self.extended_command(home, *command_type, fields)? {
                            self.unknown(format!(
                                "command {command_type} has no established effect specification"
                            ));
                        }
                    }
                }
            }
            TurnCommand::TutorialStarted { data_id, .. } => {
                self.start_tutorial_visitor(home, *data_id)?
            }
            TurnCommand::People { data_id, .. } => self.decline_visitor(*data_id),
            _ => {}
        }
        self.fill_orders(home)?;
        self.tick_visitors(home)?;
        self.save_objects(home);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::{messages::OwnHomeData, stream::Writer, Message};
    pub(crate) fn turn(a: u32, commands: &[Vec<u32>]) -> Vec<u8> {
        let mut w = Writer::new();
        w.vint(a).vint(0).vint(commands.len() as u32);
        for command in commands {
            for v in command {
                w.vint(*v);
            }
        }
        w.into_bytes()
    }
    fn apply(
        state: &mut Replay,
        home: &mut HomeData,
        a: u32,
        commands: &[Vec<u32>],
    ) -> Vec<String> {
        let bytes = turn(a, commands);
        let t = EndClientTurn::parse(&bytes).unwrap();
        assert_eq!(t.decoded.len(), commands.len());
        state.apply(home, &t, &bytes)
    }
    #[test]
    fn harvest_clear_plant_reconnect_and_level_up() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900003, 25);
        let mut state = Replay::new(&home).unwrap();
        let commands = (400000..400006)
            .flat_map(|id| [vec![2, id, 1], vec![22, id, 1], vec![23, id, 1]])
            .collect::<Vec<_>>();
        assert!(apply(&mut state, &mut home, 304, &commands).is_empty());
        assert_eq!(home.avatar_stats[&400001], 12);
        assert_eq!(home.avatar_stats[&1900002], 2);
        assert_eq!(home.avatar_stats[&1900003], 4);
        assert_eq!(home.avatar_stats[&400002], 4);
        assert_eq!(home.avatar_stats[&1900000], 435);
        assert_eq!(
            state
                .objects
                .iter()
                .filter(|o| o.instance / 100000 == 4)
                .map(|o| o.instance)
                .collect::<Vec<_>>(),
            (400006..400012).collect::<Vec<_>>()
        );
        let commands = (400006..400012)
            .map(|id| vec![1, id, 400001, 305])
            .collect::<Vec<_>>();
        assert!(apply(&mut state, &mut home, 607, &commands).is_empty());
        assert_eq!(home.avatar_stats[&400001], 6);
        assert!(
            home.c["Objects"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|o| o["ID"] == 400001)
                .count()
                == 6
        );
        let message = home.message();
        assert_eq!(message.fields_unlocked, 7);
        assert_eq!(
            OwnHomeData::from_bytes(&message.to_bytes()).unwrap(),
            message
        );
        let json = serde_json::to_vec(&home).unwrap();
        let mut reopened: HomeData = serde_json::from_slice(&json).unwrap();
        let mut reconnect = Replay::new(&reopened).unwrap();
        assert!(apply(
            &mut reconnect,
            &mut reopened,
            304,
            &[vec![2, 400000, 1], vec![23, 400000, 1]]
        )
        .is_empty());
        assert_eq!(reopened.avatar_stats[&400001], 8);
        assert_eq!(reopened.avatar_stats[&1900003], 5);
    }
    #[test]
    fn harvest_is_credited_once_even_across_interrupted_clear() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        let commands = [vec![2, 400000, 1], vec![22, 400000, 1], vec![2, 400000, 1]];
        assert!(apply(&mut state, &mut home, 304, &commands).is_empty());
        assert_eq!(home.avatar_stats[&400001], 2);
        assert_eq!(home.avatar_stats[&1900003], 1);
        let after = home.clone();
        assert_eq!(
            apply(&mut state, &mut home, 304, &commands),
            ["duplicate turn ignored"]
        );
        assert_eq!(home, after);
        let mut reconnect = Replay::new(&home).unwrap();
        assert!(apply(&mut reconnect, &mut home, 304, &[vec![2, 400000, 1]])
            .iter()
            .any(|s| s.contains("empty field")));
        assert_eq!(home.avatar_stats[&400001], 2);
    }
    #[test]
    fn first_tutorial_painting_and_collects_reach_level_two() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        let mut commands: Vec<Vec<u32>> = (400000..400006).map(|id| vec![2, id, 1]).collect();
        for (building, count) in [(1000000, 6), (1400000, 6), (1800000, 4)] {
            for part in 0..count {
                commands.push(vec![25, building, part, 2]);
            }
        }
        assert!(apply(&mut state, &mut home, 3, &commands).is_empty());
        assert_eq!(home.avatar_stats[&1900002], 1);
        assert_eq!(home.avatar_stats[&1900003], 22); // 6 harvest + 16 paint.
                                                     // Already painted parts cannot grant XP again on another turn.
        assert!(apply(&mut state, &mut home, 4, &[vec![25, 1000000, 0, 4]]).is_empty());
        assert_eq!(home.avatar_stats[&1900003], 22);
        for _ in 0..3 {
            state.create(json!({"ID":1300000,"X":1,"Y":1,"AnimalHabitatIndex":0,"Fed":true,"Timer":null})).unwrap();
        }
        assert!(apply(
            &mut state,
            &mut home,
            5,
            &[
                vec![14, 1300000, 5],
                vec![14, 1300001, 5],
                vec![14, 1300002, 5]
            ]
        )
        .is_empty());
        assert_eq!(home.avatar_stats[&1900002], 2);
        assert_eq!(home.avatar_stats[&1900003], 1); // 28 total minus 27.
        assert_eq!(home.avatar_stats[&1900000], 435);
        assert_eq!(home.avatar_stats[&400002], 4);
        let message = home.message();
        assert_eq!(
            OwnHomeData::from_bytes(&message.to_bytes()).unwrap(),
            message
        );
    }
    #[test]
    fn xp_rewards_continuations_multiple_levels_and_threshold_base_xp() {
        let mut stats = BTreeMap::new();
        award_xp(&mut stats, 48).unwrap();
        assert_eq!(stats[&1900002], 4);
        assert_eq!(stats[&1900003], 0);
        assert_eq!(stats[&400002], 4);
        assert_eq!(stats[&1900001], 2);
        assert_eq!(stats[&1900000], 185);
        let mut stats = BTreeMap::from([(1900002, 15), (1900003, 0)]);
        award_xp(&mut stats, 2881).unwrap();
        assert_eq!(stats[&1900002], 16);
        assert_eq!(stats[&1900003], 1);
        let sheep_food = crate::game::starter::resolve("Sheep Food").unwrap().0;
        assert_eq!(stats[&sheep_food], 3);
    }
    #[test]
    fn _claim_awards_xp_once_and_rolls_over_with_rewards() {
        for (target, xp) in [(600000, 3), (600001, 4)] {
            let mut home = crate::game::starter::new_home().unwrap();
            home.c["Objects"] = json!([{"ID":2100001,"TargetData":target,"X":20,"Y":25}]);
            home.avatar_stats.insert(1900002, 1);
            home.avatar_stats.insert(1900003, 26);
            let before_cash = home.avatar_stats[&1900000];
            let mut state = Replay::new(&home).unwrap();
            assert!(apply(&mut state, &mut home, 1, &[vec![16, 2100000, 1]]).is_empty());
            assert_eq!(home.avatar_stats[&1900003], 26); // Speed-up grants nothing.
            assert!(apply(&mut state, &mut home, 2, &[vec![17, 2100000, 2]]).is_empty());
            assert_eq!(home.avatar_stats[&1900002], 2);
            assert_eq!(home.avatar_stats[&1900003], xp - 1);
            assert_eq!(home.avatar_stats[&1900000], before_cash + 85);
            assert_eq!(home.avatar_stats[&400002], 4);
            let after = home.clone();
            assert!(!apply(&mut state, &mut home, 3, &[vec![17, 2100000, 3]]).is_empty());
            assert_eq!(home, after);
            let message = home.message();
            assert_eq!(
                OwnHomeData::from_bytes(&message.to_bytes()).unwrap(),
                message
            );
        }
    }
    #[test]
    fn _decoration_purchase_buyexp_and_level_cap() {
        for level in [8, 700] {
            let mut home = crate::game::starter::new_home().unwrap();
            home.c["Objects"] = json!([]);
            home.avatar_stats.insert(1900000, 1000);
            home.avatar_stats.insert(1900002, level);
            home.avatar_stats.insert(1900003, 0);
            let mut state = Replay::new(&home).unwrap();
            state
                .execute(
                    &mut home,
                    &schema_command(
                        4,
                        vec![
                            json!(300000),
                            json!(-1),
                            json!(512),
                            json!(512),
                            json!(false),
                            json!(false),
                            json!(0),
                        ],
                    ),
                )
                .unwrap();
            assert_eq!(
                home.avatar_stats[&1900003],
                if level == 700 { 0 } else { 3 }
            );
            assert_eq!(home.avatar_stats[&1900000], 976);
        }
    }
    fn schema_command(kind: u32, fields: Vec<Value>) -> TurnCommand {
        TurnCommand::Schema {
            command_type: kind,
            fields,
            raw: vec![],
        }
    }
    #[test]
    fn flip_object_is_saved_and_toggles_after_reload() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        let instance = state.objects[0].instance;
        let original = state.objects[0].value.clone();
        assert!(apply(&mut state, &mut home, 304, &[vec![53, instance, 1]]).is_empty());
        assert_eq!(home.c["Objects"][0]["Mirrored"], true);
        let saved = serde_json::to_vec(&home).unwrap();
        let mut home: HomeData = serde_json::from_slice(&saved).unwrap();
        let mut state = Replay::new(&home).unwrap();
        assert!(apply(&mut state, &mut home, 607, &[vec![53, instance, 2]]).is_empty());
        assert_eq!(home.c["Objects"][0], original);
        let before = home.clone();
        assert!(!apply(&mut state, &mut home, 910, &[vec![53, 99999999, 3]]).is_empty());
        assert_eq!(home, before);
    }
    #[test]
    fn construction_selector_and_bakery_purchase_finish_claim() {
        for (name, site) in [
            ("Bakery", 2100001),
            ("Hammermill", 2100001),
            ("Dairy", 2100000),
            ("ChickenHabitat", 2100003),
        ] {
            let row = tables()
                .values()
                .flatten()
                .find(|r| r.get("Name").is_some_and(|n| n == name))
                .unwrap();
            assert_eq!(construction_site(row).unwrap(), site, "{name}");
        }
        for instant in [false, true] {
            let mut home = crate::game::starter::new_home().unwrap();
            // Match the captured world: the starter tent has already been claimed.
            for object in home.c["Objects"].as_array_mut().unwrap() {
                if object["ID"] == 2100003 {
                    *object = json!({"ID":1100000,"X":12,"Y":21});
                }
            }
            home.avatar_stats.insert(1900000, 1000);
            home.avatar_stats.insert(1900001, 10);
            home.avatar_stats.insert(1900002, 3);
            let mut state = Replay::new(&home).unwrap();
            let command = schema_command(
                4,
                vec![
                    json!(600000),
                    json!(-1),
                    json!(20 * 512),
                    json!(25 * 512),
                    json!(true),
                    json!(false),
                    json!(0),
                ],
            );
            state.execute(&mut home, &command).unwrap();
            assert_eq!(home.avatar_stats.get(&1900003).copied().unwrap_or(0), 0);
            let site = state
                .objects
                .iter()
                .find(|o| o.instance == 2100000)
                .unwrap();
            assert_eq!(
                site.value,
                json!({"ID":2100001,"X":20,"Y":25,"Mirrored":true,"TargetData":600000,"ConstructionTimer":{"StartSeconds":10,"TicksLeft":151}})
            );
            if instant {
                assert!(apply(&mut state, &mut home, 0, &[vec![16, 2100000, 0]]).is_empty());
            } else {
                assert!(apply(&mut state, &mut home, 330, &[]).is_empty());
            }
            assert!(apply(&mut state, &mut home, 330, &[vec![17, 2100000, 330]]).is_empty());
            assert_eq!(home.avatar_stats[&1900001], 10);
            assert_eq!(home.avatar_stats[&1900003], 3);
            assert_eq!(
                home.c["Objects"].as_array().unwrap().last().unwrap(),
                &json!({"ID":600000,"X":20,"Y":25,"Mirrored":true})
            );
        }
    }
    #[test]
    fn captured_move_preserves_instance_and_persists_position() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        let instance = state
            .create(json!({"ID":400002,"X":21,"Y":23,"Mirrored":true}))
            .unwrap();
        assert_eq!(instance, 400006);
        let bytes = vec![
            0x8e, 0x0e, 0xc2, 0xfc, 0x90, 0xd2, 0x03, 0x01, 0x03, 0x86, 0xea, 0x30, 0x00, 0xc0,
            0x78, 0x00, 0x15, 0x17, 0x82, 0xea, 0x30, 0xbf, 0x0a,
        ];
        let turn = EndClientTurn::parse(&bytes).unwrap();
        let stats = home.avatar_stats.clone();
        let next = state.next.clone();
        assert!(state.apply(&mut home, &turn, &bytes).is_empty());
        assert_eq!(
            state.objects[state.position(instance as i32).unwrap()].value,
            json!({"ID":400002,"X":21,"Y":22,"Mirrored":true})
        );
        assert_eq!(state.next, next);
        assert_eq!(home.avatar_stats, stats);
        let saved = serde_json::to_vec(&home).unwrap();
        let restored: HomeData = serde_json::from_slice(&saved).unwrap();
        assert_eq!(
            restored.c["Objects"].as_array().unwrap().last().unwrap()["Y"],
            22
        );
    }
    #[test]
    fn animal_purchase_saves_relative_tiles_and_clamps_to_habitat() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900000, 1000);
        home.avatar_stats.insert(1900002, 3);
        let mut state = Replay::new(&home).unwrap();
        state
            .execute(
                &mut home,
                &schema_command(17, vec![json!(2100000), json!(0)]),
            )
            .unwrap();
        let habitat = state.position(1100000).unwrap();
        assert_eq!(state.objects[habitat].value["X"], 13);
        assert_eq!(state.objects[habitat].value["Y"], 21);
        for (x, y, expected) in [
            (14 * 512 + 351, 22 * 512 + 218, (1, 1)),
            (14 * 512 + 375, 21 * 512 + 250, (1, 0)),
            (13 * 512 + 346, 22 * 512 + 400, (0, 1)),
            (-1, 100 * 512, (0, 2)),
        ] {
            state
                .execute(
                    &mut home,
                    &schema_command(
                        5,
                        vec![
                            json!(1300000),
                            json!(x),
                            json!(y),
                            json!(1100000),
                            json!(false),
                            json!(0),
                        ],
                    ),
                )
                .unwrap();
            let animal = state.objects.last().unwrap();
            assert_eq!(
                (
                    animal.value["X"].as_i64().unwrap(),
                    animal.value["Y"].as_i64().unwrap()
                ),
                expected
            );
            assert_eq!(animal.value["AnimalHabitatIndex"], 0);
        }
        let saved = serde_json::to_vec(&home).unwrap();
        let reloaded = Replay::new(&serde_json::from_slice::<HomeData>(&saved).unwrap()).unwrap();
        assert_eq!(
            reloaded.objects.last().unwrap().value,
            state.objects.last().unwrap().value
        );
    }
    #[test]
    fn paint_claim_purchase_and_rollback() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900000, 1000);
        home.avatar_stats.insert(1900002, 3);
        let mut state = Replay::new(&home).unwrap();
        state
            .execute(
                &mut home,
                &schema_command(25, vec![json!(1000000), json!(2), json!(1)]),
            )
            .unwrap();
        assert_eq!(
            home.c["Objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["ID"] == 1000000)
                .unwrap()["BrokenParts"][2],
            false
        );
        state
            .execute(
                &mut home,
                &schema_command(17, vec![json!(2100000), json!(1)]),
            )
            .unwrap();
        assert!(!state.objects.iter().any(|o| o.value["ID"] == 2100003));
        state
            .execute(
                &mut home,
                &schema_command(
                    4,
                    vec![
                        json!(400000),
                        json!(-1),
                        json!(20 * 512),
                        json!(25 * 512),
                        json!(false),
                        json!(false),
                        json!(1),
                    ],
                ),
            )
            .unwrap();
        assert_eq!(home.avatar_stats[&1900000], 999);
        state
            .execute(
                &mut home,
                &schema_command(
                    5,
                    vec![
                        json!(1300000),
                        json!(12 * 512 + 256),
                        json!(21 * 512),
                        json!(1100000),
                        json!(false),
                        json!(2),
                    ],
                ),
            )
            .unwrap();
        let animal = state
            .objects
            .iter()
            .find(|o| o.instance == 1300000)
            .unwrap();
        assert_eq!(animal.value["AnimalHabitatIndex"], 0);
        assert_eq!(animal.value["X"], 0);
        assert_eq!(animal.value["Y"], 0);
        assert_eq!(home.avatar_stats[&1900000], 989);
        // Real command parsing includes two packed bools for purchase 4.
        let mut w = Writer::new();
        w.vint(304).vint(0).vint(1).vint(4).vint(400000);
        w.raw(&[0x7f])
            .vint(512 * 26)
            .vint(512 * 25)
            .raw(&[1]) // Mirrored=true, alternate purchase flag=false.
            .vint(1);
        let bytes = w.into_bytes();
        let t = EndClientTurn::parse(&bytes).unwrap();
        assert_eq!(t.decoded.len(), 1);
        assert!(state.apply(&mut home, &t, &bytes).is_empty());
        assert_eq!(home.avatar_stats[&1900000], 988);
        let placed = home.c["Objects"].as_array().unwrap().last().unwrap();
        assert_eq!(placed, &json!({"ID":400000,"X":26,"Y":25,"Mirrored":true}));
        // The saved home retains orientation when its replay world is loaded again.
        let saved = serde_json::to_vec(&home).unwrap();
        let restored: HomeData = serde_json::from_slice(&saved).unwrap();
        let reloaded = Replay::new(&restored).unwrap();
        assert_eq!(&reloaded.objects.last().unwrap().value, placed);
        let before = home.clone();
        assert!(!apply(&mut state, &mut home, 607, &[vec![1, 999999, 400001, 2]]).is_empty());
        assert_eq!(home, before);
    }
    #[test]
    fn errors_unknown_commands_and_old_ticks_never_suspend_replay() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        home.avatar_stats.insert(1900000, 777); // External stat edit is no blocker.
        let notes = apply(
            &mut state,
            &mut home,
            304,
            &[vec![9, 1300000, 1], vec![2, 400000, 1]],
        );
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("type=9"));
        assert_eq!(home.avatar_stats[&400001], 2);
        assert_eq!(home.avatar_stats[&1900003], 1);
        assert_eq!(home.avatar_stats[&1900000], 777);
        // A failed supported command also skips only itself. Older header ticks
        // are diagnostic, not a reason to discard the next command.
        let notes = apply(
            &mut state,
            &mut home,
            200,
            &[vec![1, 999999, 400001, 2], vec![2, 400001, 2]],
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(home.avatar_stats[&400001], 4);
        // Decode the known prefix, retain an unknown tail, then continue next packet.
        let bytes = turn(607, &[vec![2, 400002, 3], vec![305, 99, 3]]);
        let packet = EndClientTurn::parse(&bytes).unwrap();
        let notes = state.apply(&mut home, &packet, &bytes);
        assert!(!notes.is_empty());
        assert_eq!(home.avatar_stats[&400001], 6);
        assert!(apply(&mut state, &mut home, 910, &[vec![2, 400003, 4]]).is_empty());
        assert_eq!(home.avatar_stats[&400001], 8);
        assert_eq!(home.avatar_stats[&1900003], 4);
    }
}
