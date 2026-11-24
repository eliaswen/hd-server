//! Message table from a static byte-sweep of libg.so 1.43.150 (x86).
//! Columns: type, vtable (ELF VA), body decoder (ELF VA).
//!
//! Each message with a known role is preceded by a comment describing what
//! static analysis of libg.so established about it. Messages without a
//! comment are not yet documented.

pub(super) const RAW: &[(u16, u32, u32)] = &[
    (10066, 0xb34fa4, 0x4a158e),
    (10067, 0xb39f3c, 0x34fc12),
    // 10099: Client->server DecryptFailure: BE32 type of a server frame that failed decryption;
    // the client then resets the connection. Also queued at login state 5 (catalogue.rs).
    (10099, 0xb24e18, 0x146940),
    // 10101: Login: client->server encrypted login envelope (public key plus sealed
    // challenge/nonce/body). Empty decoder, send-only (messages.rs ids::LOGIN).
    (10101, 0xb27d04, 0x615010),
    (10102, 0xb258cc, 0x53ab0a),
    (10108, 0xb34908, 0x50ae4c),
    (10141, 0xb15bc0, 0x1812c6),
    (10166, 0xb1fcac, 0x5cd6be),
    // 10213: Unknown10213: seen in a visit-session trace just before PostLoginReady. Role not
    // established.
    (10213, 0xb1baa8, 0x404a68),
    // 10219: ClientInfo: phase-4 request sent in screen state 0x5d. Fields: int +0x34 (default
    // -1), +0x38, +0x3c, +0x40. No reply observed.
    (10219, 0xb441c4, 0x448990),
    (10256, 0xb49b68, 0x3f14c8),
    (10268, 0xb3e99c, 0x6bec7a),
    // 10366: PostLoginReady: fieldless client->server message after LoginOk (ctor 0xdc646, empty
    // decoder). Also sent on return from a visit.
    (10366, 0xb34dbc, 0x615010),
    (10520, 0xb314fc, 0x3bc4cc),
    (10543, 0xb43bb8, 0x688084),
    (10556, 0xb4934c, 0x251ae6),
    (10705, 0xb4ab6c, 0x3459ce),
    (10860, 0xb465e4, 0x255bc4),
    // 10901: Empty decoder (send-shaped); never seen on the wire.
    (10901, 0xb36b8c, 0x615010),
    (10987, 0xb27c3c, 0x36a98a),
    (10993, 0xb24244, 0x673bc0),
    (11023, 0xb333e8, 0x1a30b2),
    (11124, 0xb31e00, 0x615010),
    (11282, 0xb100c4, 0x469f00),
    // 11292: EndClientTurn: client->server turn batch. Header: VInt tick counter at 30 Hz and a
    // VInt state checksum, then commands. Observed roughly every 10 s.
    (11292, 0xb2c448, 0x1fd7b8),
    (11316, 0xb10a4c, 0x52642e),
    (11362, 0xb19d88, 0x604078),
    (11415, 0xb1e4e0, 0x541c2c),
    (11489, 0xb48fec, 0x20b5fc),
    (11747, 0xb38d7c, 0x198272),
    // 11833: Mode-entry notification for mode 8 (ModeManager). Handler not yet added.
    (11833, 0xb31ddc, 0x45a204),
    (11923, 0xb4a030, 0xd3e06),
    (12041, 0xb485b8, 0x6a3a28),
    (12215, 0xb14994, 0x629520),
    (12281, 0xb1a8d8, 0x2bdf5c),
    (12328, 0xb47480, 0x707bea),
    (12392, 0xb28af0, 0x6a561e),
    (12453, 0xb1df18, 0x198272),
    (12575, 0xb4a1c8, 0x414c98),
    (12651, 0xb35474, 0x96696),
    (12688, 0xb18494, 0x3d9664),
    (12771, 0xb315b4, 0x5fc47c),
    (12793, 0xb30ce8, 0x43506e),
    (12857, 0xb174e4, 0x318cf2),
    (12867, 0xb4a2f8, 0x46ff1e),
    (12886, 0xb11e60, 0xbd404),
    (13038, 0xb2edc4, 0x22782a),
    // 13198: Client->server 'Entered mode 3' (visit another farm). VInt selector, body 00 in every
    // capture; sent by ModeManager after the mode switch.
    (13198, 0xb3be88, 0x30ba20),
    (13280, 0xb42df4, 0x380c10),
    (13388, 0xb19ee0, 0x2ba552),
    // 13447: HomeRequest: phase-4 home/entity sync request, sent while world+0x230 != 2. Answered
    // by 23549.
    (13447, 0xb229f0, 0x68255c),
    (13568, 0xb3f1c8, 0x5e64d8),
    (13765, 0xb222a0, 0x1c42ae),
    // 13864: LoadingStateReady: fieldless phase-4 message sent in screen state 0x5b; writes no
    // state.
    (13864, 0xb12858, 0x615010),
    (13902, 0xb19e2c, 0x3e8650),
    (13974, 0xb403c8, 0x403b84),
    (14004, 0xb3e540, 0x668efa),
    // 14102: Server->client turn ack (EndClientTurn reply), built by the inbound factory (ctor
    // 0x128eae). Sits in the client band numerically, but is server-sent.
    (14102, 0xb47784, 0x1161f4),
    (14144, 0xb28c58, 0x5edae2),
    (14183, 0xb3d960, 0x6bb0d0),
    (14275, 0xb408d4, 0x234b4a),
    (14435, 0xb27240, 0x5013a4),
    (14497, 0xb24268, 0x66b70a),
    (14543, 0xb421cc, 0x32de46),
    // 14581: Mode-entry notification for mode 7 (ModeManager). Handler not yet added.
    (14581, 0xb3c3d8, 0x2dfc76),
    (14690, 0xb2ecbc, 0x2df740),
    (14796, 0xb239cc, 0x6bd08e),
    (14821, 0xb1fc88, 0x187424),
    (15174, 0xb29830, 0x4a2eb6),
    (15203, 0xb2cdb4, 0x371378),
    (15568, 0xb46e74, 0x198272),
    (15840, 0xb3ae30, 0x3beb92),
    (16026, 0xb13b7c, 0x198272),
    (16063, 0xb1daf0, 0x126cd4),
    // 16368: AvatarHomeIdentityReport: client->server, 8-byte body, a raw copy of avatar
    // +0x00/+0x04. Sent once; own-account vs loaded-home identity unresolved.
    (16368, 0xb2e718, 0x198272),
    (16389, 0xb2b42c, 0x231362),
    (16449, 0xb1792c, 0x1a3252),
    (16487, 0xb28acc, 0x23056a),
    (16560, 0xb39f84, 0x546482),
    (16627, 0xb17908, 0xe5f70),
    (16707, 0xb485dc, 0x1170e6),
    (16843, 0xb1832c, 0x6783a8),
    (16858, 0xb0ff8c, 0x2e4cec),
    // 16986: Mode-entry notification for mode 5 (ModeManager). Handler not yet added.
    (16986, 0xb1b834, 0x45f57c),
    (16990, 0xb35d0c, 0x189b24),
    (17096, 0xb46ea8, 0x1f9650),
    (17263, 0xb4b400, 0x615010),
    // 17274: OptionalIds: phase-4 request with two optional int64s. Its send path has no caller in
    // this build; answered by 20062.
    (17274, 0xb43b94, 0x4146a8),
    (17337, 0xb1e654, 0x5010d0),
    (17462, 0xb3a158, 0x615010),
    // 17534: Mode-entry notification for mode 4 (ModeManager). Handler not yet added.
    (17534, 0xb3c584, 0x615010),
    (17556, 0xb3b510, 0x1945cc),
    (17776, 0xb40f98, 0x615010),
    (17887, 0xb1a0f0, 0x18e768),
    (17914, 0xb3dce4, 0x615010),
    (17920, 0xb4af64, 0x198272),
    (17970, 0xb273e8, 0x2f783e),
    (18132, 0xb1316c, 0x461ee4),
    (18167, 0xb28144, 0x69ff0c),
    (18322, 0xb47c10, 0x615010),
    // 18485: VisitPlayer: client->server visit request for mode 2. VInt selector, then BE64 target
    // account id (messages.rs).
    (18485, 0xb33a04, 0x339b3a),
    (18569, 0xb3a500, 0x250d64),
    (18785, 0xb314ac, 0x389efa),
    // 19080: Client->server push-notification registration (push token). Sent only at level >= 5
    // (MinLevelForPushNotification in game_config.csv).
    (19080, 0xb20e94, 0x507b86),
    (19126, 0xb3c388, 0x715a8e),
    // 19214: Mode-entry notification for mode 6 (ModeManager). Handler not yet added.
    (19214, 0xb3fe3c, 0x615010),
    (19249, 0xb15264, 0x1da822),
    // 19336: PhaseFourPing: fieldless phase-4 ping, sent while world+0x234 != 2. Answered by
    // 22151.
    (19336, 0xb10a28, 0x4499bc),
    (19440, 0xb348bc, 0xc9850),
    (19445, 0xb2d6d4, 0x38efa4),
    (19446, 0xb23f84, 0x4891ea),
    (19471, 0xb43a70, 0x104e50),
    (19543, 0xb3a1f0, 0x198272),
    (19597, 0xb14368, 0x595c78),
    (19611, 0xb291a0, 0x5a91ae),
    (19797, 0xb2b184, 0xf9b1a),
    // 19838: FriendListRequest: empty client->server request. Sent once when the friend state is
    // 'none'; the reply is 26656.
    (19838, 0xb2210c, 0x5eff1c),
    // 20000: Connection-layer message, not routed through the game switch: after each decoded
    // message the connection notifies its listener at conn+0xac.
    (20000, 0xb2c720, 0x6f1580),
    // 20062: OptionalIdsReply: reply that sets world+0x238=2 and releases the optional 17274 path.
    // Game-switch arm 0x54fb51.
    (20062, 0xb3d5fc, 0x5eb1ca),
    (20069, 0xb0ff68, 0x6d5116),
    // 20100: ServerHello: login step 1 from the server. Verified against the session blob; the
    // client then sends Login (10101). Decoder 0xf1a70.
    (20100, 0xb41a8c, 0xf1a70),
    // 20103: StatusNotice: server status/error. Handler 0x550839 sets world phase 3 and picks one
    // of 18 messages by kind (catalogue.rs). This is the first of two factory rows for 20103; this
    // one has a body decoder.
    (20103, 0xb0f6ac, 0x213844),
    // 20103: Second factory row for 20103 (empty decoder). Which row the client uses is not
    // established.
    (20103, 0xb23704, 0x615010),
    (20108, 0xb22a14, 0x141c66),
    (20162, 0xb36b68, 0x13a6e6),
    (20214, 0xb31f44, 0x4887e2),
    (20227, 0xb27c18, 0x5f6296),
    (20247, 0xb10f98, 0x124304),
    (20316, 0xb2c2b4, 0x425e82),
    (20360, 0xb32c70, 0x23bc92),
    (20415, 0xb28120, 0x4838a2),
    (20599, 0xb477e4, 0x5e508e),
    (20890, 0xb4014c, 0x5854da),
    (20982, 0xb116b8, 0x50d234),
    // 21086: Game-switch arm 0x54f54e; no behaviour recorded.
    (21086, 0xb34220, 0x246e68),
    // 21309: Game-switch arm 0x54ff26; no behaviour recorded.
    (21309, 0xb30a28, 0x60d166),
    (21395, 0xb19d24, 0x579e0c),
    (21404, 0xb13cb8, 0x5968aa),
    // 21444: Game-switch arm 0x54f132; no behaviour recorded.
    (21444, 0xb2aae4, 0x1e763c),
    // 21608: Game-switch arm 0x5503c6: kind 0/1/2 branches that post events. Reaches no scene
    // transition.
    (21608, 0xb45b7c, 0x1d7f26),
    (21738, 0xb29ce0, 0x615010),
    // 21817: Generic scene-setup arm (0x54e9be); no behaviour recorded.
    (21817, 0xb18134, 0x3a15a8),
    (21823, 0xb1a114, 0x5983d6),
    // 21876: Game-switch arm 0x5516c3: posts event tagged 0x5574. Decoder holds string/int
    // scalars.
    (21876, 0xb3839c, 0x211baa),
    (21895, 0xb2182c, 0x5d98ba),
    // 21901: Generic scene-setup arm (0x54f8ed); no behaviour recorded.
    (21901, 0xb38578, 0x490c34),
    // 22011: Game-switch arm 0x551ae9 sets world+0x224=2. Decoder: bool plus list of 36-byte
    // entities.
    (22011, 0xb34004, 0x6c3e7a),
    // 22151: PingReply: reply to 19336 that sets world+0x234=2. Game-switch arm 0x54fc84.
    (22151, 0xb38934, 0x658350),
    (22220, 0xb395ec, 0x11be5c),
    (22242, 0xb4af88, 0x20134c),
    (22326, 0xb473fc, 0x5e2d7a),
    (22362, 0xb32b64, 0x615010),
    (22533, 0xb20040, 0x4148a8),
    // 22542: Game-switch arm 0x54ef00; no behaviour recorded.
    (22542, 0xb3db8c, 0x6d0ff6),
    // 22666: Small entity/event update. Game-switch arm 0x551e97; its sibling arm sets
    // world+0x228=2. Factory object 0x44 bytes, decoder about 988 bytes.
    (22666, 0xb29360, 0x565bfa),
    // 22756: Game-switch arm 0x55021e; no behaviour recorded.
    (22756, 0xb37618, 0x2caa44),
    (22852, 0xb1a27c, 0x5c7530),
    (22879, 0xb31aec, 0x19d0a6),
    // 23066: Profile-like: two optional strings plus six ints. Game-switch arm 0x54f7c4.
    (23066, 0xb112b0, 0x490320),
    // 23173: Tip/notification dispatch: decodes a list of ints and picks one at random. Game-
    // switch arm 0x551d5b.
    (23173, 0xb12608, 0x4fcc02),
    // 23460: Game-switch arm 0x55006c; no behaviour recorded.
    (23460, 0xb18a38, 0x2d05f0),
    // 23549: HomeSync: reply to 13447 that sets world+0x230=2. Body: three count-prefixed lists
    // plus an int. Game-switch arm 0x550d76.
    (23549, 0xb28200, 0x588992),
    // 23591: Generic scene-setup arm (0x54f3b2). String/int scalars.
    (23591, 0xb268ec, 0x4bc05a),
    (23594, 0xb1217c, 0x1a6db0),
    // 23597: No-op: recognised by the game switch and ignored.
    (23597, 0xb2d694, 0x195a7e),
    // 23631: Generic scene-setup arm (0x54ec35); no behaviour recorded.
    (23631, 0xb15a18, 0x47429e),
    (23698, 0xb2630c, 0x4b43b2),
    // 23809: String/int scalars. Among the server-band types examined during the loading-screen
    // investigation.
    (23809, 0xb280fc, 0x4c77ce),
    (23899, 0xb2ab08, 0x3786f4),
    // 23917: Game-switch arm 0x54faa1; no behaviour recorded.
    (23917, 0xb24b70, 0x5f567c),
    (23933, 0xb3ba54, 0x466cd0),
    // 24285: String/int scalars.
    (24285, 0xb29384, 0x477110),
    (24315, 0xb40714, 0x200ab6),
    (24728, 0xb227ec, 0x5b90ca),
    (24855, 0xb46cc4, 0x49b44c),
    (24878, 0xb46ce8, 0x4fecd4),
    (24979, 0xb12e98, 0x5cee22),
    (25045, 0xb21bc0, 0x9e574),
    // 25067: Handled outside the game switch by a listener/broadcast dispatcher (cmp 0x61eb at
    // 0x534209). Factory object 0x38 bytes: one bit plus an optional 36-byte object.
    (25067, 0xb36d84, 0x21188a),
    (25282, 0xb2001c, 0x63756c),
    (25362, 0xb1d2a4, 0x66e74a),
    // 25491: EntityBatch: count-capped list of 148-byte entities plus ints; the best farm-grid
    // shape. Handled on the listener path (0x607348), not the game switch.
    (25491, 0xb34af0, 0x71382a),
    // 25492: Int plus count-capped list of 48-byte entities. Not dispatched at all.
    (25492, 0xb2f04c, 0x6c8128),
    (25733, 0xb19a3c, 0x6719b2),
    // 25886: Game-switch arm 0x550178; no behaviour recorded.
    (25886, 0xb2aa0c, 0x534b22),
    // 25892: OtherDeviceConnected: server disconnect. Body: BE32 reason, then probably a string. Reason 1 =
    // another device is connecting (live-tested). Any other reason does not produce a disconnect.
    (25892, 0xb359c4, 0x2bd160),
    (25985, 0xb39f60, 0x1c5288),
    (26008, 0xb462c4, 0x4ecb50),
    // 26017: OwnHomeData: own farm/home payload. Game-switch arm 0x54f5b0. Certain all-zero body
    // lengths crash the client in this arm.
    (26017, 0xb12a88, 0x98514),
    (26204, 0xb0fa20, 0x570822),
    (26279, 0xb399b0, 0x691caa),
    // 26306: ManagerEvent: body is a single string/pair key, routed to global manager 0x1990c2.
    // Best farm-state candidate so far.
    (26306, 0xb39674, 0x565fd8),
    (26339, 0xb1e0a4, 0xd0d40),
    (26359, 0xb383c0, 0x58e390),
    // 26513: String/int scalars.
    (26513, 0xb12dcc, 0xb9ba2),
    // 26650: TrainStationDataMessage: train station data. Decoder 0x545268. Game-switch arm
    // 0x54f19e.
    (26650, 0xb45038, 0x545268),
    // 26656: FriendListReply: friend list of FriendEntry records. An empty list is body 00. Game-
    // switch arm 0x5504bf.
    (26656, 0xb1fcfc, 0x186306),
    (26697, 0xb27770, 0x9b7f8),
    // 26742: Generic scene-setup arm (0x54eb09); no behaviour recorded.
    (26742, 0xb3abf4, 0x1151ca),
    (26837, 0xb397dc, 0x2c9f50),
    // 26950: Game-switch arm 0x55180f. Its call chain reaches a UI/notification helper, not a
    // scene swap.
    (26950, 0xb47b38, 0x614fba),
    (26968, 0xb1c400, 0x5ca2b4),
    (26973, 0xb27668, 0x1b349a),
    (27066, 0xb13b58, 0x4d7954),
    // 27105: Varint plus a named field with default 900000 ms. Minimal arm: one call (0x67aae6).
    (27105, 0xb33288, 0x19ae56),
    (27171, 0xb19368, 0x3cdcfc),
    (27458, 0xb28878, 0x5a0b56),
    // 27466: Listener-tree case 0x6b4a. No scene transition.
    (27466, 0xb15240, 0x57d676),
    // 27491: Largest in-switch state update chain (arm 0x54fdc6). Body: two ints plus two strings.
    (27491, 0xb49810, 0x611d80),
    (27508, 0xb3b0a4, 0x6e9cd4),
    (27554, 0xb47d54, 0x1a4258),
    // 27626: Game-switch arm 0x54f00d; no behaviour recorded.
    (27626, 0xb18274, 0x1eabf0),
    // 27996: Listener-tree case 0x6d5c. No scene transition.
    (27996, 0xb1e4a4, 0x5615d4),
    (28004, 0xb189a4, 0x6d10d8),
    // 28054: Game-switch arm 0x550272; no behaviour recorded.
    (28054, 0xb35908, 0x5fc984),
    (28057, 0xb39b54, 0x69bc9a),
    (28185, 0xb1bf88, 0x3be34e),
    (28328, 0xb1a6e0, 0x32fa5a),
    // 28383: AIHomeData: visited AI farm ('Greg's Farm') payload. Shares the 26017 body layout.
    // Decoder 0xc1516, arm 0x54f836.
    (28383, 0xb1d2c8, 0xc1516),
    // 28430: Server->client single command. Encoder 0x4df4f6, decoder 0x11a37e. An accepted
    // command carrier; not yet confirmed as the own-farm turn reply.
    (28430, 0xb36898, 0x11a37e),
    // 28439: Game-switch arm 0x5500c6; no behaviour recorded.
    (28439, 0xb11d48, 0x4ba850),
    (28557, 0xb44ea8, 0x360adc),
    (28630, 0xb14918, 0x6ba2fa),
    // 28708: Listener-tree case 0x7024. No scene transition.
    (28708, 0xb3aa74, 0x6a229e),
    (28882, 0xb2ef74, 0x5e30da),
    // 28885: Two ints plus an optional object. Game-switch arm 0x54f4c3.
    (28885, 0xb23dac, 0xb9d40),
    (29258, 0xb27bf4, 0x5cd81e),
    (29281, 0xb4939c, 0x18dfb8),
    (29318, 0xb31154, 0x455f10),
    (29365, 0xb19e6c, 0x11b0a2),
    // 29501: LoginOk: server accepts the login. Game-switch handler 0x5506a1 needs phase 2 and
    // world+0x278 set, then sets phase 4. Decoder 0x2021fe.
    (29501, 0xb39100, 0x2021fe),
    // 29506: Listener-tree case 0x7342.
    (29506, 0xb41078, 0x2fc76a),
    (29545, 0xb49f38, 0x31c674),
    // 29637: AccountSummary: player/account summary, applied as a list on the listener path (arm
    // 0x6074a6). Decoder 0x6e04c8.
    (29637, 0xb1ce30, 0x6e04c8),
    // 29748: Nested 1304-byte object with an int and three fields. Game-switch arm 0x551a29.
    (29748, 0xb17950, 0xfd930),
    // 29950: OtherHomeData: visited home (another player's farm) payload, rendered from ai.json.
    // Decoder 0x171336 has no reader calls, so this catalogue entry may be wrong.
    (29950, 0xb17ebc, 0x171336),
    // 29997: SessionReady: tiny ack. Game-switch arm 0x5522c8 sets the game singleton's ready
    // flag; queued at login state 5. Decoder 0x6efdd0 reads nothing.
    (29997, 0xb2b724, 0x6efdd0),
];
