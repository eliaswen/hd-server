# Visitors

How the Hay Day 1.43.150 (build 1598) client creates and settles visitor ("people") deals, and how the server replays them. Findings come from static analysis of the Android client's native library (x86) and from comparing the server's replayed balances with what the client displayed. Statements are marked **verified** (read directly from the executors), **hypothesis** or **unknown**.

## Summary

The client invents every visitor offer itself, using a random number generator that is shared with the whole ambient-animal and pathing simulation. The server never sees the offer, only the command that completes it, so it cannot reproduce offers exactly. It keeps an approximate offer per visitor and applies it when the completion command arrives. The next farm load overrides any drift.

## Game data

One spawner row controls the system (values in the shipped tables):

| Column | Value | Meaning |
| --- | --- | --- |
| UnlockLevel | 6 | Minimum level for generated visitors |
| MinSpawnTime / MaxSpawnTime | 120 / 900 | Seconds between spawns |
| MinOrderSize / MaxOrderSize | 35 / 65 | Share of stock requested, in percent |
| CanCompleteOrderChance | 80 | Chance that the offer is for something the player has |
| MaxPeople | 2 | Visitors on the farm at once |
| MinTutorialPeopleToSpawnBeforeUnlock | 2 | Tutorial visitors needed before generated ones |
| GoodNumber | 2 | Number of most-stocked items considered |
| GoldMultiplier | 140 | Price percentage applied to generated offers |
| ExpMultiplier / ConstantExp | 0 / 5 | Experience paid per deal |
| ForcedUnlockLevel | 9 | Level from which spawning needs no tutorial visitors |

Tutorial visitors come from a separate three-row table: A (wheat, amount left to the client), B (1 bread, Joan) and C (bread, Greg).

## Tutorial offers versus generated offers

The two paths price an offer differently. This is the rule the server must follow:

| Path | Price | Verified |
| --- | --- | --- |
| Tutorial (command 35 starts a tutorial row that names a visitor) | `OrderPrice × count`, **no multiplier** | Yes: the build routine stores the item's `OrderPrice` in the visitor's price field |
| Generated (spawn timer, offline catch-up) | `OrderPrice × count × GoldMultiplier / 100` | Yes: the generator applies the multiplier |

For the tutorial bread visitor this is 11 coins, not the 15 a multiplied price would give. Earlier server versions applied the multiplier to tutorial offers as well, which left every new account 4 coins above the client after the second tutorial visitor (server 492, client 488). That is how the rule was found: the discrepancy equalled the difference between the two formulas (11 × 140 / 100 = 15, truncated), and the executor analysis showed the tutorial path never multiplies.

## Spawning (client)

- A timer counts up and, on reaching a random threshold between MinSpawnTime and MaxSpawnTime, attempts to spawn a visitor.
- A spawn needs fewer than MaxPeople active visitors, level ≥ UnlockLevel, and either level ≥ ForcedUnlockLevel or at least two tutorial visitors already spawned.
- On load the client catches up offline time by spawning up to `min(2 × elapsed / MaxSpawnTime, MaxPeople)` visitors.
- The visitor's person is chosen at random among those not already on the farm, with an idle time of `IdleTime × (1 to 3)`.

## Offer generation (client)

1. Choose a pool: silo goods or barn goods, whichever the other visitors request less; a random pick breaks ties.
2. Collect the candidate items of that pool that are unlocked and not already requested, and keep the GoodNumber items with the largest stock.
3. Roll 0–99. If a candidate exists and the roll is below CanCompleteOrderChance, pick one of the top-stock items at random, request `max(1, pct × stock / 100)` with pct between MinOrderSize and MaxOrderSize.
4. Otherwise request an item the player has none of (unlock level at most player level + 1), with a count derived from the level's guest order value divided by the item's order value.
5. Price is `OrderPrice × count × GoldMultiplier / 100`.

## Completion and decline

- **Command 40** (visitor instance ID, tick) removes the requested items, pays the stored price in coins, awards experience (ConstantExp, 5) and increments the completed-visitors counter.
- **Command 43** declines the visitor with no currency change (hypothesis).
- Visitors are not stored in the farm JSON; they are rebuilt from offline catch-up after loading.

## Randomness

All rolls come from one generator shared by about 160 call sites (ambient animals, pathing, animal idle states, Tom, the train and more). On the own farm it starts from a fixed seed, but how many rolls have happened by the time a visitor spawns depends on client timing, so the server cannot reproduce it.

## Server behavior

Implemented in [visitors.rs](../../src/replay/visitors.rs):

- Tutorial visitors are registered when command 35 starts a tutorial row, using the tutorial table and the unmultiplied `OrderPrice`.
- Generated visitors are approximated with the same algorithm on the server's own stock and random generator, and rebuilt after an offline gap. Offers can differ from the client's.
- On command 40 the server uses the tracked offer for that instance. If none matches, or the stock cannot cover it (more than 65% of stock), it generates a bounded replacement. If no eligible stock exists, nothing is invented.
- Every approximation is recorded as a replay warning.

## Open questions

- Whether any client message reports spawned offers (none seen in captures).
- The exact tutorial amount for visitor A, which the table leaves at 0.
- Confirming the offer formulas with a debugger on a running client.
