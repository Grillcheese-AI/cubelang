//! The key a name or a string literal compiles to (fix list P6-9, 2026-11-13).
//!
//! Every identifier and string literal used to compile to the first TWO bytes of its BLAKE3 hash, so two spellings
//! with one key read as one: `"thing" == "government"` was true (both `15d0`), two variables with one key shared a
//! register, and two labels with one key jumped to the same place. 121 of 500 programs CubbyLLM's 0067 generated
//! carried such a pair. A key is now the first EIGHT bytes, carried by the operand tags `0x09`..`0x0D` (Named, Type,
//! Role, Global, Field); the compiler refuses a program in which two spellings still share a key (`KeyCollision`).
//! No opcode byte changes. The 2-byte tags `0x01`, `0x03`..`0x05`, `0x07` still decode, for `.cubebin` files
//! written before version 5: such a program keeps its 2-byte keys (`CompiledProgram::narrow_keys`).
//!
//! Sync note for the cubemind and opcode-vsa-rs VMs: the operand tags `0x09` Named, `0x0A` Type, `0x0B` Role,
//! `0x0C` Global and `0x0D` Field are the 2-byte ones with an 8-byte key (BLAKE3's first 8 bytes of the UTF-8
//! spelling) in place of the 2-byte one; the rest of each operand is unchanged.

/// A key: BLAKE3's first 8 bytes of a spelling, or its first 2 (`wide == false`, a pre-v5 `.cubebin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub bytes: [u8; 8],
    pub wide: bool,
}

/// How many bytes a key takes in the bytecode.
pub const WIDE: usize = 8;
pub const NARROW: usize = 2;

impl Key {
    /// The 8-byte key of `name` (every program compiled since P6-9).
    pub fn wide(name: &str) -> Key {
        let h = blake3::hash(name.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&h.as_bytes()[..8]);
        Key { bytes, wide: true }
    }

    /// The 2-byte key of `name` (a program from a pre-v5 `.cubebin`).
    pub fn narrow(name: &str) -> Key {
        let h = blake3::hash(name.as_bytes());
        Key::from_narrow([h.as_bytes()[0], h.as_bytes()[1]])
    }

    pub fn from_narrow(b: [u8; 2]) -> Key {
        Key { bytes: [b[0], b[1], 0, 0, 0, 0, 0, 0], wide: false }
    }

    pub fn from_wide(b: &[u8]) -> Key {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&b[..8]);
        Key { bytes, wide: true }
    }

    /// `name`'s key at this width.
    pub fn of(name: &str, wide: bool) -> Key {
        if wide { Key::wide(name) } else { Key::narrow(name) }
    }

    /// The bytes as written in the bytecode.
    pub fn as_slice(&self) -> &[u8] {
        if self.wide { &self.bytes[..] } else { &self.bytes[..2] }
    }

    /// Lower-case hex of the bytes (4 digits narrow, 16 wide): what a register name carries.
    pub fn hex(&self) -> String {
        let mut s = String::with_capacity(16);
        self.push_hex(&mut s);
        s
    }

    /// `prefix` + `hex()` in one allocation (P6-10: a register's name is built on every operand of every
    /// instruction; eight `format!` calls and a collect per name made the VM 1.5-3.5x slower than the 2-byte keys).
    pub fn named(&self, prefix: &str) -> String {
        let mut s = String::with_capacity(prefix.len() + 16);
        s.push_str(prefix);
        self.push_hex(&mut s);
        s
    }

    fn push_hex(&self, s: &mut String) {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for b in self.as_slice() {
            s.push(DIGITS[(b >> 4) as usize] as char);
            s.push(DIGITS[(b & 15) as usize] as char);
        }
    }

    /// Parse the hex back (the inverse of `hex`), or None.
    pub fn from_hex(hex: &str) -> Option<Key> {
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        match hex.len() {
            4 => Some(Key::from_narrow([byte(0)?, byte(2)?])),
            16 => {
                let mut b = [0u8; 8];
                for (k, slot) in b.iter_mut().enumerate() {
                    *slot = byte(2 * k)?;
                }
                Some(Key { bytes: b, wide: true })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_found_pairs_share_a_narrow_key_and_not_a_wide_one() {
        assert_eq!(Key::narrow("thing"), Key::narrow("government"));
        assert_ne!(Key::wide("thing"), Key::wide("government"));
        assert_eq!(Key::narrow("eptwcv"), Key::narrow("rmcens"));
        assert_ne!(Key::wide("eptwcv"), Key::wide("rmcens"));
    }

    #[test]
    fn hex_round_trips_at_both_widths() {
        for k in [Key::wide("x"), Key::narrow("x")] {
            assert_eq!(Key::from_hex(&k.hex()), Some(k));
        }
        assert_eq!(Key::narrow("x").hex().len(), 4);
        assert_eq!(Key::wide("x").hex().len(), 16);
    }
}
