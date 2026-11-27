# Usage

```sh
hd-server --config config.toml [OPTIONS]
```

## Configuration

Settings come from a TOML file (`--config`), with CLI flags taking precedence.
`config.toml.example` is annotated. Unknown TOML keys fail startup. Relative
paths are resolved from the working directory.

### Main settings

| TOML key | CLI flag | Default | Meaning |
| --- | --- | --- | --- |
| `resources_dir` | `--resources-dir` | `resources` | Runtime data tables from hd-dumper |
| `log_level` | `--log-level` | `info` | `off`, `error`, `warn`, `info`, `debug`, `trace` |
| `http_listen` | `--http-listen` | `127.0.0.1:8080` | HTTP bootstrap listener |
| `game_listen` | `--game-listen` | `127.0.0.1:9339` | Encrypted game listener |
| `game_host` | `--game-host` | | Address advertised as `serverHost` |
| `asset_url` | `--asset-url` | | Advertised in `assetUrlList` (not served) |
| `after_ready_delay_ms` | `--after-ready-delay-ms` | `7000` | Delay between PostLoginReady and the farm. Shorter delays are untested |
| `idle_secs` | `--idle-secs` | `900` | Close a game connection after this long without client bytes |
| `dev_friends` | `--dev-friends` | `false` | Development entries in the friends bar |
| `replay_commands` | `--replay-commands` | `false` | Opt existing farms into command replay |
| `admin_enabled` | `--admin` / `--no-admin` | `false` | Admin API |
| `admin_listen` | `--admin-listen` | `127.0.0.1:8081` | Admin listener |
| `dump_enabled` | `--no-dump` | `true` | Record every game connection (see [Captures](#captures)) |

### New-account options

These apply only when an account is created.

| Flag | Meaning |
| --- | --- |
| `--cash N`, `--diamonds N` | Starting coin and diamond balances for new accounts (each sets the total balance) |
| `--stat ID=VALUE` | Any stat by data ID, repeatable |
| `--tutorial completed\|none` | Initial tutorial state |
| `--tutorial-done ROWS` | Rows of `tutorials.csv` to mark complete (0–173) |
| `--home-json FILE` | Farm document to use instead of the starter farm |

### Session options

| Flag | Meaning |
| --- | --- |
| `--xp N`, `--level N` | Override XP and level (1–700) in the farm sent this session. Not saved. No automatic level-up |
| `--friends-file FILE` | Reply to the friend-list request with entries from a JSON file (experimental) |
| `--env-settings FILE` | Replace the bootstrap JSON, see [bootstrap](#bootstrap) |

### Research options

These change protocol behaviour and exist for experiments. The defaults are
the verified values.

| Flag | Meaning |
| --- | --- |
| `--after-ready SPEC,...` | Messages sent after PostLoginReady, as `TYPE`, `TYPE:HEXBODY`, `TYPE@FILE` or a name (`own-home`, `session-ready`, ...). `none` sends nothing |
| `--after-ready-once` | Send them only on the first PostLoginReady, not on each return from a visit |
| `--reply TRIGGER=SPEC[+SPEC]` | Extra request/reply rule, sent once per connection |
| `--reply-every TRIGGER=SPEC` | Same, on every occurrence |
| `--no-default-replies` | Drop the built-in rules (19336 → 22151, 13447 → 22151 + 23549) |
| `--login-ok-tail trimmed\|legacy\|boxed` | LoginOk ending. Non-default values have caused client crashes |
| `--s2c-nonce-steps N,...` | Server-to-client nonce advance per message |
| `--scenario TEXT` | Experiment label written in every transcript line |

Run `hd-server --help` for the complete list.

### Maintenance, chat, and area messages

The message specs `maintenance:300` and `chat-silence` emit the maintenance
countdown (300 seconds) and neighborhood chat-silence notice. They work in
`--after-ready`, `--reply`, and `--reply-every`. For example,
`--reply 19336=maintenance:300` sends the warning on the first PhaseFourPing.

The area specs are `own-fishing@FILE`, `other-fishing@FILE`,
`own-town@FILE`, `other-town@FILE`, and `ai-town@FILE`. Supply a home JSON
file with the target area's `a`, `b`, and `c` blobs. These are explicit
snapshot senders for protocol testing. Gameplay replay currently uses the
own-farm simulation.

### Testing status notices

Run `python3 tools/test_status_notices.py` to choose a 20103 notice from an
interactive list. The tool builds and starts the server for the selected kind;
reconnect the client to receive it, then press Enter to stop the server and
choose another. These notices put the client into an error state, so each test
requires a reconnect. Stop any other server using the configured ports first.

Use `python3 tools/test_status_notices.py 9 --message 'Example'` for one
notice, or add `--flag` to try kind 9's other dialog. `--tail1` controls the
kind 17 warning value (default 60); `--tail2` selects kind 18's branch (default
-2, also try -3). `--list`, `--body-hex`, and `--help` show the available
variants and fields. The tool uses `config.toml` by default; pass `--config`
or put extra server options after `--`. The explicit `--test-status-notice`
server option used by the tool replaces the normal after-ready messages with
the supplied raw notice body for that run.

## Bootstrap

`GET /env_settings` returns:

```json
{
  "environment": "prod",
  "serverHost": "<game_host>",
  "assetUrlList": ["<asset_url>"],
  "status": "ok",
  "storeReady": "true"
}
```

- Other paths return 404, other methods 405, malformed requests 400.
- Proxy-form requests (`GET http://host/env_settings`) are accepted.
- `--env-settings FILE` serves a JSON file instead, re-read on every request.
  `--asset-url` still overrides its `assetUrlList`.
- `"environment": "maintenance"` shows the client's maintenance dialog.
- `"status": "update"` is refused with a 500, because it keeps the client from
  entering the game.

## Accounts and farms

All account data lives under the directory of `--accounts-file` (default
`data/accounts.json`):

| Path | Contents |
| --- | --- |
| `data/accounts.json` | Account index: ID, token, creation time |
| `data/<id>/home.json` | The saved farm |
| `data/<id>/turns.jsonl` | Every client turn received, one JSON object per line |
| `data/.hd-server.lock` | Lock that stops two servers sharing the directory |

- Writes are atomic.
- Login adopts whatever ID and token the client presents. A client with ID 0
  gets a new account. This is deliberate local-server behaviour, **not
  authentication**.
- New accounts get a starter farm generated from `resources/`. Existing farms
  are never migrated.
- New farms replay commands automatically. Older farms need
  `replay_commands = true`.
- Replay is partial. When a command's effect is not understood, the server
  logs a warning and continues. See [progress](progress.md#gameplay-commands).
- Greg's AI farm, shown when visiting Greg, is read from
  `resources/home-greg.json`. Without that file, Greg visits are disabled.

To back up or reset an account, stop the server and copy or delete its
directory and index entry, or use the [admin API](#admin-api).

## Logs

The log level is chosen by `--log-level`, then `log_level` in the TOML, then
`RUST_LOG`, then `info`.

| Level | Shows |
| --- | --- |
| `info` | Connections, logins, farm deliveries, sampled busy messages |
| `debug` | Every message and every replay decision |
| `trace` | Adds plaintext message bodies |

Session keys and login bodies are never logged. Replay warnings are never
sampled.

## Captures

Every game connection is recorded in `dumps/server/` (`--dump-dir`), unless
`--no-dump` (or `dump_enabled = false`) is set. Captures are never deleted, so
turn them off where storage is tight, such as on a phone:

| File | Contents |
| --- | --- |
| `*-c2s.bin` | Raw client-to-server bytes |
| `*-s2c.bin` | Raw server-to-client bytes |
| `*-plain.jsonl` | Decrypted transcript, one message per line, both directions |

## Admin API

Disabled by default. Enable it with `--admin` (or `admin_enabled = true`). It
listens on `127.0.0.1:8081`. A non-loopback address also needs
`--admin-insecure`. **There is no authentication.**

| Request | Response |
| --- | --- |
| `GET /accounts` | Account IDs and creation times, without tokens |
| `GET /accounts/<id>/turns` | The last 100 turns, oldest first |
| `PUT /accounts/<id>/home` | Replace the farm with an `{"a":{},"b":{},"c":{}}` document |

Errors are JSON `{"error": "..."}` with status 400, 404 or 405.

## Development friends

With `dev_friends = true`, the friends bar gets extra entries. Visiting one
triggers an action on your own account and reloads the farm:

- reset the account to a new starter farm,
- add coins and diamonds,
- skip the tutorial.

For local testing only.
