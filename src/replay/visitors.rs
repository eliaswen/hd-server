//! Bounded visitor replay. World AI RNG drift prevents exact offers.
//! Visitors are session-only, never emitted into the farm Objects array.
use super::extended::bump;
use super::*;
use serde::Deserialize;
#[derive(Clone, Debug)]
struct Offer {
    item: u32,
    count: u32,
    price: u32,
    xp: u32,
}
#[derive(Clone, Debug, Default)]
pub(super) struct Visitors {
    seed: u32,
    next: u32,
    last_tick: u32,
    seconds: u64,
    next_spawn: u64,
    offers: BTreeMap<i32, Offer>,
    tutorial: BTreeSet<String>,
}
#[derive(Deserialize)]
struct Resource {
    id: u32,
    #[serde(rename = "type")]
    kind: u32,
    op: u32,
    ov: u32,
    ul: u32,
}
fn resources() -> &'static Vec<Resource> {
    static R: OnceLock<Vec<Resource>> = OnceLock::new();
    R.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("order-resources.json")).unwrap()
    })
}
fn config() -> &'static Row {
    &tables()["people_spawners"][0]
}
impl Visitors {
    pub(super) fn load(home: &HomeData) -> Self {
        let mut state = Self::default();
        if let Some(names) = home.replay_state["TutorialVisitors"].as_array() {
            state.tutorial = names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        if let Some(saved) = home.replay_state["VisitorSavedAtMs"].as_u64() {
            let elapsed = now_ms().saturating_sub(saved) / 1000;
            let count = (elapsed.saturating_mul(2)
                / u64::from(number(config(), "MaxSpawnTime").unwrap_or(900).max(1)))
            .min(u64::from(number(config(), "MaxPeople").unwrap_or(2)));
            let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
            if level >= number(config(), "UnlockLevel").unwrap_or(6)
                && (level >= number(config(), "ForcedUnlockLevel").unwrap_or(9)
                    || state.tutorial.len() >= 2)
            {
                if count > 0 {
                    tracing::warn!(
                        count,
                        "offline visitor population approximated; shared AI RNG is not observable"
                    );
                }
                for _ in 0..count {
                    let _ = state.spawn(home);
                }
            }
        }
        state
    }
    fn rand(&mut self, max: u32) -> u32 {
        if max == 0 {
            return 0;
        }
        let mut s = if self.seed == 0 { u32::MAX } else { self.seed };
        s ^= s << 13;
        s ^= ((s as i32) >> 17) as u32;
        s ^= s << 5;
        self.seed = s;
        (s as i32).unsigned_abs() % max
    }
    fn offer(&mut self, home: &HomeData, only_stock: bool) -> Result<Option<Offer>> {
        let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
        let cfg = config();
        let k = number(cfg, "GoodNumber")?.max(1) as usize;
        let requested: BTreeSet<_> = self.offers.values().map(|o| o.item).collect();
        let silo_count = self
            .offers
            .values()
            .filter(|o| [4, 61].contains(&(o.item / 100000)))
            .count();
        let barn_count = self.offers.len() - silo_count;
        let silo = if silo_count < barn_count {
            true
        } else if barn_count < silo_count {
            false
        } else {
            self.rand(2) == 0
        };
        let mut candidates: Vec<(u32, u32, u32, u32)> = Vec::new();
        for resource in resources() {
            if requested.contains(&resource.id)
                || resource.id == 400000
                || resource.ul > level + 1
                || [0, 17, 19, 77, 185].contains(&resource.kind)
            {
                continue;
            }
            if [4, 61].contains(&resource.kind) != silo {
                continue;
            }
            let (price, value) = if resource.kind == 22 {
                let row = definition(resource.id)?;
                let v = number(row, "DumbValue")?;
                (v, v)
            } else {
                (resource.op, resource.ov)
            };
            if price == 0 || value == 0 {
                continue;
            }
            let stock = home.avatar_stats.get(&resource.id).copied().unwrap_or(0);
            if only_stock && stock == 0 {
                continue;
            }
            candidates.push((resource.id, stock, price, value));
        }
        // On a completed deal, never remove an item the server doesn't have.
        if candidates.is_empty() && only_stock {
            for r in resources() {
                let stock = home.avatar_stats.get(&r.id).copied().unwrap_or(0);
                if stock > 0
                    && r.op > 0
                    && !requested.contains(&r.id)
                    && ![19, 17, 77, 185].contains(&r.kind)
                {
                    candidates.push((r.id, stock, r.op, r.ov.max(1)));
                }
            }
        }
        let mut slots: Vec<Option<usize>> = vec![None; k];
        let mut stocks = vec![0; k];
        for (i, (_, stock, _, _)) in candidates.iter().enumerate() {
            let min = *stocks.iter().min().unwrap();
            if *stock > min {
                let j = stocks.iter().position(|s| *s == min).unwrap();
                stocks[j] = *stock;
                slots[j] = Some(i);
            }
        }
        let roll = self.rand(100);
        let available = slots.iter().any(Option::is_some);
        let picked = if available && (only_stock || roll < number(cfg, "CanCompleteOrderChance")?) {
            let start = self.rand(k as u32) as usize;
            let index = (0..k).find_map(|j| slots[(start + j) % k]).unwrap();
            let min = number(cfg, "MinOrderSize")?;
            let pct = min + self.rand(number(cfg, "MaxOrderSize")?.saturating_sub(min));
            let (id, stock, price, _) = candidates[index];
            let count = ((u64::from(pct) * u64::from(stock) / 100) as u32)
                .max(1)
                .min((u64::from(stock) * 65).div_ceil(100) as u32);
            Some((id, count, price))
        } else {
            let empty: Vec<_> = candidates
                .iter()
                .filter(|(id, s, _, _)| *s == 0 && *id / 100000 != 22)
                .copied()
                .collect();
            if empty.is_empty() {
                None
            } else {
                let (id, _, price, value) = empty[self.rand(empty.len() as u32) as usize];
                let count = (number(level_row(level)?, "GuestOrderValue")? / value.max(1)).max(1);
                Some((id, count, price))
            }
        };
        Ok(picked.map(|(item, count, price)| Offer {
            item,
            count,
            price: (u64::from(price)
                * u64::from(count)
                * u64::from(number(cfg, "GoldMultiplier").unwrap_or(140))
                / 100) as u32,
            xp: number(cfg, "ConstantExp").unwrap_or(5),
        }))
    }
    fn spawn(&mut self, home: &HomeData) -> Result<bool> {
        if let Some(offer) = self.offer(home, false)? {
            let id = 4900000 + self.next;
            self.next += 1;
            self.offers.insert(id as i32, offer);
            // Person choice and idle duration also consume the shared RNG.
            let count = tables()["people"]
                .iter()
                .filter(|r| r.get("Name").is_some_and(|s| !s.is_empty()))
                .count() as u32;
            let _person = self.rand(count);
            let _idle_multiplier = self.rand(3) + 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
impl Replay {
    pub(super) fn tick_visitors(&mut self, home: &HomeData) -> Result<()> {
        if !self.visitors.offers.is_empty() {
            self.extra["VisitorSavedAtMs"] = json!(now_ms());
        }
        let tick = self.clock_tick;
        let elapsed = tick.saturating_sub(self.visitors.last_tick);
        self.visitors.last_tick = tick;
        self.visitors.seconds += u64::from(elapsed);
        let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1);
        let c = config();
        if level < number(c, "UnlockLevel")?
            || (level < number(c, "ForcedUnlockLevel")?
                && self.visitors.tutorial.len()
                    < number(c, "MinTutorialPeopleToSpawnBeforeUnlock")? as usize)
        {
            return Ok(());
        }
        if self.visitors.next_spawn == 0 {
            self.visitors.next_spawn = u64::from(number(c, "MinSpawnTime")?) * 30;
        }
        if self.visitors.seconds >= self.visitors.next_spawn
            && self.visitors.offers.len() < number(c, "MaxPeople")? as usize
        {
            self.visitors.seconds = 0;
            let min = number(c, "MinSpawnTime")?;
            let delay = min
                + self
                    .visitors
                    .rand(number(c, "MaxSpawnTime")?.saturating_sub(min));
            self.visitors.next_spawn = u64::from(delay) * 30;
            if self.visitors.spawn(home)? {
                tracing::warn!("visitor spawn/offer approximated: shared world-AI RNG, candidate pools and spawn clock drift are not observable");
            }
        }
        Ok(())
    }
    pub(super) fn start_tutorial_visitor(&mut self, home: &HomeData, id: i32) -> Result<()> {
        let row = definition(id as u32)?;
        if let Some(name) = row.get("PeopleOrder").filter(|s| !s.is_empty()) {
            if self.visitors.tutorial.insert(name.clone()) {
                self.extra["TutorialVisitors"] =
                    json!(self.visitors.tutorial.iter().collect::<Vec<_>>());
                let row = tables()["tutorial_people_orders"]
                    .iter()
                    .find(|r| r.get("Name") == Some(name))
                    .context("tutorial visitor order missing")?;
                let item = crate::game::starter::resolve(&row["Good"])?.0;
                let count = number(row, "GoodAmount")?;
                // Tutorial offers pay the plain OrderPrice; only generated offers apply GoldMultiplier.
                let price = resources()
                    .iter()
                    .find(|r| r.id == item)
                    .map(|r| r.op)
                    .unwrap_or(0)
                    * count;
                let serial = 4900000 + self.visitors.next;
                self.visitors.next += 1;
                self.visitors.offers.insert(
                    serial as i32,
                    Offer {
                        item,
                        count,
                        price,
                        xp: number(config(), "ConstantExp")?,
                    },
                );
                self.unknown("tutorial visitor offer tracked; client AI person/instance sequence remains inferred");
            }
        }
        let _ = home;
        Ok(())
    }
    pub(super) fn decline_visitor(&mut self, id: i32) {
        self.visitors.offers.remove(&id);
    }
    pub(super) fn complete_visitor(&mut self, home: &mut HomeData, id: i32) -> Result<()> {
        self.unknown(format!("visitor {id}:  bounded offer approximation; shared AI RNG prevents exact item/count/price parity"));
        let mut offer = self.visitors.offers.remove(&id);
        if offer.is_none() {
            offer = self.visitors.offers.pop_first().map(|(_, offer)| offer);
        }
        if (4_900_000..5_000_000).contains(&id) {
            self.visitors.next = self.visitors.next.max((id - 4900000) as u32 + 1);
        }
        if offer.as_ref().is_none_or(|o| {
            let stock = home.avatar_stats.get(&o.item).copied().unwrap_or(0);
            o.count > (u64::from(stock) * 65).div_ceil(100) as u32
        }) {
            offer = self.visitors.offer(home, true)?;
        }
        if let Some(offer) = offer {
            if offer.count > 0 {
                remove(&mut home.avatar_stats, offer.item, offer.count)?;
            }
            add(&mut home.avatar_stats, 1900000, offer.price)?;
            award_xp(&mut home.avatar_stats, offer.xp)?;
            bump(home, "PeopleOrderCompleteCount", 1)?;
        } else {
            self.unknown(
                "completed visitor had no eligible server stock; no item/currency invented",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn _bounded_sale_decline_and_offline_population() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 9);
        home.avatar_stats.insert(400001, 100);
        let mut replay = Replay::new(&home).unwrap();
        let before = home.avatar_stats[&1900000];
        replay.complete_visitor(&mut home, 4900004).unwrap();
        replay.save_objects(&mut home);
        let used = 100 - home.avatar_stats[&400001];
        assert!((1..=65).contains(&used));
        let wheat = resources().iter().find(|r| r.id == 400001).unwrap();
        assert_eq!(
            home.avatar_stats[&1900000],
            before + wheat.op * used * 140 / 100
        );
        assert_eq!(home.avatar_stats[&1900003], 5);
        assert_eq!(home.avatar_stats[&1900025], 1);
        assert!(replay.warnings.iter().any(|s| s.contains("approximation")));
        assert!(!home.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["ID"].as_u64().unwrap() / 100000 == 49));
        let before = home.avatar_stats.clone();
        replay.decline_visitor(4900001);
        assert_eq!(home.avatar_stats, before);
        home.replay_state["VisitorSavedAtMs"] = json!(now_ms().saturating_sub(1_800_000));
        let visitors = Visitors::load(&home);
        assert_eq!(visitors.offers.len(), 2);
    }
    #[test]
    fn _tutorial_offer_pays_plain_order_price() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(2400000, 5);
        let mut replay = Replay::new(&home).unwrap();
        replay.start_tutorial_visitor(&home, 3600027).unwrap();
        let before = home.avatar_stats[&1900000];
        replay.complete_visitor(&mut home, 4900000).unwrap();
        assert_eq!(home.avatar_stats[&1900000], before + 11);
        assert_eq!(home.avatar_stats[&2400000], 4);
    }
    #[test]
    fn _tutorial_offer_is_tracked_once_and_declined() {
        let mut home = crate::game::starter::new_home().unwrap();
        let mut replay = Replay::new(&home).unwrap();
        replay.start_tutorial_visitor(&home, 3600016).unwrap();
        assert_eq!(replay.visitors.offers.len(), 1);
        replay.start_tutorial_visitor(&home, 3600019).unwrap();
        assert_eq!(replay.visitors.offers.len(), 1);
        replay.decline_visitor(4900000);
        assert!(replay.visitors.offers.is_empty());
        replay.save_objects(&mut home);
        assert_eq!(home.replay_state["TutorialVisitors"], json!(["A"]));
    }
}
