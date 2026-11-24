//! Hay Day 1.43.150 application protocol: ByteStream codec, the client's
//! message catalogue, and typed message bodies.

pub mod catalogue;
pub mod crypto;
pub mod messages;
pub mod scframe;
pub mod stream;

pub use messages::Message;
