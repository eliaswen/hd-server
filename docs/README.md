# Technical index

How the Hay Day client and this server talk to each other, and where each part
is documented. Read this page first, then follow the links for detail.

Target: Hay Day 1.43.150 (build 1598).

## The model in one paragraph

The client is authoritative for gameplay. It simulates the farm locally: a
tap on a field plants a crop immediately, with no round trip. The server does
not validate or run that simulation. Instead the client reports what the player
did, and the server **replicates** those actions into its own saved copy of the
farm. The saved copy is what the client receives the next time it loads the
farm. The server never needs to push gameplay changes to a connected client.

## Connection lifecycle

```
Client                                        Server
  |  GET /env_settings (HTTP)  ------------->  |   bootstrap: game host, asset URLs
  |  <-------------------------  JSON          |
  |                                            |
  |  ClientHello 10100 (cleartext) ----------> |   version check
  |  <---------------------- ServerHello 20100 |   24-byte challenge
  |  Login 10101 (X25519 + secretbox) -------> |   account ID + token adopted
  |  <------------------------- LoginOk 29501  |   session key; encrypted from here on
  |                                            |
  |  PostLoginReady 10366 -------------------> |
  |  <------------- OwnHomeData 26017 (delay)  |   full farm snapshot
  |                                            |
  |  EndClientTurn 11292 (about every 10 s) -> |   batch of commands, replicated
  |  EndClientTurn 11292 -------------------> |   (no reply required)
  |  ...                                       |
```

1. **Bootstrap.** A plain HTTP request tells the client which host to use for
   the game connection.
2. **Handshake and login.** Cleartext hellos, then an X25519 key exchange. After
   LoginOk every frame in both directions is encrypted with per-direction
   nonces. See [Login and encryption](technical/messages.md#login-and-encryption).
3. **Farm load.** After PostLoginReady the server waits briefly, then sends
   OwnHomeData (26017): avatar stats, unlock bitsets and the farm objects as
   compressed JSON. This is the authoritative snapshot; it overrides whatever
   the client had.
4. **Play.** The client sends EndClientTurn (11292) batches. The server
   replicates them and saves the result. See below.
5. **Mode changes.** Visiting another farm (for example Greg's AI farm, 28383)
   and returning are signalled by dedicated messages. The server keeps the
   player's replay state aside while they visit so that returning resumes the
   same objects. See [Session flow](technical/messages.md#session-flow-after-login).

Frames are `type (2 bytes) | length (3) | version (2) | payload`. Client
messages are 10000–19999 and server messages 20000–29999. See
[Frames](technical/messages.md#frames).

## How the server replicates client actions

Each EndClientTurn carries a tick counter (30 ticks per second), a state
checksum and a list of commands. Every command has a type (1 to 304), fields
that depend on the type, and the tick at which the client executed it.

For each turn the server:

1. **Decodes** the commands. Unknown or truncated commands are kept raw and the
   rest of the turn still replays.
2. **Records** the raw turn in `data/<id>/turns.jsonl`, whether or not it can be
   replayed.
3. **Replays** each supported command against a candidate copy of the farm
   (`src/replay/`): plant, harvest, feed, produce, orders, XP, coins and so on.
   Game data tables from `resources/` supply costs, durations and rewards.
4. **Commits or skips.** If a command fails or is not understood, only that
   command is rolled back and a warning is logged. Replay continues with the
   next one. Arrival order wins; duplicate turns are ignored.
5. **Saves** the farm atomically to `data/<id>/home.json`.

Things to keep in mind when working on replay:

- **Replay is partial.** Roughly 60 of the 304 command types have effects. The
  rest are captured but not applied, so the saved farm can fall behind what the
  client displayed. Status is tracked in [progress](progress.md).
- **The next load corrects drift.** Because the client trusts the 26017
  snapshot, the server only has to be right at load time, not continuously.
- **Timers are replayed by tick.** Production, growth and similar timers are
  advanced from the command ticks, and again from wall-clock time when a farm is
  delivered, so progress continues while the player is offline.
- **Object identity is positional.** Instance serials come from the order of
  the objects in the snapshot the server last sent. Commands refer to those
  serials, so the server must rebuild them from the exact objects it delivered.
- **Some state is invented by the client.** Visitor offers come from the client's
  shared RNG, which the server cannot see. The server keeps an approximate offer
  and applies it when the completion command arrives. See
  [Visitors](technical/visitors.md).

## Documents

| Document | What it covers |
| --- | --- |
| [Messages](technical/messages.md) | Wire format, login and encryption, session flow, and every known message class |
| [EndClientTurn commands](technical/end-client-turn-commands.md) | Command IDs, payload layouts and how certain each label is |
| [Visitors](technical/visitors.md) | How the client generates visitor deals, and how the server approximates them |
| [Server-driven UI](technical/server-ui.md) | What the server can make the client display (notices, dialogs, news) and the actions that report back |
| [Progress](progress.md) | What is implemented, partial or missing |
| [Usage](usage.md) | Configuration, storage layout, logs and captures, admin API |
| [Install](install.md) | Building, resources, patching the client, network setup |

## Where it lives in the code

| Concern | Location |
| --- | --- |
| HTTP bootstrap | `src/net/http.rs` |
| Per-connection state machine (handshake, login, replies) | `src/net/session.rs`, `src/net/connection.rs` |
| Frame codec, ByteStream, encryption | `src/protocol/scframe.rs`, `stream.rs`, `crypto.rs` |
| Message catalogue and typed bodies, including EndClientTurn | `src/protocol/catalogue.rs`, `messages.rs` |
| Farm model, starter farm, progression, command schema | `src/game/` |
| Command replay (orders, production, timers, visitors) | `src/replay/` |
| Account index, saved farms, turn capture | `src/storage/accounts.rs` |
| Runtime tables loaded from `resources/` | `src/data/resource_data.rs` |

## How the facts were obtained

Most protocol facts come from static analysis of the client's native library
(x86), checked against runtime captures and live tests. Each technical page
says which method backs which claim, and marks hypotheses as such. Treat
anything not marked verified as provisional.
