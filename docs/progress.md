# Progress

The status of each part of the server for Hay Day 1.43.150 (build 1598).

**Legend:** ✅ works · 🟡 partial · ❌ not implemented

## Connection and login

| Feature | Status | Notes |
| --- | :---: | --- |
| HTTP bootstrap (`GET /env_settings`) | ✅ | Static or file-backed JSON, proxy-form requests accepted |
| Maintenance / update status in bootstrap | ✅ | Set through `--env-settings` |
| ClientHello / ServerHello | ✅ | Client version is checked; the asset fingerprint is logged |
| Encrypted login (X25519 + secretbox) | ✅ | Server key generated on first start |
| LoginOk | ✅ | Default "trimmed" ending; other endings are research options |
| Session encryption in both directions | ✅ | |
| Decrypt-failure reports from the client (10099) | 🟡 | Logged as an error, no action taken (no info on whether action should be taken) |
| Idle connection timeout | ✅ | `idle_secs`, default 900 s |
| Status notices (20103) | 🟡 | Builder exists; never sent automatically |
| x86 client | ✅ | |
| arm64-v8a client | ✅ | |
| armeabi-v7a client | ❌ | No support planned for now, I don't have the hardware to test it + APK patching requires considerable work. |

## Accounts and farm

| Feature | Status | Notes |
| --- | :---: | --- |
| Account creation and reconnect | ✅ | Any presented ID and token are adopted (no authentication) |
| Persistent per-account farm | ✅ | `data/<id>/home.json`, atomic writes |
| Starter farm for new accounts | ✅ | Generated from `resources/` |
| Own farm loading (26017) | ✅ | Sent after PostLoginReady, default 7 s delay, can be safely be reduced (tested down to 100ms) |
| Turn capture (11292) | ✅ | Every turn kept in `turns.jsonl` |
| Turn acknowledgement (14102) | ❌ | Not needed by the client so far |
| Command replay into the saved farm | 🟡 | About 60 of 304 command types, see below |
| XP and levels | 🟡 | XP from replayed actions; `--xp`/`--level` session overrides |
| Tutorial state | 🟡 | Can be marked done or partial for new accounts |
| Coins and diamonds | 🟡 | Spent and earned by replayed commands; starting values configurable. Coins and diamonds are each one balance (FreeCash and FreeDiamonds are merged in, including in older saves) |
| Offline progress | 🟡 | Timers advance on load for construction, production queues, animals and trade orders (`elapsed_home`, 66 ms ticks). Crops are instantly grown upon reload. Visitors are rebuilt by approximate catch-up, see [visitors](technical/visitors.md) |
| Farm migration between server versions | ❌ | Existing farms are never migrated |
| Multi-device support | ❌ | No way at the moment to authenticate multiple devices for the same account |
| Disconnect message ("another device is connecting") (25892) | 🟡 | Builder exists; never sent automatically. Unlikely to be implemented soon due to the difficulty of having multi device support for one farm |
| Rotating buildings | ❌ | Command is likely 53, not confirmed. The server does not acknowledge it. |

## Gameplay commands

The client sends gameplay as commands inside turns. The server decodes every
command type it has a schema for, and replays the ones listed here into the
saved farm. Unsupported commands are logged with a warning and skipped.

| Area | Status | Commands replayed | Notes |
| --- | :---: | --- | --- |
| Crops (plant, harvest) | ✅ | 1, 2 | |
| Buying and placing objects | ✅ | 4, 5 | |
| Moving objects | ✅ | 3 | |
| Animals (feed, collect, speed up) | ✅ | 9, 14, 15 | |
| Production buildings | ✅ | 10, 12, 13 | |
| Construction (speed up, claim) | ✅ | 16, 17 | |
| Storage and building upgrades | 🟡 | 11 | |
| Painting buildings | ✅ | 25 | |
| Trees and bushes | 🟡 | 59–63 | Chop (61) consumes the tool and removes the tree; speed-up, regrow and fruit picking work. Revive (63) only counts the event; its cost and XP are unknown |
| Rocks and forest obstacles | 🟡 | 18–20 | Clearing consumes the tool (Dynamite and similar), then a timer, speed-up and claim. The timer key the client uses is not confirmed |
| Random drops | 🟡 | | Level-up goods are granted. Threshold rewards (for example the Axe at `RewardSetML2`) are not modeled. Mine ore, mystery box and collectible rewards are not rolled |
| Truck orders | 🟡 | 27, 28, 29, 33, 41 | |
| Visitors | 🟡 | 40 and related | Offers are approximated; tutorial visitors priced exactly, see [visitors](technical/visitors.md) |
| Roadside shop | 🟡 | 30, 31, 32, 36, 37 | AI purchases recognized but not applied |
| Buying missing goods with diamonds | 🟡 | 39 | |
| Mystery boxes | 🟡 | 44, 45 | |
| Rotating buildings | ❌ | 53? | |
| Mine | 🟡 | 64 | Consumes a tool; ore yields are not applied (they come from the client's shared RNG) |
| Tom (hire, offers) | 🟡 | 68–74 | |
| Train | 🟡 | 90 | |
| Generic flags | 🟡 | 131 | |
| Boat orders | ❌ | | |
| Fishing, lobsters, ducks | ❌ | |
| Town and town visitors | ❌ | | |
| Pets, sanctuary | ❌ | | |
| Gifting, in-app purchases | ❌ | | |
| Layouts | ❌ | | |

## Gameplay progression

This tables tracks the playability of the game. As the level increases, more features are unlocked, which requires more server support.
✅ indicates perfect parity with the original game, 🟡 indicates limited parity, support for some features, while leaving others out, or a desynchronization in client/server progress despite replay working, and ❌ indicates that the features of the level aren't implemented at all.

| Level | Status | Notes |
| --- | :---: | --- |
| Level 1 - 4 | ✅ | All tested features appear to work |
| Level 5 | 🟡 | Visting Greg works, some tree/rock is missing, random drops of removal tools are not implemented |
| Level 6 | 🟡 | Cows and related features (milk, cream, dairy) work. Achievements are not implemented. Visitor rewards are approximated (see [visitors](technical/visitors.md)). |
| Level 7+ | ❌ | Untested. Some features may work. |

## Social and visits

| Feature | Status | Notes |
| --- | :---: | --- |
| Friend list request (19838) | 🟡 | Empty list, a JSON file, or development entries |
| Visiting Greg's AI farm (28383) | ✅ | Needs `resources/home-greg.json` |
| Visiting another player's farm (29950) | ❌ | Request is decoded; no farm is returned |
| Neighborhoods, chat, derby | ❌ | |
| Push notification registration (19080) | ❌ | Ignored |

## Other

| Feature | Status | Notes |
| --- | :---: | --- |
| Serving game assets | ❌ | The client keeps its bundled assets; `asset_url` is only advertised |
| Admin HTTP API | 🟡 | List accounts, read turns, replace a farm |
| Traffic captures and decrypted transcripts | ✅ | `dumps/server/` |
| Development friends (reset, resources, skip tutorial) | 🟡 | Off by default |

## Protocol coverage

The client knows 237 message classes. 28 of them have a name and an
established role; the rest are listed in [messages](technical/messages.md)
with whatever is known about them.
