//! Partial-effect replay. Unknown sub-effects warn; known effects commit.
use super::*;
fn stat(name: &str) -> Result<u32> {
    static IDS: OnceLock<BTreeMap<String, u32>> = OnceLock::new();
    IDS.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("money-ids.json")).unwrap()
    })
    .get(name)
    .copied()
    .with_context(|| format!("unknown money row {name}"))
}
pub(super) fn bump(home: &mut HomeData, name: &str, n: u32) -> Result<()> {
    add(&mut home.avatar_stats, stat(name)?, n)
}
fn spend_tool(home: &mut HomeData, row: &Row, key: &str) -> Result<()> {
    if let Some(name) = row.get(key).filter(|name| !name.is_empty()) {
        let amount = number(row, &format!("{key}Amount"))?;
        ensure!(amount > 0, "missing collection tool amount for {name}");
        let (id, _) = crate::game::starter::resolve(name)?;
        remove(&mut home.avatar_stats, id, amount)?;
    }
    Ok(())
}

fn continuation_rows(
    table: &'static [Row],
    base: &'static Row,
) -> Result<impl Iterator<Item = &'static Row>> {
    let offset = table
        .iter()
        .position(|row| std::ptr::eq(row, base))
        .context("base row missing")?;
    Ok(table[offset + 1..]
        .iter()
        .take_while(|row| row.get("Name").is_none_or(String::is_empty)))
}

impl Replay {
    pub(super) fn unknown(&mut self, detail: impl Into<String>) {
        self.warnings.push(detail.into());
    }
    fn bits(&mut self, kind: u32, row: u32, set: bool) -> Result<()> {
        let (_, _, rows, _, count) = crate::game::starter::layout()
            .iter()
            .find(|r| r.0 == kind)
            .context("unknown bitset type")?;
        ensure!((row as usize) < *rows, "bitset row out of bounds");
        if !self.extra["Bits"].is_object() {
            self.extra["Bits"] = json!({});
        }
        let key = kind.to_string();
        let mut words: Vec<u32> = serde_json::from_value(self.extra["Bits"][&key].clone())
            .unwrap_or_else(|_| vec![0; *count]);
        ensure!(words.len() == *count, "incorrect saved bitset length");
        let word = &mut words[row as usize / 32];
        let mask = 1 << (row % 32);
        if set {
            *word |= mask;
        } else {
            *word &= !mask;
        }
        self.extra["Bits"][&key] = json!(words);
        Ok(())
    }
    fn generic_cost(&mut self, home: &mut HomeData, row: &Row, timer: &Value) -> Result<()> {
        let total = duration(row)?.max(1) as u64;
        let remaining = (timers::remaining(timer) * 66 / 1000).max(1);
        let cost = if home.avatar_stats.get(&1900002).copied().unwrap_or(1)
            < replay_config()["FREE_DIAMOND_SPEED_UP_BEFORE_LEVEL"]
        {
            0
        } else {
            (u64::from(number(row, "InstantComplete")?) * remaining)
                .div_ceil(total)
                .max(1)
                .try_into()?
        };
        spend_diamonds(&mut home.avatar_stats, cost)
    }
    pub(super) fn extended_command(
        &mut self,
        home: &mut HomeData,
        kind: u32,
        f: &[Value],
    ) -> Result<bool> {
        let arg = |i: usize| -> Result<i32> {
            f.get(i)
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .context("expected command integer")
        };
        match kind {
            6 | 88 => {
                self.unknown("cosmetic AI action has no persistent effect; its shared world-RNG rolls are not reproduced");
            }
            42 | 65 | 66 => {}
            7 => {
                let id = u32::try_from(arg(0)?)?;
                ensure!(id / 100000 == 17, "field booster data required");
                definition(id)?;
                let target = self.position(arg(1)?)?;
                ensure!(self.objects[target].instance / 100000 == 4, "field target required");
                remove(&mut home.avatar_stats, id, 1)?;
                self.unknown("field booster consumed; booster-specific timer effect not traced");
            }
            8 => {
                let id = u32::try_from(arg(0)?)?;
                ensure!(id == 400000, "empty field data required");
                let row = definition(id)?;
                let placed = self.objects.iter().filter(|o| o.instance / 100000 == 4).count() as u32;
                let stored = home.avatar_stats.get(&id).copied().unwrap_or(0);
                let count = placed.checked_add(stored).context("field count overflow")?;
                let cash = purchase_price(row, count)?;
                spend(&mut home.avatar_stats, cash)?;
                spend_diamonds(&mut home.avatar_stats, number(row, "DiamondPrice")?)?;
                add(&mut home.avatar_stats, id, 1)?;
            }
            11 => {
                let i = self.position(arg(0)?)?;
                let data = self.objects[i].value["ID"].as_u64().unwrap() as u32;
                if (134..=139).contains(&(data / 100000)) {
                    let upgrade = match arg(1)? {
                        1 => "ServiceSlots",
                        2 => "CoinBonus",
                        3 => "ExpAndRepBonus",
                        4 => "TimeBonus",
                        _ => anyhow::bail!("invalid service upgrade type"),
                    };
                    let state = &mut self.objects[i].state;
                    let rank = state["ServiceUpgrades"][upgrade].as_u64().unwrap_or(0);
                    let next_rank = rank.checked_add(1).context("service upgrade overflow")?;
                    if data / 100000 == 134 {
                        let base = definition(data)?;
                        let rows = &tables()["service_buildings"];
                        let row = continuation_rows(rows, base)?.nth(usize::try_from(rank)?).context("service upgrade rank out of range")?;
                        ensure!(row.contains_key("CollectionTool1Amount"), "service upgrade amounts missing; regenerate resources");
                        ensure!(number(row, upgrade)? > 0, "service upgrade already at maximum");
                        for key in ["CollectionTool1", "CollectionTool2", "CollectionTool3"] {
                            spend_tool(home, row, key)?;
                        }
                    }
                    state["ServiceUpgrades"][upgrade] = json!(next_rank);
                    self.unknown("service upgrade counter saved in replay metadata; client JSON keys unresolved");
                    return Ok(true);
                }
                let name = match data / 100000 {
                    18 => "SiloRank",
                    14 => "WarehouseRank",
                    102 => "TackleBoxRank",
                    107 => "LobsterPoolRank",
                    154 => "DuckSalonRank",
                    _ => {
                        self.unknown("upgrade target cost/state unknown");
                        return Ok(true);
                    }
                };
                bump(home, name, 1)?;
                let rank = home.avatar_stats[&stat(name)?];
                self.objects[i].value["Rank"] = json!(rank);
                self.unknown("upgrade rank applied; required tool amounts/missing-tool purchase option unknown and not charged");
            }
            18..=20 => {
                let i = self.position(arg(0)?)?;
                ensure!(self.objects[i].instance / 100000 == 5, "forest obstacle required");
                let id = self.objects[i].value["ID"].as_u64().unwrap() as u32;
                let row = definition(id)?;
                let started = self.objects[i].state["Started"] == true;
                if kind == 18 {
                    ensure!(!started, "obstacle clearing already started");
                }
                if kind == 18 || (kind == 19 && !started) {
                    if let Some(tool) = row.get("Tool").filter(|s| !s.is_empty()) {
                        remove(&mut home.avatar_stats, crate::game::starter::resolve(tool)?.0, 1)?;
                    }
                    self.objects[i].state = json!({
                        "Started":true,"Timer":timers::new(duration(row)?)
                    }
                    );
                }
                if kind == 19 {
                    let timer = self.objects[i].state["Timer"].clone();
                    if timers::remaining(&timer) > 0 {
                        self.generic_cost(home, row, &timer)?;
                    }
                    self.objects[i].state["Timer"] = json!({});
                }
                if kind == 20 {
                    ensure!(started, "obstacle clearing not started");
                    ensure!(timers::remaining(&self.objects[i].state["Timer"]) == 0, "obstacle not ready");
                    award_xp(&mut home.avatar_stats, number(row, "ExpClear")?)?;
                    bump(home, "CounterForestCollect", 1)?;
                    self.objects.remove(i);
                } else {
                    self.unknown("forest clearing timer saved server-side; client JSON timer key not established");
                }
            }
            30..=32 | 36 | 37 => self.market(home, kind, f)?,
            39 => {
                let items = f[0].as_array().context("missing purchase list")?;
                ensure!(!items.is_empty(), "empty buy list");
                let context = arg(1)?;
                let mut cost = 0u32;
                for item in items {
                    let id = item[0].as_u64().context("invalid item")? as u32;
                    let amount = u32::try_from(item[1].as_i64().context("invalid amount")?)?;
                    ensure!((1..20_000_000).contains(&amount), "invalid purchase amount");
                    let unit = buy_goods_unit_price(definition(id)?)?;
                    cost = cost.checked_add(amount.checked_mul(unit).context("price overflow")?).context("price overflow")?;
                }
                spend_diamonds(&mut home.avatar_stats, cost)?;
                for item in items {
                    add(&mut home.avatar_stats, item[0].as_u64().unwrap() as u32, item[1].as_u64().unwrap() as u32)?;
                }
                if [-65, -64, -63, -62, -61, -60, -59, -46, -45, -38, -24].contains(&context) {
                    self.unknown("special diamond-purchase delivery context unknown; goods credited to storage instead");
                }
            }
            44 | 45 => {
                let i = self.position(arg(0)?)?;
                ensure!(self.objects[i].instance / 100000 == 53, "mystery box required");
                if kind == 44 {
                    let reward = &self.objects[i].state["MysteryBoxReward"];
                    if reward.is_null() {
                        self.unknown("opened box removed; pre-rolled reward unavailable");
                    } else {
                        let id = reward["ID"].as_u64().context("missing mystery box reward ID")?;
                        let amount = reward["Amount"].as_u64().context("missing mystery box reward amount")?;
                        ensure!(id > 0 && amount > 0, "invalid mystery box reward");
                        add(&mut home.avatar_stats, u32::try_from(id)?, u32::try_from(amount)?)?;
                    }
                }
                self.objects.remove(i);
            }
            46 | 48 => {
                ensure!(u32::try_from(arg(0)?)? / 100000 == 53, "mystery box required");
                self.unknown("mystery box reveal step recognized; saved-state effect unresolved");
            }
            54 => {
                let i = self.position(arg(0)?)?;
                if self.objects[i].instance / 100000 == 45 {
                    self.objects.remove(i);
                } else {
                    self.unknown("generic removal target class has no verified removal rules");
                }
            }
            47 => self.unknown("AI roadside purchase recognized; stand selection, item/price and AIRoadsideShopBits mapping unknown"),
            49 => {
                home.avatar_stats.insert(stat("AppRateState")?, u32::try_from(arg(0)?)?);
            }
            50 => {
                let row = u32::try_from(arg(0)?)?;
                ensure!(row < 5, "ambient row not spawnable");
                let x = arg(1)?;
                let y = arg(2)?;
                ensure!(
                    self.objects.iter().filter(|o| o.instance / 100000 == 45).count() < (replay_config()["MaxAmbientAnimals"] as usize),
                    "ambient list full"
                );
                self.create(json!({
                    "ID":4500000+row,"X":x/512,"Y":y/512,"AccurateX":x,"AccurateY":y
                }
                ))?;
                self.unknown("ambient object saved; target flags and shared world-AI RNG motion not replayed");
            }
            51 => {
                let id = u32::try_from(arg(0)?)?;
                ensure!(id / 100000 == 59, "achievement data required");
                let row = definition(id)?;
                if !self.extra["Achievements"].is_object() {
                    self.extra["Achievements"] = json!({});
                }
                let key = id.to_string();
                if self.extra["Achievements"][&key] != true {
                    award_xp(&mut home.avatar_stats, number(row, "Exp")?)?;
                    add(&mut home.avatar_stats, 1900001, number(row, "DiamondReward")?)?;
                    self.extra["Achievements"][key] = json!(true);
                    self.unknown("achievement base reward applied; objective validation, tier/custom/decor reward unmodeled");
                }
            }
            52 => {
                spend_diamonds(&mut home.avatar_stats, replay_config()["RoadsideShopStandAdvertisementDiamondPrice"])?;
                bump(home, "AdvertisementDiamondPurchases", 1)?;
            }
            56 => {
                ensure!(arg(0)? >= 0, "invalid boat crate index");
                let counter = if f.get(1) == Some(&Value::Bool(true)) {
                    "BoatOrderSharedCrateCount"
                } else {
                    "BoatCrateCompleteCount"
                };
                bump(home, counter, 1)?;
                self.unknown("boat crate completion counted; generated crate goods and reward unavailable");
            }
            57 => {
                home.avatar_stats.insert(stat("ConsecutiveBoatOrdersCompleted")?, 0);
                self.unknown("boat streak reset; current order state unavailable");
            }
            68 | 70 | 71 => {
                let field = match kind {
                    68 => "HireIndex",
                    70 => "SelectedOffer",
                    _ => "SearchData",
                };
                self.extra["Tom"][field] = json!(arg(0)?);
                self.unknown("Tom action recorded; hire and offer prices, timing and world RNG unresolved");
            }
            86 => {
                let i = self.position(arg(0)?)?;
                let habitat = &self.objects[i];
                ensure!(habitat.instance / 100000 == 79, "pet habitat required");
                let data_id = habitat.value["ID"].as_u64().unwrap() as u32;
                let row = definition(data_id)?;
                let feed = row.get("Feed").context("pet habitat feed definition missing")?;
                let (id, _) = crate::game::starter::resolve(feed)?;
                remove(&mut home.avatar_stats, id, number(row, "FeedAmount")?)?;
                bump(home, "PetFeedCount", 1)?;
                self.unknown("pet food consumed; bowl selection and pet state transitions unresolved");
            }
            100 => self.unknown("coin package amount unresolved; diamond and coin balances left unchanged"),
            59..=63 => self.tree(home, kind, f)?,
            64 => {
                let tool = u32::try_from(arg(0)?)?;
                ensure!(tool / 100000 == 22, "mine requires a tool");
                let row = definition(tool)?;
                ensure!(!matches!(row["Name"].as_str(), "Axe" | "Saw"), "tool is not used for mining");
                remove(&mut home.avatar_stats, tool, 1)?;
                self.unknown("mining tool consumed; world-RNG ore yields unknown");
            }
            73 => self.unknown("Tom collection recognized; purchased goods and offer state unavailable"),
            69 | 72 | 74 => {
                if !self.extra["Tom"].is_object() {
                    self.extra["Tom"] = json!({});
                }
                if kind == 72 {
                    self.extra["Tom"]["OfferRejected"] = json!(true);
                } else {
                    self.extra["Tom"]["State"] = json!(if kind == 69 { 13 } else { 3 });
                }
                self.unknown("Tom state transition applied server-side; offer, timing and JSON persistence incomplete");
            }
            67 => {
                let flag = f.first().and_then(Value::as_bool).context("expected reward flag")?;
                home.avatar_stats.insert(stat("FacebookLoginRewardGiven")?, u32::from(flag));
            }
            90 => {
                let id = u32::try_from(arg(0)?)?;
                let kind = id / 100000;
                ensure!([80, 133].contains(&kind), "land expansion area required");
                let row = id % 100000;
                let key = kind.to_string();
                let word = self.extra["Bits"][&key].get((row / 32) as usize).and_then(Value::as_u64).unwrap_or(0);
                if word & (1 << (row % 32)) == 0 {
                    let base = definition(id)?;
                    let table = &tables()[&type_tables()[&kind]];
                    for row in std::iter::once(base).chain(continuation_rows(table, base)?) {
                        spend_tool(home, row, "CollectionTool")?;
                        spend_tool(home, row, "LimitedCollectionTool")?;
                    }
                    self.bits(kind, row, true)?;
                }
            }
            92 | 95 => {
                home.avatar_stats.insert(stat(if kind == 92 { "FBRequirementIgnored" } else { "FacebookAdClicked" })?, 1);
            }
            101 => {
                let hash = arg(0)?;
                if let Ok(hash) = u32::try_from(hash) {
                    home.avatar_stats.insert(stat("LastOpenedFriendLeaderboardHash")?, hash);
                }
                self.extra["LastOpenedFriendLeaderboardHash"] = json!(hash);
                if hash < 0 {
                    home.avatar_stats.remove(&stat("LastOpenedFriendLeaderboardHash")?);
                    self.unknown("signed leaderboard hash saved in replay metadata; outgoing stat encoding is unsigned");
                }
            }
            131 => {
                let id = arg(0)? as u32;
                let table = arg(1)? as u32;
                ensure!([115, 120].contains(&table) && id / 100000 == table, "invalid settings bit type");
                self.bits(table, id % 100000, f[2] == true)?;
            }
            211 => {
                let opt_in = f.get(2).and_then(Value::as_bool).context("expected derby opt-in flag")?;
                self.extra["NeighborhoodDerbyOptIn"] = json!(opt_in);
                self.unknown("derby opt-in saved in replay metadata; neighborhood membership and wire bit mapping unresolved");
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn market(&mut self, home: &mut HomeData, kind: u32, f: &[Value]) -> Result<()> {
        let config = replay_config();
        let count_id = stat("RoadsideShopStandCount")?;
        let count = home
            .avatar_stats
            .get(&count_id)
            .copied()
            .filter(|&count| count != 0)
            .unwrap_or(config["InitialRoadsideShopStandCount"]);
        if kind == 30 {
            ensure!(
                count < config["MaxRoadsideShopStandCount"],
                "roadside shop is full"
            );
            let price_id = stat("RoadsideShopStandDiamondPriceCurrent")?;
            let price = home
                .avatar_stats
                .get(&price_id)
                .copied()
                .filter(|&price| price != 0)
                .unwrap_or(config["RoadsideShopStandDiamondPrice"]);
            spend_diamonds(&mut home.avatar_stats, price)?;
            home.avatar_stats.insert(count_id, count + 1);
            let next_price = price
                .checked_add(config["RoadsideShopStandDiamondPriceIncrease"])
                .context("stand price overflow")?;
            ensure!(
                next_price <= i32::MAX as u32,
                "stand price exceeds stat range"
            );
            home.avatar_stats.insert(price_id, next_price);
            return Ok(());
        }
        let stand = f
            .first()
            .and_then(Value::as_u64)
            .context("invalid roadside stand")?;
        let friend_stands = home
            .avatar_stats
            .get(&stat("RoadsideShopStandCountFromFriendGate")?)
            .copied()
            .unwrap_or(0);
        ensure!(
            stand < u64::from(count) + u64::from(friend_stands),
            "roadside stand out of range"
        );
        let stand = stand.to_string();
        if !self.extra["Market"]["Stands"].is_object() {
            self.extra["Market"]["Stands"] = json!({});
        }
        let listing = &self.extra["Market"]["Stands"][&stand];
        match kind {
            32 => {
                ensure!(listing.is_null(), "roadside stand already listed");
                let id = u32::try_from(f[1].as_u64().context("invalid shop item")?)?;
                let amount = u32::try_from(f[2].as_u64().context("invalid shop quantity")?)?;
                ensure!(
                    (1..=config["RoadsideShopMaxStackAmount"]).contains(&amount),
                    "invalid roadside stack size"
                );
                let price = f[3]
                    .as_u64()
                    .filter(|&p| p > 0)
                    .context("invalid roadside price")?;
                if f[5] == true {
                    self.unknown("roadside alternate item source unresolved");
                    return Ok(());
                }
                remove(&mut home.avatar_stats, id, amount)?;
                self.extra["Market"]["Stands"][&stand] =
                    json!({"ID":id,"Amount":amount,"Price":price,"Sold":false});
                if f[4] == true {
                    Self::advertise(home)?;
                }
            }
            31 => {
                let price = u32::try_from(
                    listing["Price"]
                        .as_u64()
                        .context("stand has no sale to collect")?,
                )?;
                add(&mut home.avatar_stats, crate::game::home::FREE_CASH, price)?;
                bump(home, "RoadsideShopSoldGoodsValue", price)?;
                self.unknown("roadside collection uses the client's sale report; AI buyer state is unavailable");
            }
            36 => {
                ensure!(!listing.is_null(), "no roadside listing to advertise");
                Self::advertise(home)?;
            }
            37 => {
                ensure!(!listing.is_null(), "no roadside listing to discard");
                spend_diamonds(
                    &mut home.avatar_stats,
                    config["RoadsideShopDiscardDiamondPrice"],
                )?;
            }
            _ => unreachable!("roadside command dispatch"),
        }
        if matches!(kind, 31 | 37) {
            self.extra["Market"]["Stands"]
                .as_object_mut()
                .unwrap()
                .remove(&stand);
        }
        self.unknown(
            "roadside stand state saved in replay metadata; client stand JSON keys unresolved",
        );
        Ok(())
    }
    fn advertise(home: &mut HomeData) -> Result<()> {
        let id = stat("AdvertisementDiamondPurchases")?;
        if home.avatar_stats.get(&id).copied().unwrap_or(0) > 0 {
            remove(&mut home.avatar_stats, id, 1)?;
        } else {
            spend_diamonds(
                &mut home.avatar_stats,
                replay_config()["RoadsideShopStandAdvertisementDiamondPrice"],
            )?;
        }
        bump(home, "AdsPlacedToNewspaperCount", 1)
    }
    fn tree(&mut self, home: &mut HomeData, kind: u32, f: &[Value]) -> Result<()> {
        let i = self.position(f[0].as_i64().context("tree id missing")? as i32)?;
        let id = self.objects[i].value["ID"].as_u64().unwrap() as u32;
        ensure!(
            id / 100000 == 62 || (id / 100000 == 148 && matches!(kind, 59 | 60)),
            "tree or gatherer target required"
        );
        let row = definition(id)?;
        match kind {
            60 => {
                ensure!(
                    timers::remaining(&self.objects[i].state["Timer"]) == 0,
                    "regrowth already started"
                );
                let object = &mut self.objects[i];
                object.state["Timer"] = timers::new(duration(row)?);
                if id / 100000 == 62 {
                    let count = object.value["Fruits"]
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or(number(row, "FruitCount")? as usize);
                    object.value["Fruits"] = json!(vec![false; count]);
                    object.state["RefillFruits"] = json!(true);
                    timers::advance_state(&mut object.value, &mut object.state, 0);
                }
                if id / 100000 == 148 {
                    self.unknown("gatherer tool requirements and yield remain unresolved");
                }
            }
            59 => {
                let timer = self.objects[i].state["Timer"].clone();
                ensure!(
                    timers::remaining(&timer) > 0,
                    "no running tree or gatherer timer"
                );
                self.generic_cost(home, row, &timer)?;
                let object = &mut self.objects[i];
                object.state["Timer"] = Value::Null;
                timers::advance_state(&mut object.value, &mut object.state, 0);
            }
            61 => {
                if let Some(tool) = row.get("Tool").filter(|tool| !tool.is_empty()) {
                    remove(
                        &mut home.avatar_stats,
                        crate::game::starter::resolve(tool)?.0,
                        1,
                    )?;
                }
                self.objects.remove(i);
            }
            62 => {
                let slot = f[1].as_u64().context("fruit slot missing")? as usize;
                ensure!(
                    timers::remaining(&self.objects[i].state["Timer"]) == 0,
                    "fruit not ripe"
                );
                ensure!(id / 100000 == 62, "fruit tree required");
                let tree = row.get("Name").context("tree name missing")?;
                let fruit_row = tables()["fruits"]
                    .iter()
                    .find(|r| r.get("ProcessingBuilding") == Some(tree))
                    .context("no fruit matches tree")?;
                let (fruit, _) = crate::game::starter::resolve(&fruit_row["Name"])?;
                let object = &mut self.objects[i];
                let slots = if object.value["Fruits"].is_array() {
                    &mut object.value["Fruits"]
                } else {
                    if object.state["FruitSlots"].is_null() {
                        object.state["FruitSlots"] =
                            json!(vec![true; number(row, "FruitCount")? as usize]);
                    }
                    &mut object.state["FruitSlots"]
                };
                let slot = slots
                    .get_mut(slot)
                    .filter(|v| **v == true)
                    .context("fruit slot out of range or already collected")?;
                *slot = json!(false);
                add(&mut home.avatar_stats, fruit, 1)?;
                award_xp(&mut home.avatar_stats, number(fruit_row, "ExpCollect")?)?;
                let counter = if row
                    .get("IsFruit")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"))
                {
                    "FruitsPicked"
                } else {
                    "BerriesPicked"
                };
                bump(home, counter, 1)?;
            }
            63 => {
                if self.objects[i].state["Revived"] == true {
                    return Ok(());
                }
                bump(home, "FruitTreesRevived", 1)?;
                self.objects[i].state["Revived"] = json!(true);
                self.unknown("tree revival counted; XP and revive/help cost unknown");
            }
            _ => {}
        }
        if matches!(kind, 59 | 60) {
            self.unknown("tree timer stored in replay metadata; client timer key unresolved");
        }
        Ok(())
    }
}

/// Command 39 (LogicBuyGoodsWithDiamondsCommand) unit price, per the data-item
/// constructor 0x323846 and pricer 0x4cfe2c (static, R40): `OrderValue` when
/// positive, otherwise `DiamondPrice`, otherwise 0; then at least 1. No level
/// discount and no context-dependent price; each item costs `amount * unit`.
fn buy_goods_unit_price(row: &Row) -> Result<u32> {
    let order_value = number(row, "OrderValue")?;
    let unit = if order_value > 0 {
        order_value
    } else {
        number(row, "DiamondPrice")?
    };
    Ok(unit.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(state: &mut Replay, home: &mut HomeData, kind: u32, fields: Vec<Value>) {
        state
            .execute(
                home,
                &TurnCommand::Schema {
                    command_type: kind,
                    fields,
                    raw: vec![],
                },
            )
            .unwrap();
    }
    #[test]
    fn buy_goods_price_prefers_order_value_then_diamond_price() {
        let price = |id| buy_goods_unit_price(definition(id).unwrap()).unwrap();
        assert_eq!(price(1500001), 6); // Milk: OrderValue 6, no DiamondPrice.
        assert_eq!(price(1600000), 2); // Chicken Food: no OrderValue, DiamondPrice 2.
        assert_eq!(price(400001), 1); // Wheat: both 1.
    }
    #[test]
    fn buy_missing_milk_costs_six_per_unit() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900001, 18);
        let mut state = Replay::new(&home).unwrap();
        run(
            &mut state,
            &mut home,
            39,
            vec![
                json!([[1500001, 2]]),
                json!(-26),
                json!(-1),
                json!(-1),
                json!(1),
            ],
        );
        assert_eq!(home.avatar_stats[&1900001], 6);
        assert_eq!(home.avatar_stats[&1500001], 2);
    }
    #[test]
    fn _buy_goods_decoder_keeps_list_pairs_and_signed_context() {
        use crate::protocol::stream::Writer;
        let mut w = Writer::new();
        w.vint(304)
            .vint(0)
            .vint(1)
            .vint(39)
            .u8(1)
            .vint(1600000)
            .vint(1)
            .u8((-27i8) as u8)
            .raw(&[0x7f, 0x7f])
            .vint(1);
        let parsed = EndClientTurn::parse(&w.into_bytes()).unwrap();
        if let TurnCommand::Schema { fields, .. } = &parsed.decoded[0] {
            assert_eq!(fields[0], json!([[1600000, 1]]));
            assert_eq!(fields[1], -27);
        } else {
            panic!("wrong buy command");
        }
    }
    #[test]
    fn _partial_upgrade_warns_but_commits_and_later_harvest_runs() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        let bytes = crate::replay::tests::turn(10, &[vec![11, 1800000, 0, 1], vec![2, 400000, 2]]);
        let notes = state.apply(&mut home, &EndClientTurn::parse(&bytes).unwrap(), &bytes);
        assert!(notes
            .iter()
            .any(|n| n.contains("tool amounts") && n.contains("known effects applied")));
        assert_eq!(home.avatar_stats[&1900006], 1);
        assert_eq!(home.avatar_stats[&400001], 2);
        assert_eq!(home.avatar_stats[&1900003], 1);
    }
    #[test]
    fn _buy_feed_list_then_feed_and_persist_settings_ambient() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900001, 10);
        let mut state = Replay::new(&home).unwrap();
        run(
            &mut state,
            &mut home,
            39,
            vec![
                json!([[1600000, 1]]),
                json!(-27),
                json!(-1),
                json!(-1),
                json!(1),
            ],
        );
        assert_eq!(home.avatar_stats[&1600000], 10);
        assert_eq!(home.avatar_stats[&1900001], 8);
        run(
            &mut state,
            &mut home,
            131,
            vec![json!(12000003), json!(120), json!(true), json!(2)],
        );
        run(
            &mut state,
            &mut home,
            50,
            vec![
                json!(4),
                json!(-1875),
                json!(11110),
                json!(0),
                json!(0),
                json!(false),
                json!(3),
            ],
        );
        let last = home.c["Objects"].as_array().unwrap().last().unwrap();
        assert_eq!(
            last,
            &json!({"ID":4500004,"X":-3,"Y":21,"AccurateX":-1875,"AccurateY":11110})
        );
        let message = home.message();
        assert_eq!(message.unlocked[&120], [8]);
        use crate::protocol::Message;
        assert_eq!(
            crate::protocol::messages::OwnHomeData::from_bytes(&message.to_bytes()).unwrap(),
            message
        );
        let saved: HomeData = serde_json::from_slice(&serde_json::to_vec(&home).unwrap()).unwrap();
        let mut replay = Replay::new(&saved).unwrap();
        let mut saved = saved;
        run(
            &mut replay,
            &mut saved,
            131,
            vec![json!(12000003), json!(120), json!(false), json!(4)],
        );
        assert!(!saved.message().unlocked.contains_key(&120));
    }
    #[test]
    fn expansion_purchases_charge_materials_once_and_roll_back_on_shortage() {
        use crate::protocol::stream::Writer;
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(crate::game::home::DIAMONDS, 300);
        let mut replay = Replay::new(&home).unwrap();
        let mut wire = Writer::new();
        wire.vint(10).vint(0).vint(2).vint(39).u8(3);
        for material in 3800006..=3800008 {
            wire.vint(material).vint(6);
        }
        wire.u8((-38i8) as u8).vint(8000007).vint(0).vint(1);
        wire.vint(90).vint(8000007).vint(2);
        let body = wire.into_bytes();
        let turn = EndClientTurn::parse(&body).unwrap();
        assert_eq!(turn.decoded.len(), 2);
        replay.apply(&mut home, &turn, &body);
        assert_eq!(home.avatar_stats[&crate::game::home::DIAMONDS], 84);
        for material in 3800006..=3800008 {
            assert_eq!(home.avatar_stats[&material], 1);
        }
        assert_eq!(home.message().unlocked[&80][0], 1 << 7);
        let mut replay = Replay::new(&home).unwrap();
        let body = crate::replay::tests::turn(20, &[vec![90, 8000007, 4], vec![90, 8000008, 5]]);
        let turn = EndClientTurn::parse(&body).unwrap();
        let notes = replay.apply(&mut home, &turn, &body);
        assert!(notes
            .iter()
            .any(|note| note.contains("insufficient inventory")));
        assert_eq!(home.message().unlocked[&80][0], 1 << 7);
        for material in 3800006..=3800008 {
            assert_eq!(home.avatar_stats[&material], 1);
        }
    }

    #[test]
    fn service_upgrade_and_pet_feed_use_table_materials() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.c["Objects"].as_array_mut().unwrap().extend([
            json!({"ID": 13400000, "X": 10, "Y": 10}),
            json!({"ID": 7900000, "X": 9, "Y": 9, "Bowls": [false, false, false]}),
        ]);
        let mut replay = Replay::new(&home).unwrap();
        let tools = ["Bolt", "Duct Tape", "Stone Block"]
            .map(|name| crate::game::starter::resolve(name).unwrap().0);
        home.avatar_stats.extend(tools.map(|id| (id, 3)));
        let milk = crate::game::starter::resolve("Milk").unwrap().0;
        home.avatar_stats.insert(milk, 2);
        run(
            &mut replay,
            &mut home,
            11,
            vec![json!(13400000), json!(1), json!(1)],
        );
        run(&mut replay, &mut home, 86, vec![json!(7900000), json!(2)]);
        for tool in tools {
            assert_eq!(home.avatar_stats[&tool], 1);
        }
        assert_eq!(home.avatar_stats[&milk], 1);
        assert_eq!(home.avatar_stats[&stat("PetFeedCount").unwrap()], 1);
        let reloaded = Replay::new(&home).unwrap();
        let index = reloaded.position(13400000).unwrap();
        assert_eq!(
            reloaded.objects[index].state["ServiceUpgrades"]["ServiceSlots"],
            1
        );
    }

    #[test]
    fn fruit_refills_offline_and_a_berry_can_only_be_picked_once() {
        let mut home = crate::game::starter::new_home().unwrap();
        let index = home.c["Objects"].as_array().unwrap().len();
        home.c["Objects"]
            .as_array_mut()
            .unwrap()
            .push(json!({"ID":6200003,"X":5,"Y":5,"Fruits":[false,false,false]}));
        let mut replay = Replay::new(&home).unwrap();
        let body = crate::replay::tests::turn(10, &[vec![60, 6200000, 0]]);
        replay.apply(&mut home, &EndClientTurn::parse(&body).unwrap(), &body);
        assert_eq!(
            home.c["Objects"][index]["Fruits"],
            json!([false, false, false])
        );
        let now = home.timer_saved_at_ms.unwrap() + 2 * 24 * 60 * 60 * 1000;
        let mut home = elapsed_home(home, now);
        assert_eq!(
            home.c["Objects"][index]["Fruits"],
            json!([true, true, true])
        );
        let mut replay = Replay::new(&home).unwrap();
        let mut w = crate::protocol::stream::Writer::new();
        w.vint(20).vint(0).vint(2);
        for tick in [1, 2] {
            w.vint(62).vint(6200000).raw(&0u16.to_be_bytes()).vint(tick);
        }
        let body = w.into_bytes();
        let notes = replay.apply(&mut home, &EndClientTurn::parse(&body).unwrap(), &body);
        assert!(notes.iter().any(|note| note.contains("already collected")));
        let (berry, _) = crate::game::starter::resolve("Blackberry").unwrap();
        assert_eq!(home.avatar_stats[&berry], 1);
        assert_eq!(home.avatar_stats[&stat("BerriesPicked").unwrap()], 1);
        assert_eq!(
            home.c["Objects"][index]["Fruits"],
            json!([false, true, true])
        );
    }

    #[test]
    fn _forest_tool_timer_collect_once_and_shop_base_sale() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut state = Replay::new(&home).unwrap();
        let i = state
            .objects
            .iter()
            .position(|o| {
                o.instance / 100000 == 5
                    && definition(o.value["ID"].as_u64().unwrap() as u32)
                        .unwrap()
                        .get("Tool")
                        .is_some()
            })
            .unwrap();
        let instance = state.objects[i].instance;
        let data = state.objects[i].value["ID"].as_u64().unwrap() as u32;
        let row = definition(data).unwrap();
        let tool = crate::game::starter::resolve(&row["Tool"]).unwrap().0;
        home.avatar_stats.insert(tool, 2);
        run(&mut state, &mut home, 18, vec![json!(instance), json!(1)]);
        assert_eq!(home.avatar_stats[&tool], 1);
        state.objects[i].state["Timer"] = json!({});
        run(&mut state, &mut home, 20, vec![json!(instance), json!(2)]);
        assert!(!state.objects.iter().any(|o| o.instance == instance));
        assert_eq!(home.avatar_stats[&stat("CounterForestCollect").unwrap()], 1);
        home.avatar_stats.insert(400001, 10);
        run(
            &mut state,
            &mut home,
            32,
            vec![
                json!(0),
                json!(400001),
                json!(3),
                json!(20),
                json!(false),
                json!(false),
                json!(3),
            ],
        );
        assert_eq!(home.avatar_stats[&400001], 7);
        let before = home.avatar_stats[&1900000];
        run(&mut state, &mut home, 31, vec![json!(0), json!(4)]);
        assert_eq!(home.avatar_stats[&1900000], before + 20);
    }
}
