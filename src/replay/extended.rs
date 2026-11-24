//! Partial-effect replay. Unknown sub-effects warn; known effects commit.
use super::*;
fn stat(name: &str) -> Result<u32> {
    static IDS: OnceLock<BTreeMap<String, u32>> = OnceLock::new();
    IDS.get_or_init(|| serde_json::from_str(&crate::data::resource_data::load("money-ids.json")).unwrap())
        .get(name)
        .copied()
        .with_context(|| format!("unknown money row {name}"))
}
pub(super) fn bump(home: &mut HomeData, name: &str, n: u32) -> Result<()> {
    add(&mut home.avatar_stats, stat(name)?, n)
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
        if set {
            words[row as usize / 32] |= 1 << (row % 32);
        } else {
            words[row as usize / 32] &= !(1 << (row % 32));
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
            6|88=>{
                self.unknown("cosmetic AI action has no persistent effect; its shared world-RNG rolls are not reproduced");
            },
            42|65|66|94=>{
            },
            7=>{
                let id=u32::try_from(arg(0)?)?;
                self.position(arg(1)?)?;
                remove(&mut home.avatar_stats,id,1)?;
                self.unknown("field booster consumed; booster-specific timer effect not traced");
            },
            8=>{
                let id=u32::try_from(arg(0)?)?;
                ensure!(id/100000==4,"field data required");
                let row=definition(id)?;
                let n=self.objects.iter().filter(|o|o.instance/100000==4).count() as u32+home.avatar_stats.get(&id).copied().unwrap_or(0);
                let every=number(row,"DoPriceIncreaseEveryNObject")?;
                let cash=number(row,"Price")?+match n.checked_div(every){
                    None=>0,
                    Some(tiers)=>tiers*number(row,"PriceIncrease")?,
                };
                spend(&mut home.avatar_stats,cash)?;
                spend_diamonds(&mut home.avatar_stats,number(row,"DiamondPrice")?)?;
                add(&mut home.avatar_stats,id,1)?;
            },
            11=>{
                let i=self.position(arg(0)?)?;
                let data=self.objects[i].value["ID"].as_u64().unwrap() as u32;
                let name=match data/100000{
                    18=>"SiloRank",14=>"WarehouseRank",102=>"TackleBoxRank",107=>"LobsterPoolRank",154=>"DuckSalonRank",_=>{
                        self.unknown("upgrade target cost/state unknown");
                        return Ok(true)
                    }
                }
                ;
                bump(home,name,1)?;
                let rank=home.avatar_stats[&stat(name)?];
                self.objects[i].value["Rank"]=json!(rank);
                self.unknown("upgrade rank applied; required tool amounts/missing-tool purchase option unknown and not charged");
            },
            18..=20=>{
                let i=self.position(arg(0)?)?;
                ensure!(self.objects[i].instance/100000==5,"forest obstacle required");
                let id=self.objects[i].value["ID"].as_u64().unwrap() as u32;
                let row=definition(id)?;
                let started=self.objects[i].state["Started"]==true;
                if kind==18 {
                    ensure!(!started,"obstacle clearing already started");
                }
                if kind==18 || (kind==19 && !started){
                    if let Some(tool)=row.get("Tool").filter(|s|!s.is_empty()){
                        remove(&mut home.avatar_stats,crate::game::starter::resolve(tool)?.0,1)?;
                    }
                    self.objects[i].state=json!({
                        "Started":true,"Timer":timers::new(duration(row)?)
                    }
                    );
                }
                if kind==19 {
                    let timer=self.objects[i].state["Timer"].clone();
                    self.generic_cost(home,row,&timer)?;
                    self.objects[i].state["Timer"]=json!({
                    }
                    );
                }
                if kind==20 {
                    ensure!(started,"obstacle clearing not started");
                    ensure!(timers::remaining(&self.objects[i].state["Timer"])==0,"obstacle not ready");
                    award_xp(&mut home.avatar_stats,number(row,"ExpClear")?)?;
                    bump(home,"CounterForestCollect",1)?;
                    self.objects.remove(i);
                }
                else{
                    self.unknown("forest clearing timer saved server-side; client JSON timer key not established");
                }
            },
            30..=32|36|37=>self.market(home,kind,f)?,
            39=>{
                let items=f[0].as_array().context("missing purchase list")?;
                ensure!(!items.is_empty(),"empty buy list");
                let context=arg(1)?;
                let mut cost=0u32;
                for item in items{
                    let id=item[0].as_u64().context("invalid item")? as u32;
                    let amount=u32::try_from(item[1].as_i64().context("invalid amount")?)?;
                    ensure!((1..20_000_000).contains(&amount),"invalid purchase amount");
                    let row=definition(id);
                    let unit=if [4,19,98,105].contains(&(id/100000)){
                        self.unknown("special diamond/cash conversion price untraced; no invented charge");
                        0
                    }
                    else{
                        number(row?,"DiamondPrice")?.max(1)
                    }
                    ;
                    cost=cost.checked_add(amount.checked_mul(unit).context("price overflow")?).context("price overflow")?;
                }
                spend_diamonds(&mut home.avatar_stats,cost)?;
                for item in items{
                    add(&mut home.avatar_stats,item[0].as_u64().unwrap() as u32,item[1].as_u64().unwrap() as u32)?;
                }
                if [-65,-64,-63,-62,-61,-60,-59,-46,-45,-38,-24].contains(&context){
                    self.unknown("special diamond-purchase delivery context unknown; goods credited to storage instead");
                }
            },
            44=>{
                let i=self.position(arg(0)?)?;
                self.unknown("mystery-box pre-rolled reward fields unknown; removing opened box without inventing reward");
                self.objects.remove(i);
            },
            45|54=>{
                let i=self.position(arg(0)?)?;
                self.objects.remove(i);
                if kind==54{
                    self.unknown("generic remove applied; other object-specific effects unknown");
                }
            },
            47=>self.unknown("AI roadside purchase recognized; stand selection, item/price and AIRoadsideShopBits mapping unknown"),
            49=>{
                home.avatar_stats.insert(stat("AppRateState")?,u32::try_from(arg(0)?)?);
            },
            50=>{
                let row=u32::try_from(arg(0)?)?;
                ensure!(row<5,"ambient row not spawnable");
                let x=arg(1)?;
                let y=arg(2)?;
                ensure!(self.objects.iter().filter(|o|o.instance/100000==45).count()<(replay_config()["MaxAmbientAnimals"] as usize),"ambient list full");
                self.create(json!({
                    "ID":4500000+row,"X":x/512,"Y":y/512,"AccurateX":x,"AccurateY":y
                }
                ))?;
                self.unknown("ambient object saved; target flags and shared world-AI RNG motion not replayed");
            },
            51=>{
                let id=u32::try_from(arg(0)?)?;
                let row=definition(id)?;
                if !self.extra["Achievements"].is_object(){
                    self.extra["Achievements"]=json!({
                    }
                    );
                }
                let key=id.to_string();
                if self.extra["Achievements"][&key]!=true{
                    award_xp(&mut home.avatar_stats,number(row,"Exp")?)?;
                    add(&mut home.avatar_stats,1900001,number(row,"DiamondReward")?)?;
                    self.extra["Achievements"][key]=json!(true);
                    self.unknown("achievement base reward applied; objective validation, tier/custom/decor reward unmodeled");
                }
            },
            52|55..=58|68|70|71|73|75|77|79..=82|86|87|89|93|96..=100|103=>{
                self.unknown(format!("command {kind} recognized; state, random reward or price details remain unknown"));
                if kind==52 {
                    bump(home,"AdvertisementDiamondPurchases",1)?;
                    self.unknown("ad token granted; diamond purchase price untraced");
                }
                if kind==93 {
                    let i=self.position(arg(0)?)?;
                    self.objects.remove(i);
                    self.unknown("collectible removed; pre-rolled reward mapping unknown");
                }
                if [68,70,71,73].contains(&kind){
                    if !self.extra["Tom"].is_object(){
                        self.extra["Tom"]=json!({
                        }
                        );
                    }
                    self.extra["Tom"][match kind{
                        68=>"HireIndex",70=>"SelectedOffer",71=>"SearchData",_=>"ClaimRequested"
                    }
                    ]=json!(arg(0)?);
                }
                if kind==100{
                    spend_diamonds(&mut home.avatar_stats,number(definition(arg(0)? as u32)?,"DiamondCost")?)?;
                    self.unknown("coin-package diamond cost applied; level-scaled coin amount unknown");
                }
                if kind==57 {
                    home.avatar_stats.insert(stat("ConsecutiveBoatOrdersCompleted")?,0);
                }
                if kind==56 {
                    bump(home,if f.get(1)==Some(&json!(true)){
                        "BoatOrderSharedCrateCount"
                    }
                    else{
                        "BoatCrateCompleteCount"
                    },1)?;
                    self.unknown("boat crate completion counted; generated crate goods/reward unknown");
                }
                if kind==86{
                    bump(home,"PetFeedCount",1)?;
                    self.unknown("pet-food item/amount not established");
                }
            },
            59..=63=>self.tree(home,kind,f)?,
            64=>{
                ensure!(arg(0)? as u32/100000==22,"mine requires a tool");
                remove(&mut home.avatar_stats,arg(0)? as u32,1)?;
                self.unknown("mining tool consumed; world-RNG ore yields unknown");
            },
            69|72|74=>{
                if !self.extra["Tom"].is_object(){
                    self.extra["Tom"]=json!({
                    }
                    );
                }
                if kind==72{
                    self.extra["Tom"]["OfferRejected"]=json!(true);
                }
                else{
                    self.extra["Tom"]["State"]=json!(if kind==69{
                        13
                    }
                    else{
                        3
                    }
                    );
                }
                self.unknown("Tom state transition applied server-side; offer, timing and JSON persistence incomplete");
            },
            67=>{
                if f[0]==true{
                    home.avatar_stats.insert(stat("FacebookLoginRewardGiven")?,1);
                }
                self.unknown("Facebook reward flag recorded; reward amount unknown");
            },
            90=>{
                let id=arg(0)? as u32;
                self.bits(id/100000,id%100000,true)?;
                self.unknown("land-area bit persisted; associated tool/coin cost unknown");
            },
            92|95=>{
                home.avatar_stats.insert(stat(if kind==92{
                    "FBRequirementIgnored"
                }
                else{
                    "FacebookAdClicked"
                }
                )?,1);
            },
            101=>{
                self.extra["LastOpenedFriendLeaderboardHash"]=json!([arg(0)?,arg(1)?]);
                self.unknown("leaderboard hash pair stored as metadata; scalar stat packing unconfirmed");
            },
            131=>{
                let id=arg(0)? as u32;
                let table=arg(1)? as u32;
                ensure!([115,120].contains(&table) && id/100000==table,"invalid settings bit type");
                self.bits(table,id%100000,f[2]==true)?;
            },
            _=>return Ok(false)
        }
        Ok(true)
    }
    fn market(&mut self, home: &mut HomeData, kind: u32, f: &[Value]) -> Result<()> {
        if !self.extra["Market"].is_object() {
            self.extra["Market"] = json!({"Stands":{}});
        }
        let stand = f.first().and_then(Value::as_i64).unwrap_or(0).to_string();
        match kind {
            30 => {
                bump(home, "RoadsideShopStandCount", 1)?;
                self.unknown(
                    "shop stand expanded; diamond expansion price/next-price formula unknown",
                );
            }
            32 => {
                let id = f[1].as_u64().context("invalid shop item")? as u32;
                let amount = f[2].as_u64().context("invalid shop quantity")? as u32;
                if f[5] == true {
                    self.unknown("shop alternate source unknown; listing saved without inventing ordinary inventory removal");
                } else {
                    remove(&mut home.avatar_stats, id, amount)?;
                }
                self.extra["Market"]["Stands"][&stand] =
                    json!({"ID":id,"Amount":amount,"Price":f[3],"Sold":false});
                if f[4] == true {
                    self.advertise(home)?;
                }
            }
            31 => {
                let s = self.extra["Market"]["Stands"][&stand].clone();
                let price = s["Price"].as_u64().unwrap_or(0) as u32;
                add(&mut home.avatar_stats, 1900000, price)?;
                self.extra["Market"]["Stands"]
                    .as_object_mut()
                    .unwrap()
                    .remove(&stand);
                self.unknown(
                    "shop listed price collected; client AI sold-state/random buyer not replayed",
                );
            }
            36 => self.advertise(home)?,
            37 => {
                self.extra["Market"]["Stands"]
                    .as_object_mut()
                    .unwrap()
                    .remove(&stand);
                self.unknown("shop contents discarded; diamond discard cost unknown");
            }
            _ => {}
        }
        self.unknown("roadside stand state persisted as server metadata; client stand JSON keys not established");
        Ok(())
    }
    fn advertise(&mut self, home: &mut HomeData) -> Result<()> {
        let id = stat("AdvertisementDiamondPurchases")?;
        if home.avatar_stats.get(&id).copied().unwrap_or(0) > 0 {
            remove(&mut home.avatar_stats, id, 1)?;
        } else {
            self.unknown("advertisement diamond price unknown; no invented charge");
        }
        bump(home, "AdsPlacedToNewspaperCount", 1)
    }
    fn tree(&mut self, home: &mut HomeData, kind: u32, f: &[Value]) -> Result<()> {
        let i = self.position(f[0].as_i64().context("tree id missing")? as i32)?;
        let id = self.objects[i].value["ID"].as_u64().unwrap() as u32;
        let row = definition(id)?;
        match kind {
            60 => {
                self.objects[i].state["Timer"] = timers::new(duration(row)?);
                self.objects[i].state["FruitSlots"] =
                    json!(vec![true; number(row, "FruitCount")? as usize]);
                self.unknown(
                    "tree regrow state stored server-side; mine/tool requirements unknown",
                );
            }
            59 => {
                let timer = self.objects[i].state["Timer"].clone();
                self.generic_cost(home, row, &timer)?;
                self.objects[i].state["Timer"] = json!({});
            }
            61 => {
                if let Some(tool) = row.get("Tool") {
                    remove(&mut home.avatar_stats, crate::game::starter::resolve(tool)?.0, 1)?;
                }
                self.objects.remove(i);
            }
            62 => {
                let slot = f[1].as_u64().context("fruit slot missing")? as usize;
                ensure!(
                    timers::remaining(&self.objects[i].state["Timer"]) == 0,
                    "fruit not ripe"
                );
                let fruit_name = row
                    .get("Name")
                    .context("tree name missing")?
                    .trim_end_matches("Tree")
                    .trim_end_matches("Bush");
                let fruit = crate::game::starter::resolve(fruit_name).map(|r| r.0);
                if let Ok(fruit) = fruit {
                    if self.objects[i].state["FruitSlots"].is_null() {
                        self.objects[i].state["FruitSlots"] =
                            json!(vec![true; number(row, "FruitCount")? as usize]);
                    }
                    let slots = self.objects[i].state["FruitSlots"]
                        .as_array_mut()
                        .context("fruit slots missing")?;
                    ensure!(
                        slot < slots.len() && slots[slot] == true,
                        "fruit slot already collected"
                    );
                    slots[slot] = json!(false);
                    add(&mut home.avatar_stats, fruit, 1)?;
                    award_xp(
                        &mut home.avatar_stats,
                        number(definition(fruit)?, "ExpCollect")?,
                    )?;
                    bump(
                        home,
                        if row
                            .get("IsFruit")
                            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
                        {
                            "FruitsPicked"
                        } else {
                            "BerriesPicked"
                        },
                        1,
                    )?;
                } else {
                    self.unknown("tree-to-fruit resource mapping unresolved; no invented item");
                }
            }
            63 => {
                bump(home, "FruitTreesRevived", 1)?;
                self.objects[i].state["Revived"] = json!(true);
                self.unknown("tree revival counted; XP and revive/help cost unknown");
            }
            _ => {}
        }
        self.unknown("tree lifecycle JSON slots/timer keys unverified; state stored server-side");
        Ok(())
    }
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
