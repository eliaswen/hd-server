# Server-driven UI

This page covers what the server can make the client show: error dialogs,
custom text, news entries, and the player actions that report back to the
server. Everything here comes from static analysis of the x86 `libg.so`
unless marked live-tested. Addresses are ELF virtual addresses. See
[messages.md](messages.md) for the wire format and the full message list.

Strings in `libg.so` are XOR-obfuscated; they were decoded with the 128-byte
keystream at file offset `0xBC5E31`. Text keys (`TID_*`) resolve against
`assets/localization/texts.csv`.

## Summary

| Goal | Possible? | How |
|---|---|---|
| Show custom text | Yes, but it ends the session | StatusNotice (20103) kind 9 |
| Show a fixed built-in error dialog | Yes | StatusNotice (20103), other kinds |
| Show custom text in game without disconnecting | Unconfirmed | Mail (25985), or neighbourhood news entry (26279); neither showed anything when sent unprompted |
| Custom popup with several choices reported back | Not found | Only feature-specific requests exist |

The client localizes `TID_*` keys through `getLocalizedName` (`0x482181`).
An unknown key logs `getLocalizedName: No TID for <key>` and returns an empty
string; there is no raw-text fallback. So custom text only works where the
client displays a string field as-is.

## ByteStream readers

| Function | Reads |
|---|---|
| `0x450a60` | BE32 int |
| `0x46f7b0` | int (probably VInt) |
| `0x407af0` | bool |
| `0x146220(stream, debugName, 900000)` | string: BE32 length, then UTF-8 bytes |
| `0x602660` | LogicLong: two BE32 ints, high then low |
| `0x5f2da0` | true when the stream has no more data (guards optional fields) |

## StatusNotice (20103)

Decoder `0x213844`, vtable `0xb0f6ac`. Field names come from the decoder's
debug labels; the class is `LoginFailedMessage` in the client.

| Order | Offset | Type | Meaning |
|---|---|---|---|
| 1 | +0x34 | BE32 | kind |
| 2 | +0x3c | string | resource fingerprint |
| 3 | +0x40 | string | reason text |
| 4 (optional) | +0x38 | BE32 | extra int, used by kind 10 |
| 5 (optional) | +0x44 | bool | used by kind 9 |
| 6 (optional) | +0x48 | string | unknown |
| 7 | +0x4c | int | time value, used by kind 17 |
| 8 | +0x50 | int | sub-reason, used by kind 18 |

Fields 4–6 are read only while data remains. Fields 7 and 8 are read after
that block; whether they need fields 4–6 present was not checked.

The handler (`0x550839`) always sets the world to phase 3, so every kind
ends the session. It then switches on kind:

| Kind | Result |
|---|---|
| 1, 2 | Calls a global object; no dialog. Purpose unknown. |
| 3–6, and any value above 18 | Login-failed dialog: "Could not connect to the server. Error code ‹kind›" (kind 3 live-tested) |
| 7 | Remote resource-descriptor (JSON) update; no dialog |
| 8 | "Login failed" path in the startup code, given the reason text. Whether the text is shown is not verified. |
| 9 | **"Connection failed" dialog. If the reason text is non-empty it is the message; otherwise the stock "Unable to establish connection with the server. Please try again later."** If the bool is true, the banned dialog with Contact Support / Cancel is used instead. |
| 10 | Like kind 8, also given the extra int |
| 11 | "Resource update done" path; no dialog |
| 12, 13, 14 | Login-failed dialog with "Error code 12/13/14" |
| 15, 16 | Calls a global object; no dialog. Purpose unknown. |
| 17 | Account-deletion warning with a time, if the time value is positive |
| 18 | Branches on the sub-reason; −2 shows "account deleted on request" |

### Showing custom text

Send 20103 with kind 9, the bool false (or omitted), and the message in the
reason text field. The dialog has one button and reports nothing back. It is
not yet live-tested.

### Native dialog types

The handler shows dialogs through the native-dialog builder `0x3a63a2`
(called via `0x6a7be2`). Its 19 types, by text key:

| Type | Dialog |
|---|---|
| 1 | `CONNECTION_LOST` |
| 2 | `VERSION_NOT_UP_TO_DATE` |
| 3 | `VERSION_NOT_UP_TO_DATE_STORE_NOT_READY` |
| 4 | `LOGIN_FAILED`, with `<CODE>` |
| 5 | Stage-build version mismatch (developer builds, hard-coded English) |
| 6 | `UNABLE_TO_CONNECT` |
| 7 | `GAME_SERVER_NOT_RESPONDING` |
| 8 | `BILLING_SERVER_NOT_RESPONDING` |
| 9 | `BANNED_TITLE` ("Connection failed"), custom message |
| 10 | `SHUTDOWN_STARTED`, with `<MINUTES>` |
| 11 | `MAINTENANCE`, with a Forums button |
| 12 | `CHAT_JUST_BANNED` |
| 13 | `LOGGED_FROM_ANOTHER_DEVICE` |
| 14 | Banned, with Contact Support / Cancel |
| 15 | `MAINTENANCE_MESSAGE`, estimated soon / in minutes |
| 16 | Terms of Service / Privacy Policy / OK |
| 17 | Account deleted on request |
| 18 | Account deleted due to inactivity |
| 19 | Account deletion warning, with `<TIME>` |

Keys are `TID_NATIVE_DIALOG_*`, `TID_ERROR_POP_UP_*`, `TID_NOTICE_DIALOG_*`
or `TID_ACCOUNT_*`. No button in these dialogs was found to send anything to
the server.

## OtherDeviceConnected (25892)

Decoder `0x2bd160` reads a single BE32 reason; no string was found. Reason 1
shows "Another device is connecting to this farm" (live-tested). Any other
reason only logs `TitanDisconnectedMessage unknown reason %d`.

## News stream (26279, 25282)

- **AvatarStreamMessage (26279)**, decoder `0x691caa`: a count, then that
  many entries. Each entry starts with an int entry type; the factory
  `0x5bdcbc` accepts only types 3–7 and logs "Corrupted AvatarStreamMessage"
  otherwise. All five are neighbourhood (clan) entries.
- Types 3, 5, 6 and 7 carry raw "message text" and "neighborhood name"
  strings; type 4 carries one unlabeled string.
- **25282**, decoder `0x63756c`: a single LogicLong. It carries no text.
- If no game-log manager exists, the client logs "GameLogManager is null,
  AvatarStreamMessage ignored".

Not confirmed: that the strings are displayed as-is (they were traced only
until the entry joins a client-side list), and whether a player without a
neighbourhood sees them. Entries have no action fields.

Every entry decode first calls the shared `AvatarStreamEntry::decode`
(`0x2f2a0c`), which reads: LogicLong, LogicLong, string, VInt, VInt, bool,
bool. Field meanings are not known. VInts use Supercell's encoding (one byte
for 0–63); consecutive bools are packed as bits of one byte.

Candidate test body for 26279 with one type-3 entry
(JoinNeighborhoodResponse):

```
01                         ; VInt entry count
03                         ; VInt entry type
00000000 00000001          ; base: LogicLong (entry id?)
00000000 00000000          ; base: LogicLong
00000006 536572766572      ; base: string "Server"
00 00                      ; base: VInt, VInt
00                         ; base: two packed bools (false, false)
00000000 00000000          ; type 3: LogicLong neighbourhood id
00000004 54657374          ; type 3: string neighbourhood name "Test"
00000011 48656c6c6f2066726f6d20736572766572  ; type 3: message "Hello from server"
```

## Other text-carrying classes

- **MailList (25985)**, decoder `0x1c5288`: VInt count, then that many
  MailEntry objects (`0x1a4e7c`, no entry-type selector). Each entry is read
  in this order: VInt, VInt, 64-bit int, string, 4 × VInt, VInt, string,
  string, VInt, string, string, 6 × VInt, **string custom subject**,
  **string custom body**, VInt, VInt, then two fields from stream reader
  `0x718cc0` (format unknown). Sent unprompted after login it had no visible
  effect (live-tested); the client handler is not found, so it may need to
  answer a request such as opening the mailbox. Annotated example:
  `hayday-research/analysis/mailbox-custom-text.md`.
- **DynamicAdEntry** has link URL and pre-tap text fields; carrier unknown.
- **ManagerEvent (26306)** reads one string, but it is an event key, not
  displayed text.
- **Tip dispatch (23173)**: a list of ints; the client shows a random
  built-in tip.

## Client-side popups and deep links

- A generic three-button popup builder exists (`0x6b5b90`), but its only
  caller is the client's own rate-the-app prompt.
- The Terms of Service dialog has three fixed buttons, triggered from client
  configuration.
- A maintenance dialog reads `MaintenanceURL` and `MaintenanceSeconds` from
  configuration; which configuration source is not established.
- `hayday-inbox://` links (`OpenClanProfile`, `Share`, `OpenWeb`) are handled
  on the client.

## Player choices reported to the server

There is no generic "player chose option N" message. Choices reach the
server only through feature requests, for example the Friend Book:

| ID | Action | Body | Sent from |
|---|---|---|---|
| 10066 | Remove follower | LogicLong follower ID | Followers tab "Remove", or the "Remove Follower?" popup |
| 16858 | Unfollow | LogicLong followed player ID | Following tab "Unfollow", or the "Stop following?" popup |
| 17920 | Block | LogicLong player ID | Friend Book "Block" button; possibly other player popups |

All three share the sender shape `(ctx, LogicLong*)`: 10066 from `0x32d4ae`,
16858 from `0x43add6`, 17920 from `0x3d3962`. The Friend Book row class has
vtable `0xb13cec` (builder `0x417f10`); the confirm popup has vtable
`0xb1d570` (builder `0x67b860`, mode 1 = remove follower, mode 2 = stop
following). None expects a reply; 10066 has an explicit no-op handler on the
client.

## Open questions

- Live-test 20103 kind 9 with custom text.
- Whether kinds 8 and 10 show the reason text.
- Whether 26279 news text is displayed as-is, and to whom.
- Which client request 25985 answers, and the format of its last two fields.
- What kinds 1, 2, 15 and 16 do.
