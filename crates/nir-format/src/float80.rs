//! Finite x87 extended values, serialized as exact bits rather than a JSON
//! number. Software arithmetic is identical on native and WebAssembly hosts.
use rustc_apfloat::{ieee::X87DoubleExtended, Float, Status};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{cmp::Ordering, fmt};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(with = "String"))]
#[derive(Clone, Copy)]
pub struct Float80(u128);
impl Float80 {
    pub fn from_bits(bits: u128) -> Option<Self> {
        let exponent = (bits >> 64) & 0x7fff;
        let integer = (bits >> 63) & 1;
        // Reject pseudo-denormals and unsupported encodings as well as NaN /
        // infinity. Do not silently canonicalize an authored bit pattern.
        (bits >> 80 == 0 && exponent != 0x7fff && (integer != 0) == (exponent != 0))
            .then_some(Self(bits))
    }
    pub fn from_le_bytes(bytes: [u8; 10]) -> Option<Self> {
        let mut padded = [0; 16];
        padded[..10].copy_from_slice(&bytes);
        Self::from_bits(u128::from_le_bytes(padded))
    }
    pub fn from_i32(value: i32) -> Self {
        Self(X87DoubleExtended::from_i128(value.into()).value.to_bits())
    }
    pub fn bits(self) -> u128 {
        self.0
    }
    pub fn to_i32(self) -> Option<i32> {
        let result = self.number().to_i128(32);
        (!result.status.contains(Status::INVALID_OP)).then_some(result.value as i32)
    }
    pub fn is_zero(self) -> bool {
        self.number().is_zero()
    }
    fn number(self) -> X87DoubleExtended {
        X87DoubleExtended::from_bits(self.0)
    }
    pub fn checked_binary(self, op: crate::BinaryOp, rhs: Self) -> Option<Self> {
        use crate::BinaryOp::*;
        let a = self.number();
        let b = rhs.number();
        let result = match op {
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            Div => a / b,
            // fmod truncates the quotient toward zero; IEEE remainder rounds
            // it to nearest. The Story remainder contract uses fmod.
            Rem => a.c_fmod(b),
            _ => return None,
        };
        if result
            .status
            .intersects(Status::INVALID_OP | Status::DIV_BY_ZERO | Status::OVERFLOW)
        {
            return None;
        }
        Self::from_bits(result.value.to_bits())
    }
}
impl PartialEq for Float80 {
    fn eq(&self, other: &Self) -> bool {
        self.number() == other.number()
    }
}
impl Eq for Float80 {}
impl PartialOrd for Float80 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.number().partial_cmp(&other.number())
    }
}
impl fmt::Display for Float80 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.number().fmt(f)
    }
}
impl fmt::Debug for Float80 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Float80({:020x})", self.0)
    }
}
impl Serialize for Float80 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:020x}", self.0))
    }
}
impl<'de> Deserialize<'de> for Float80 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if value.len() != 20
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(serde::de::Error::custom(
                "expected 20 lowercase hexadecimal x87 bits",
            ));
        }
        u128::from_str_radix(&value, 16)
            .ok()
            .and_then(Self::from_bits)
            .ok_or_else(|| serde::de::Error::custom("expected canonical finite x87 bits"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BinaryOp::*;
    #[test]
    fn arithmetic_preserves_extended_precision_and_checks_failures() {
        let one = Float80::from_i32(1);
        let next = Float80::from_bits((0x3fffu128 << 64) | (1u128 << 63) | 1).unwrap();
        let difference = next.checked_binary(Sub, one).unwrap();
        assert!(!difference.is_zero()); // This subtraction is zero with f64.
        assert_eq!(difference.bits(), (0x3fc0u128 << 64) | (1u128 << 63));
        let minus_seven = Float80::from_i32(-7);
        assert_eq!(
            minus_seven
                .checked_binary(Div, Float80::from_i32(2))
                .unwrap()
                .to_i32(),
            Some(-3)
        );
        assert_eq!(
            minus_seven
                .checked_binary(Rem, Float80::from_i32(2))
                .unwrap()
                .to_i32(),
            Some(-1)
        );
        assert!(one.checked_binary(Div, Float80::from_i32(0)).is_none());
        assert!(Float80::from_bits(0x7fffu128 << 64 | 1u128 << 63).is_none());
        assert!(Float80::from_bits(1u128 << 63).is_none());
        let encoded = serde_json::to_string(&next).unwrap();
        assert_eq!(
            serde_json::from_str::<Float80>(&encoded).unwrap().bits(),
            next.bits()
        );
        assert!(serde_json::from_str::<Float80>("\"7fff8000000000000000\"").is_err());
    }
}
