# EndClientTurn commands

Commands sent in [EndClientTurn (11292)](messages.md#endclientturn-11292) for Hay Day 1.43.150 (build 1598). Payload fields follow the command type and end with the execution tick. `VInt` is a signed variable-length integer; fixed-width integers are big-endian, and consecutive booleans are packed. Instance IDs identify farm objects; data IDs identify definitions. A dash means the payload is not listed here. Names and effects come from static analysis of client serializers and executors, analytics strings, and captured traffic; a label does not establish every effect. Certainty rates the action label: **high** means supported by executor analysis or captures; **medium** means a broader or partially inferred label; **unknown** means the effect is unresolved. It does not rate replay completeness.

| ID | Certainty | Command | Payload (after type) | Notes |
| --- | --- | --- | --- | --- |
| 1 | High | Plant crop | VInt field instance, VInt crop data ID, VInt tick |  |
| 2 | High | Harvest crop | VInt field instance, VInt tick |  |
| 3–5 | High | Move object; buy/place object; buy animal | — |  |
| 6 | High | Tap animal enclosure | VInt habitat instance, VInt tick |  |
| 7–8 | Medium | Use field booster/tool; buy field into storage | — |  |
| 9 | High | Feed animal | VInt animal instance, VInt tick |  |
| 10 | High | Start production | VInt building instance, VInt product data ID, VInt tick |  |
| 11 | Medium | Upgrade a building | VInt object instance ID, VInt upgrade type, VInt tick | Upgrade types: 1 = new slot, 2 = coin bonus, 3 = exp and reputation bonus, 4 = time bonus (the `ServiceSlots`, `CoinBonus`, `ExpAndRepBonus`, `TimeBonus` columns of `service_buildings.csv`). Each type increments its own counter on the object (+0x84, +0x88, +0x8c, +0x90) and calls a per-type refresh. The object is looked up in any class. For classes 134–139 (service buildings, passenger train, train station, town hall, personal train) the validator (`0x3c08ba`) rejects types outside 1–4; other upgradable classes in the switch are Shed (14), Silo (18), TackleBox (102), LobsterPool (107) and DuckSalon (154). Service-building cost: `CollectionTool1–3` / `CollectionTool1–3Amount` from the next level row; whether `UpgradeCashPrice` coins are charged is unconfirmed. JSON keys for the counters are unknown. Vtable 0xb34f78, execute 0xdeae2; see `hayday-research/analysis/command-11-upgrade.md`. |
| 12–17 | Medium | Finish or collect production; collect or speed up animal production; speed up or claim construction | — |  |
| 18–20 | Medium | Start, speed up, or collect obstacle clearing | — |  |
| 21, 24 | Medium | Select animal in habitat UI; crop ripe notification | — |  |
| 22 | High | Harvest follow-up | VInt field instance, VInt tick |  |
| 23 | High | Clear harvested field | VInt field instance, VInt tick |  |
| 25 | High | Paint building part | VInt object instance, signed byte part index, VInt tick |  |
| 27 | High | Trash truck order | signed byte order slot, VInt tick |  |
| 28 | High | Complete truck order | signed byte order slot, VInt tick |  |
| 29–33 | Medium | Speed up a trashed-order timer; expand shop; collect a roadside sale; list goods; acknowledge level-up unlocks | — |  |
| 34 | High | Complete tutorial row | VInt tutorial data ID, VInt tick |  |
| 35 | High | Start tutorial row | VInt tutorial data ID, VInt tick |  |
| 36–37 | Medium | Advertise a roadside stand; discard its contents | — |  |
| 39 | High | Buy goods with diamonds | signed-byte count and list of (VInt data ID, VInt amount), signed byte context, VInt target A, VInt target B, VInt tick |  |
| 40, 42 | Medium | Complete visitor deal; tap ambient animal | — |  |
| 41 | High | Collect pending truck reward | VInt tick |  |
| 43 | High | Decline visitor deal | VInt visitor data ID, VInt tick |  |
| 44 | Medium | Open mystery box | VInt box instance ID, VInt tick | Class 53, `mystery_boxes.csv`. After a check (`0x57c6e8`), `0x2dc6d2` grants the contents rolled when the box spawned (amount at box +0x50, item at +0x6c) and removes the box. Analytics label "mystery box opened". Replaying it needs the box's saved reward fields (see [farm-objects.md](farm-objects.md)). |
| 45 | Medium | Discard mystery box | VInt box instance ID, VInt tick | Removes the box without opening it (`0x302372`, `0x3ecff6`). Analytics label "mystery box discarded". |
| 47 | Medium | Buy from AI roadside shop | — |  |
| 48 | Low | Mystery-box action (probably start opening) | VInt box instance ID, VInt tick | Looks up a mystery box (class 53) by instance ID, checks its state (`0x1fc304`), then sets box +0x5c = 3 and +0x60 = 45, probably starting the reveal animation that precedes 44. Vtable 0xb26040, execute 0x5fe744. |
| 49–52 | Medium | Set app rating state; spawn ambient animal; claim achievement; buy advertisement token | — |  |
| 53 | Medium | Flip (mirror) object | VInt target instance ID, VInt tick | Edit mode. Toggles the object's flipped flag (`Mirrored` in the farm JSON), which swaps the footprint's width and height; a non-square object is removed from and re-placed on the grid. Vtable 0xb3af90, execute 0x1ebb7a. |
| 55–58 | Medium | Complete boat order; fill crate; discard order; speed up boat | — |  |
| 59–65 | Medium | Speed up or start tree/mine activity; remove dead tree; pick fruit; revive tree; mine; request help | — |  |
| 68–74 | Medium | Tom hiring, offer, search and state actions | — |  |
| 86–90 | Medium | Feed or interact with pets and their habitat; unlock land area | — |  |
| 88 | High | Tap pet | VInt pet instance, VInt tick |  |
| 93–97 | Medium | Collect/remove object; offer notification; Facebook ad state; buy mastery star or tool offer | — |  |
| 131 | High | Toggle settings/notification bit | VInt data ID, VInt table type, packed bool set, VInt tick |  |

The command count and schemas determine boundaries: commands have no general length prefix. The server retains the raw remainder if it cannot decode a command. Command 53 toggles the target's persisted `Mirrored` flag; live reconnect validation is pending. Some decoded effects remain unmodeled or approximate, including visitor offers; see [replay/mod.rs](../../src/replay/mod.rs) and adjacent modules for current behavior.
