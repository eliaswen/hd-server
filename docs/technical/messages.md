# Messages

This page documents the wire protocol of Hay Day 1.43.150 (build 1598) and
every message class the client knows, including the ones nothing is known
about yet.

## How this information was obtained

Each fact below comes from one of these methods:

- **Static analysis**: reading the client's native library (`libg.so`, x86)
  without running it. The message inventory comes from a byte sweep of the
  library for type getters (`mov eax, imm16; ret`) and the class vtables that
  reference them. Addresses are ELF virtual addresses of the x86 build.
- **Runtime capture**: messages observed in decrypted transcripts between a
  patched client and this server.
- **Live test**: a value sent deliberately to a client to see how it reacts.

When a row says only what a decoder or handler does, that is static analysis.
"Seen", "observed" and "in every capture" mean runtime capture. "Live-tested"
means live test.

## Frames

Every message, in both directions, is one frame:

| Bytes | Field |
| --- | --- |
| 2 | Message type, big-endian |
| 3 | Payload length, big-endian |
| 2 | Version, big-endian (0 for ClientHello) |
| n | Payload |

Types 10000–19999 are client-to-server and 20000–29999 server-to-client, with
one known exception (14102 is sent by the server).

Bodies are written with a "ByteStream" encoding: big-endian integers,
variable-length integers (VInt), length-prefixed strings (length -1 = null)
and packed booleans.

## Login and encryption

Established by static analysis and verified against a real client.

1. **ClientHello (10100)**, cleartext. Contains the client version (3, 14, 1,
   43, 150 for this build) and a 40-character asset fingerprint: the SHA-1 of
   the concatenated per-file hashes of the client's asset manifest.
2. **ServerHello (20100)**, cleartext. Carries a 24-byte challenge.
3. **Login (10101)**. The client's 32-byte X25519 public key, then a box
   sealed with the shared key and a BLAKE2b-24 nonce of both public keys. The
   plaintext is the challenge, a 24-byte client nonce, then the login body
   (account ID high/low and token, followed by fields not yet decoded).
4. **LoginOk (29501)**, sealed with a nonce over the client nonce and both
   public keys. Contains a server nonce, a 32-byte session key, then the body
   (account ID, token and session settings).

After LoginOk both directions use the session key with per-direction nonces.
The cipher is a secretbox built from HChaCha (22 rounds), ChaCha (18 rounds)
and Poly1305. Each nonce is incremented by 2 (little-endian) per message.

## Session flow after login

Observed by runtime capture:

1. The client sends **PostLoginReady (10366)**. The server waits
   (`after_ready_delay_ms`, 7 s by default) and sends **OwnHomeData (26017)**.
2. While loading, the client sends **HomeRequest (13447)** and
   **PhaseFourPing (19336)**. The server answers with **PingReply (22151)**
   and **HomeSync (23549)**.
3. During play, the client sends **EndClientTurn (11292)** roughly every 10 s.
   No reply is required.
4. Each mode change sends a notification. Mode 1 (own farm) is 10366, mode 2
   (visit a player) is 18485, mode 3 (visit Greg) is 13198. Modes 4 to 8 use
   17534, 16986, 19214, 14581 and 11833; their meaning is unknown.

## Known messages

### OwnHomeData (26017), AiHomeData (28383)

The farm. Found by static analysis of the handler and checked by live tests.
The body holds avatar stats, unlock bitsets and three zlib-compressed JSON
documents (`a`, `b`, `c`) that describe the farm objects. 28383 (Greg's farm)
uses the same layout. Certain all-zero body lengths crash the client.

### EndClientTurn (11292)

The client's batch of commands. A VInt tick counter (30 per second), a VInt
state checksum, a command count, then commands. Each command starts with a
VInt command type (1 to 304); its fields are read from a schema recovered by
static analysis, and the last field is always the execution tick. Command
meanings come from the client's own analytics labels and from runtime capture.

### OtherDeviceConnected (25892)

A single big-endian 32-bit reason (decoder 0x2bd160 reads no string).
Reason 1 shows "Another device is connecting to this farm" (live-tested).
Any other number only logs the message on the client.

### StatusNotice (20103)

Body: BE32 kind, string resource fingerprint, string reason text, then
optional fields. The handler sets the world to phase 3 (the session ends)
and switches on kind (1–18). Kind 3 shows "Could not connect to the server.
Error code 3" (live-tested). Kind 9 shows the reason text as the message of a
"Connection failed" dialog, so the server can display custom text this way.
The full field list, every kind and the built-in dialogs are in
[server-ui.md](server-ui.md). The factory has two classes for 20103, one with
a body decoder and one without; which one the client uses is not
established.

### DecryptFailure (10099)

Sent by the client with the big-endian 32-bit type of a server message it
could not decrypt. The client then resets the connection.

### SessionReady (29997)

Tiny acknowledgement. Its handler sets the game's ready flag. It does not
advance the nonce.

### Mailbox (25985)

The client's mailbox (top-left icon, unlocked at level 7) is filled by a list
of mail entries. Each entry has a long fixed sequence of fields; among them
are a "custom subject" and a "custom body" string, which makes mail the most
likely way to put custom text in the game without disconnecting. The handler
that turns 25985 into mailbox items has not been found; it is not in the
main game switch, so the client may only accept it as the reply to some
request. Sending it unprompted right after the farm loads showed nothing
(live-tested). Layout details are in [server-ui.md](server-ui.md).

### Server-driven UI

Custom text, built-in dialogs, news entries and player actions that report
back to the server are documented in [server-ui.md](server-ui.md).

### Friends and visits

- **FriendListRequest (19838)**: empty, sent once when the client has no
  friend data. The reply is **FriendListReply (26656)**, a list of friend
  entries; an empty list is the single byte `00`.
- **VisitPlayer (18485)**: VInt selector, then the target account ID as a
  big-endian 64-bit integer. The expected reply is **OtherHomeData (29950)**,
  whose decoder is still unclear.
- **Unknown13198 (13198)**: entering Greg's farm. A VInt selector, `00` in
  every capture.

## All message classes

The client's factory has 237 classes (236 types; 20103 has two). ClientHello
(10100) has a type getter but no vtable match, so it is not in the table.

Columns:

- **Body**: "empty decoder" means the class uses the shared empty decoder, so
  the client parses no fields. Usually a fieldless or send-only message.
- **Server**: *Sent* means this server sends it in normal play. *Handled*
  means it reacts to it. *Builder only* means it can build the message but
  only sends it when asked to through research options. "—" means unused.
- **Notes**: "Game-switch arm" is the branch of the client's per-type handler
  that processes the message; "listener" means it is handled outside that
  switch.

| Type | Name | Direction | Body | Server | Notes |
|---:|---|---|---|---|---|
| 10066 | RemoveFollower | C→S | decoded | — | Friend Book "Remove" on a follower (also the "Remove Follower?" confirm popup). Body: LogicLong follower account id (BE32 high, BE32 low). Client has a no-op inbound arm; no reply expected. |
| 10067 | — | C→S | decoded | — | Not documented. |
| 10099 | DecryptFailure | C→S | decoded | Handled | Client->server DecryptFailure: BE32 type of a server frame that failed decryption; the client then resets the connection. Also queued at login state 5. |
| 10101 | Login | C→S | empty decoder | Handled | client->server encrypted login envelope (public key plus sealed challenge/nonce/body). Empty decoder, send-only. |
| 10102 | — | C→S | decoded | — | Not documented. |
| 10108 | — | C→S | decoded | — | Not documented. |
| 10141 | — | C→S | decoded | — | Not documented. |
| 10166 | — | C→S | decoded | — | Not documented. |
| 10213 | Unknown10213 | C→S | decoded | — | seen in a visit-session trace just before PostLoginReady. Role not established. |
| 10219 | ClientInfo | C→S | decoded | — | phase-4 request sent in screen state 0x5d. Fields: int +0x34 (default -1), +0x38, +0x3c, +0x40. No reply observed. |
| 10256 | — | C→S | decoded | — | Not documented. |
| 10268 | — | C→S | decoded | — | Not documented. |
| 10366 | PostLoginReady | C→S | empty decoder | Handled | fieldless client->server message after LoginOk (ctor 0xdc646, empty decoder). Also sent on return from a visit. |
| 10520 | — | C→S | decoded | — | Not documented. |
| 10543 | — | C→S | decoded | — | Not documented. |
| 10556 | — | C→S | decoded | — | Not documented. |
| 10705 | — | C→S | decoded | — | Not documented. |
| 10860 | — | C→S | decoded | — | Not documented. |
| 10901 | — | C→S | empty decoder | — | Empty decoder (send-shaped); never seen on the wire. |
| 10987 | — | C→S | decoded | — | Not documented. |
| 10993 | — | C→S | decoded | — | Not documented. |
| 11023 | — | C→S | decoded | — | Not documented. |
| 11124 | — | C→S | empty decoder | — | Not documented. |
| 11282 | — | C→S | decoded | — | Not documented. |
| 11292 | EndClientTurn | C→S | decoded | Handled | client->server turn batch. Header: VInt tick counter at 30 Hz and a VInt state checksum, then commands. Observed roughly every 10 s. |
| 11316 | — | C→S | decoded | — | Not documented. |
| 11362 | — | C→S | decoded | — | Not documented. |
| 11415 | — | C→S | decoded | — | Not documented. |
| 11489 | — | C→S | decoded | — | Not documented. |
| 11747 | — | C→S | decoded | — | Not documented. |
| 11833 | — | C→S | decoded | Handled | Mode-entry notification for mode 8 (ModeManager). Handler not yet added. |
| 11923 | — | C→S | decoded | — | Not documented. |
| 12041 | — | C→S | decoded | — | Not documented. |
| 12215 | — | C→S | decoded | — | Not documented. |
| 12281 | — | C→S | decoded | — | Not documented. |
| 12328 | — | C→S | decoded | — | Not documented. |
| 12392 | — | C→S | decoded | — | Not documented. |
| 12453 | — | C→S | decoded | — | Not documented. |
| 12575 | — | C→S | decoded | — | Not documented. |
| 12651 | — | C→S | decoded | — | Not documented. |
| 12688 | — | C→S | decoded | — | Not documented. |
| 12771 | — | C→S | decoded | — | Not documented. |
| 12793 | — | C→S | decoded | — | Not documented. |
| 12857 | — | C→S | decoded | — | Not documented. |
| 12867 | — | C→S | decoded | — | Not documented. |
| 12886 | — | C→S | decoded | — | Not documented. |
| 13038 | — | C→S | decoded | — | Not documented. |
| 13198 | Unknown13198 | C→S | decoded | Handled | Client->server 'Entered mode 3' (visit another farm). VInt selector, body 00 in every capture; sent by ModeManager after the mode switch. |
| 13280 | — | C→S | decoded | — | Not documented. |
| 13388 | — | C→S | decoded | — | Not documented. |
| 13447 | HomeRequest | C→S | decoded | Handled | phase-4 home/entity sync request, sent while world+0x230 != 2. Answered by 23549. |
| 13568 | — | C→S | decoded | — | Not documented. |
| 13765 | — | C→S | decoded | — | Not documented. |
| 13864 | LoadingStateReady | C→S | empty decoder | — | fieldless phase-4 message sent in screen state 0x5b; writes no state. |
| 13902 | — | C→S | decoded | — | Not documented. |
| 13974 | — | C→S | decoded | — | Not documented. |
| 14004 | — | C→S | decoded | — | Not documented. |
| 14102 | Unknown14102 | S→C | decoded | — | Server->client turn ack (EndClientTurn reply), built by the inbound factory (ctor 0x128eae). Sits in the client band numerically, but is server-sent. |
| 14144 | — | C→S | decoded | — | Not documented. |
| 14183 | — | C→S | decoded | — | Not documented. |
| 14275 | — | C→S | decoded | — | Not documented. |
| 14435 | — | C→S | decoded | — | Not documented. |
| 14497 | — | C→S | decoded | — | Not documented. |
| 14543 | — | C→S | decoded | — | Not documented. |
| 14581 | — | C→S | decoded | Handled | Mode-entry notification for mode 7 (ModeManager). Handler not yet added. |
| 14690 | — | C→S | decoded | — | Not documented. |
| 14796 | — | C→S | decoded | — | Not documented. |
| 14821 | — | C→S | decoded | — | Not documented. |
| 15174 | — | C→S | decoded | — | Not documented. |
| 15203 | — | C→S | decoded | — | Not documented. |
| 15568 | — | C→S | decoded | — | Not documented. |
| 15840 | — | C→S | decoded | — | Not documented. |
| 16026 | — | C→S | decoded | — | Not documented. |
| 16063 | — | C→S | decoded | — | Not documented. |
| 16368 | AvatarHomeIdentityReport | C→S | decoded | Handled | client->server, 8-byte body, a raw copy of avatar +0x00/+0x04. Sent once; own-account vs loaded-home identity unresolved. |
| 16389 | — | C→S | decoded | — | Not documented. |
| 16449 | — | C→S | decoded | — | Not documented. |
| 16487 | — | C→S | decoded | — | Not documented. |
| 16560 | — | C→S | decoded | — | Not documented. |
| 16627 | — | C→S | decoded | — | Not documented. |
| 16707 | — | C→S | decoded | — | Not documented. |
| 16843 | — | C→S | decoded | — | Not documented. |
| 16858 | Unfollow | C→S | decoded | — | Friend Book "Unfollow" / "Stop following?" confirm. Body: LogicLong id of the followed player. No reply expected. |
| 16986 | — | C→S | decoded | Handled | Mode-entry notification for mode 5 (ModeManager). Handler not yet added. |
| 16990 | — | C→S | decoded | — | Not documented. |
| 17096 | — | C→S | decoded | — | Not documented. |
| 17263 | — | C→S | empty decoder | — | Not documented. |
| 17274 | OptionalIds | C→S | decoded | — | phase-4 request with two optional int64s. Its send path has no caller in this build; answered by 20062. |
| 17337 | — | C→S | decoded | — | Not documented. |
| 17462 | — | C→S | empty decoder | — | Not documented. |
| 17534 | — | C→S | empty decoder | Handled | Mode-entry notification for mode 4 (ModeManager). Handler not yet added. |
| 17556 | — | C→S | decoded | — | Not documented. |
| 17776 | — | C→S | empty decoder | — | Not documented. |
| 17887 | — | C→S | decoded | — | Not documented. |
| 17914 | — | C→S | empty decoder | — | Not documented. |
| 17920 | Block | C→S | decoded | — | Friend Book "Block" button (may also be sent from other player popups). Body: LogicLong id of the blocked player. No reply expected. |
| 17970 | — | C→S | decoded | — | Not documented. |
| 18132 | — | C→S | decoded | — | Not documented. |
| 18167 | — | C→S | decoded | — | Not documented. |
| 18322 | — | C→S | empty decoder | — | Not documented. |
| 18485 | VisitPlayer | C→S | decoded | Handled | client->server visit request for mode 2. VInt selector, then BE64 target account id. |
| 18569 | — | C→S | decoded | — | Not documented. |
| 18785 | — | C→S | decoded | — | Not documented. |
| 19080 | — | C→S | decoded | — | Client->server push-notification registration (push token). Sent only at level >= 5 (MinLevelForPushNotification in game_config.csv). |
| 19126 | — | C→S | decoded | — | Not documented. |
| 19214 | — | C→S | empty decoder | Handled | Mode-entry notification for mode 6 (ModeManager). Handler not yet added. |
| 19249 | — | C→S | decoded | — | Not documented. |
| 19336 | PhaseFourPing | C→S | decoded | Handled | fieldless phase-4 ping, sent while world+0x234 != 2. Answered by 22151. |
| 19440 | — | C→S | decoded | — | Not documented. |
| 19445 | — | C→S | decoded | — | Not documented. |
| 19446 | — | C→S | decoded | — | Not documented. |
| 19471 | — | C→S | decoded | — | Not documented. |
| 19543 | — | C→S | decoded | — | Not documented. |
| 19597 | — | C→S | decoded | — | Not documented. |
| 19611 | — | C→S | decoded | — | Not documented. |
| 19797 | — | C→S | decoded | — | Not documented. |
| 19838 | FriendListRequest | C→S | decoded | Handled | empty client->server request. Sent once when the friend state is 'none'; the reply is 26656. |
| 20000 | — | Connection layer | decoded | — | Connection-layer message, not routed through the game switch: after each decoded message the connection notifies its listener at conn+0xac. |
| 20062 | OptionalIdsReply | S→C | decoded | Builder only | reply that sets world+0x238=2 and releases the optional 17274 path. Game-switch arm 0x54fb51. |
| 20069 | — | S→C | decoded | — | Not documented. |
| 20100 | ServerHello | S→C | decoded | Sent | login step 1 from the server. Verified against the session blob; the client then sends Login (10101). Decoder 0xf1a70. |
| 20103 | StatusNotice | S→C | decoded | Builder only | server status/error. Handler 0x550839 sets world phase 3 and switches on kind 1–18; kind 9 shows the body's reason text as a custom message (see server-ui.md). This is the first of two factory rows for 20103; this one has a body decoder. |
| 20103 | StatusNotice | S→C | empty decoder | Builder only | Second factory row for 20103 (empty decoder). Which row the client uses is not established. |
| 20108 | — | S→C | decoded | — | Not documented. |
| 20162 | — | S→C | decoded | — | Not documented. |
| 20214 | — | S→C | decoded | — | Not documented. |
| 20227 | — | S→C | decoded | — | Not documented. |
| 20247 | — | S→C | decoded | — | Not documented. |
| 20316 | — | S→C | decoded | — | Not documented. |
| 20360 | — | S→C | decoded | — | Not documented. |
| 20415 | — | S→C | decoded | — | Not documented. |
| 20599 | — | S→C | decoded | — | Not documented. |
| 20890 | — | S→C | decoded | — | Not documented. |
| 20982 | — | S→C | decoded | — | Not documented. |
| 21086 | — | S→C | decoded | — | Game-switch arm 0x54f54e; no behaviour recorded. |
| 21309 | — | S→C | decoded | — | Game-switch arm 0x54ff26; no behaviour recorded. |
| 21395 | — | S→C | decoded | — | Not documented. |
| 21404 | — | S→C | decoded | — | Not documented. |
| 21444 | — | S→C | decoded | — | Game-switch arm 0x54f132; no behaviour recorded. |
| 21608 | — | S→C | decoded | — | Game-switch arm 0x5503c6: kind 0/1/2 branches that post events. Reaches no scene transition. |
| 21738 | — | S→C | empty decoder | — | Not documented. |
| 21817 | — | S→C | decoded | — | Generic scene-setup arm (0x54e9be); no behaviour recorded. |
| 21823 | — | S→C | decoded | — | Not documented. |
| 21876 | — | S→C | decoded | — | Game-switch arm 0x5516c3: posts event tagged 0x5574. Decoder holds string/int scalars. |
| 21895 | — | S→C | decoded | — | Not documented. |
| 21901 | — | S→C | decoded | — | Generic scene-setup arm (0x54f8ed); no behaviour recorded. |
| 22011 | — | S→C | decoded | — | Game-switch arm 0x551ae9 sets world+0x224=2. Decoder: bool plus list of 36-byte entities. |
| 22151 | PingReply | S→C | decoded | Sent | reply to 19336 that sets world+0x234=2. Game-switch arm 0x54fc84. |
| 22220 | — | S→C | decoded | — | Not documented. |
| 22242 | — | S→C | decoded | — | Not documented. |
| 22326 | — | S→C | decoded | — | Not documented. |
| 22362 | — | S→C | empty decoder | — | Not documented. |
| 22533 | — | S→C | decoded | — | Not documented. |
| 22542 | — | S→C | decoded | — | Game-switch arm 0x54ef00; no behaviour recorded. |
| 22666 | — | S→C | decoded | — | Small entity/event update. Game-switch arm 0x551e97; its sibling arm sets world+0x228=2. Factory object 0x44 bytes, decoder about 988 bytes. |
| 22756 | — | S→C | decoded | — | Game-switch arm 0x55021e; no behaviour recorded. |
| 22852 | — | S→C | decoded | — | Not documented. |
| 22879 | — | S→C | decoded | — | Not documented. |
| 23066 | — | S→C | decoded | — | Profile-like: two optional strings plus six ints. Game-switch arm 0x54f7c4. |
| 23173 | — | S→C | decoded | — | Tip/notification dispatch: decodes a list of ints and picks one at random. Game- switch arm 0x551d5b. |
| 23460 | — | S→C | decoded | — | Game-switch arm 0x55006c; no behaviour recorded. |
| 23549 | HomeSync | S→C | decoded | Sent | reply to 13447 that sets world+0x230=2. Body: three count-prefixed lists plus an int. Game-switch arm 0x550d76. |
| 23591 | — | S→C | decoded | — | Generic scene-setup arm (0x54f3b2). String/int scalars. |
| 23594 | — | S→C | decoded | — | Not documented. |
| 23597 | — | S→C | decoded | — | No-op: recognised by the game switch and ignored. |
| 23631 | — | S→C | decoded | — | Generic scene-setup arm (0x54ec35); no behaviour recorded. |
| 23698 | — | S→C | decoded | — | Not documented. |
| 23809 | — | S→C | decoded | — | String/int scalars. Among the server-band types examined during the loading-screen investigation. |
| 23899 | — | S→C | decoded | — | Not documented. |
| 23917 | — | S→C | decoded | — | Game-switch arm 0x54faa1; no behaviour recorded. |
| 23933 | — | S→C | decoded | — | Not documented. |
| 24285 | — | S→C | decoded | — | String/int scalars. |
| 24315 | — | S→C | decoded | — | Not documented. |
| 24728 | — | S→C | decoded | — | Not documented. |
| 24855 | — | S→C | decoded | — | Not documented. |
| 24878 | — | S→C | decoded | — | Not documented. |
| 24979 | — | S→C | decoded | — | Not documented. |
| 25045 | — | S→C | decoded | — | Not documented. |
| 25067 | — | S→C | decoded | — | Handled outside the game switch by a listener/broadcast dispatcher (cmp 0x61eb at 0x534209). Factory object 0x38 bytes: one bit plus an optional 36-byte object. |
| 25282 | — | S→C | decoded | — | Body: a single LogicLong (decoder 0x63756c). Routed with the news stream; carries no text. |
| 25362 | — | S→C | decoded | — | Not documented. |
| 25491 | EntityBatch | S→C | decoded | Builder only | count-capped list of 148-byte entities plus ints; the best farm-grid shape. Handled on the listener path (0x607348), not the game switch. |
| 25492 | — | S→C | decoded | — | Int plus count-capped list of 48-byte entities. Not dispatched at all. |
| 25733 | — | S→C | decoded | — | Not documented. |
| 25886 | — | S→C | decoded | — | Game-switch arm 0x550178; no behaviour recorded. |
| 25892 | OtherDeviceConnected | S→C | decoded | Builder only | server disconnect. Body: BE32 reason only. Reason 1 = another device is connecting (live-tested). Other reasons log on the client but are not acted upon. |
| 25985 | MailList | S→C | decoded | — | Mailbox entries: VInt count, then MailEntry objects (decoder 0x1a4e7c) with custom subject and custom body strings. Sent unprompted after login: no visible effect (live-tested). Client handler not found. See server-ui.md. |
| 26008 | — | S→C | decoded | — | Not documented. |
| 26017 | OwnHomeData | S→C | decoded | Sent | own farm/home payload. Game-switch arm 0x54f5b0. Certain all-zero body lengths crash the client in this arm. |
| 26204 | — | S→C | decoded | — | Not documented. |
| 26279 | AvatarStreamMessage | S→C | decoded | — | News stream: count, then entries of type 3–7 (all neighbourhood entries; most carry raw message text and a neighbourhood name). See server-ui.md. |
| 26306 | ManagerEvent | S→C | decoded | — | body is a single string key (not displayed text), routed to global manager 0x1990c2. Best farm-state candidate so far. |
| 26339 | — | S→C | decoded | — | Not documented. |
| 26359 | — | S→C | decoded | — | Not documented. |
| 26513 | — | S→C | decoded | — | String/int scalars. |
| 26650 | — | S→C | decoded | — | TrainStationDataMessage: train station data. Decoder 0x545268. Game-switch arm 0x54f19e. |
| 26656 | FriendListReply | S→C | decoded | Sent | friend list of FriendEntry records. An empty list is body 00. Game- switch arm 0x5504bf. |
| 26697 | — | S→C | decoded | — | Not documented. |
| 26742 | — | S→C | decoded | — | Generic scene-setup arm (0x54eb09); no behaviour recorded. |
| 26837 | — | S→C | decoded | — | Not documented. |
| 26950 | — | S→C | decoded | — | Game-switch arm 0x55180f. Its call chain reaches a UI/notification helper, not a scene swap. |
| 26968 | — | S→C | decoded | — | Not documented. |
| 26973 | — | S→C | decoded | — | Not documented. |
| 27066 | — | S→C | decoded | — | Not documented. |
| 27105 | — | S→C | decoded | — | Varint plus a named field with default 900000 ms. Minimal arm: one call (0x67aae6). |
| 27171 | — | S→C | decoded | — | Not documented. |
| 27458 | — | S→C | decoded | — | Not documented. |
| 27466 | — | S→C | decoded | — | Listener-tree case 0x6b4a. No scene transition. |
| 27491 | — | S→C | decoded | — | Largest in-switch state update chain (arm 0x54fdc6). Body: two ints plus two strings. |
| 27508 | — | S→C | decoded | — | Not documented. |
| 27554 | — | S→C | decoded | — | Not documented. |
| 27626 | — | S→C | decoded | — | Game-switch arm 0x54f00d; no behaviour recorded. |
| 27996 | — | S→C | decoded | — | Listener-tree case 0x6d5c. No scene transition. |
| 28004 | — | S→C | decoded | — | Not documented. |
| 28054 | — | S→C | decoded | — | Game-switch arm 0x550272; no behaviour recorded. |
| 28057 | — | S→C | decoded | — | Not documented. |
| 28185 | — | S→C | decoded | — | Not documented. |
| 28328 | — | S→C | decoded | — | Not documented. |
| 28383 | AiHomeData | S→C | decoded | Sent | AIHomeData: visited AI farm ('Greg's Farm') payload. Shares the 26017 body layout. Decoder 0xc1516, arm 0x54f836. |
| 28430 | — | S→C | decoded | — | Server->client single command. Encoder 0x4df4f6, decoder 0x11a37e. An accepted command carrier; not yet confirmed as the own-farm turn reply. |
| 28439 | — | S→C | decoded | — | Game-switch arm 0x5500c6; no behaviour recorded. |
| 28557 | — | S→C | decoded | — | Not documented. |
| 28630 | — | S→C | decoded | — | Not documented. |
| 28708 | — | S→C | decoded | — | Listener-tree case 0x7024. No scene transition. |
| 28882 | — | S→C | decoded | — | Not documented. |
| 28885 | — | S→C | decoded | — | Two ints plus an optional object. Game-switch arm 0x54f4c3. |
| 29258 | — | S→C | decoded | — | Not documented. |
| 29281 | — | S→C | decoded | — | Not documented. |
| 29318 | — | S→C | decoded | — | Not documented. |
| 29365 | — | S→C | decoded | — | Not documented. |
| 29501 | LoginOk | S→C | decoded | Sent | server accepts the login. Game-switch handler 0x5506a1 needs phase 2 and world+0x278 set, then sets phase 4. Decoder 0x2021fe. |
| 29506 | — | S→C | decoded | — | Listener-tree case 0x7342. |
| 29545 | — | S→C | decoded | — | Not documented. |
| 29637 | AccountSummary | S→C | decoded | Builder only | player/account summary, applied as a list on the listener path (arm 0x6074a6). Decoder 0x6e04c8. |
| 29748 | — | S→C | decoded | — | Nested 1304-byte object with an int and three fields. Game-switch arm 0x551a29. |
| 29950 | OtherHomeData | S→C | decoded | Builder only | visited home (another player's farm) payload, rendered from ai.json. Decoder 0x171336 has no reader calls, so this catalogue entry may be wrong. |
| 29997 | SessionReady | S→C | decoded | Builder only | tiny ack. Game-switch arm 0x5522c8 sets the game singleton's ready flag; queued at login state 5. Decoder 0x6efdd0 reads nothing. |
