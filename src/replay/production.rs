//! Recipe and production replay. JSON nesting, entry time fields and queued
//! timer start are deliberately documented hypotheses, not runtime evidence.
use super::*;

fn recipe(id: u32) -> Result<Vec<(u32, u32)>> {
    let table = type_tables()
        .get(&(id / 100000))
        .context("recipe table missing")?;
    let rows = &tables()[table];
    let start = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.get("Name").is_some_and(|n| !n.is_empty()))
        .nth((id % 100000) as usize)
        .map(|(i, _)| i)
        .context("recipe row missing")?;
    let mut needs = BTreeMap::<u32, u32>::new();
    for (i, row) in rows[start..].iter().enumerate() {
        if i > 0 && row.get("Name").is_some_and(|s| !s.is_empty()) {
            break;
        }
        if let Some(name) = row.get("Requirement").filter(|s| !s.is_empty()) {
            let (id, _) = crate::game::starter::resolve(name)?;
            ensure!(
                id / 100000 != 77,
                "voucher recipe requirement is not modeled"
            );
            let amount = number(row, "RequirementAmount")?;
            let held = needs.entry(id).or_default();
            *held = held.checked_add(amount).context("recipe amount overflow")?;
        }
    }
    Ok(needs.into_iter().collect())
}
fn queue(building: &mut Value) -> Result<&mut serde_json::Map<String, Value>> {
    if building.get("ProductionList").is_none_or(Value::is_null) {
        building["ProductionList"] = json!({});
    }
    let list = building["ProductionList"]
        .as_object_mut()
        .context("unrecognized ProductionList nesting")?;
    for key in ["Productions", "FinishedProductions"] {
        if !list.contains_key(key) {
            list.insert(key.into(), json!([]));
        }
        ensure!(list[key].is_array(), "unrecognized {key} nesting");
    }
    Ok(list)
}
fn units(row: &Row) -> Result<u32> {
    if row.get("Units").is_some_and(|v| !v.is_empty()) {
        number(row, "Units")
    } else {
        Ok(1)
    }
}
/// Basic barn/silo accounting for ordinary items. Vouchers, boosters, currencies
/// and puzzle pieces use other storage. Missing storage buildings are unlimited
/// in this development server. Capacity/category parity is not runtime tested.
fn storage_room(home: &HomeData, id: u32, amount: u32) -> Result<bool> {
    let silo = [4, 61].contains(&(id / 100000));
    let building_kind = if silo { 18 } else { 14 };
    let objects = home.c["Objects"]
        .as_array()
        .context("home objects missing")?;
    let Some(building) = objects
        .iter()
        .find(|o| o["ID"].as_u64().unwrap_or(0) / 100000 == building_kind)
    else {
        return Ok(true);
    };
    let data_id = building["ID"].as_u64().context("storage ID missing")? as u32;
    let table = type_tables()
        .get(&(data_id / 100000))
        .context("storage table missing")?;
    let definition = definition(data_id)?;
    let rows = &tables()[table];
    let start = rows
        .iter()
        .position(|r| r.get("Name") == definition.get("Name"))
        .context("storage row missing")?;
    let rank = building
        .get("Rank")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let row = rows.get(start + rank - 1).context("storage rank missing")?;
    let capacity = number(row, "Capacity")?;
    let held: u64 = home
        .avatar_stats
        .iter()
        .filter(|(id, _)| {
            let kind = **id / 100000;
            if silo {
                [4, 61].contains(&kind)
            } else {
                kind != 0 && ![4, 61, 17, 19, 77, 185].contains(&kind)
            }
        })
        .map(|(_, n)| u64::from(*n))
        .sum();
    Ok(held + u64::from(amount) <= u64::from(capacity))
}
impl Replay {
    fn building(&self, id: i32, collect: bool) -> Result<usize> {
        let i = self.position(id)?;
        let kind = self.objects[i].instance / 100000;
        ensure!(
            [6, 100, 106].contains(&kind) || (collect && kind == 63),
            "instance is not a production building"
        );
        Ok(i)
    }
    /// False means no space; callers such as instant-complete keep the finished
    /// item in FinishedProductions and still commit the completed timer/cost.
    fn collect_finished(&mut self, home: &mut HomeData, i: usize, slot: usize) -> Result<bool> {
        let entry = queue(&mut self.objects[i].value)?["FinishedProductions"]
            .as_array()
            .unwrap()
            .get(slot)
            .cloned()
            .context("no finished product at requested slot")?;
        let id = u32::try_from(
            entry["ID"]
                .as_u64()
                .context("finished product ID missing")?,
        )?;
        let row = definition(id)?;
        let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
        ensure!(number(row, "UnlockLevel")? <= level, "product is locked");
        let amount = units(row)?;
        if !storage_room(home, id, amount)? {
            return Ok(false);
        }
        add(&mut home.avatar_stats, id, amount)?;
        award_xp(&mut home.avatar_stats, number(row, "ExpCollect")?)?;
        queue(&mut self.objects[i].value)?["FinishedProductions"]
            .as_array_mut()
            .unwrap()
            .remove(slot);
        Ok(true)
    }
    pub(super) fn start_production(
        &mut self,
        home: &mut HomeData,
        product: i32,
        building: i32,
    ) -> Result<()> {
        let i = self.building(building, false)?;
        let product = u32::try_from(product)?;
        let row = definition(product)?;
        let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
        ensure!(number(row, "UnlockLevel")? <= level, "product is locked");
        let producer = row
            .get("ProcessingBuilding")
            .filter(|s| !s.is_empty())
            .context("product has no processing-building recipe")?;
        let building_data = definition(self.objects[i].value["ID"].as_u64().unwrap() as u32)?;
        ensure!(
            building_data.get("Name") == Some(producer),
            "product belongs to a different processing building"
        );
        timers::advance_object(&mut self.objects[i].value, 0);
        if !queue(&mut self.objects[i].value)?["FinishedProductions"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            self.collect_finished(home, i, 0)?;
        }
        for (id, amount) in recipe(product)? {
            remove(&mut home.avatar_stats, id, amount)?;
        }
        let seconds = duration(row)?;
        let entry = json!({"ID":product,"Timer":timers::new(seconds),
            "ProductionTime":seconds,"ReducedProductionTime":seconds});
        queue(&mut self.objects[i].value)?["Productions"]
            .as_array_mut()
            .unwrap()
            .push(entry);
        timers::advance_object(&mut self.objects[i].value, 0);
        Ok(())
    }
    pub(super) fn collect_production(&mut self, home: &mut HomeData, building: i32) -> Result<()> {
        let i = self.building(building, true)?;
        timers::advance_object(&mut self.objects[i].value, 0);
        ensure!(self.collect_finished(home, i, 0)?, "storage is full");
        Ok(())
    }
    pub(super) fn instant_production(
        &mut self,
        home: &mut HomeData,
        slot: i32,
        building: i32,
    ) -> Result<()> {
        let i = self.building(building, false)?;
        let slot = usize::try_from(slot)?;
        let list = queue(&mut self.objects[i].value)?;
        let entry = list["Productions"]
            .as_array()
            .unwrap()
            .get(slot)
            .context("production slot missing")?;
        let id = u32::try_from(entry["ID"].as_u64().context("product ID missing")?)?;
        let cost = instant_cost(home, id, &entry["Timer"])?;
        spend_diamonds(&mut home.avatar_stats, cost)?;
        let list = queue(&mut self.objects[i].value)?;
        let mut entry = list["Productions"].as_array_mut().unwrap().remove(slot);
        entry["Timer"] = Value::Null;
        let finished = list["FinishedProductions"].as_array_mut().unwrap();
        finished.push(entry);
        let completed = finished.len() - 1;
        // Command 12 auto-collects the just-completed entry, not an older one.
        self.collect_finished(home, i, completed)?;
        timers::advance_object(&mut self.objects[i].value, 0);
        Ok(())
    }
    pub(super) fn feed_animal(&mut self, home: &mut HomeData, animal: i32) -> Result<()> {
        let i = self.position(animal)?;
        ensure!(
            self.objects[i].instance / 100000 == 13,
            "feed requires an animal"
        );
        ensure!(
            self.objects[i].value.get("Fed").and_then(Value::as_bool) != Some(true),
            "animal is producing or ready"
        );
        let row = definition(self.objects[i].value["ID"].as_u64().unwrap() as u32)?;
        let (feed, _) = crate::game::starter::resolve(row.get("Feed").context("animal feed missing")?)?;
        let (good, _) = crate::game::starter::resolve(row.get("Good").context("animal good missing")?)?;
        remove(&mut home.avatar_stats, feed, 1)?;
        self.objects[i].value["Fed"] = json!(true);
        self.objects[i].value["Timer"] = timers::new(duration(definition(good)?)?);
        Ok(())
    }
    pub(super) fn collect_animal(&mut self, home: &mut HomeData, animal: i32) -> Result<()> {
        let i = self.position(animal)?;
        ensure!(
            self.objects[i].instance / 100000 == 13,
            "collect requires an animal"
        );
        ensure!(
            self.objects[i].value["Fed"] == true
                && timers::remaining(&self.objects[i].value["Timer"]) == 0,
            "animal is not ready"
        );
        let row = definition(self.objects[i].value["ID"].as_u64().unwrap() as u32)?;
        let (good, _) = crate::game::starter::resolve(&row["Good"])?;
        let product = definition(good)?;
        ensure!(
            number(product, "UnlockLevel")?
                <= home.avatar_stats.get(&1900002).copied().unwrap_or(1),
            "animal product is locked"
        );
        ensure!(storage_room(home, good, 1)?, "storage is full");
        add(&mut home.avatar_stats, good, 1)?;
        award_xp(&mut home.avatar_stats, number(product, "ExpCollect")?)?;
        self.objects[i].value["Fed"] = json!(false);
        self.objects[i].value["Timer"] = Value::Null;
        Ok(())
    }
    pub(super) fn instant_animal(&mut self, home: &mut HomeData, animal: i32) -> Result<()> {
        let i = self.position(animal)?;
        ensure!(
            self.objects[i].instance / 100000 == 13,
            "instant-complete requires an animal"
        );
        ensure!(
            self.objects[i].value["Fed"] == true
                && timers::remaining(&self.objects[i].value["Timer"]) > 0,
            "animal has no running timer"
        );
        let row = definition(self.objects[i].value["ID"].as_u64().unwrap() as u32)?;
        let (good, _) = crate::game::starter::resolve(&row["Good"])?;
        let cost = instant_cost(home, good, &self.objects[i].value["Timer"])?;
        spend_diamonds(&mut home.avatar_stats, cost)?;
        self.objects[i].value["Timer"] = Value::Null;
        Ok(())
    }
    pub(super) fn instant_construction(&mut self, _home: &mut HomeData, site: i32) -> Result<()> {
        let i = self.position(site)?;
        ensure!(
            self.objects[i].instance / 100000 == 21,
            "speed-up requires construction site"
        );
        if timers::remaining(&self.objects[i].value["ConstructionTimer"]) == 0 {
            return Ok(());
        }
        // The function at 0x3de978 returns TargetData, not a diamond cost.
        // Until 0x63ba46 is traced, finish without charging (ignore old overrides).
        self.objects[i].value["ConstructionTimer"] = Value::Null;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{messages::OwnHomeData, Message};
    fn farm(level: u32) -> HomeData {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.extend([
            (1900002, level),
            (1900001, 10),
            (1900015, 0),
            (400001, 20),
            (400002, 10),
        ]);
        let objects = home.c["Objects"].as_array_mut().unwrap();
        for o in objects.iter_mut() {
            if o["ID"] == 2100003 {
                o["ID"] = json!(1100000);
                o.as_object_mut().unwrap().remove("TargetData");
            }
        }
        objects.extend([
            json!({"ID":1300000,"X":14,"Y":22,"AnimalHabitatIndex":0,"Fed":false}),
            json!({"ID":600000,"X":25,"Y":25}),
            json!({"ID":600001,"X":28,"Y":25}),
        ]);
        home
    }
    fn apply(
        state: &mut Replay,
        home: &mut HomeData,
        tick: u32,
        commands: &[Vec<u32>],
    ) -> Vec<String> {
        let bytes = crate::replay::tests::turn(tick, commands);
        let turn = EndClientTurn::parse(&bytes).unwrap();
        assert_eq!(turn.decoded.len(), commands.len());
        state.apply(home, &turn, &bytes)
    }
    fn object(home: &HomeData, id: u32) -> &Value {
        home.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["ID"] == id)
            .unwrap()
    }
    #[test]
    fn feed_wait_collect_and_visual_selection() {
        let mut home = farm(3);
        let mut state = Replay::new(&home).unwrap();
        assert!(apply(
            &mut state,
            &mut home,
            0,
            &[vec![9, 1300000, 0], vec![21, 1100000, 1300000, 0]]
        )
        .is_empty());
        assert_eq!(home.avatar_stats[&1600000], 8);
        assert_eq!(object(&home, 1300000)["Fed"], true);
        assert_eq!(timers::remaining(&object(&home, 1300000)["Timer"]), 18181);
        assert!(apply(&mut state, &mut home, 300, &[]).is_empty());
        assert_eq!(timers::remaining(&object(&home, 1300000)["Timer"]), 18030);
        assert!(!apply(&mut state, &mut home, 301, &[vec![14, 1300000, 301]]).is_empty());
        assert!(!home.avatar_stats.contains_key(&1500000));
        assert!(apply(&mut state, &mut home, 36000, &[vec![14, 1300000, 36000]]).is_empty());
        assert_eq!(home.avatar_stats[&1500000], 1);
        assert_eq!(home.avatar_stats[&1900003], 2);
        assert_eq!(object(&home, 1300000)["Fed"], false);
        assert_eq!(object(&home, 1300000)["Timer"], Value::Null);
        let before = home.clone();
        assert!(apply(
            &mut state,
            &mut home,
            36001,
            &[vec![21, 1100000, 1300000, 36001]]
        )
        .is_empty());
        // Timer snapshot clock is metadata, not a gameplay change.
        assert_eq!(home.c, before.c);
        assert_eq!(home.avatar_stats, before.avatar_stats);
    }
    #[test]
    fn instant_animal_is_free_below_three_and_collection_is_separate() {
        for level in [2, 3] {
            let mut home = farm(level);
            let mut state = Replay::new(&home).unwrap();
            assert!(apply(&mut state, &mut home, 0, &[vec![9, 1300000, 0]]).is_empty());
            // Half of the 20-minute egg timer has elapsed: cost 1 at level 3.
            assert!(apply(&mut state, &mut home, 18000, &[vec![15, 1300000, 18000]]).is_empty());
            assert_eq!(home.avatar_stats[&1900001], if level == 2 { 10 } else { 9 });
            assert_eq!(object(&home, 1300000)["Fed"], true);
            assert_eq!(object(&home, 1300000)["Timer"], Value::Null);
            assert!(!home.avatar_stats.contains_key(&1500000));
            assert!(apply(&mut state, &mut home, 18001, &[vec![14, 1300000, 18001]]).is_empty());
            assert_eq!(home.avatar_stats[&1500000], 1);
        }
    }
    #[test]
    fn full_recipe_feed_units_and_auto_collect_before_next_recipe() {
        let mut home = farm(3);
        let mut state = Replay::new(&home).unwrap();
        assert_eq!(recipe(1600000).unwrap(), [(400001, 2), (400002, 1)]);
        assert!(apply(&mut state, &mut home, 0, &[vec![10, 1600000, 600001, 0]]).is_empty());
        assert_eq!(home.avatar_stats[&400001], 18);
        assert_eq!(home.avatar_stats[&400002], 9);
        assert!(apply(&mut state, &mut home, 9000, &[vec![13, 600001, 9000]]).is_empty());
        assert_eq!(home.avatar_stats[&1600000], 12);
        assert_eq!(home.avatar_stats[&1900003], 1);
        assert!(apply(
            &mut state,
            &mut home,
            9001,
            &[vec![10, 2400000, 600000, 9001]]
        )
        .is_empty());
        assert!(apply(
            &mut state,
            &mut home,
            18002,
            &[vec![10, 2400000, 600000, 18002]]
        )
        .is_empty());
        assert_eq!(home.avatar_stats[&2400000], 1);
        assert_eq!(home.avatar_stats[&1900003], 4);
        assert_eq!(home.avatar_stats[&400001], 12);
        assert_eq!(
            object(&home, 600000)["ProductionList"]["Productions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn queue_runs_only_head_and_reconnect_preserves_json() {
        let mut home = farm(3);
        let mut state = Replay::new(&home).unwrap();
        assert!(apply(
            &mut state,
            &mut home,
            0,
            &[vec![10, 2400000, 600000, 0], vec![10, 2400000, 600000, 0]]
        )
        .is_empty());
        assert!(apply(&mut state, &mut home, 4500, &[]).is_empty());
        let entries = &object(&home, 600000)["ProductionList"]["Productions"];
        for entry in entries.as_array().unwrap() {
            assert!(entry.get("HelperType").is_none());
            assert!(entry.get("BoosterID").is_none());
        }
        assert_eq!(timers::remaining(&entries[0]["Timer"]), 2273);
        assert_eq!(timers::remaining(&entries[1]["Timer"]), 4545);
        assert!(apply(&mut state, &mut home, 9000, &[]).is_empty());
        assert_eq!(
            object(&home, 600000)["ProductionList"]["FinishedProductions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            timers::remaining(&object(&home, 600000)["ProductionList"]["Productions"][0]["Timer"]),
            4545
        );
        home.timer_saved_at_ms = Some(1000);
        let saved = serde_json::to_vec(&home).unwrap();
        let loaded: HomeData = serde_json::from_slice(&saved).unwrap();
        let entries = &object(&loaded, 600000)["ProductionList"]["Productions"];
        for entry in entries.as_array().unwrap() {
            assert!(entry.get("HelperType").is_none());
            assert!(entry.get("BoosterID").is_none());
        }
        let elapsed = elapsed_home(loaded.clone(), 301000);
        assert_eq!(serde_json::to_vec(&loaded).unwrap(), saved);
        assert!(object(&elapsed, 600000)["ProductionList"]["Productions"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            object(&elapsed, 600000)["ProductionList"]["FinishedProductions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(elapsed_home(elapsed.clone(), 301000), elapsed);
        let packet = elapsed.message();
        assert_eq!(OwnHomeData::from_bytes(&packet.to_bytes()).unwrap(), packet);
        let mut state = Replay::new(&elapsed).unwrap();
        let mut elapsed = elapsed;
        assert!(apply(&mut state, &mut elapsed, 0, &[vec![13, 600000, 0]]).is_empty());
        assert_eq!(elapsed.avatar_stats[&2400000], 1);
    }
    #[test]
    fn instant_slot_collects_selected_entry_and_keeps_full_storage_reward() {
        for level in [2, 3] {
            let mut home = farm(level);
            let mut state = Replay::new(&home).unwrap();
            assert!(apply(
                &mut state,
                &mut home,
                0,
                &[vec![10, 2400000, 600000, 0], vec![10, 2400000, 600000, 0]]
            )
            .is_empty());
            let i = state.position(600000).unwrap();
            queue(&mut state.objects[i].value).unwrap()["FinishedProductions"]
                .as_array_mut()
                .unwrap()
                .push(json!({"ID":2400000,"Timer":null,"marker":"old"}));
            state.save_objects(&mut home);
            assert!(apply(&mut state, &mut home, 1, &[vec![12, 1, 600000, 1]]).is_empty());
            assert_eq!(home.avatar_stats[&1900001], if level == 2 { 10 } else { 9 });
            assert_eq!(home.avatar_stats[&2400000], 1);
            assert_eq!(home.avatar_stats[&1900003], 3); // One auto-collected Bread.
            assert_eq!(
                object(&home, 600000)["ProductionList"]["FinishedProductions"][0]["marker"],
                "old"
            );
            assert_eq!(
                object(&home, 600000)["ProductionList"]["Productions"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
        let mut home = farm(3);
        home.avatar_stats.insert(1600000, 50);
        let mut state = Replay::new(&home).unwrap();
        assert!(apply(
            &mut state,
            &mut home,
            0,
            &[vec![10, 2400000, 600000, 0], vec![12, 0, 600000, 0]]
        )
        .is_empty());
        assert_eq!(home.avatar_stats[&1900001], 9);
        assert!(!home.avatar_stats.contains_key(&2400000));
        assert_eq!(
            object(&home, 600000)["ProductionList"]["FinishedProductions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        home.avatar_stats.insert(1600000, 49);
        assert!(apply(&mut state, &mut home, 1, &[vec![13, 600000, 1]]).is_empty());
        assert_eq!(home.avatar_stats[&2400000], 1);
    }
    #[test]
    fn construction_never_charges_unconfirmed_cost_then_claims() {
        for cost in [None, Some(3)] {
            let mut home = crate::game::starter::new_home().unwrap();
            home.avatar_stats.insert(1900001, 10);
            if let Some(cost) = cost {
                home.construction_speed_up_costs.insert(1100000, cost);
            }
            let tent = home.c["Objects"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|o| o["ID"] == 2100003)
                .unwrap();
            tent["ConstructionTimer"] = timers::new(60);
            let mut state = Replay::new(&home).unwrap();
            assert!(apply(
                &mut state,
                &mut home,
                0,
                &[vec![16, 2100000, 0], vec![17, 2100000, 0]]
            )
            .is_empty());
            assert_eq!(home.avatar_stats[&1900001], 10);
            assert!(home.c["Objects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["ID"] == 1100000));
        }
    }
    #[test]
    fn failed_recipe_rolls_back_ingredients_and_never_stops_following_commands() {
        let mut home = farm(3);
        home.avatar_stats.insert(400002, 0);
        let mut state = Replay::new(&home).unwrap();
        let notes = apply(
            &mut state,
            &mut home,
            0,
            &[
                vec![10, 1600000, 600001, 0],
                vec![21, 1100000, 1300000, 0],
                vec![2, 400000, 0],
            ],
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(home.avatar_stats[&400001], 22);
        assert!(object(&home, 600001).get("ProductionList").is_none());
        assert!(apply(&mut state, &mut home, 1, &[vec![9, 1300000, 1]]).is_empty());
        assert_eq!(object(&home, 1300000)["Fed"], true);
    }
    #[test]
    fn timer_aging_on_reconnect_and_insufficient_diamonds() {
        let mut home = farm(3);
        let mut state = Replay::new(&home).unwrap();
        assert!(apply(&mut state, &mut home, 0, &[vec![9, 1300000, 0]]).is_empty());
        home.timer_saved_at_ms = Some(1000);
        let elapsed = elapsed_home(home.clone(), 601000);
        assert_eq!(timers::remaining(&object(&elapsed, 1300000)["Timer"]), 9091);
        let mut state = Replay::new(&elapsed).unwrap();
        let mut elapsed = elapsed;
        elapsed.avatar_stats.insert(1900001, 0);
        elapsed.avatar_stats.insert(1900015, 0);
        let notes = apply(
            &mut state,
            &mut elapsed,
            0,
            &[vec![15, 1300000, 0], vec![2, 400000, 0]],
        );
        assert_eq!(notes.len(), 1);
        assert!(timers::remaining(&object(&elapsed, 1300000)["Timer"]) > 0);
        assert_eq!(elapsed.avatar_stats[&400001], 22);
    }
}
