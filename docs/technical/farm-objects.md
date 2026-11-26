# Farm object state

Notes on fields the client saves in the farm JSON (blob `c`, `Objects`) and
how the server must write them. Static analysis of the x86 `libg.so` unless
marked live-tested. See also
[end-client-turn-commands.md](end-client-turn-commands.md).

## Production queue entries

Machines keep `ProductionList` with `Productions` (queued or running) and
`FinishedProductions`. Each queued entry has these keys:

| Key | Meaning |
|---|---|
| `ID` | Product data ID |
| `Timer` | Production timer |
| `HelperType` | Which helper queued the item. **Omit the key for items the player queued.** |
| `BoosterID` | Booster applied, if any. Omit when none. |
| `ProductionTime` | Full production time, seconds |
| `ReducedProductionTime` | Time after reductions, seconds |

### HelperType

- Loader (`0x5fb406`): reads `HelperType` and accepts only 0 or 1; any other
  value keeps the default.
- Saver (`0x2eb3fa`): writes `HelperType` only when the field is not −1, so
  −1 (key absent) means "queued by the player".
- Helpers (texts.csv): `TID_HELPER_CARETAKER` = **Rose**; the other helper is
  Ernest. Value 0 is Rose. The mapping of 1 to Ernest is inferred.

Writing `"HelperType": 0` makes the client show Rose's portrait on items the
player queued (live-observed). The server's `start_production`
(`src/replay/production.rs`) omits both keys for player-queued items with no
booster. The client saves `BoosterID` only when a booster is present
(`0x2eb436`). Existing saved queue entries are not rewritten.

## Crop fields (growth timer)

Static analysis of the x86 APK. The server behaviour below has been
live-tested: growth stays in sync across reconnects, and speed-up costs match
the client.

### JSON format

The field saver is `0x237a2a` (vtable slot +0x54) and the loader is
`0x44bfe4` (slot +0x60) (High). Keys: `ID`, `X`, `Y`, `Timer`, plus the
optional `Mirrored` and `BoosterList`.

| Timer key | Meaning |
|---|---|
| `StartSeconds` | Total growth duration in seconds. This is a duration, not a timestamp. |
| `TicksLeft` | Remaining simulation ticks (about 66 ms each): `trunc(1000*s/66)` |

- The saver writes `Timer` only when `TicksLeft > 0` and the field's flags are
  clear. A field with no `Timer` is a finished (ripe) crop (High).
- A missing `Timer`, a non-object `Timer`, or missing members all load as 0,
  which also means ripe (High).
- The 66 ms tick conversion was live-confirmed: server-simulated growth stays
  in sync with the client.

```json
{"ID":400001,"X":19,"Y":20,"Timer":{"StartSeconds":120,"TicksLeft":1818}}
```

That is a growing field. The same object without `Timer` is ripe. Wheat being
data ID 400001 has not been independently confirmed.

### Planting and ripeness

- **Planting (command 1):** jump case `0x53bb80`, decoder `0x700060`, handler
  `0x6ae562`. The timer is initialised by `0x611c94` → `0x61d094`, and
  `0x44197a` converts seconds to ticks (High).
- **Duration:** `0x4e31fe` computes `TimeMin*60 + TimeSec`. An optional
  modifier is applied, and what that modifier is remains unknown (High).
- **Ripeness:** predicate `0x2e7364` treats a crop as finished when its ticks
  left are ≤ 0, or when `StartSeconds` is missing or 0. Progress is counted in
  ticks, not compared against a timestamp (High for the predicate, Medium for
  the whole trigger chain).
- **Command 24:** this command does more than notify. Handler `0x16432a`
  (call site `0x1643c0`) gets the timer through `0x6011ea` (`field+0x50`), then
  sets `TicksLeft` to 0 through `0x6a7fe8` (High). See
  [command 24](end-client-turn-commands.md).
- Q2's `+0x54`/`+0x58` are the same fields as `Timer+4`/`Timer+8`
  (`StartSeconds`/`TicksLeft`).

### On reload

- The loader copies `TicksLeft` as-is. It reads no clock and subtracts no
  elapsed time (High).
- The per-field update `0x169192` → `0x1f38a2` lowers the remaining ticks by a
  delta (High).
- A bulk update chain (`0x4aae32` → … → `0x563ce4`) can also advance timers.
  The live test shows a normal reconnect does not apply offline time a second
  time on top of the server's.

The loader expects the server to supply the remaining ticks, so the server
applies offline catch-up itself (live-confirmed).

### Server behaviour (live-tested)

- **Planting (command 1):** sets
  `Timer: {StartSeconds: TimeMin*60 + TimeSec, TicksLeft: trunc(1000*s/66)}`.
  The client's optional duration modifier is not modelled.
- **Growth:** field timers advance with turn ticks during play, and with wall
  time (`elapsed_home`) on load. A crop that reaches 0 loads as ripe.
- **Speed-up (command 24):** costs diamonds, then removes `Timer`. The cost is
  `ceil(InstantComplete * remaining_s / total_s)`, at least 1, and free below
  level 3 (`FREE_DIAMOND_SPEED_UP_BEFORE_LEVEL`). This is the same formula as
  for animals and production. The diamond balance matched the client's in a
  live test. With too few diamonds the command is rejected and the timer
  keeps running.
- **Server already counts the crop as ripe:** when a speed-up or harvest
  arrives and the server's timer has already run out, nothing is charged. A
  warning is logged.
- **Harvest before the server's timer ends:** the harvest is still credited,
  because the client is authoritative. A warning reports the remaining ticks.
- Fields planted before this change have no `Timer` and stay ripe.
- Still not modelled: the client's optional duration modifier (possibly a
  field booster). Boosted crops may grow at a different speed on the server.

## Truck order board (object 3200000)

`Seed` plus nine `Orders` slots. An active order has `Lvl`, `Cash`, `Exp`,
`CashExpMultiplier`, `Datas`, `Amounts` and `Receiver`; an empty slot has
`Lvl` and `Timer`.

- **`Receiver` is a global ID**, `3400000 + row` of `orders.csv` (kind 34),
  as in every client-written farm we have. Writing the bare row index makes
  the client show every server-generated order as "Kindergarten" (row 0)
  (live-observed). The client picks free receivers by `Receiver % 10`, so
  both forms pick the same receivers; only the display is wrong.
- **Generation (live-tested):** the server's port of the client generator
  produced the same items, amounts, coins and XP as the client in-session
  for three completions (28) and one trash followed by a diamond speed-up
  (27, 29). **The receiver differs**: with seed −1056698259, active
  receivers {1, 2, 3} and the completed order's receiver 8, the server
  picked 8 (Frank's Factory) and the client 5 (Mike's Mill). Only receiver
  exclusion sets such as {0,1,2,3}, {1,2,3,4} or {0,1,2,3,8} reproduce the
  client's pick, so the client's "available receivers" list (`0x63bacc`)
  is built differently from the server's (which excludes only the other
  active orders). Rewards are unaffected, but the receiver shown in-session
  changes to the server's pick at the next login. Data points (seed before
  generation → client pick, server pick; other active receivers 1, 2, 3 in
  both): −1056698259 → 5 vs 8; −1086256922 → 3 vs 6. In the second case the
  client picked a receiver that was already active, so it does not simply
  exclude the other active orders. `0x63bacc` reads each slot's receiver
  from order +0x44 (data pointer, `id % 100000`) for slots whose +0x14 is
  non-zero; what +0x14 is, and which orders it covers, is the open point.
- **Pending truck reward:** the client keeps the reward of the last
  completed order until the next completion (28) or collection (41). The
  server cannot send it in the client's format (key unknown), so a stash
  from a previous session exists only on the server: the client does not pay
  it, the server does at the next 28/41, and the HUD jumps to the server's
  totals on the next login (live-observed: +76 coins, +12 XP).

## Mirrored objects (command 53)

Objects carry `"Mirrored": true` when flipped; the key is absent otherwise
(Greg's farm uses it). Flipping swaps the footprint's width and height.

## Mystery boxes (commands 44–46 and 48)

- Class 53, data rows in `mystery_boxes.csv`; object ID = `5300000 + row`
  (Box1 = 5300000, a bronze box with 1 Bolt). Boxes are 2×2 tiles.
- The reward is rolled when the box spawns and stored on the box (amount at
  object +0x50, item at +0x6c). Opening (44) grants that stored reward;
  nothing is rolled at open time. The JSON key names for these two fields
  are **not yet known**.
- The client spawns boxes itself during play, probably seeded by the
  avatar's `MysteryBoxRandomSeed`; the server does not simulate this, so a
  box spawned on the client is missing from the stored farm and command 44
  will not find it.
- Command 44 removes the box and grants its pre-rolled reward when the save
  supplies `MysteryBoxReward: {"ID": ..., "Amount": ...}` in the corresponding
  `replay_state.ObjectStates` entry. With no reward record it logs the missing
  reward. Command 45 removes the box. Commands 46 and 48 log the unresolved
  reveal steps.

### Finding the reward keys

1. Add a bare box to a test account's stored farm through the admin API
   (`PUT /accounts/<id>/home`), e.g. `{"ID":5300000,"X":20,"Y":20}` on free
   ground.
2. Log in and check it appears. If the client rejects it, the reward keys
   are required and must be found statically.
3. Dump the farm from the running client with
   `hayday-research/scratch/r30-dump.sh`. The client's serializer writes
   every field, so the box object shows the key names for its item and
   amount.
4. Open the box and compare the reward with the dumped values.

Then command 44 can read those keys, add the amount of that item to the
avatar stats, and remove the box.
