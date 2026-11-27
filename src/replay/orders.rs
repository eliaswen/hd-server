//! Reference order generator. Base payouts are credited at command 28;
//! command 41 never duplicates them. Immediate completion refills the order
//! board, while trash and skip timers are modeled. Event/voucher bonuses are
//! zero without events;
//! mastery persistence and non-board unlock targets remain unmodeled.
use super::*;
use serde::Deserialize;

#[derive(Clone, Copy, Debug)]
struct LogicRandom(i32);
impl LogicRandom {
    fn rand(&mut self, max: i32) -> i32 {
        if max <= 0 {
            return 0;
        }
        let mut s = if self.0 == 0 { -1 } else { self.0 };
        s ^= s.wrapping_shl(13);
        s ^= s >> 17; // Signed arithmetic shift, deliberately not logical.
        s ^= s.wrapping_shl(5);
        self.0 = s;
        (if s < 0 { s.wrapping_neg() } else { s }) % max
    }
    fn index(&mut self, len: usize) -> Result<usize> {
        let index = self.rand(i32::try_from(len)?);
        ensure!(
            index >= 0,
            " RNG INT_MIN yields a negative index; behavior unmodeled"
        );
        Ok(index as usize)
    }
}
#[derive(Deserialize)]
struct Resource {
    id: u32,
    #[serde(rename = "type")]
    kind: u32,
    ov: i64,
    op: i64,
    oe: i64,
    ul: u32,
}
pub(super) fn preload() {
    resources();
}

fn resources() -> &'static [Resource] {
    static DATA: OnceLock<Vec<Resource>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(&crate::data::resource_data::load("order-resources.json")).unwrap()
    })
}
fn groups(table: &str) -> Vec<(&Row, Vec<&Row>)> {
    let mut out: Vec<(&Row, Vec<&Row>)> = Vec::new();
    for row in &tables()[table] {
        if row.get("Name").is_some_and(|s| !s.is_empty()) {
            out.push((row, vec![row]));
        } else if let Some((_, rows)) = out.last_mut() {
            rows.push(row);
        }
    }
    out
}
fn excluded(kind: u32) -> bool {
    (kind | 1) == 0xb9
        || [0x96, 0x4d, 0x4b, 0x26, 0x10, 0x16].contains(&kind)
        || (kind | 1) == 0x41
        || (kind | 2) == 0x13
}
fn active(order: &Value) -> bool {
    order["Datas"].as_array().is_some_and(|a| !a.is_empty())
}
/// An empty slot as the client saves it. The order reader (x86 0x2002d6)
/// treats a non-null `Timer` object as "empty/cooling down" and skips the
/// rest; with `Timer` null or absent it parses a full order and calls size()
/// on `Datas`/`Amounts` without a null check, segfaulting at 0x3bf044.
pub(super) fn empty_slot(level: u32) -> Value {
    json!({"Lvl":level,"Timer":{}})
}
/// Give every inactive slot a `Timer` object so the client never takes the
/// full-order path for it (see [`empty_slot`]).
pub(super) fn normalize_empty_timers(orders: &mut [Value]) {
    for order in orders {
        if !active(order) && order.is_object() && !order["Timer"].is_object() {
            order["Timer"] = json!({});
        }
    }
}
/// Order receivers are saved as global IDs (`orders.csv`, kind 34), as the
/// client writes them; a bare row index displays as receiver row 0.
const RECEIVER_BASE: u64 = 3_400_000;
fn receiver(row: &Row) -> Result<Option<usize>> {
    row.get("OrderReceiver")
        .filter(|s| !s.is_empty())
        .map(|name| {
            groups("orders")
                .iter()
                .position(|(r, _)| r.get("Name") == Some(name))
                .context("unknown order receiver")
        })
        .transpose()
}
fn goods(rows: &[&Row]) -> Result<Vec<(u32, u32)>> {
    rows.iter()
        .filter(|r| r.get("Goods").is_some_and(|s| !s.is_empty()))
        .map(|r| {
            Ok((
                crate::game::starter::resolve(&r["Goods"])?.0,
                number(r, "GoodAmounts")?,
            ))
        })
        .collect()
}
fn generate(
    rng: &mut LogicRandom,
    level: u32,
    orders: &[Value],
    done: &mut u32,
    special_count: usize,
) -> Result<(Value, bool)> {
    let lv = level_row(level)?;
    let receivers = groups("orders").len();
    ensure!(receivers > 0, "no order receivers");
    let mut available: Vec<usize> = (0..receivers).collect();
    for order in orders.iter().filter(|o| active(o)) {
        if let Some(id) = order["Receiver"].as_u64() {
            available.retain(|&r| r != id as usize % receivers);
        }
    }
    let mut recv = if available.is_empty() {
        0
    } else {
        // 0x63bacc passes the random index itself to the receiver table, not
        // the free receiver at that index: the free list only sets the bound
        // (live-verified with GDB). The pick can repeat an active receiver.
        rng.index(available.len())?
    };
    let cem = number(lv, "CashExpMultiplier")? as i32;
    let shift = if cem > 0 {
        rng.rand(cem.checked_mul(2).context("order split overflow")?) - cem
    } else {
        cem
    };
    let mut items = Vec::new();
    let mut is_special = false;
    for (i, (row, rows)) in groups("predefined_orders").iter().enumerate() {
        ensure!(i < 32, "predefined order bitset exceeds one word");
        if *done & (1 << i) == 0 {
            *done |= 1 << i;
            items = goods(rows)?;
            if let Some(r) = receiver(row)? {
                recv = r;
            }
            break;
        }
    }
    if items.is_empty()
        && special_count < number(lv, "SpecialOrderMaxCount")? as usize
        && rng.rand(100) <= number(lv, "SpecialOrderProbablity")? as i32
    {
        let eligible: Vec<_> = groups("special_orders")
            .into_iter()
            .filter(|(r, _)| {
                r.get("Probability").is_some_and(|s| !s.is_empty())
                    && number(r, "UnlockLevel").is_ok_and(|n| n <= level)
            })
            .collect();
        let total = eligible.iter().try_fold(0u32, |s, (r, _)| -> Result<u32> {
            s.checked_add(number(r, "Probability")?)
                .context("special probability overflow")
        })?;
        if total > 0 {
            let pick = rng.rand(i32::try_from(total)?) as u32;
            let mut cumulative = 0;
            for (row, rows) in eligible {
                cumulative += number(row, "Probability")?;
                if pick >= cumulative {
                    continue;
                }
                let choices = goods(&rows)?;
                ensure!(!choices.is_empty(), "empty special order goods");
                let target = i64::from(number(lv, "OrderMaxValue")?)
                    * i64::from(number(row, "OrderValueMultiplier")?)
                    / 100;
                let mut value = 0;
                for attempt in 0..100_000 {
                    let id = choices[rng.index(choices.len())?].0;
                    let resource = resources()
                        .iter()
                        .find(|r| r.id == id)
                        .context("special good missing from resource table")?;
                    ensure!(resource.ov > 0, "special order good has no value");
                    if let Some((_, amount)) = items.iter_mut().find(|(d, _)| *d == id) {
                        *amount += 1;
                    } else {
                        items.push((id, 1));
                    }
                    value += resource.ov;
                    if value >= target {
                        break;
                    }
                    ensure!(attempt < 99_999, "special order amount limit");
                }
                if let Some(r) = receiver(row)? {
                    recv = r;
                }
                is_special = true;
                break;
            }
        }
    }
    if items.is_empty() {
        let min = number(lv, "MinGoodsInOrderDelivery")?;
        let max = number(lv, "MaxGoodsInOrderDelivery")?;
        let count = min + rng.rand(i32::try_from(max.saturating_sub(min))?) as u32;
        let min_value = number(lv, "OrderMinValue")?;
        let max_value = number(lv, "OrderMaxValue")?;
        let target =
            min_value + rng.rand(i32::try_from(max_value.saturating_sub(min_value))?) as u32;
        let mut candidates: Vec<_> = resources()
            .iter()
            .filter(|r| !excluded(r.kind) && r.id != 400000 && r.ul <= level)
            .collect();
        ensure!(
            !candidates.is_empty() && count > 0,
            "unable to generate order"
        );
        for _ in 0..count {
            let r = candidates.remove(rng.index(candidates.len())?);
            items.push((r.id, 0));
            if candidates.is_empty() {
                break;
            }
        }
        let per_pass: i64 = items
            .iter()
            .map(|(id, _)| resources().iter().find(|r| r.id == *id).unwrap().ov)
            .sum();
        ensure!(per_pass > 0, "order candidates have zero total value");
        let amount = ((i64::from(target) + per_pass - 1) / per_pass).max(1);
        for (_, n) in &mut items {
            *n = u32::try_from(amount)?;
        }
    }
    let mut cash = 0i64;
    let mut xp = 0i64;
    for (id, amount) in &items {
        let r = resources()
            .iter()
            .find(|r| r.id == *id)
            .context("order good missing from resource table")?;
        cash += r.op * i64::from(*amount);
        xp += r.oe * i64::from(*amount);
    }
    let split = |base: i64, percent: i32| -> Result<u32> {
        let product = base
            .checked_mul(i64::from(percent))
            .context("order reward overflow")?;
        u32::try_from(if product < 100 { 1 } else { product / 100 })
            .context("order reward outside uint32")
    };
    Ok((
        json!({"Lvl":level,"Cash":split(cash,100+shift)?,"Exp":split(xp,100-shift)?,
        "CashExpMultiplier":shift,"Datas":items.iter().map(|(id,_)| id).collect::<Vec<_>>(),
        "Amounts":items.iter().map(|(_,n)| n).collect::<Vec<_>>(),"Receiver":RECEIVER_BASE+recv as u64}),
        is_special,
    ))
}

/// The client keeps the uncollected truck reward on the truck (object
/// 200000): `State` 3 with `Cash`/`Exp`; after collection `State` 1 and no
/// reward keys (live GDB farm dumps).
fn set_truck_reward(objects: &mut Vec<Value>, reward: Option<&Value>) {
    // Stored farms usually lack the truck: the client creates it at load at
    // this position (live dumps). A pending reward needs it present.
    if reward.is_some() && !objects.iter().any(|o| o["ID"] == 200000) {
        objects.push(json!({"ID":200000,"X":17,"Y":21,"State":1,"VoucherCount":1}));
    }
    for truck in objects.iter_mut().filter(|o| o["ID"] == 200000) {
        let Some(truck) = truck.as_object_mut() else { continue };
        match reward {
            Some(r) => {
                truck.insert("State".into(), json!(3));
                truck.insert("Cash".into(), r["Cash"].clone());
                truck.insert("Exp".into(), r["Exp"].clone());
            }
            None => {
                truck.insert("State".into(), json!(1));
                truck.remove("Cash");
                truck.remove("Exp");
            }
        }
    }
}
/// Prepare only the normal own-home copy. Persistence happens on the next turn.
/// Verified board position; seed 0 matches the initial client-created board.
/// Other client-created special objects remain untouched.
pub(crate) fn prepare(mut home: HomeData) -> Result<HomeData> {
    let pending = home.replay_state["OrderReward"].clone();
    let objects = home
        .c
        .get_mut("Objects")
        .and_then(Value::as_array_mut)
        .context("farm Objects missing")?;
    if !objects.iter().any(|o| o["ID"] == 3200000) {
        objects.push(json!({"ID":3200000,"X":14,"Y":19,"Seed":0,"Orders":vec![json!({"Lvl":1,"Timer":{}});9]}));
    }
    // Boards saved by older server builds hold bare receiver row indices.
    for board in objects.iter_mut().filter(|o| o["ID"] == 3200000) {
        if let Some(orders) = board.get_mut("Orders").and_then(Value::as_array_mut) {
            for order in orders {
                if let Some(id) = order["Receiver"].as_u64().filter(|&id| id < RECEIVER_BASE) {
                    order["Receiver"] = json!(RECEIVER_BASE + id);
                }
            }
        }
    }
    set_truck_reward(objects, pending.is_object().then_some(&pending));
    let mut replay = Replay::new(&home)?;
    replay.fill_orders(&mut home)?;
    replay.save_objects(&mut home);
    Ok(home)
}
impl Replay {
    pub(super) fn fill_orders(&mut self, home: &mut HomeData) -> Result<()> {
        // Snapshot preparation may generate without writing the stored home.
        // Preserve its bitset/classification when the first turn is committed.
        home.predefined_orders = self.predefined_orders;
        home.special_order_slots = self.special_order_slots.clone();
        home.order_board_unlocked = self.order_board_unlocked;
        let Some(index) = self.objects.iter().position(|o| o.value["ID"] == 3200000) else {
            return Ok(());
        };
        let level = home.avatar_stats.get(&1900002).copied().unwrap_or(1).max(1);
        let limit = (number(level_row(level)?, "MaxOrderCount")? as usize).min(9);
        let board = &mut self.objects[index].value;
        let mut slots = board
            .get("Orders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        slots.resize(9, empty_slot(level));
        slots.truncate(9);
        for (i, slot) in slots.iter_mut().enumerate().skip(limit) {
            *slot = empty_slot(level);
            home.special_order_slots.remove(&i);
        }
        let seed = board["Seed"].as_i64().unwrap_or(0);
        ensure!(
            (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&seed),
            "order seed outside 32 bits"
        );
        let mut rng = LogicRandom(seed as i32);
        for i in 0..limit {
            if active(&slots[i]) || timers::remaining(&slots[i]["Timer"]) != 0 {
                continue;
            }
            let seed_before = rng.0;
            let (order, special) = generate(
                &mut rng,
                level,
                &slots,
                &mut home.predefined_orders,
                home.special_order_slots.len(),
            )?;
            tracing::debug!(
                slot = i,
                level,
                seed_before,
                seed_after = rng.0,
                predefined_bits = home.predefined_orders,
                special,
                "generated truck order using the modeled algorithm"
            );
            slots[i] = order;
            if special {
                home.special_order_slots.insert(i);
            } else {
                home.special_order_slots.remove(&i);
            }
        }
        normalize_empty_timers(&mut slots);
        board["Orders"] = json!(slots);
        board["Seed"] = json!(rng.0);
        self.predefined_orders = home.predefined_orders;
        self.special_order_slots = home.special_order_slots.clone();
        Ok(())
    }
    fn order_slot(&self, slot: i32) -> Result<(usize, usize)> {
        let board = self
            .objects
            .iter()
            .position(|o| o.value["ID"] == 3200000)
            .context("order board missing from loaded home")?;
        let slot = usize::try_from(slot).context("negative order slot")?;
        ensure!(
            self.objects[board].value["Orders"]
                .as_array()
                .is_some_and(|a| slot < a.len()),
            "order slot outside board"
        );
        Ok((board, slot))
    }
    pub(super) fn trash_order(&mut self, home: &mut HomeData, slot: i32) -> Result<()> {
        let (board, slot) = self.order_slot(slot)?;
        let order = &self.objects[board].value["Orders"][slot];
        ensure!(active(order), "cannot trash an empty order slot");
        let level = u32::try_from(order["Lvl"].as_u64().context("order level missing")?)?;
        let seconds = number(level_row(level)?, "OrderCancelTimeMin")?
            .checked_mul(60)
            .context("order cancel duration overflow")?;
        self.objects[board].value["Orders"][slot] =
            json!({"Lvl":level,"Timer":timers::new(seconds)});
        self.special_order_slots.remove(&slot);
        home.special_order_slots.remove(&slot);
        self.fill_orders(home)?;
        Ok(())
    }
    pub(super) fn speed_up_order(&mut self, home: &mut HomeData, slot: i32) -> Result<()> {
        let (board, slot) = self.order_slot(slot)?;
        let order = &self.objects[board].value["Orders"][slot];
        ensure!(!active(order), "cannot speed up an active order");
        let remaining = timers::remaining(&order["Timer"]);
        let level = u32::try_from(order["Lvl"].as_u64().context("order level missing")?)?;
        let row = level_row(level)?;
        let cost = if home.avatar_stats.get(&1900002).copied().unwrap_or(1)
            < replay_config()["FREE_DIAMOND_SPEED_UP_BEFORE_LEVEL"]
        {
            0
        } else {
            let total = u64::from(number(row, "OrderCancelTimeMin")?)
                .checked_mul(60)
                .context("cancel duration overflow")?
                .max(1);
            let seconds = (remaining * 66 / 1000).max(1);
            u32::try_from(
                (u64::from(number(row, "OrderSpeedUpPrice")?) * seconds)
                    .div_ceil(total)
                    .max(1),
            )?
        };
        spend_diamonds(&mut home.avatar_stats, cost)?;
        self.objects[board].value["Orders"][slot]["Timer"] = json!({});
        self.fill_orders(home)?;
        Ok(())
    }
    pub(super) fn acknowledge_order_unlock(
        &mut self,
        home: &mut HomeData,
        level: i32,
    ) -> Result<()> {
        let current = home.avatar_stats.get(&1900002).copied().unwrap_or(1).max(1);
        ensure!(
            (0..=700).contains(&level) && level as u32 <= current,
            "invalid acknowledged level"
        );
        // The observed command identifies the board target only. Its transient state+0x88 has
        // no established saved JSON key; preserve acknowledgement as metadata.
        if number(level_row(current)?, "MaxOrderCount")? > 0
            && self.objects.iter().any(|o| o.value["ID"] == 3200000)
        {
            self.order_board_unlocked = true;
            home.order_board_unlocked = true;
        }
        if let Some(tom) = tables()["features"]
            .iter()
            .find(|r| r.get("Name").is_some_and(|n| n == "Boy"))
        {
            if current + 1 >= number(tom, "UnlockLevel")?
                && self.extra["Tom"]["State"].as_u64().unwrap_or(9) == 9
            {
                if !self.extra["Tom"].is_object() {
                    self.extra["Tom"] = json!({});
                }
                self.extra["Tom"]["State"] = json!(1);
            }
        }
        self.unknown("known board/Tom unlock state applied; remaining unlock target and client state JSON mapping untraced");
        self.fill_orders(home)?;
        Ok(())
    }
    pub(super) fn complete_order(&mut self, home: &mut HomeData, slot: i32) -> Result<()> {
        self.fill_orders(home)?;
        let index = self
            .objects
            .iter()
            .position(|o| o.value["ID"] == 3200000)
            .context("order board missing from loaded home")?;
        let slot = usize::try_from(slot).context("negative order index")?;
        let order = self.objects[index].value["Orders"]
            .as_array()
            .and_then(|a| a.get(slot))
            .context("order slot outside board")?
            .clone();
        ensure!(active(&order), "order slot is not active");
        // Native 28 obtains the selected order before collecting the previous
        // stash. A payout can level up; refill only after this order is cleared.
        self.collect_order_reward(home)?;
        // : no server events => zero event/voucher terms. Mastery bonuses
        // are zero for new farms; star/hour persistence remains unmodeled.
        // Don't invent nonzero event/voucher/help payouts on imported orders.
        for key in [
            "Voucher",
            "BonusRewardEnabled",
            "BonusCount",
            "HID",
            "HRD",
            "HC",
            "HG",
        ] {
            if !(order[key].is_null() || order[key] == false || order[key] == 0) {
                self.unknown(format!(
                    "order {key} bonus/help effect unknown; applying base completion"
                ));
            }
        }
        let ids = order["Datas"].as_array().context("order Datas missing")?;
        let amounts = order["Amounts"]
            .as_array()
            .context("order Amounts missing")?;
        ensure!(
            ids.len() == amounts.len(),
            "order goods/amounts length mismatch"
        );
        for (id, amount) in ids.iter().zip(amounts) {
            remove(
                &mut home.avatar_stats,
                u32::try_from(id.as_u64().context("invalid order item")?)?,
                u32::try_from(amount.as_u64().context("invalid order amount")?)?,
            )?;
        }
        self.extra["OrderReward"] = json!({"Cash":order["Cash"],"Exp":order["Exp"]});
        self.sync_truck()?;
        self.objects[index].value["Orders"][slot] =
            empty_slot(home.avatar_stats.get(&1900002).copied().unwrap_or(1));
        home.special_order_slots.remove(&slot);
        self.special_order_slots.remove(&slot);
        self.fill_orders(home)?;
        Ok(())
    }
    pub(super) fn collect_order_reward(&mut self, home: &mut HomeData) -> Result<()> {
        if let Some(reward) = self
            .extra
            .get("OrderReward")
            .filter(|v| !v.is_null())
            .cloned()
        {
            add(
                &mut home.avatar_stats,
                1900000,
                reward["Cash"].as_u64().unwrap_or(0).try_into()?,
            )?;
            award_xp(
                &mut home.avatar_stats,
                reward["Exp"].as_u64().unwrap_or(0).try_into()?,
            )?;
            self.extra.as_object_mut().unwrap().remove("OrderReward");
            self.sync_truck()?;
        }
        Ok(())
    }
    /// Mirror the server's pending reward onto the truck object.
    fn sync_truck(&mut self) -> Result<()> {
        let reward = self.extra.get("OrderReward").filter(|v| v.is_object()).cloned();
        if reward.is_some() && !self.objects.iter().any(|o| o.value["ID"] == 200000) {
            self.create(json!({"ID":200000,"X":17,"Y":21,"State":1,"VoucherCount":1}))?;
        }
        for o in self.objects.iter_mut().filter(|o| o.value["ID"] == 200000) {
            let mut one = vec![o.value.clone()];
            set_truck_reward(&mut one, reward.as_ref());
            o.value = one.pop().unwrap();
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_order_slots_never_send_null_timer() {
        // Null Timer + no Datas/Amounts segfaults the client's order reader.
        let check = |home: &HomeData| {
            let board = home.c["Objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["ID"] == 3200000)
                .unwrap();
            for order in board["Orders"].as_array().unwrap() {
                assert!(active(order) || order["Timer"].is_object(), "{order}");
            }
        };
        let fresh = prepare(crate::game::starter::new_home().unwrap()).unwrap();
        check(&fresh);
        // Saved homes written by the buggy server are repaired on send.
        let mut legacy = fresh.clone();
        for o in legacy.c["Objects"].as_array_mut().unwrap() {
            if o["ID"] == 3200000 {
                o["Orders"] = json!(vec![json!({"Lvl":3,"Timer":null}); 9]);
            }
        }
        check(&prepare(legacy).unwrap());
    }
    fn apply(
        replay: &mut Replay,
        home: &mut HomeData,
        tick: u32,
        commands: &[Vec<u32>],
    ) -> Vec<String> {
        let bytes = crate::replay::tests::turn(tick, commands);
        replay.apply(home, &EndClientTurn::parse(&bytes).unwrap(), &bytes)
    }
    fn board(home: &HomeData) -> &Value {
        home.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ID"] == 3200000)
            .unwrap()
    }
    fn farm(level: u32) -> HomeData {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, level);
        prepare(home).unwrap()
    }
    #[test]
    fn _trash_uses_order_level_and_preserves_seed_until_timer_finishes() {
        let mut home = farm(10);
        home.avatar_stats.insert(1900002, 11); // The order itself is still level 10.
        let mut replay = Replay::new(&home).unwrap();
        let seed = board(&home)["Seed"].clone();
        let stats = home.avatar_stats.clone();
        assert!(apply(&mut replay, &mut home, 0, &[vec![27, 0, 0]]).is_empty());
        assert_eq!(
            board(&home)["Orders"][0],
            json!({"Lvl":10,"Timer":{"StartSeconds":420,"TicksLeft":6363}})
        );
        assert_eq!(board(&home)["Seed"], seed);
        assert_eq!(home.avatar_stats, stats);
        let saved = home.clone();
        let offline = elapsed_home(saved.clone(), saved.timer_saved_at_ms.unwrap() + 420_000);
        assert!(active(&board(&prepare(offline).unwrap())["Orders"][0]));
        assert!(apply(&mut replay, &mut home, 12597, &[]).is_empty());
        assert!(!active(&board(&home)["Orders"][0]));
        assert_eq!(board(&home)["Seed"], seed);
        assert!(apply(&mut replay, &mut home, 12600, &[]).is_empty());
        assert!(active(&board(&home)["Orders"][0]));
        assert_eq!(board(&home)["Orders"][0]["Lvl"], 11);
        assert_ne!(board(&home)["Seed"], seed);
        assert_eq!(home.avatar_stats, stats);
    }
    #[test]
    fn _speed_up_cost_refills_and_failure_keeps_timer() {
        let mut home = farm(10);
        home.avatar_stats.insert(1900001, 10);
        let mut replay = Replay::new(&home).unwrap();
        assert!(apply(&mut replay, &mut home, 0, &[vec![27, 0, 0]]).is_empty());
        assert!(apply(&mut replay, &mut home, 6300, &[]).is_empty());
        let seed = board(&home)["Seed"].clone();
        assert!(apply(&mut replay, &mut home, 6301, &[vec![29, 0, 6301]]).is_empty());
        assert_eq!(home.avatar_stats[&1900001], 8);
        assert!(active(&board(&home)["Orders"][0]));
        assert_ne!(board(&home)["Seed"], seed);
        assert!(apply(&mut replay, &mut home, 6302, &[vec![27, 0, 6302]]).is_empty());
        home.avatar_stats.insert(1900001, 0);
        home.avatar_stats.insert(1900015, 0);
        let before = board(&home).clone();
        let notes = apply(
            &mut replay,
            &mut home,
            6302,
            &[vec![29, 0, 6302], vec![2, 400000, 6302]],
        );
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("insufficient diamonds"));
        assert_eq!(board(&home), &before);
        assert_eq!(home.avatar_stats[&400001], 2); // Later harvest still executes.
    }
    #[test]
    fn _acknowledgement_has_no_rewards_and_visitor_is_explicitly_unmodeled() {
        let mut home = farm(4);
        let mut replay = Replay::new(&home).unwrap();
        let stats = home.avatar_stats.clone();
        // Cmd 33 encodes its level as BE16, not a VInt.
        assert!(apply(&mut replay, &mut home, 1, &[vec![33, 0, 4, 1]])
            .iter()
            .any(|n| n.contains("known effects applied")));
        assert!(home.order_board_unlocked);
        assert_eq!(home.avatar_stats, stats);
        assert_eq!(prepare(home.clone()).unwrap(), home);
        assert!(!apply(&mut replay, &mut home, 2, &[vec![33, 0, 5, 2]]).is_empty());
        let notes = apply(&mut replay, &mut home, 3, &[vec![40, 4900000, 3]]);
        assert!(!notes.is_empty());
        assert!(notes[0].contains("complete visitor deal"));
        assert!(notes[0].contains("4900000"));
        assert_eq!(home.avatar_stats, stats);
        assert!(apply(&mut replay, &mut home, 4, &[vec![2, 400000, 4]]).is_empty());
        assert_eq!(home.avatar_stats[&400001], 2);
    }
    #[test]
    fn signed_rng_zero_and_nonpositive_bounds() {
        let mut r = LogicRandom(0);
        assert_eq!(r.rand(0), 0);
        assert_eq!(r.0, 0);
        assert_eq!(r.rand(-1), 0);
        assert_eq!(r.0, 0);
        // First zero-seed step starts from -1 and uses SAR, not SHR.
        assert_eq!(r.rand(100), 83);
        assert_eq!(r.0, 253983);
    }
    #[test]
    fn ready_slot_timer_and_special_limit_are_tracked() {
        assert_eq!(resources().len(), 608);
        assert_eq!(
            resources()
                .iter()
                .filter(|r| !excluded(r.kind) && r.id != 400000)
                .count(),
            229
        );
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 4);
        let mut home = prepare(home).unwrap();
        let board = home.c["Objects"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|o| o["ID"] == 3200000)
            .unwrap();
        board["Orders"][0] = json!({"Lvl":4,"Timer":timers::new(1)});
        let mut replay = Replay::new(&home).unwrap();
        let bytes = crate::replay::tests::turn(30, &[]);
        assert!(replay
            .apply(&mut home, &EndClientTurn::parse(&bytes).unwrap(), &bytes)
            .is_empty());
        assert_eq!(home.predefined_orders, 3); // Completed timer generates B.
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 60);
        home.predefined_orders = 127;
        let home = prepare(home).unwrap();
        assert!(
            home.special_order_slots.len()
                <= number(level_row(60).unwrap(), "SpecialOrderMaxCount").unwrap() as usize
        );
        assert_eq!(prepare(home.clone()).unwrap(), home);
    }
    #[test]
    fn first_turn_preserves_snapshot_bits_and_level_up_fills_board() {
        let mut stored = crate::game::starter::new_home().unwrap();
        stored.avatar_stats.insert(1900002, 4);
        let sent = prepare(stored.clone()).unwrap();
        assert_eq!(sent.predefined_orders, 1);
        assert_eq!(stored.predefined_orders, 0);
        let mut replay = Replay::new(&sent).unwrap();
        let bytes = crate::replay::tests::turn(1, &[]);
        assert!(replay
            .apply(&mut stored, &EndClientTurn::parse(&bytes).unwrap(), &bytes)
            .is_empty());
        assert_eq!(stored.predefined_orders, 1);
        assert_eq!(stored.c, sent.c);
        // A fresh level-3 world generates no orders, then fills when it hits 4.
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 3);
        home.avatar_stats.insert(1900003, 13);
        let mut home = prepare(home).unwrap();
        let mut replay = Replay::new(&home).unwrap();
        let bytes = crate::replay::tests::turn(1, &[vec![2, 400000, 1]]);
        assert!(replay
            .apply(&mut home, &EndClientTurn::parse(&bytes).unwrap(), &bytes)
            .is_empty());
        assert_eq!(home.avatar_stats[&1900002], 4);
        assert_eq!(home.predefined_orders, 1);
    }
    #[test]
    fn receivers_are_saved_as_global_ids_and_old_indices_migrate() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 10);
        let home = prepare(home).unwrap();
        let receivers: Vec<u64> = board(&home)["Orders"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|o| o["Receiver"].as_u64())
            .collect();
        assert!(!receivers.is_empty());
        assert!(receivers.iter().all(|&r| (3_400_000..3_400_010).contains(&r)));
        let mut old = home.clone();
        for o in old.c["Objects"].as_array_mut().unwrap() {
            if o["ID"] == 3200000 {
                o["Orders"][0]["Receiver"] = json!(7);
            }
        }
        assert_eq!(board(&prepare(old).unwrap())["Orders"][0]["Receiver"], 3_400_007);
    }
    #[test]
    fn complete_refill_persist_and_collect_never_pay_twice() {
        let mut home = crate::game::starter::new_home().unwrap();
        home.avatar_stats.insert(1900002, 4);
        let mut home = prepare(home).unwrap();
        let board = home.c["Objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ID"] == 3200000)
            .unwrap()
            .clone();
        let order = board["Orders"][0].clone();
        for (id, amount) in order["Datas"]
            .as_array()
            .unwrap()
            .iter()
            .zip(order["Amounts"].as_array().unwrap())
        {
            home.avatar_stats.insert(
                id.as_u64().unwrap() as u32,
                amount.as_u64().unwrap() as u32 + 10,
            );
        }
        let mut state = Replay::new(&home).unwrap();
        let cash = home.avatar_stats.get(&1900000).copied().unwrap_or(0);
        let mut expected = home.avatar_stats.clone();
        award_xp(&mut expected, order["Exp"].as_u64().unwrap() as u32).unwrap();
        let turn = crate::replay::tests::turn(10, &[vec![28, 0, 1]]);
        assert!(state
            .apply(&mut home, &EndClientTurn::parse(&turn).unwrap(), &turn)
            .is_empty());
        assert_eq!(home.avatar_stats[&1900003], 0);
        assert_eq!(home.avatar_stats[&1900000], cash);
        assert!(home.replay_state["OrderReward"].is_object());
        let truck = |home: &HomeData| {
            home.c["Objects"].as_array().unwrap().iter().find(|o| o["ID"] == 200000).unwrap().clone()
        };
        assert_eq!(truck(&home)["State"], 3);
        assert_eq!(truck(&home)["Cash"], order["Cash"]);
        assert_eq!(truck(&home)["Exp"], order["Exp"]);
        let after = home.clone();
        assert_eq!(
            state.apply(&mut home, &EndClientTurn::parse(&turn).unwrap(), &turn),
            ["duplicate turn ignored"]
        );
        // A new session must retain the uncollected reward, not pay it on load.
        home =
            prepare(serde_json::from_slice(&serde_json::to_vec(&home).unwrap()).unwrap()).unwrap();
        assert_eq!(home.avatar_stats[&1900000], cash);
        assert_eq!(truck(&home)["Cash"], order["Cash"]);
        state = Replay::new(&home).unwrap();
        let collect = crate::replay::tests::turn(11, &[vec![41, 11]]);
        assert!(state
            .apply(
                &mut home,
                &EndClientTurn::parse(&collect).unwrap(),
                &collect
            )
            .is_empty());
        assert_ne!(home.avatar_stats, after.avatar_stats);
        assert_eq!(home.avatar_stats[&1900003], expected[&1900003]);
        assert_eq!(
            home.avatar_stats[&1900000],
            cash + order["Cash"].as_u64().unwrap() as u32
        );
        assert_eq!(truck(&home)["State"], 1);
        assert!(truck(&home).get("Cash").is_none());
        let paid = home.avatar_stats.clone();
        let again = crate::replay::tests::turn(12, &[vec![41, 12]]);
        state.apply(&mut home, &EndClientTurn::parse(&again).unwrap(), &again);
        assert_eq!(home.avatar_stats, paid);
        let restored: HomeData =
            serde_json::from_slice(&serde_json::to_vec(&home).unwrap()).unwrap();
        assert_eq!(prepare(restored).unwrap(), home);
        let message = home.message();
        use crate::protocol::Message;
        assert_eq!(
            crate::protocol::messages::OwnHomeData::from_bytes(&message.to_bytes()).unwrap(),
            message
        );
        assert!(message.unlocked.contains_key(&33));
        // A second order cannot consume absent ingredients or partially pay.
        home.avatar_stats.retain(|id, _| *id / 100000 == 19);
        let before = home.clone();
        let fail = crate::replay::tests::turn(12, &[vec![28, 1, 12]]);
        assert!(!state
            .apply(&mut home, &EndClientTurn::parse(&fail).unwrap(), &fail)
            .is_empty());
        assert_eq!(home.avatar_stats, before.avatar_stats);
        assert_eq!(home.c, before.c);
    }
}
