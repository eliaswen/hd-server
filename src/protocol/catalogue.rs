//! The client's complete message-factory inventory (237 classes) and what is
//! known about each type.

#[path = "catalogue_table.rs"]
mod table;

use super::{
    messages::{AiHomeData, OtherHomeData},
    Message,
};

/// The shared empty base decoder (`0x615010`). A class with this decoder has
/// no body fields the client parses: fieldless when server-sent, or
/// send-only when client-built.
pub const EMPTY_DECODER: u32 = 0x615010;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Client,
    Server,
}

#[derive(Debug, Clone, Copy)]
pub struct Class {
    /// ELF VA of the class vtable.
    pub vtable: u32,
    /// ELF VA of the body decoder (vtable slot `+0x0c`).
    pub decoder: u32,
}

impl Class {
    pub fn has_body_decoder(&self) -> bool {
        self.decoder != EMPTY_DECODER
    }
}

/// Every factory class registered for `id`. Usually one; 20103 has two.
pub fn classes(id: u16) -> impl Iterator<Item = Class> {
    table::RAW
        .iter()
        .filter(move |(t, ..)| *t == id)
        .map(|&(_, vtable, decoder)| Class { vtable, decoder })
}

/// Whether the client's factory can instantiate `id`. Sending an unknown type
/// is pointless: the router has nothing to create.
pub fn is_known(id: u16) -> bool {
    classes(id).next().is_some()
}

pub fn origin(id: u16) -> Option<Origin> {
    match id {
        10000..=19999 => Some(Origin::Client),
        20000..=29999 => Some(Origin::Server),
        _ => None,
    }
}

/// Names only for types whose role is established by the analysis. Names are
/// descriptive of verified behaviour, not borrowed from other Supercell games.
pub fn name(id: u16) -> Option<&'static str> {
    Some(match id {
        10100 => "ClientHello",
        10101 => "Login",
        // Fieldless client message sent right after LoginOk (ctor 0xdc646).
        10366 => "PostLoginReady",
        // Sent with the BE32 type of a server message that failed the
        // client's decryption; the client then resets the connection.
        10099 => "DecryptFailure",
        20100 => "ServerHello",
        // Handler sets world phase 3 and shows a status/error (18 kinds);
        // kind 3 live-verified: "Could not connect to the server. Error code 3".
        20103 => "StatusNotice",
        29501 => "LoginOk",
        // Handler only sets the game singleton's ready flag (+0x5e).
        29997 => "SessionReady",
        // One scalar, routed to an id-keyed global manager (0x1990c2).
        26306 => "ManagerEvent",
        // Phase-4 client requests (loading-phase4-requests.md).
        13447 => "HomeRequest",
        19336 => "PhaseFourPing",
        10219 => "ClientInfo",
        13864 => "LoadingStateReady",
        17274 => "OptionalIds",
        // Replies that set the request state words to 2 (HOME_PROTOCOL.md).
        23549 => "HomeSync",
        22151 => "PingReply",
        20062 => "OptionalIdsReply",
        25491 => "EntityBatch",
        29637 => "AccountSummary",
        26017 => "OwnHomeData",
        // Avatar/home identity report: raw copy of avatar+0x00/+0x04.
        // Own-account versus loaded-home identity is unresolved; no default reply.
        16368 => "AvatarHomeIdentityReport",
        11292 => "EndClientTurn",
        AiHomeData::TYPE => "AiHomeData",
        OtherHomeData::TYPE => "OtherHomeData",
        13198 => "Unknown13198",
        18485 => "VisitPlayer",
        // : opening friends requests the list; 26656 returns its entries.
        19838 => "FriendListRequest",
        26656 => "FriendListReply",
        10213 => "Unknown10213",
        14102 => "Unknown14102",
        // Live-tested: reason 1 = "Another device is connecting to this farm."
        25892 => "OtherDeviceConnected",
        _ => return None,
    })
}

/// Display label: `Name(id)` or `id` with a `?` when the factory lacks it.
pub fn label(id: u16) -> String {
    match (name(id), is_known(id)) {
        (Some(name), _) => format!("{name}({id})"),
        (None, true) => id.to_string(),
        (None, false) => format!("{id}?"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_anchors() {
        assert_eq!(table::RAW.len(), 237);
        let hello = classes(20100).next().unwrap();
        assert_eq!((hello.vtable, hello.decoder), (0xb41a8c, 0xf1a70));
        let ok = classes(29501).next().unwrap();
        assert_eq!((ok.vtable, ok.decoder), (0xb39100, 0x2021fe));
        assert!(!classes(10366).next().unwrap().has_body_decoder());
        assert!(classes(29997).next().unwrap().has_body_decoder());
        assert_eq!(classes(20103).count(), 2);
        assert!(!is_known(20104));
        assert_eq!(label(29501), "LoginOk(29501)");
        assert_eq!(label(20104), "20104?");
        assert_eq!(name(26017), Some("OwnHomeData"));
        assert_eq!(name(16368), Some("AvatarHomeIdentityReport"));
        assert_eq!(name(11292), Some("EndClientTurn"));
        for (id, expected) in [
            (28383, "AiHomeData"),
            (29950, "OtherHomeData"),
            (13198, "Unknown13198"),
            (19838, "FriendListRequest"),
            (26656, "FriendListReply"),
            (10213, "Unknown10213"),
            (14102, "Unknown14102"),
        ] {
            assert_eq!(name(id), Some(expected));
        }
    }
}
