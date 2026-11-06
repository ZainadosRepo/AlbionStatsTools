//! Photon Protocol16 binary deserializer and encoder.
//!
//! All multi-byte numbers inside a Protocol16 payload are big-endian (`network byte order`).
//! Fixed-point game numbers (`FixPoint`) use a scale factor of `10_000` (`raw_i64 / 10000.0`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Fixed-point scaling factor used across Albion network payloads (`10_000`).
pub const FIXPOINT_FACTOR: i64 = 10_000;

/// Guard against corrupted or malicious packet lengths.
const MAX_COLLECTION_LEN: usize = 16_384;
const MAX_BYTE_ARRAY_LEN: usize = 512 * 1024;
const MAX_RECURSION_DEPTH: u8 = 16;

/// Fixed-point value transmitted as a raw 64-bit integer scaled by [`FIXPOINT_FACTOR`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct FixPoint(pub i64);

impl FixPoint {
    pub const ZERO: Self = Self(0);

    #[inline]
    pub fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    #[inline]
    pub fn from_f64(val: f64) -> Self {
        Self((val * (FIXPOINT_FACTOR as f64)).round() as i64)
    }

    #[inline]
    pub fn raw(self) -> i64 {
        self.0
    }

    #[inline]
    pub fn as_f64(self) -> f64 {
        (self.0 as f64) / (FIXPOINT_FACTOR as f64)
    }

    #[inline]
    pub fn as_i64_rounded(self) -> i64 {
        self.as_f64().round() as i64
    }
}

/// 16-byte entity/player GUID carried in Protocol16 `ByteArray(16)` parameters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Guid16(pub [u8; 16]);

impl Guid16 {
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 16 {
            return None;
        }
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes);
        Some(Self(arr))
    }

    /// Formats the 16-byte array as a canonical lowercase UUID string.
    pub fn to_uuid_string(self) -> String {
        let b = self.0;
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

impl fmt::Debug for Guid16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid16({})", self.to_uuid_string())
    }
}

impl fmt::Display for Guid16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_uuid_string())
    }
}

impl Serialize for Guid16 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_uuid_string())
    }
}

impl<'de> Deserialize<'de> for Guid16 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if hex.len() != 32 {
            return Err(serde::de::Error::custom(
                "expected 32 hex digits for Guid16",
            ));
        }
        let mut out = [0u8; 16];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot =
                u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(serde::de::Error::custom)?;
        }
        Ok(Self(out))
    }
}

/// Wire type tags defined by Photon Protocol16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Protocol16Tag {
    Unknown = 0,
    Null = 42,               // '*'
    Dictionary = 68,         // 'D'
    StringArray = 97,        // 'a'
    Byte = 98,               // 'b'
    Double = 100,            // 'd'
    EventData = 101,         // 'e'
    Float = 102,             // 'f'
    Hashtable = 104,         // 'h'
    Integer = 105,           // 'i'
    Short = 107,             // 'k'
    Long = 108,              // 'l'
    IntArray = 110,          // 'n'
    Boolean = 111,           // 'o'
    OperationResponse = 112, // 'p'
    OperationRequest = 113,  // 'q'
    String = 115,            // 's'
    ByteArray = 120,         // 'x'
    Array = 121,             // 'y'
    ObjectArray = 122,       // 'z'
}

impl Protocol16Tag {
    pub fn from_u8(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Unknown),
            42 => Some(Self::Null),
            68 => Some(Self::Dictionary),
            97 => Some(Self::StringArray),
            98 => Some(Self::Byte),
            100 => Some(Self::Double),
            101 => Some(Self::EventData),
            102 => Some(Self::Float),
            104 => Some(Self::Hashtable),
            105 => Some(Self::Integer),
            107 => Some(Self::Short),
            108 => Some(Self::Long),
            110 => Some(Self::IntArray),
            111 => Some(Self::Boolean),
            112 => Some(Self::OperationResponse),
            113 => Some(Self::OperationRequest),
            115 => Some(Self::String),
            120 => Some(Self::ByteArray),
            121 => Some(Self::Array),
            122 => Some(Self::ObjectArray),
            _ => None,
        }
    }
}

/// Decoded Protocol16 value tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Protocol16Value {
    Null,
    Byte(u8),
    Boolean(bool),
    Short(i16),
    Integer(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    ByteArray(Vec<u8>),
    IntArray(Vec<i32>),
    StringArray(Vec<String>),
    Array(Vec<Protocol16Value>),
    ObjectArray(Vec<Protocol16Value>),
    Hashtable(Vec<(Protocol16Value, Protocol16Value)>),
    Dictionary(Vec<(Protocol16Value, Protocol16Value)>),
    EventData(Box<EventDataPayload>),
    OperationRequest(Box<OperationRequestPayload>),
    OperationResponse(Box<OperationResponsePayload>),
}

impl Protocol16Value {
    /// Coerces any integer/byte/short/long value to `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Byte(v) => Some(*v as i64),
            Self::Short(v) => Some(*v as i64),
            Self::Integer(v) => Some(*v as i64),
            Self::Long(v) => Some(*v),
            _ => None,
        }
    }

    /// Coerces any integer/byte/short/long value to `i32`.
    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|v| i32::try_from(v).ok())
    }

    /// Coerces any integer/byte/short/long value to `i16`.
    pub fn as_i16(&self) -> Option<i16> {
        self.as_i64().and_then(|v| i16::try_from(v).ok())
    }

    /// Coerces any integer/byte/short/long value to `u8`.
    pub fn as_u8(&self) -> Option<u8> {
        self.as_i64().and_then(|v| u8::try_from(v).ok())
    }

    /// Coerces any numeric value (`Float`, `Double`, or integer) to `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v as f64),
            Self::Double(v) => Some(*v),
            other => other.as_i64().map(|v| v as f64),
        }
    }

    /// Coerces any numeric value (`Float`, `Double`, or integer) to `f32`.
    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|v| v as f32)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            Self::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_guid(&self) -> Option<Guid16> {
        match self {
            Self::ByteArray(bytes) => Guid16::from_slice(bytes),
            _ => None,
        }
    }

    pub fn as_i64_vec(&self) -> Option<Vec<i64>> {
        match self {
            Self::IntArray(v) => Some(v.iter().map(|&x| x as i64).collect()),
            Self::ByteArray(v) => Some(v.iter().map(|&x| x as i64).collect()),
            Self::Array(v) | Self::ObjectArray(v) => v.iter().map(Self::as_i64).collect(),
            _ => None,
        }
    }

    pub fn as_i32_vec(&self) -> Option<Vec<i32>> {
        match self {
            Self::IntArray(v) => Some(v.clone()),
            Self::ByteArray(v) => Some(v.iter().map(|&x| x as i32).collect()),
            Self::Array(v) | Self::ObjectArray(v) => v.iter().map(Self::as_i32).collect(),
            _ => None,
        }
    }

    pub fn as_f64_vec(&self) -> Option<Vec<f64>> {
        match self {
            Self::Array(v) | Self::ObjectArray(v) => v.iter().map(Self::as_f64).collect(),
            Self::IntArray(v) => Some(v.iter().map(|&x| x as f64).collect()),
            _ => None,
        }
    }

    pub fn as_f32_vec(&self) -> Option<Vec<f32>> {
        match self {
            Self::Array(v) | Self::ObjectArray(v) => v.iter().map(Self::as_f32).collect(),
            Self::IntArray(v) => Some(v.iter().map(|&x| x as f32).collect()),
            _ => None,
        }
    }

    pub fn as_string_vec(&self) -> Option<Vec<String>> {
        match self {
            Self::StringArray(v) => Some(v.clone()),
            Self::Array(v) | Self::ObjectArray(v) => {
                v.iter().map(|x| x.as_str().map(str::to_owned)).collect()
            }
            _ => None,
        }
    }

    pub fn as_guid_vec(&self) -> Option<Vec<Guid16>> {
        match self {
            Self::Array(v) | Self::ObjectArray(v) => v.iter().map(Self::as_guid).collect(),
            _ => None,
        }
    }
}

/// Key-value parameter map attached to every Photon Event, OperationRequest, or OperationResponse.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParameterTable {
    pub entries: BTreeMap<u8, Protocol16Value>,
}

impl ParameterTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: u8, value: Protocol16Value) {
        self.entries.insert(key, value);
    }

    pub fn get(&self, key: u8) -> Option<&Protocol16Value> {
        self.entries.get(&key)
    }

    pub fn get_i64(&self, key: u8) -> Option<i64> {
        self.get(key).and_then(Protocol16Value::as_i64)
    }

    pub fn get_i32(&self, key: u8) -> Option<i32> {
        self.get(key).and_then(Protocol16Value::as_i32)
    }

    pub fn get_i16(&self, key: u8) -> Option<i16> {
        self.get(key).and_then(Protocol16Value::as_i16)
    }

    pub fn get_u8(&self, key: u8) -> Option<u8> {
        self.get(key).and_then(Protocol16Value::as_u8)
    }

    pub fn get_f64(&self, key: u8) -> Option<f64> {
        self.get(key).and_then(Protocol16Value::as_f64)
    }

    pub fn get_f32(&self, key: u8) -> Option<f32> {
        self.get(key).and_then(Protocol16Value::as_f32)
    }

    pub fn get_bool(&self, key: u8) -> Option<bool> {
        self.get(key).and_then(Protocol16Value::as_bool)
    }

    pub fn get_str(&self, key: u8) -> Option<&str> {
        self.get(key).and_then(Protocol16Value::as_str)
    }

    pub fn get_fixpoint(&self, key: u8) -> Option<FixPoint> {
        self.get_i64(key).map(FixPoint::from_raw)
    }

    pub fn get_guid(&self, key: u8) -> Option<Guid16> {
        self.get(key).and_then(Protocol16Value::as_guid)
    }

    pub fn get_i64_vec(&self, key: u8) -> Option<Vec<i64>> {
        self.get(key).and_then(Protocol16Value::as_i64_vec)
    }

    pub fn get_i32_vec(&self, key: u8) -> Option<Vec<i32>> {
        self.get(key).and_then(Protocol16Value::as_i32_vec)
    }

    pub fn get_f64_vec(&self, key: u8) -> Option<Vec<f64>> {
        self.get(key).and_then(Protocol16Value::as_f64_vec)
    }

    pub fn get_f32_vec(&self, key: u8) -> Option<Vec<f32>> {
        self.get(key).and_then(Protocol16Value::as_f32_vec)
    }

    pub fn get_string_vec(&self, key: u8) -> Option<Vec<String>> {
        self.get(key).and_then(Protocol16Value::as_string_vec)
    }

    pub fn get_guid_vec(&self, key: u8) -> Option<Vec<Guid16>> {
        self.get(key).and_then(Protocol16Value::as_guid_vec)
    }
}

/// Photon `EventData` (`MessageType = 4`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventDataPayload {
    pub code: u8,
    pub parameters: ParameterTable,
}

/// Photon `OperationRequest` (`MessageType = 2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationRequestPayload {
    pub operation_code: u8,
    pub parameters: ParameterTable,
}

/// Photon `OperationResponse` (`MessageType = 3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationResponsePayload {
    pub operation_code: u8,
    pub return_code: i16,
    pub debug_message: Option<String>,
    pub parameters: ParameterTable,
}

/// Deserialization errors for malformed or truncated Protocol16 payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol16Error {
    UnexpectedEof,
    UnknownTypeTag(u8),
    CollectionTooLarge(usize),
    RecursionLimitExceeded,
    InvalidUtf8,
}

impl fmt::Display for Protocol16Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of Protocol16 stream"),
            Self::UnknownTypeTag(t) => write!(f, "unknown Protocol16 type tag {t}"),
            Self::CollectionTooLarge(n) => {
                write!(f, "Protocol16 collection length {n} exceeds cap")
            }
            Self::RecursionLimitExceeded => write!(f, "Protocol16 recursion depth exceeded"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 string in Protocol16 payload"),
        }
    }
}

impl std::error::Error for Protocol16Error {}

/// Zero-copy cursor over a big-endian Protocol16 byte slice.
pub struct Protocol16Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Protocol16Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Protocol16Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(Protocol16Error::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(Protocol16Error::UnexpectedEof);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, Protocol16Error> {
        Ok(self.take(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, Protocol16Error> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_i16(&mut self) -> Result<i16, Protocol16Error> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_u16(&mut self) -> Result<u16, Protocol16Error> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_i32(&mut self) -> Result<i32, Protocol16Error> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i64(&mut self) -> Result<i64, Protocol16Error> {
        let b = self.take(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_f32(&mut self) -> Result<f32, Protocol16Error> {
        let b = self.take(4)?;
        Ok(f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f64(&mut self) -> Result<f64, Protocol16Error> {
        let b = self.take(8)?;
        Ok(f64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_string(&mut self) -> Result<String, Protocol16Error> {
        let len = self.read_u16()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Protocol16Error::InvalidUtf8)
    }

    pub fn read_byte_array(&mut self) -> Result<Vec<u8>, Protocol16Error> {
        let raw_len = self.read_i32()?;
        if raw_len < 0 {
            return Err(Protocol16Error::CollectionTooLarge(raw_len as usize));
        }
        let len = raw_len as usize;
        if len > MAX_BYTE_ARRAY_LEN {
            return Err(Protocol16Error::CollectionTooLarge(len));
        }
        Ok(self.take(len)?.to_vec())
    }

    pub fn read_int_array(&mut self) -> Result<Vec<i32>, Protocol16Error> {
        let raw_len = self.read_i32()?;
        if raw_len < 0 {
            return Err(Protocol16Error::CollectionTooLarge(raw_len as usize));
        }
        let len = raw_len as usize;
        if len > MAX_COLLECTION_LEN {
            return Err(Protocol16Error::CollectionTooLarge(len));
        }
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_i32()?);
        }
        Ok(out)
    }

    pub fn read_string_array(&mut self) -> Result<Vec<String>, Protocol16Error> {
        let len = self.read_u16()? as usize;
        if len > MAX_COLLECTION_LEN {
            return Err(Protocol16Error::CollectionTooLarge(len));
        }
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_string()?);
        }
        Ok(out)
    }

    pub fn read_parameter_table(&mut self, depth: u8) -> Result<ParameterTable, Protocol16Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Protocol16Error::RecursionLimitExceeded);
        }
        let count = self.read_u16()? as usize;
        if count > MAX_COLLECTION_LEN {
            return Err(Protocol16Error::CollectionTooLarge(count));
        }
        let mut table = ParameterTable::new();
        for _ in 0..count {
            let key = self.read_u8()?;
            let tag = self.read_u8()?;
            let val = self.read_value_with_tag(tag, depth + 1)?;
            table.insert(key, val);
        }
        Ok(table)
    }

    pub fn read_event_data(&mut self, depth: u8) -> Result<EventDataPayload, Protocol16Error> {
        let code = self.read_u8()?;
        let parameters = self.read_parameter_table(depth + 1)?;
        Ok(EventDataPayload { code, parameters })
    }

    pub fn read_operation_request(
        &mut self,
        depth: u8,
    ) -> Result<OperationRequestPayload, Protocol16Error> {
        let operation_code = self.read_u8()?;
        let parameters = self.read_parameter_table(depth + 1)?;
        Ok(OperationRequestPayload {
            operation_code,
            parameters,
        })
    }

    pub fn read_operation_response(
        &mut self,
        depth: u8,
    ) -> Result<OperationResponsePayload, Protocol16Error> {
        let operation_code = self.read_u8()?;
        let return_code = self.read_i16()?;
        let debug_tag = self.read_u8()?;
        let debug_val = self.read_value_with_tag(debug_tag, depth + 1)?;
        let debug_message = match debug_val {
            Protocol16Value::String(s) => Some(s),
            _ => None,
        };
        let parameters = self.read_parameter_table(depth + 1)?;
        Ok(OperationResponsePayload {
            operation_code,
            return_code,
            debug_message,
            parameters,
        })
    }

    pub fn read_value_with_tag(
        &mut self,
        tag_code: u8,
        depth: u8,
    ) -> Result<Protocol16Value, Protocol16Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Protocol16Error::RecursionLimitExceeded);
        }
        let tag =
            Protocol16Tag::from_u8(tag_code).ok_or(Protocol16Error::UnknownTypeTag(tag_code))?;
        match tag {
            Protocol16Tag::Unknown | Protocol16Tag::Null => Ok(Protocol16Value::Null),
            Protocol16Tag::Byte => Ok(Protocol16Value::Byte(self.read_u8()?)),
            Protocol16Tag::Boolean => Ok(Protocol16Value::Boolean(self.read_bool()?)),
            Protocol16Tag::Short => Ok(Protocol16Value::Short(self.read_i16()?)),
            Protocol16Tag::Integer => Ok(Protocol16Value::Integer(self.read_i32()?)),
            Protocol16Tag::Long => Ok(Protocol16Value::Long(self.read_i64()?)),
            Protocol16Tag::Float => Ok(Protocol16Value::Float(self.read_f32()?)),
            Protocol16Tag::Double => Ok(Protocol16Value::Double(self.read_f64()?)),
            Protocol16Tag::String => Ok(Protocol16Value::String(self.read_string()?)),
            Protocol16Tag::ByteArray => Ok(Protocol16Value::ByteArray(self.read_byte_array()?)),
            Protocol16Tag::IntArray => Ok(Protocol16Value::IntArray(self.read_int_array()?)),
            Protocol16Tag::StringArray => {
                Ok(Protocol16Value::StringArray(self.read_string_array()?))
            }
            Protocol16Tag::Array => {
                let len = self.read_u16()? as usize;
                if len > MAX_COLLECTION_LEN {
                    return Err(Protocol16Error::CollectionTooLarge(len));
                }
                let elem_tag = self.read_u8()?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    out.push(self.read_value_with_tag(elem_tag, depth + 1)?);
                }
                Ok(Protocol16Value::Array(out))
            }
            Protocol16Tag::ObjectArray => {
                let len = self.read_u16()? as usize;
                if len > MAX_COLLECTION_LEN {
                    return Err(Protocol16Error::CollectionTooLarge(len));
                }
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    let elem_tag = self.read_u8()?;
                    out.push(self.read_value_with_tag(elem_tag, depth + 1)?);
                }
                Ok(Protocol16Value::ObjectArray(out))
            }
            Protocol16Tag::Hashtable => {
                let len = self.read_u16()? as usize;
                if len > MAX_COLLECTION_LEN {
                    return Err(Protocol16Error::CollectionTooLarge(len));
                }
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    let k_tag = self.read_u8()?;
                    let k = self.read_value_with_tag(k_tag, depth + 1)?;
                    let v_tag = self.read_u8()?;
                    let v = self.read_value_with_tag(v_tag, depth + 1)?;
                    out.push((k, v));
                }
                Ok(Protocol16Value::Hashtable(out))
            }
            Protocol16Tag::Dictionary => {
                let key_type = self.read_u8()?;
                let val_type = self.read_u8()?;
                let len = self.read_u16()? as usize;
                if len > MAX_COLLECTION_LEN {
                    return Err(Protocol16Error::CollectionTooLarge(len));
                }
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    let actual_k_tag = if key_type == 0 || key_type == 42 {
                        self.read_u8()?
                    } else {
                        key_type
                    };
                    let k = self.read_value_with_tag(actual_k_tag, depth + 1)?;
                    let actual_v_tag = if val_type == 0 || val_type == 42 {
                        self.read_u8()?
                    } else {
                        val_type
                    };
                    let v = self.read_value_with_tag(actual_v_tag, depth + 1)?;
                    out.push((k, v));
                }
                Ok(Protocol16Value::Dictionary(out))
            }
            Protocol16Tag::EventData => Ok(Protocol16Value::EventData(Box::new(
                self.read_event_data(depth + 1)?,
            ))),
            Protocol16Tag::OperationRequest => Ok(Protocol16Value::OperationRequest(Box::new(
                self.read_operation_request(depth + 1)?,
            ))),
            Protocol16Tag::OperationResponse => Ok(Protocol16Value::OperationResponse(Box::new(
                self.read_operation_response(depth + 1)?,
            ))),
        }
    }
}

/// Encodes a [`Protocol16Value`] with its leading 1-byte type tag into `out`.
pub fn encode_tagged_value(out: &mut Vec<u8>, value: &Protocol16Value) {
    match value {
        Protocol16Value::Null => out.push(Protocol16Tag::Null as u8),
        Protocol16Value::Byte(v) => {
            out.push(Protocol16Tag::Byte as u8);
            out.push(*v);
        }
        Protocol16Value::Boolean(b) => {
            out.push(Protocol16Tag::Boolean as u8);
            out.push(u8::from(*b));
        }
        Protocol16Value::Short(v) => {
            out.push(Protocol16Tag::Short as u8);
            out.extend_from_slice(&v.to_be_bytes());
        }
        Protocol16Value::Integer(v) => {
            out.push(Protocol16Tag::Integer as u8);
            out.extend_from_slice(&v.to_be_bytes());
        }
        Protocol16Value::Long(v) => {
            out.push(Protocol16Tag::Long as u8);
            out.extend_from_slice(&v.to_be_bytes());
        }
        Protocol16Value::Float(v) => {
            out.push(Protocol16Tag::Float as u8);
            out.extend_from_slice(&v.to_be_bytes());
        }
        Protocol16Value::Double(v) => {
            out.push(Protocol16Tag::Double as u8);
            out.extend_from_slice(&v.to_be_bytes());
        }
        Protocol16Value::String(s) => {
            out.push(Protocol16Tag::String as u8);
            let bytes = s.as_bytes();
            out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            out.extend_from_slice(bytes);
        }
        Protocol16Value::ByteArray(bytes) => {
            out.push(Protocol16Tag::ByteArray as u8);
            out.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            out.extend_from_slice(bytes);
        }
        Protocol16Value::IntArray(ints) => {
            out.push(Protocol16Tag::IntArray as u8);
            out.extend_from_slice(&(ints.len() as i32).to_be_bytes());
            for &x in ints {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Protocol16Value::StringArray(strings) => {
            out.push(Protocol16Tag::StringArray as u8);
            out.extend_from_slice(&(strings.len() as u16).to_be_bytes());
            for s in strings {
                let bytes = s.as_bytes();
                out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
                out.extend_from_slice(bytes);
            }
        }
        Protocol16Value::Array(items) | Protocol16Value::ObjectArray(items) => {
            out.push(Protocol16Tag::ObjectArray as u8);
            out.extend_from_slice(&(items.len() as u16).to_be_bytes());
            for item in items {
                encode_tagged_value(out, item);
            }
        }
        Protocol16Value::Hashtable(pairs) => {
            out.push(Protocol16Tag::Hashtable as u8);
            out.extend_from_slice(&(pairs.len() as u16).to_be_bytes());
            for (k, v) in pairs {
                encode_tagged_value(out, k);
                encode_tagged_value(out, v);
            }
        }
        Protocol16Value::Dictionary(pairs) => {
            out.push(Protocol16Tag::Dictionary as u8);
            out.push(0); // untyped key -> each key carries its own tag
            out.push(0); // untyped val -> each val carries its own tag
            out.extend_from_slice(&(pairs.len() as u16).to_be_bytes());
            for (k, v) in pairs {
                encode_tagged_value(out, k);
                encode_tagged_value(out, v);
            }
        }
        Protocol16Value::EventData(ev) => {
            out.push(Protocol16Tag::EventData as u8);
            encode_event_data_body(out, ev);
        }
        Protocol16Value::OperationRequest(req) => {
            out.push(Protocol16Tag::OperationRequest as u8);
            encode_operation_request_body(out, req);
        }
        Protocol16Value::OperationResponse(resp) => {
            out.push(Protocol16Tag::OperationResponse as u8);
            encode_operation_response_body(out, resp);
        }
    }
}

pub fn encode_parameter_table(out: &mut Vec<u8>, table: &ParameterTable) {
    out.extend_from_slice(&(table.entries.len() as u16).to_be_bytes());
    for (&k, v) in &table.entries {
        out.push(k);
        encode_tagged_value(out, v);
    }
}

pub fn encode_event_data_body(out: &mut Vec<u8>, ev: &EventDataPayload) {
    out.push(ev.code);
    encode_parameter_table(out, &ev.parameters);
}

pub fn encode_operation_request_body(out: &mut Vec<u8>, req: &OperationRequestPayload) {
    out.push(req.operation_code);
    encode_parameter_table(out, &req.parameters);
}

pub fn encode_operation_response_body(out: &mut Vec<u8>, resp: &OperationResponsePayload) {
    out.push(resp.operation_code);
    out.extend_from_slice(&resp.return_code.to_be_bytes());
    match &resp.debug_message {
        Some(msg) => encode_tagged_value(out, &Protocol16Value::String(msg.clone())),
        None => out.push(Protocol16Tag::Null as u8),
    }
    encode_parameter_table(out, &resp.parameters);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixpoint_roundtrip_and_scaling() {
        let fp = FixPoint::from_raw(125_000_000);
        assert_eq!(fp.as_f64(), 12_500.0);
        assert_eq!(fp.as_i64_rounded(), 12_500);
        assert_eq!(FixPoint::from_f64(42.5).raw(), 425_000);
    }

    #[test]
    fn guid16_formatting_and_serde_roundtrip() {
        let raw = [
            0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff, 0x01,
        ];
        let g = Guid16(raw);
        assert_eq!(g.to_uuid_string(), "10203040-5060-7080-90aa-bbccddeeff01");
        let json = serde_json::to_string(&g).unwrap();
        let back: Guid16 = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn protocol16_roundtrips_all_value_types() {
        let mut table = ParameterTable::new();
        table.insert(0, Protocol16Value::Long(987_654_321_000));
        table.insert(1, Protocol16Value::String("Caerleon".into()));
        table.insert(2, Protocol16Value::Double(-482.5));
        table.insert(3, Protocol16Value::Float(1.25));
        table.insert(4, Protocol16Value::Boolean(true));
        table.insert(5, Protocol16Value::Byte(7));
        table.insert(6, Protocol16Value::Short(2701));
        table.insert(7, Protocol16Value::Integer(1003));
        table.insert(8, Protocol16Value::IntArray(vec![1001, 1100, 1200]));
        table.insert(
            9,
            Protocol16Value::StringArray(vec!["Kaelen".into(), "Lyra".into()]),
        );
        table.insert(10, Protocol16Value::ByteArray(vec![1, 2, 3, 4]));
        table.insert(
            11,
            Protocol16Value::Dictionary(vec![(
                Protocol16Value::Integer(1),
                Protocol16Value::Long(550_000),
            )]),
        );
        table.insert(252, Protocol16Value::Short(6));

        let ev = EventDataPayload {
            code: 1,
            parameters: table,
        };
        let mut buf = Vec::new();
        encode_event_data_body(&mut buf, &ev);

        let mut reader = Protocol16Reader::new(&buf);
        let decoded = reader.read_event_data(0).unwrap();
        assert_eq!(reader.remaining(), 0);
        assert_eq!(decoded.code, 1);
        assert_eq!(decoded.parameters.get_i64(0), Some(987_654_321_000));
        assert_eq!(decoded.parameters.get_str(1), Some("Caerleon"));
        assert_eq!(decoded.parameters.get_f64(2), Some(-482.5));
        assert_eq!(decoded.parameters.get_f32(3), Some(1.25));
        assert_eq!(decoded.parameters.get_bool(4), Some(true));
        assert_eq!(decoded.parameters.get_u8(5), Some(7));
        assert_eq!(decoded.parameters.get_i16(6), Some(2701));
        assert_eq!(decoded.parameters.get_i32(7), Some(1003));
        assert_eq!(
            decoded.parameters.get_i32_vec(8),
            Some(vec![1001, 1100, 1200])
        );
        assert_eq!(
            decoded.parameters.get_string_vec(9),
            Some(vec!["Kaelen".to_string(), "Lyra".to_string()])
        );
        assert_eq!(decoded.parameters.get_i16(252), Some(6));
    }
}
