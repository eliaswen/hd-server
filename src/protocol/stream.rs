//! Supercell ByteStream primitives as used by the Hay Day 1.43.150 decoders.
//!
//! Native readers (ELF VAs): int `0x450a60` (big-endian), VInt `0x46f7b0`
//! (6 bits + sign bit 0x40 in the first byte, then 7 bits per byte), string
//! `0x146220` (int length, negative = null), bit-packed bool `0x407af0`,
//! end-of-stream test `0x5f2da0` (used for optional trailing fields).
//! Negative VInts are not implemented yet.

use anyhow::{bail, ensure, Result};

/// Upper bound for any decoded string or byte array, to bound allocations on
/// untrusted input.
const MAX_FIELD: usize = 1 << 20;

#[derive(Default, Debug, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.buf.push(value);
        self
    }
    /// A single boolean. The native reader (`0x407af0`) bit-packs
    /// consecutive booleans into one byte (LSB first) and every non-bool read
    /// resets the bit cursor, so one isolated bool is one byte, 0 or 1.
    /// Do not write two bools back to back with this method.
    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.u8(value as u8)
    }
    pub fn int(&mut self, value: u32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }
    pub fn long(&mut self, value: u64) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }
    /// Non-negative VInt: 6 data bits in the first byte (bit 6 is the sign and
    /// stays clear), then 7 bits per byte, bit 7 = continuation.
    pub fn vint(&mut self, value: u32) -> &mut Self {
        let mut rest = value >> 6;
        self.buf
            .push((value as u8 & 0x3f) | if rest != 0 { 0x80 } else { 0 });
        while rest != 0 {
            let next = rest >> 7;
            self.buf
                .push((rest as u8 & 0x7f) | if next != 0 { 0x80 } else { 0 });
            rest = next;
        }
        self
    }
    pub fn string(&mut self, value: &str) -> &mut Self {
        self.bytes(value.as_bytes())
    }
    /// Length-prefixed (`int`) byte array.
    pub fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.int(value.len() as u32);
        self.raw(value)
    }
    pub fn raw(&mut self, value: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(value);
        self
    }
}

#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    pub fn is_empty(&self) -> bool {
        self.pos == self.buf.len()
    }
    pub fn finish(&self) -> Result<()> {
        ensure!(
            self.is_empty(),
            "{} trailing bytes after message body",
            self.buf.len() - self.pos
        );
        Ok(())
    }
    pub fn raw(&mut self, len: usize) -> Result<&'a [u8]> {
        ensure!(
            self.buf.len() - self.pos >= len,
            "truncated: need {len} bytes at offset {}, have {}",
            self.pos,
            self.buf.len() - self.pos
        );
        let out = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.raw(N)?.try_into().unwrap())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.raw(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn int(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    pub fn long(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    pub fn vint(&mut self) -> Result<u32> {
        let first = self.u8()?;
        if first & 0x40 != 0 {
            bail!("negative VInt encoding is not supported yet");
        }
        let mut value = (first & 0x3f) as u64;
        let mut more = first & 0x80 != 0;
        let mut shift = 6;
        while more {
            ensure!(shift < 32, "VInt too long");
            let byte = self.u8()?;
            value |= ((byte & 0x7f) as u64) << shift;
            more = byte & 0x80 != 0;
            shift += 7;
        }
        u32::try_from(value).map_err(|_| anyhow::anyhow!("VInt overflows u32"))
    }
    /// Length-prefixed byte array; `None` for the `-1` null marker.
    pub fn bytes(&mut self) -> Result<Option<&'a [u8]>> {
        let len = self.int()?;
        if len == u32::MAX {
            return Ok(None);
        }
        ensure!((len as usize) <= MAX_FIELD, "field length {len} too large");
        self.raw(len as usize).map(Some)
    }
    pub fn string(&mut self) -> Result<Option<String>> {
        self.bytes()?
            .map(|b| String::from_utf8(b.to_vec()).map_err(Into::into))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vint_round_trips_and_matches_legacy_encoding() {
        for value in [0, 1, 43, 63, 64, 150, 8191, 8192, 1 << 20, u32::MAX >> 1] {
            let mut w = Writer::new();
            w.vint(value);
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes);
            assert_eq!(r.vint().unwrap(), value, "{value}");
            r.finish().unwrap();
        }
        let mut w = Writer::new();
        w.vint(150);
        // 150 = 0b10_010110: first byte 0x96 (cont + 0x16), then 0x02.
        assert_eq!(w.into_bytes(), [0x96, 0x02]);
    }

    #[test]
    fn strings_and_null() {
        let mut w = Writer::new();
        w.string("FR").int(u32::MAX).long(7).bool(true);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.string().unwrap().as_deref(), Some("FR"));
        assert_eq!(r.string().unwrap(), None);
        assert_eq!(r.long().unwrap(), 7);
        assert!(r.bool().unwrap());
        r.finish().unwrap();
        assert!(Reader::new(&[0, 0, 0, 9, 1]).string().is_err());
    }

    #[test]
    fn experience_stat_ids() {
        for (id, expected) in [
            (1_900_002, [0xa2, 0xf7, 0xe7, 0x01]),
            (1_900_003, [0xa3, 0xf7, 0xe7, 0x01]),
        ] {
            let mut w = Writer::new();
            w.vint(id);
            assert_eq!(w.into_bytes(), expected);
            assert_eq!(Reader::new(&expected).vint().unwrap(), id);
        }
    }
}
