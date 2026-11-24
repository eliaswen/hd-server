//! Supercell "titan" wire-frame codec.
//!
//! Every message on a Supercell TCP connection (Hay Day, Clash, Brawl, …) is
//! framed with a **cleartext** 7-byte header, regardless of whether the payload
//! is encrypted:
//!
//! ```text
//!   offset  size  field
//!   0       2     message type   (u16, big-endian)
//!   2       3     payload length (u24, big-endian)
//!   5       2     message version(u16, big-endian)
//!   7       N     payload        (N = payload length; may be encrypted)
//! ```
//!
//! Because the header is never encrypted, a man-in-the-middle can split the
//! stream into messages and log type/length/version without any crypto. Only
//! the payload bytes are (for most messages) encrypted.

/// Fixed size of the frame header in bytes.
pub const HEADER_LEN: usize = 7;

/// A single wire message, with its raw on-wire bytes preserved so a proxy can
/// forward the exact original bytes unchanged.
#[derive(Clone)]
pub struct Frame {
    pub msg_type: u16,
    pub version: u16,
    /// Payload bytes only (header stripped). May be encrypted.
    pub payload: Vec<u8>,
}

impl Frame {
    /// The full on-wire representation (header + payload).
    pub fn to_wire(&self) -> Vec<u8> {
        let len = self.payload.len();
        let mut out = Vec::with_capacity(HEADER_LEN + len);
        out.extend_from_slice(&self.msg_type.to_be_bytes());
        out.push((len >> 16) as u8);
        out.push((len >> 8) as u8);
        out.push(len as u8);
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Try to take one complete frame off the front of `buf`.
///
/// Returns `Ok(Some(frame))` and drains its bytes when a whole frame is
/// buffered, `Ok(None)` when more bytes are needed, and `Err` when the header
/// announces a payload larger than `max_payload` (stream is not framed the way
/// we expect). Never consumes bytes on `Ok(None)` or `Err`.
pub fn try_parse(buf: &mut Vec<u8>, max_payload: usize) -> Result<Option<Frame>, String> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    let len = ((buf[2] as usize) << 16) | ((buf[3] as usize) << 8) | buf[4] as usize;
    let version = u16::from_be_bytes([buf[5], buf[6]]);
    if len > max_payload {
        return Err(format!(
            "header announces {len}-byte payload (type {msg_type}), over max {max_payload}"
        ));
    }
    if buf.len() < HEADER_LEN + len {
        return Ok(None);
    }
    let payload = buf[HEADER_LEN..HEADER_LEN + len].to_vec();
    buf.drain(..HEADER_LEN + len);
    Ok(Some(Frame {
        msg_type,
        version,
        payload,
    }))
}
