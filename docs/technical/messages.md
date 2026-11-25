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

## Message class names found in the client

The client's log and error strings name these message classes (decoded from
the obfuscated string table). Most are not yet tied to a type number: the
strings sit in decode and handler code that the client reaches through
virtual calls, so a direct cross-reference does not work. Names already
matched to a type are used in the table below. The rest are candidates for
the unnamed rows (for example `ChangeHomeNameMessage` for 11489,
`RequestJoinNeighborhoodMessage` for 18132, `SearchNeighborhoodMessage` for
12392, `FollowMessage` for 16389, `BindAmazonAccountMessage` for 11282 and
`NewspaperDataRequestMessage` for 17462), but these pairings are not proven.

`AIHomeDataMessage`, `AITownDataMessage`, `AlreadyBoughtMessage`, `AlreadyFilledMessage`, `AmazonAccountAlreadyBoundMessage`, `AnalyticEventMessage`, `AskForAddableFriendsMessage`, `AvatarDataMessage`, `AvatarInfoListRequestMessage`, `AvatarInfoListResponseMessage`, `AvatarInfoResponseMessage`, `AvatarStreamMessage`, `BindAccountMessage`, `BindAmazonAccountMessage`, `BindFacebookAccountMessage`, `BindGamecenterAccountMessage`, `BindGoogleServiceAccountMessage`, `BuyFromStandResponseMessage`, `ChangeAccountSettingsMessage`, `ChangeHomeNameMessage`, `ChangeHomeNameResponseMessage`, `ChangeLayoutNamesMessage`, `ChangeNeighborhoodSettingsMessage`, `ChatToNeighborhoodStreamMessage`, `ClientCapabilitiesMessage`, `CreateNeighborhoodMessage`, `EditPetNameMessage`, `ErrorMessage`, `FacebookAccountAlreadyBoundMessage`, `FacebookAccountBoundMessage`, `FishingDataMessage`, `FollowMessage`, `GamecenterAccountAlreadyBoundMessage`, `GamecenterAccountBoundMessage`, `GoogleServiceAccountAlreadyBoundMessage`, `GoogleServiceAccountBoundMessage`, `HelpWithBoatOrderResponseMessage`, `LayoutNameListMessage`, `LeagueMemberListMessage`, `LogicDeviceLinkDoneMessage`, `LogicDeviceLinkResponseMessage`, `LoginFailedMessage`, `LoginMessage`, `LoginOkMessage`, `NeighborhoodListMessage`, `NewspaperDataRequestMessage`, `NotifyOnBoatHelpRequestedMessage`, `NotifyVisitorsOnBoatHelpReceivedMessage`, `OtherFishingHomeDataMessage`, `OtherHomeDataMessage`, `OtherTrainStationDataMessage`, `OwnHomeDataMessage`, `PickUpPassengerResponseMessage`, `PostNeighborhoodMessage`, `PostNeighborhoodNotificationStreamMessage`, `PreviousEventStatusResponseMessage`, `ProductListMessage`, `ReceiptVerificationMessage`, `RemoveNeighborhoodMessage`, `RequestJoinNeighborhoodMessage`, `SCIDAccountBoundMessage`, `SCIDLogoutAllDevicesResultMessage`, `SearchNeighborhoodMessage`, `SecureConnectionOKMessage`, `SendClientCapabilitiesMessage`, `ServerHelloMessage`, `TitanDisconnectedMessage`, `TrackingMessage`, `TrainStationDataMessage`, `UnlockAccountMessage`, `UnlockAccountOkMessage`, `VerifyAmazonBillingReceiptMessage`, `VerifyBillingReceiptMessage`, `VerifyGoogleBillingReceiptMessage`, `VerifySamsungBillingReceiptMessage`

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
  Rows marked "static scan" list the fields in the order the client's
  decoder reads them; `object(0x…)` is a nested structure read by the
  function at that address (not yet expanded), and `{…}` is a nested
  structure whose fields are shown. LogicLong is BE32 high then BE32 low.
  Names marked "(descriptive)" are ours, chosen from the sending code's
  strings and TIDs; the client's class names are stripped. Senders were
  found by locating each message's constructor (the code that stores its
  vtable) and the functions that call it and then send it through the
  client's send routine 0x34f4e6.

| Type | Name | Direction | Body | Server | Notes |
|---:|---|---|---|---|---|
| 10066 | RemoveFollower | C→S | decoded | — | Friend Book "Remove" on a follower (also the "Remove Follower?" confirm popup). Body: LogicLong follower account id (BE32 high, BE32 low). Client has a no-op inbound arm; no reply expected. |
| 10067 | — | C→S | decoded | — | Decoder 0x34fc12 reads no fields. Purpose not established (static scan). |
| 10099 | DecryptFailure | C→S | decoded | Handled | Client->server DecryptFailure: BE32 type of a server frame that failed decryption; the client then resets the connection. Also queued at login state 5. |
| 10101 | Login | C→S | empty decoder | Handled | client->server encrypted login envelope (public key plus sealed challenge/nonce/body). Empty decoder, send-only. |
| 10102 | — | C→S | decoded | — | Body (decoder 0x53ab0a): 2× BE32, 5× string. Purpose not established (static scan). |
| 10108 | — | C→S | decoded | — | Decoder 0x50ae4c reads no fields. Purpose not established (static scan). |
| 10141 | — | C→S | decoded | — | Body (decoder 0x1812c6): VInt, bool, 2× LogicLong. Probably a Derby task take/reserve: the ctor is reached from code that logs `[DerbyGUITasks::reserveTask]` and `TaskTakenStart`/`TaskTakenEnd` (Low). |
| 10166 | — | C→S | decoded | — | Body (decoder 0x5cd6be): bool, LogicLong. Purpose not established (static scan). |
| 10213 | Unknown10213 | C→S | decoded | — | seen in a visit-session trace just before PostLoginReady. Role not established. |
| 10219 | ClientInfo | C→S | decoded | — | phase-4 request sent in screen state 0x5d. Fields: int +0x34 (default -1), +0x38, +0x3c, +0x40. No reply observed. |
| 10256 | — | C→S | decoded | — | Body (decoder 0x3f14c8): 4× VInt, bool. Purpose not established (static scan). |
| 10268 | — | C→S | decoded | — | Decoder 0x6bec7a reads no fields. Purpose not established (static scan). |
| 10366 | PostLoginReady | C→S | empty decoder | Handled | fieldless client->server message after LoginOk (ctor 0xdc646, empty decoder). Also sent on return from a visit. |
| 10520 | — | C→S | decoded | — | Body (decoder 0x3bc4cc): object(0x6539a0), object(0x6bb7f0), object(0x6539a0), object(0x6bb7f0), VInt. Purpose not established (static scan). |
| 10543 | — | C→S | decoded | — | Decoder 0x688084 reads no fields. Purpose not established (static scan). |
| 10556 | — | C→S | decoded | — | Body (decoder 0x251ae6): LogicLong. Purpose not established (static scan). |
| 10705 | — | C→S | decoded | — | Decoder 0x3459ce reads no fields. Sent from code in the FacebookManager class (`facebookLoginFailed`, `TID_FACEBOOK_POPUP_TEXT_SIGN_IN_FAILED`); Facebook-related (Low). |
| 10860 | — | C→S | decoded | — | Body (decoder 0x255bc4): LogicLong. Purpose not established (static scan). |
| 10901 | — | C→S | empty decoder | — | Empty decoder (send-shaped); never seen on the wire. |
| 10987 | — | C→S | decoded | — | Body (decoder 0x36a98a): VInt, object(0x9e942), object(0x815e30), string, object(0x7278a6), VInt, object(0x9e942). Sender 0x435534 belongs to the farm layout-slot UI ("layout slot at index %d activated", "could not activate layout slot at index %d"); probably activating or naming a saved farm layout (Medium for the feature). Captured once with a 22-byte body: `05`, 20 zero bytes, `01`. |
| 10993 | — | C→S | decoded | — | Body (decoder 0x673bc0): 2× LogicLong. Purpose not established (static scan). |
| 11023 | — | C→S | decoded | — | Body (decoder 0x1a30b2): string. Purpose not established (static scan). |
| 11124 | — | C→S | empty decoder | — | Fieldless (no body). No feature-specific sender found statically; the ctor is reached only from the generic factory. Purpose not established. |
| 11282 | BindAmazonAccount (descriptive) | C→S | decoded | — | Body (decoder 0x469f00): 2× string, bool. Sent by `AmazonManager::sendBindAccountMessage` (caller 0x327eca), which refuses an empty id or token; the two strings are presumably the Amazon account id and token (Medium). |
| 11292 | EndClientTurn | C→S | decoded | Handled | client->server turn batch. Header: VInt tick counter at 30 Hz and a VInt state checksum, then commands. Observed roughly every 10 s. |
| 11316 | — | C→S | decoded | — | Decoder 0x52642e reads no fields. Sender 0x58ef9c is called from code with `TaskBoughtStart`/`TaskBoughtEnd` (likely Derby task purchase) (Low). |
| 11362 | CreateNeighborhood (likely) | C→S | decoded | — | Body (decoder 0x604078): object(0x2ded88), object(0x3530ce), 2× VInt, bool, 3× string, object(0x6539a0), object(0x6bb7f0). Sent from the same neighbourhood editor function as 12793 (0x6c57ec), whose window shows `TID_NEIGHBORHOOD_PROFANITIES_IN_NAME/TEXT/DESCRIPTION` and `TID_NEIGHBORHOOD_NAME_TOO_SHORT`. It has no LogicLong, unlike 12793, which fits creating a new neighbourhood; the client has a `CreateNeighborhoodMessage` class (Medium). |
| 11415 | — | C→S | decoded | — | Body (decoder 0x541c2c): LogicLong. Purpose not established (static scan). |
| 11489 | SetFarmName (descriptive) | C→S | decoded | — | Body (decoder 0x20b5fc): string. Sent from the farm-naming flow: caller 0xddd02 references `TID_DEFAULT_FARM_NAME`/`DefaultFarmName`, caller 0x1a1daa references `TID_NAME_FARM_WAITING`. The string is presumably the chosen farm name (Medium). |
| 11747 | — | C→S | decoded | — | Body (decoder 0x198272): LogicLong. Purpose not established (static scan). |
| 11833 | — | C→S | decoded | Handled | Mode-entry notification for mode 8 (ModeManager). Handler not yet added. |
| 11923 | — | C→S | decoded | — | Body (decoder 0xd3e06): VInt. Sent from the farm edit-mode slots window (0x2f60a8: `TID_EDIT_MODE_EDIT`, `..._COPY`, `..._ACTIVATE`); the VInt is probably the layout slot (Low). Captured once with body `01` (VInt 1), 5 s before a 10987. |
| 12041 | — | C→S | decoded | — | Body (decoder 0x6a3a28): LogicLong. Purpose not established (static scan). |
| 12215 | — | C→S | decoded | — | Body (decoder 0x629520): 4× VInt. Purpose not established (static scan). |
| 12281 | — | C→S | decoded | — | Body (decoder 0x2bdf5c): bool, 3× VInt. Sender 0x6460ac is called from code with `CompletedNeighbourhood*` labels; neighbourhood-related (Low). |
| 12328 | — | C→S | decoded | — | Body (decoder 0x707bea): 2× VInt, 2× LogicLong. Sender reached from Derby task UI (`nh_derby_content_processing_results`); Derby-related (Low). |
| 12392 | SearchNeighborhoods (descriptive) | C→S | decoded | — | Body (decoder 0x6a561e): string, 3× BE32, object(0x5f2da0), LogicLong. Sent from the neighbourhood search screen (caller 0x3eefb0: `TID_NHSEARCH_PROMPT`, `TID_NEIGHBORHOOD_SEARCH_ADVANCED_OPTIONS_INFO`); the string is presumably the search text and the ints the advanced filters (Medium). |
| 12453 | — | C→S | decoded | — | Body (decoder 0x198272): LogicLong. Purpose not established (static scan). |
| 12575 | — | C→S | decoded | — | Body (decoder 0x414c98): bool. One sender (0x6a3ec2) is called from a letters window (`Letters`, `Letter`, `ClaimAllButton`); probably a letter claim / claim-all (Low). |
| 12651 | — | C→S | decoded | — | Body (decoder 0x96696): LogicLong. Purpose not established (static scan). |
| 12688 | — | C→S | decoded | — | Body (decoder 0x3d9664): 2× LogicLong. Purpose not established (static scan). |
| 12771 | — | C→S | decoded | — | Body (decoder 0x5fc47c): 2× VInt, 2× LogicLong, bool. Purpose not established (static scan). |
| 12793 | ChangeNeighborhoodSettings (likely) | C→S | decoded | — | Body (decoder 0x43506e): object(0x2ded88), object(0x3530ce), 3× string, 2× VInt, bool, string, LogicLong. Sent from the neighbourhood editor (0x6c57ec, see 11362). The trailing LogicLong (presumably the neighbourhood id) and the extra string fit editing an existing neighbourhood; the client has a `ChangeNeighborhoodSettingsMessage` class that decodes a "preferred language" string (Medium). |
| 12857 | — | C→S | decoded | — | Body (decoder 0x318cf2): string. Purpose not established (static scan). |
| 12867 | — | C→S | decoded | — | Decoder 0x46ff1e reads no fields. Purpose not established (static scan). Captured four times with an empty body, twice together with 14497. |
| 12886 | — | C→S | decoded | — | Body (decoder 0xbd404): 2× VInt, 3× LogicLong. Sender 0x5cdfd2 is called from code referencing `OrderInfo` and `TID_NEIGHBORHOOD_TRADING_DONATE_FAILED_REQUEST_COMPLETED`; probably a neighbourhood donation (Low). |
| 13038 | — | C→S | decoded | — | Body (decoder 0x22782a): 2× LogicLong. Sender reached from Derby task UI (`nh_derby_content_processing_results`, `TaskTakenStart`); Derby-related (Low). |
| 13198 | Unknown13198 | C→S | decoded | Handled | Client->server 'Entered mode 3' (visit another farm). VInt selector, body 00 in every capture; sent by ModeManager after the mode switch. |
| 13280 | BindGamecenterAccount (likely) | C→S | decoded | — | Body (decoder 0x380c10): 3× string, object(0xc32e0), object(0x2fc970), object(0xc32e0), object(0x2fc970), object(0xc32e0), object(0x2fc970), bool. Sender 0x4eb724 is a method of the class whose vtable also holds `GamecenterManager::accountBoundSuccess` code (`TID_GAME_CENTER`). Three strings and a bool, like the other account-bind messages (Medium). |
| 13388 | — | C→S | decoded | — | Body (decoder 0x2ba552): VInt. Purpose not established (static scan). |
| 13447 | HomeRequest | C→S | decoded | Handled | phase-4 home/entity sync request, sent while world+0x230 != 2. Answered by 23549. |
| 13568 | — | C→S | decoded | — | Body (decoder 0x5e64d8): VInt, object(0x815e30), object(0x6539a0), object(0x6bb7f0), object(0x74e642). Sender 0x6d811c is called from the neighbourhood chat window (`ChatInput`, `TID_NEIGHBORHOOD_POST_PUBLIC_MESSAGE_BUTTON`); probably posting a chat message (Low). |
| 13765 | — | C→S | decoded | — | Body (decoder 0x1c42ae): VInt, LogicLong. One sender (0x429e84) is a method of the neighbourhood member-row UI (`TextRoleStatus`, `TextLeader`, `Level`); the VInt plus member id fit a member action such as a role change or kick (Low). |
| 13864 | LoadingStateReady | C→S | empty decoder | — | fieldless phase-4 message sent in screen state 0x5b; writes no state. |
| 13902 | JoinNeighborhood (descriptive) | C→S | decoded | — | Body (decoder 0x3e8650): LogicLong. Sent by `NeighborhoodGUINeighborhoodInfo::join()` (callers 0x52b57c, 0x59ebe6); the LogicLong is presumably the neighbourhood id (High for the feature, Medium for the field). |
| 13974 | — | C→S | decoded | — | Body (decoder 0x403b84): LogicLong. Purpose not established (static scan). |
| 14004 | — | C→S | decoded | — | Body (decoder 0x668efa): string, 3× object(0x259990). Purpose not established (static scan). |
| 14102 | Unknown14102 | S→C | decoded | — | Server->client turn ack (EndClientTurn reply), built by the inbound factory (ctor 0x128eae). Sits in the client band numerically, but is server-sent. |
| 14144 | — | C→S | decoded | — | Decoder 0x5edae2 reads no fields. Sent from GamecenterManager code (`TID_GAME_CENTER`, vtable 0xb12530); Game Center related (Low). |
| 14183 | — | C→S | decoded | — | Body (decoder 0x6bb0d0): VInt. Purpose not established (static scan). |
| 14275 | — | C→S | decoded | — | Body (decoder 0x234b4a): 2× LogicLong. Sender 0x38fef0 is called from neighbourhood trading UI (`TID_NEIGHBORHOOD_TRADING_REQUEST_TEXT`, `..._DONATE_INFO`); probably a neighbourhood item request/donation action (Low). |
| 14435 | — | C→S | decoded | — | Body (decoder 0x5013a4): VInt, object(0x3760ea), object(0x815e30), VInt, object(0x7acb36). Purpose not established (static scan). |
| 14497 | — | C→S | decoded | — | Body (decoder 0x66b70a): object(0x815ea0), LogicLong. Purpose not established (static scan). Captured five times; the body was always 8 zero bytes (LogicLong 0). Twice it was sent in the same instant as 12867. |
| 14543 | — | C→S | decoded | — | Body (decoder 0x32de46): LogicLong. Purpose not established (static scan). |
| 14581 | — | C→S | decoded | Handled | Mode-entry notification for mode 7 (ModeManager). Handler not yet added. |
| 14690 | — | C→S | decoded | — | Body (decoder 0x2df740): VInt, object(0x815e30), object(0x6539a0), object(0x6bb7f0), object(0x74e642), string. Two near-identical senders (0x1ab94a, 0x429784) allocate it (0x3c bytes, base ctor 0x67a280), then append list entries built by 0xff2a4 with arguments 1, 2, 3 (the three captured entries end in 1, 2, 3, so the list index or kind comes from there). 0x429784 is called from the neighbourhood search screen 0x3eefb0 (`TID_NHSEARCH_PROMPT`). The value `7fffffff` (INT_MAX) in each entry looks like a "no limit" filter value. So this is probably a filtered neighbourhood search or listing request (Low–Medium). Captured three times on 2026-11-24 (14:38:40–44) with the same 31-byte body `03` + 3× (`7fffffff`, `0000000n` for n=1..3) + string "EN": a VInt count 3, three 8-byte entries, then a language code. So the nested objects are a 3-entry list. The "EN" language fits the search screen's language filter, or a newspaper request (Low). |
| 14796 | — | C→S | decoded | — | Decoder 0x6bd08e reads no fields. Purpose not established (static scan). |
| 14821 | — | C→S | decoded | — | Body (decoder 0x187424): 2× LogicLong. Purpose not established (static scan). |
| 15174 | — | C→S | decoded | — | Body (decoder 0x4a2eb6): LogicLong, bool. Purpose not established (static scan). |
| 15203 | — | C→S | decoded | — | Body (decoder 0x371378): VInt. Purpose not established (static scan). |
| 15568 | — | C→S | decoded | — | Body (decoder 0x198272): LogicLong. Purpose not established (static scan). |
| 15840 | — | C→S | decoded | — | Body (decoder 0x3beb92): bool, LogicLong. Sender reached from the Derby league leaderboard UI (`TID_DERBY_LEAGUE_LEADERBOARD_EMPTY`, `nh_derby_leaderboard_first_week`); Derby leaderboard request (Low). |
| 16026 | — | C→S | decoded | — | Body (decoder 0x198272): LogicLong. Purpose not established (static scan). |
| 16063 | — | C→S | decoded | — | Body (decoder 0x126cd4): bool, LogicLong. Sender 0x3ff60e is reached from the leaderboards window (`TID_LEADERBOARD_HUD_TITLE`); probably a leaderboard request (Low). |
| 16368 | AvatarHomeIdentityReport | C→S | decoded | Handled | client->server, 8-byte body, a raw copy of avatar +0x00/+0x04. Sent once; own-account vs loaded-home identity unresolved. |
| 16389 | FollowPlayer (descriptive) | C→S | decoded | — | Body (decoder 0x231362): LogicLong. Sent after the "add friend" confirm popup (caller 0x42a2b4: `TID_CONFIRM_POPUP_ADD_FRIEND_*`, `TID_MAX_FOLLOWERS`); the LogicLong is presumably the account to follow (Medium). |
| 16449 | — | C→S | decoded | — | Body (decoder 0x1a3252): bool, LogicLong, string. Sent from `TypeConfirmationPopup::handleOkPressed` (0x6f05a2) and from code that logs "Ghosting current account." (0x12953c); an account-level confirm action (Low). |
| 16487 | LikeFarm (descriptive) | C→S | decoded | — | Body (decoder 0x23056a): LogicLong. Caller 0x3bfc50 references `TID_FARM_LIKED_SHORT` / `TID_FARM_ALREADY_LIKED_SHORT`; the LogicLong is presumably the liked farm's account id (Medium). |
| 16560 | — | C→S | decoded | — | Body (decoder 0x546482): object(0x4f1270). Purpose not established (static scan). |
| 16627 | — | C→S | decoded | — | Decoder 0xe5f70 reads no fields. Purpose not established (static scan). |
| 16707 | — | C→S | decoded | — | Body (decoder 0x1170e6): VInt. Purpose not established (static scan). |
| 16843 | — | C→S | decoded | — | Body (decoder 0x6783a8): 4× string, object(0xc32e0), object(0x42b5c0), object(0x2fc970), object(0x5f2da0), BE32, object(0x5f2da0), BE32. Purpose not established (static scan). |
| 16858 | Unfollow | C→S | decoded | — | Friend Book "Unfollow" / "Stop following?" confirm. Body: LogicLong id of the followed player. No reply expected. |
| 16986 | — | C→S | decoded | Handled | Mode-entry notification for mode 5 (ModeManager). Handler not yet added. |
| 16990 | — | C→S | decoded | — | Body (decoder 0x189b24): 2× string, LogicLong, string. Purpose not established (static scan). |
| 17096 | — | C→S | decoded | — | Decoder 0x1f9650 reads no fields. Purpose not established (static scan). |
| 17263 | FriendBookRefresh (descriptive) | C→S | empty decoder | — | Fieldless. Built by helper 0x11517c (ctor 0x37e2ae with a constant argument, then send 0x34f4e6). That helper is called from 0x62a5bc, a method of the Friend Book "Requests" tab class (its vtable 0xb30e38 also holds `FriendBookGUIRequests::friendListUpdated`), and from 0x5f8936, a method of a sibling Friend Book tab class (vtable 0xb417fc). So the client sends it when a Friend Book tab is opened or refreshed, presumably to ask for the friend / friend-request list (Medium). A second path (0xb0d70) sends it after three state checks (0x4eb92a, 0x4bac44, 0x4b71e0) whose meaning is not established. Captured 9 times with an empty body, sometimes three times within 10 s, which fits tab switching. The reply is not identified; FriendListReply (26656) is the likely candidate. |
| 17274 | OptionalIds | C→S | decoded | — | phase-4 request with two optional int64s. Its send path has no caller in this build; answered by 20062. |
| 17337 | — | C→S | decoded | — | Body (decoder 0x5010d0): object(0xc32e0), object(0x2fc970). Purpose not established (static scan). |
| 17462 | NewsInboxRequest (descriptive) | C→S | empty decoder | — | Fieldless. Sent once per session when the news inbox window is built (`InboxWindow`, `new_inbox`, `TID_SETTINGS_TAB_NEWS`): 0x50ffac checks a sent-once flag at +0x246 of a global, constructs the message (ctor 0x339ba2) and sends it with 0x34f4e6, then sets the flag. Presumably asks the server for news entries; the reply is not identified (Medium). |
| 17534 | — | C→S | empty decoder | Handled | Mode-entry notification for mode 4 (ModeManager). Handler not yet added. |
| 17556 | — | C→S | decoded | — | Body (decoder 0x1945cc): LogicLong. Purpose not established (static scan). |
| 17776 | — | C→S | empty decoder | — | Fieldless (no body). No feature-specific sender found statically; the ctor is reached only from the generic factory. Purpose not established. |
| 17887 | — | C→S | decoded | — | Body (decoder 0x18e768): VInt, object(0x815e30), object(0x6539a0), object(0x6bb7f0), object(0x74e642), bool, LogicLong. Purpose not established (static scan). Captured once (14:43:11) with body `03` + the same 3-entry list as 14690 + `01` (bool true) + 8 zero bytes (LogicLong 0). |
| 17914 | — | C→S | empty decoder | — | Fieldless (no body). No feature-specific sender found statically; the ctor is reached only from the generic factory. Purpose not established. |
| 17920 | Block | C→S | decoded | — | Friend Book "Block" button (may also be sent from other player popups). Body: LogicLong id of the blocked player. No reply expected. |
| 17970 | — | C→S | decoded | — | Body (decoder 0x2f783e): LogicLong, bool. Purpose not established (static scan). |
| 18132 | RequestToJoinNeighborhood (descriptive) | C→S | decoded | — | Body (decoder 0x461ee4): LogicLong, string. Caller 0x181cd6 builds the default text `TID_NEIGHBORHOOD_REQUEST_TO_JOIN_DEFAULT_MESSAGE` with `<NEIGHBORHOOD>`; the LogicLong is presumably the neighbourhood id and the string the request message (Medium). |
| 18167 | — | C→S | decoded | — | Body (decoder 0x69ff0c): 2× LogicLong, VInt. Purpose not established (static scan). |
| 18322 | — | C→S | empty decoder | — | Fieldless (no body). No feature-specific sender found statically; the ctor is reached only from the generic factory. Purpose not established. |
| 18485 | VisitPlayer | C→S | decoded | Handled | client->server visit request for mode 2. VInt selector, then BE64 target account id. |
| 18569 | — | C→S | decoded | — | Body (decoder 0x250d64): 2× object(0x4f1270). Purpose not established (static scan). |
| 18785 | — | C→S | decoded | — | Body (decoder 0x389efa): VInt, object(0x815e30), string, object(0x7278a6), VInt, object(0x815e30), string, object(0x7278a6). Purpose not established (static scan). |
| 19080 | — | C→S | decoded | — | Client->server push-notification registration (push token). Sent only at level >= 5 (MinLevelForPushNotification in game_config.csv). |
| 19126 | — | C→S | decoded | — | Body (decoder 0x715a8e): 2× string, bool. Purpose not established (static scan). |
| 19214 | — | C→S | empty decoder | Handled | Mode-entry notification for mode 6 (ModeManager). Handler not yet added. |
| 19249 | — | C→S | decoded | — | Body (decoder 0x1da822): {LogicLong}, bool. Purpose not established (static scan). |
| 19336 | PhaseFourPing | C→S | decoded | Handled | fieldless phase-4 ping, sent while world+0x234 != 2. Answered by 22151. |
| 19440 | — | C→S | decoded | — | Body (decoder 0xc9850): object(0xa27d6), object(0x23e7e0), object(0x815ea0), object(0x6ba958), object(0x200b0c). Sender 0x3f347e is reached from ad-view code (`AdBegin`, `AdEnd`, `AdNotServed`); probably an ad-watch report (Low). |
| 19445 | — | C→S | decoded | — | Body (decoder 0x38efa4): bool, LogicLong. Sender 0x301c4c is called from code showing `TID_DERBY_NOT_IN_LEAGUE_MESSAGE`; Derby-related (Low). |
| 19446 | — | C→S | decoded | — | Body (decoder 0x4891ea): bool, LogicLong. Purpose not established (static scan). |
| 19471 | — | C→S | decoded | — | Body (decoder 0x104e50): LogicLong. Purpose not established (static scan). |
| 19543 | — | C→S | decoded | — | Body (decoder 0x198272): LogicLong. Purpose not established (static scan). |
| 19597 | BindAccount (descriptive) | C→S | decoded | — | Body (decoder 0x595c78): 2× string, bool. Sent by a `sendBindAccountMessage` (caller 0x4aa284) with the same "id is empty"/"token is empty" checks and the same layout as 11282; an account-binding request for another platform (Medium; which platform not established). |
| 19611 | — | C→S | decoded | — | Decoder 0x5a91ae reads no fields. Purpose not established (static scan). |
| 19797 | — | C→S | decoded | — | Body (decoder 0xf9b1a): string, BE32. Purpose not established (static scan). |
| 19838 | FriendListRequest | C→S | decoded | Handled | empty client->server request. Sent once when the friend state is 'none'; the reply is 26656. |
| 20000 | — | Connection layer | decoded | — | Connection-layer message, not routed through the game switch: after each decoded message the connection notifies its listener at conn+0xac. |
| 20062 | OptionalIdsReply | S→C | decoded | Builder only | reply that sets world+0x238=2 and releases the optional 17274 path. Game-switch arm 0x54fb51. |
| 20069 | — | S→C | decoded | — | Body (decoder 0x6d5116): VInt. Handled in the game switch (case code at 0x5507d0). Purpose not established (static scan). |
| 20100 | ServerHello | S→C | decoded | Sent | login step 1 from the server. Verified against the session blob; the client then sends Login (10101). Decoder 0xf1a70. |
| 20103 | StatusNotice | S→C | decoded | Builder only | server status/error. Handler 0x550839 sets world phase 3 and switches on kind 1–18; kind 9 shows the body's reason text as a custom message (see server-ui.md). This is the first of two factory rows for 20103; this one has a body decoder. |
| 20103 | StatusNotice | S→C | empty decoder | Builder only | Second factory row for 20103 (empty decoder). Which row the client uses is not established. |
| 20108 | — | S→C | decoded | — | Decoder 0x141c66 reads no fields. Its game-switch case jumps straight to the switch's shared exit (0x54e94c), so the client takes no action on it (Medium). |
| 20162 | — | S→C | decoded | — | Body (decoder 0x13a6e6): 2× VInt. Purpose not established (static scan). |
| 20214 | — | S→C | decoded | — | Body (decoder 0x4887e2): object(0x5f2da0), object(0xc32e0), object(0x2fc970), object(0x42b5c0). Purpose not established (static scan). |
| 20227 | — | S→C | decoded | — | Body (decoder 0x5f6296): bool, VInt. Purpose not established (static scan). |
| 20247 | — | S→C | decoded | — | Body (decoder 0x124304): LogicLong, VInt. Handled in the game switch (case code at 0x551e15). Purpose not established (static scan). |
| 20316 | — | S→C | decoded | — | Body (decoder 0x425e82): 2× LogicLong, bool. Purpose not established (static scan). |
| 20360 | — | S→C | decoded | — | Body (decoder 0x23bc92): VInt. Purpose not established (static scan). |
| 20415 | — | S→C | decoded | — | Body (decoder 0x4838a2): BE32. Handled in the game switch (case code at 0x5500fe). Purpose not established (static scan). |
| 20599 | — | S→C | decoded | — | Body (decoder 0x5e508e): LogicLong. Handled in the game switch (case code at 0x551049). Purpose not established (static scan). |
| 20890 | — | S→C | decoded | — | Body (decoder 0x5854da): 2× VInt, object(0x815e30), object(0x234f12), object(0x746f6c), object(0xce10c). Purpose not established (static scan). |
| 20982 | — | S→C | decoded | — | Body (decoder 0x50d234): LogicLong. Purpose not established (static scan). |
| 21086 | — | S→C | decoded | — | Game-switch arm 0x54f54e; no behaviour recorded. |
| 21309 | — | S→C | decoded | — | Game-switch arm 0x54ff26; no behaviour recorded. |
| 21395 | — | S→C | decoded | — | Body (decoder 0x579e0c): VInt, object(0x2db9d8), object(0x815e30), object(0x57d666), object(0xea3e2), object(0x72a738). Handled in the game switch (case code at 0x550b95). The case code calls `SoilSocialManager::setFriendMetas` (0x6e75bc) and `NeighborhoodManager::setFriendMetas` (0x6d451a), so it delivers friend metadata to the social managers (Medium). |
| 21404 | — | S→C | decoded | — | Body (decoder 0x5968aa): VInt. Handled in the game switch (case code at 0x550c45). Purpose not established (static scan). |
| 21444 | — | S→C | decoded | — | Game-switch arm 0x54f132; no behaviour recorded. |
| 21608 | — | S→C | decoded | — | Game-switch arm 0x5503c6: kind 0/1/2 branches that post events. Reaches no scene transition. |
| 21738 | — | S→C | empty decoder | — | Fieldless (no body). No feature-specific sender found statically; the ctor is reached only from the generic factory. Purpose not established. |
| 21817 | — | S→C | decoded | — | Generic scene-setup arm (0x54e9be); no behaviour recorded. |
| 21823 | — | S→C | decoded | — | Body (decoder 0x5983d6): VInt, object(0x6424b4), object(0x815e30), object(0x57d666), object(0xea3e2), object(0x72a738). Handled in the game switch (case code at 0x551666). Purpose not established (static scan). |
| 21876 | — | S→C | decoded | — | Game-switch arm 0x5516c3: posts event tagged 0x5574. Decoder holds string/int scalars. |
| 21895 | — | S→C | decoded | — | Body (decoder 0x5d98ba): VInt, object(0x815e30), object(0x21cef8), object(0x759ce6), object(0x1015d4). Purpose not established (static scan). |
| 21901 | — | S→C | decoded | — | Generic scene-setup arm (0x54f8ed); no behaviour recorded. |
| 22011 | — | S→C | decoded | — | Game-switch arm 0x551ae9 sets world+0x224=2. Decoder: bool plus list of 36-byte entities. |
| 22151 | PingReply | S→C | decoded | Sent | reply to 19336 that sets world+0x234=2. Game-switch arm 0x54fc84. |
| 22220 | — | S→C | decoded | — | Body (decoder 0x11be5c): VInt. Handled in the game switch (case code at 0x5509a8). Purpose not established (static scan). |
| 22242 | — | S→C | decoded | — | Body (decoder 0x20134c): 2× string. Purpose not established (static scan). |
| 22326 | — | S→C | decoded | — | Body (decoder 0x5e2d7a): VInt, object(0x815e30), string, object(0x7278a6). Handled in the game switch (case code at 0x5509df). The case code reaches PurchaseManager JNI calls (`addSku`, `updateDetails`), so it is probably the in-app product/SKU list (Low). |
| 22362 | — | S→C | empty decoder | — | Fieldless (no body). No feature-specific sender found statically; the ctor is reached only from the generic factory. Purpose not established. |
| 22533 | — | S→C | decoded | — | Body (decoder 0x4148a8): VInt, object(0x815e30), object(0x1b1294), object(0x72a10e), object(0x5b94da), VInt. Purpose not established (static scan). |
| 22542 | — | S→C | decoded | — | Game-switch arm 0x54ef00; no behaviour recorded. |
| 22666 | — | S→C | decoded | — | Small entity/event update. Game-switch arm 0x551e97; its sibling arm sets world+0x228=2. Factory object 0x44 bytes, decoder about 988 bytes. |
| 22756 | — | S→C | decoded | — | Game-switch arm 0x55021e; no behaviour recorded. |
| 22852 | — | S→C | decoded | — | Body (decoder 0x5c7530): object(0x3683f2), object(0x42b5c0). Handled in the game switch (case code at 0x55119b). Purpose not established (static scan). |
| 22879 | SCIDAccountBoundMessage | S→C | decoded | — | Body (decoder 0x19d0a6): BE32, object(0x4f1270). Handled in the game switch (case code at 0x551377). Name from the client's own log string in the case code (0x551377): "SCIDAccountBoundMessage: invalid result code". Supercell ID account-binding result; the case switches on a result code (High for the name). |
| 23066 | — | S→C | decoded | — | Profile-like: two optional strings plus six ints. Game-switch arm 0x54f7c4. |
| 23173 | — | S→C | decoded | — | Tip/notification dispatch: decodes a list of ints and picks one at random. Game- switch arm 0x551d5b. |
| 23460 | — | S→C | decoded | — | Game-switch arm 0x55006c; no behaviour recorded. |
| 23549 | HomeSync | S→C | decoded | Sent | reply to 13447 that sets world+0x230=2. Body: three count-prefixed lists plus an int. Game-switch arm 0x550d76. |
| 23591 | — | S→C | decoded | — | Generic scene-setup arm (0x54f3b2). String/int scalars. |
| 23594 | — | S→C | decoded | — | Body (decoder 0x1a6db0): VInt, object(0x815e30), object(0x51163a), object(0x755052), ?, VInt, object(0x815e30), object(0x51163a), object(0x755052), ?, 2× VInt. Purpose not established (static scan). |
| 23597 | — | S→C | decoded | — | No-op: recognised by the game switch and ignored. |
| 23631 | — | S→C | decoded | — | Generic scene-setup arm (0x54ec35); no behaviour recorded. |
| 23698 | — | S→C | decoded | — | Body (decoder 0x4b43b2): VInt, object(0x5bdcbc), ?. Purpose not established (static scan). |
| 23809 | — | S→C | decoded | — | String/int scalars. Among the server-band types examined during the loading-screen investigation. |
| 23899 | — | S→C | decoded | — | Body (decoder 0x3786f4): 3× string, LogicLong. Handled in the game switch (case code at 0x551919). Purpose not established (static scan). |
| 23917 | — | S→C | decoded | — | Game-switch arm 0x54faa1; no behaviour recorded. |
| 23933 | — | S→C | decoded | — | Body (decoder 0x466cd0): VInt, object(0x815e30), object(0x21cef8), object(0x759ce6), object(0x1015d4), string. Purpose not established (static scan). |
| 24285 | — | S→C | decoded | — | String/int scalars. |
| 24315 | — | S→C | decoded | — | Decoder 0x200ab6 reads no fields. Purpose not established (static scan). |
| 24728 | — | S→C | decoded | — | Body (decoder 0x5b90ca): VInt, object(0x35b9fe), ?. Purpose not established (static scan). |
| 24855 | — | S→C | decoded | — | Body (decoder 0x49b44c): 2× VInt. Its game-switch case jumps straight to the switch's shared exit (0x54e94c), so the client takes no action on it in this switch (Medium). |
| 24878 | — | S→C | decoded | — | Decoder 0x4fecd4 reads no fields. Its game-switch case jumps straight to the switch's shared exit (0x54e94c), so the client takes no action on it in this switch (Medium). |
| 24979 | — | S→C | decoded | — | Body (decoder 0x5cee22): VInt, LogicLong. Handled in the game switch (case code at 0x5508e0). Purpose not established (static scan). |
| 25045 | — | S→C | decoded | — | Body (decoder 0x9e574): {LogicLong}, VInt. Purpose not established (static scan). |
| 25067 | — | S→C | decoded | — | Handled outside the game switch by a listener/broadcast dispatcher (cmp 0x61eb at 0x534209). Factory object 0x38 bytes: one bit plus an optional 36-byte object. |
| 25282 | — | S→C | decoded | — | Body: a single LogicLong (decoder 0x63756c). Routed with the news stream; carries no text. |
| 25362 | — | S→C | decoded | — | Body (decoder 0x66e74a): LogicLong. Handled in the game switch (case code at 0x5508fd). Purpose not established (static scan). |
| 25491 | EntityBatch | S→C | decoded | Builder only | count-capped list of 148-byte entities plus ints; the best farm-grid shape. Handled on the listener path (0x607348), not the game switch. |
| 25492 | — | S→C | decoded | — | Int plus count-capped list of 48-byte entities. Not dispatched at all. |
| 25733 | — | S→C | decoded | — | Body (decoder 0x6719b2): VInt, bool, VInt, object(0x42b710), ?. Purpose not established (static scan). |
| 25886 | — | S→C | decoded | — | Game-switch arm 0x550178; no behaviour recorded. |
| 25892 | OtherDeviceConnected | S→C | decoded | Builder only | server disconnect. Body: BE32 reason only. Reason 1 = another device is connecting (live-tested). Other reasons log on the client but are not acted upon. |
| 25985 | MailList | S→C | decoded | — | Mailbox entries: VInt count, then MailEntry objects (decoder 0x1a4e7c) with custom subject and custom body strings. Sent unprompted after login: no visible effect (live-tested). Client handler not found. See server-ui.md. |
| 26008 | — | S→C | decoded | — | Body (decoder 0x4ecb50): VInt, object(0x815e30), object(0x169b08), object(0x74864c), object(0x41d182). Handled in the game switch (case code at 0x55114a). Purpose not established (static scan). |
| 26017 | OwnHomeData | S→C | decoded | Sent | own farm/home payload. Game-switch arm 0x54f5b0. Certain all-zero body lengths crash the client in this arm. |
| 26204 | — | S→C | decoded | — | Body (decoder 0x570822): VInt. Purpose not established (static scan). |
| 26279 | AvatarStreamMessage | S→C | decoded | — | News stream: count, then entries of type 3–7 (all neighbourhood entries; most carry raw message text and a neighbourhood name). See server-ui.md. |
| 26306 | ManagerEvent | S→C | decoded | — | body is a single string key (not displayed text), routed to global manager 0x1990c2. Best farm-state candidate so far. |
| 26339 | — | S→C | decoded | — | Body (decoder 0xd0d40): LogicLong. Its game-switch case jumps straight to the switch's shared exit (0x54e94c), so the client takes no action on it in this switch (Medium). |
| 26359 | — | S→C | decoded | — | Body (decoder 0x58e390): object(0x6db62c), object(0x42b5c0). Handled in the game switch (case code at 0x550ca6). Purpose not established (static scan). |
| 26513 | — | S→C | decoded | — | String/int scalars. |
| 26650 | — | S→C | decoded | — | TrainStationDataMessage: train station data. Decoder 0x545268. Game-switch arm 0x54f19e. |
| 26656 | FriendListReply | S→C | decoded | Sent | friend list of FriendEntry records. An empty list is body 00. Game- switch arm 0x5504bf. |
| 26697 | — | S→C | decoded | — | Body (decoder 0x9b7f8): bool. Purpose not established (static scan). |
| 26742 | — | S→C | decoded | — | Generic scene-setup arm (0x54eb09); no behaviour recorded. |
| 26837 | — | S→C | decoded | — | Body (decoder 0x2c9f50): VInt, 2× LogicLong. Handled in the game switch (case code at 0x5516fb). Purpose not established (static scan). |
| 26950 | — | S→C | decoded | — | Game-switch arm 0x55180f. Its call chain reaches a UI/notification helper, not a scene swap. |
| 26968 | — | S→C | decoded | — | Body (decoder 0x5ca2b4): object(0x61ecca), object(0x56d0e6). Purpose not established (static scan). |
| 26973 | — | S→C | decoded | — | Body (decoder 0x1b349a): VInt. Purpose not established (static scan). |
| 27066 | — | S→C | decoded | — | Body (decoder 0x4d7954): VInt, string, bool, LogicLong, string, object(0x20cb3c), object(0x446a4c). Purpose not established (static scan). |
| 27105 | — | S→C | decoded | — | Varint plus a named field with default 900000 ms. Minimal arm: one call (0x67aae6). |
| 27171 | — | S→C | decoded | — | Body (decoder 0x3cdcfc): VInt, object(0x815e30), VInt, object(0x42b710), object(0x752c5a), ?, object(0x3b5820), object(0x19e2f0), object(0x23e7e0). Purpose not established (static scan). |
| 27458 | SCIDLogoutAllDevicesResultMessage | S→C | decoded | — | Body (decoder 0x5a0b56): VInt. Handled in the game switch (case code at 0x551b33). Name from the log string in the case code (0x551b33): "SCIDLogoutAllDevicesResultMessage: invalid result code". Result of a Supercell ID "log out all devices" request (High for the name). |
| 27466 | — | S→C | decoded | — | Listener-tree case 0x6b4a. No scene transition. |
| 27491 | — | S→C | decoded | — | Largest in-switch state update chain (arm 0x54fdc6). Body: two ints plus two strings. |
| 27508 | — | S→C | decoded | — | Body (decoder 0x6e9cd4): 2× object(0x815ea0), bool, LogicLong, bool, LogicLong, 2× VInt. Handled in the game switch (case code at 0x550a61). Purpose not established (static scan). |
| 27554 | — | S→C | decoded | — | Body (decoder 0x1a4258): VInt, object(0x815e30), VInt, object(0x42b710), object(0x752c5a), ?, object(0x3b5820), object(0x19e2f0), object(0x23e7e0). Handled in the game switch (case code at 0x550b21). Purpose not established (static scan). |
| 27626 | — | S→C | decoded | — | Game-switch arm 0x54f00d; no behaviour recorded. |
| 27996 | — | S→C | decoded | — | Listener-tree case 0x6d5c. No scene transition. |
| 28004 | HelpWithBoatOrderResponseMessage | S→C | decoded | — | Body (decoder 0x6d10d8): 2× object(0x815ea0), bool, LogicLong, bool, LogicLong, 11× VInt. Handled in the game switch (case code at 0x552082). Name from the log string in the case code (0x552082): "HelpWithBoatOrderResponseMessage RESPONSE_CODE_SERVER_ERROR". Reply to helping fill another player's boat crate (High for the name). |
| 28054 | — | S→C | decoded | — | Game-switch arm 0x550272; no behaviour recorded. |
| 28057 | — | S→C | decoded | — | Body (decoder 0x69bc9a): bool, object(0x61ecca), object(0x56d0e6), 2× VInt. Purpose not established (static scan). |
| 28185 | — | S→C | decoded | — | Body (decoder 0x3be34e): object(0x6db62c), object(0x42b5c0). Handled in the game switch (case code at 0x5513f7). Purpose not established (static scan). |
| 28328 | — | S→C | decoded | — | Body (decoder 0x32fa5a): VInt, object(0x66ac80), object(0x815e30), object(0x627976), object(0x7d7fd0), object(0x1a4e7c). Handled in the game switch (case code at 0x551497). The case code references `TID_GREG`; related to Greg's farm or Greg as a sender (Low). |
| 28383 | AiHomeData | S→C | decoded | Sent | AIHomeData: visited AI farm ('Greg's Farm') payload. Shares the 26017 body layout. Decoder 0xc1516, arm 0x54f836. |
| 28430 | — | S→C | decoded | — | Server->client single command. Encoder 0x4df4f6, decoder 0x11a37e. An accepted command carrier; not yet confirmed as the own-farm turn reply. |
| 28439 | — | S→C | decoded | — | Game-switch arm 0x5500c6; no behaviour recorded. |
| 28557 | — | S→C | decoded | — | Body (decoder 0x360adc): object(0x3683f2), object(0x42b5c0). Handled in the game switch (case code at 0x550eb1). Purpose not established (static scan). |
| 28630 | — | S→C | decoded | — | Decoder 0x6ba2fa reads no fields. Purpose not established (static scan). |
| 28708 | — | S→C | decoded | — | Listener-tree case 0x7024. No scene transition. |
| 28882 | — | S→C | decoded | — | Body (decoder 0x5e30da): VInt, string, bool, LogicLong, string, bool, object(0x20cb3c), object(0x446a4c). Handled in the game switch (case code at 0x550f3d). Purpose not established (static scan). |
| 28885 | — | S→C | decoded | — | Two ints plus an optional object. Game-switch arm 0x54f4c3. |
| 29258 | — | S→C | decoded | — | Body (decoder 0x5cd81e): {LogicLong}, VInt. Purpose not established (static scan). |
| 29281 | — | S→C | decoded | — | Body (decoder 0x18dfb8): object(0x234f12), object(0xce10c). Purpose not established (static scan). |
| 29318 | — | S→C | decoded | — | Body (decoder 0x455f10): VInt, object(0x815e30), object(0x6f73aa), object(0x7c1990), object(0x64dff4). Purpose not established (static scan). |
| 29365 | — | S→C | decoded | — | Body (decoder 0x11b0a2): object(0x511154), object(0x2c888e). Its game-switch case jumps straight to the switch's shared exit (0x54e94c), so the client takes no action on it in this switch (Medium). |
| 29501 | LoginOk | S→C | decoded | Sent | server accepts the login. Game-switch handler 0x5506a1 needs phase 2 and world+0x278 set, then sets phase 4. Decoder 0x2021fe. |
| 29506 | — | S→C | decoded | — | Listener-tree case 0x7342. |
| 29545 | — | S→C | decoded | — | Body (decoder 0x31c674): LogicLong, VInt. Purpose not established (static scan). |
| 29637 | AccountSummary | S→C | decoded | Builder only | player/account summary, applied as a list on the listener path (arm 0x6074a6). Decoder 0x6e04c8. |
| 29748 | — | S→C | decoded | — | Nested 1304-byte object with an int and three fields. Game-switch arm 0x551a29. |
| 29950 | OtherHomeData | S→C | decoded | Builder only | visited home (another player's farm) payload, rendered from ai.json. Decoder 0x171336 has no reader calls, so this catalogue entry may be wrong. |
| 29997 | SessionReady | S→C | decoded | Builder only | tiny ack. Game-switch arm 0x5522c8 sets the game singleton's ready flag; queued at login state 5. Decoder 0x6efdd0 reads nothing. |
