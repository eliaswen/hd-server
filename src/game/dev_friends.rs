//! Opt-in development controls exposed as synthetic friend entries.
use crate::protocol::messages::FriendEntry;

pub const RESET_ID: u64 = 0x7fff_ffff_0000_0001;
pub const RESOURCES_ID: u64 = 0x7fff_ffff_0000_0002;
pub const SKIP_TUTORIAL_ID: u64 = 0x7fff_ffff_0000_0003;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reset,
    Resources,
    SkipTutorial,
}

pub fn action(id: u64) -> Option<Action> {
    match id {
        RESET_ID => Some(Action::Reset),
        RESOURCES_ID => Some(Action::Resources),
        SKIP_TUTORIAL_ID => Some(Action::SkipTutorial),
        _ => None,
    }
}

pub fn entries() -> Vec<FriendEntry> {
    [
        (RESET_ID, "RESET MY ACCOUNT"),
        (RESOURCES_ID, "ADD 10,000 GOLD + DIAMONDS"),
        (SKIP_TUTORIAL_ID, "SKIP TUTORIAL"),
    ]
    .into_iter()
    .map(|(id, name)| FriendEntry {
        id,
        name: name.into(),
        facebook_id: None,
        gamecenter_id: None,
        // Values accepted in the live synthetic-friend experiment;
        // the individual scalar meanings are still unconfirmed.
        values: [1, 0, 0, 0],
    })
    .collect()
}
