//! Photon Protocol18 binary deserializer and encoder.
//!
//! Modern Albion Online clients communicate over Photon Protocol18:
//! - Parameter tables start with a 1-byte (`u8`) entry count.
//! - Fixed-width scalars (`short`, `ushort`, `float`, `double`) and `OperationResponse.ReturnCode`
//!   are little-endian.
//! - Lengths and general `int`/`long` values use unsigned LEB128 (with ZigZag encoding for signed integers).

use crate::protocol16::{
    EventDataPayload, OperationRequestPayload, OperationResponsePayload, ParameterTable,
    Protocol16Value,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Guard against corrupted or malicious packet lengths.
const MAX_COLLECTION_LEN: usize = 65_536;
const MAX_BYTE_ARRAY_LEN: usize = 16 * 1024 * 1024;
const MAX_RECURSION_DEPTH: u8 = 16;
const MAX_SLIM_CUSTOM_TYPE_CODE: u8 = 228;

/// Wire type tags defined by Photon Protocol18.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Protocol18Tag {
    Unknown = 0,
    Boolean = 2,
    Byte = 3,
    Short = 4,
    Float = 5,
    Double = 6,
    String = 7,
    Null = 8,
    CompressedInt = 9,
    CompressedLong = 10,
    Int1 = 11,
    Int1Negative = 12,
    Int2 = 13,
    Int2Negative = 14,
    Long1 = 15,
    Long1Negative = 16,
    Long2 = 17,
    Long2Negative = 18,
    Custom = 19,
    Dictionary = 20,
    Hashtable = 21,
    ObjectArray = 23,
    OperationRequest = 24,
    OperationResponse = 25,
    EventData = 26,
    BooleanFalse = 27,
    BooleanTrue = 28,
    ShortZero = 29,
    IntZero = 30,
    LongZero = 31,
    FloatZero = 32,
    DoubleZero = 33,
    ByteZero = 34,
    Array = 64,
    BooleanArray = 66,
    ByteArray = 67,
    ShortArray = 68,
    FloatArray = 69,
    DoubleArray = 70,
    StringArray = 71,
    CompressedIntArray = 73,
    CompressedLongArray = 74,
    CustomTypeArray = 83,
    DictionaryArray = 84,
    HashtableArray = 85,
    CustomTypeSlim = 128,
}

impl Protocol18Tag {
    pub fn from_u8(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Unknown),
            2 => Some(Self::Boolean),
            3 => Some(Self::Byte),
            4 => Some(Self::Short),
            5 => Some(Self::Float),
            6 => Some(Self::Double),
            7 => Some(Self::String),
            8 => Some(Self::Null),
            9 => Some(Self::CompressedInt),
            10 => Some(Self::CompressedLong),
            11 => Some(Self::Int1),
            12 => Some(Self::Int1Negative),
            13 => Some(Self::Int2),
            14 => Some(Self::Int2Negative),
            15 => Some(Self::Long1),
            16 => Some(Self::Long1Negative),
            17 => Some(Self::Long2),
            18 => Some(Self::Long2Negative),
            19 => Some(Self::Custom),
            20 => Some(Self::Dictionary),
            21 => Some(Self::Hashtable),
            23 => Some(Self::ObjectArray),
            24 => Some(Self::OperationRequest),
            25 => Some(Self::OperationResponse),
            26 => Some(Self::EventData),
            27 => Some(Self::BooleanFalse),
            28 => Some(Self::BooleanTrue),
            29 => Some(Self::ShortZero),
            30 => Some(Self::IntZero),
            31 => Some(Self::LongZero),
            32 => Some(Self::FloatZero),
            33 => Some(Self::DoubleZero),
            34 => Some(Self::ByteZero),
            64 => Some(Self::Array),
            66 => Some(Self::BooleanArray),
            67 => Some(Self::ByteArray),
            68 => Some(Self::ShortArray),
            69 => Some(Self::FloatArray),
            70 => Some(Self::DoubleArray),
            71 => Some(Self::StringArray),
            73 => Some(Self::CompressedIntArray),
            74 => Some(Self::CompressedLongArray),
            83 => Some(Self::CustomTypeArray),
            84 => Some(Self::DictionaryArray),
            85 => Some(Self::HashtableArray),
            128 => Some(Self::CustomTypeSlim),
            _ => None,
        }
    }
}

/// Deserialization errors for malformed or truncated Protocol18 payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol18Error {
    UnexpectedEof,
    UnknownTypeTag(u8),
    CollectionTooLarge(usize),
    RecursionLimitExceeded,
    InvalidUtf8,
}

impl fmt::Display for Protocol18Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of Protocol18 stream"),
            Self::UnknownTypeTag(t) => write!(f, "unknown Protocol18 type tag {t}"),
            Self::CollectionTooLarge(n) => {
                write!(f, "Protocol18 collection length {n} exceeds cap")
            }
            Self::RecursionLimitExceeded => write!(f, "Protocol18 recursion depth exceeded"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 string in Protocol18 payload"),
        }
    }
}

impl std::error::Error for Protocol18Error {}

#[inline]
fn decode_zigzag32(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

#[inline]
fn decode_zigzag64(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[inline]
fn encode_zigzag32(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

#[inline]
fn encode_zigzag64(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Zero-copy cursor over a Protocol18 byte slice.
pub struct Protocol18Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Protocol18Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Protocol18Error> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(Protocol18Error::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(Protocol18Error::UnexpectedEof);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn read_u8(&mut self) -> Result<u8, Protocol18Error> {
        Ok(self.take(1)?[0])
    }

    pub fn read_bool(&mut self) -> Result<bool, Protocol18Error> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_i16(&mut self) -> Result<i16, Protocol18Error> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_u16(&mut self) -> Result<u16, Protocol18Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_f32(&mut self) -> Result<f32, Protocol18Error> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f64(&mut self) -> Result<f64, Protocol18Error> {
        let b = self.take(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_compressed_u32(&mut self) -> Result<u32, Protocol18Error> {
        let mut value = 0u32;
        let mut shift = 0u32;
        while shift != 35 {
            let current = self.read_u8()?;
            value |= ((current & 0x7F) as u32) << shift;
            shift += 7;
            if (current & 0x80) == 0 {
                return Ok(value);
            }
        }
        Ok(value)
    }

    pub fn read_compressed_u64(&mut self) -> Result<u64, Protocol18Error> {
        let mut value = 0u64;
        let mut shift = 0u32;
        while shift != 70 {
            let current = self.read_u8()?;
            value |= ((current & 0x7F) as u64) << shift;
            shift += 7;
            if (current & 0x80) == 0 {
                return Ok(value);
            }
        }
        Ok(value)
    }

    #[inline]
    pub fn read_compressed_i32(&mut self) -> Result<i32, Protocol18Error> {
        Ok(decode_zigzag32(self.read_compressed_u32()?))
    }

    #[inline]
    pub fn read_compressed_i64(&mut self) -> Result<i64, Protocol18Error> {
        Ok(decode_zigzag64(self.read_compressed_u64()?))
    }

    fn read_collection_len(&mut self) -> Result<usize, Protocol18Error> {
        let len = self.read_compressed_u32()? as usize;
        if len > MAX_COLLECTION_LEN {
            return Err(Protocol18Error::CollectionTooLarge(len));
        }
        Ok(len)
    }

    pub fn read_string(&mut self) -> Result<String, Protocol18Error> {
        let len = self.read_compressed_u32()? as usize;
        if len > MAX_BYTE_ARRAY_LEN {
            return Err(Protocol18Error::CollectionTooLarge(len));
        }
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Protocol18Error::InvalidUtf8)
    }

    pub fn read_byte_array(&mut self) -> Result<Vec<u8>, Protocol18Error> {
        let len = self.read_compressed_u32()? as usize;
        if len > MAX_BYTE_ARRAY_LEN {
            return Err(Protocol18Error::CollectionTooLarge(len));
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        Ok(self.take(len)?.to_vec())
    }

    pub fn read_boolean_array(&mut self) -> Result<Vec<bool>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let byte_len = len.div_ceil(8);
        let packed = self.take(byte_len)?;
        let mut out = Vec::with_capacity(len);
        let full_bytes = len / 8;
        for &byte in &packed[..full_bytes] {
            for bit in 0..8 {
                out.push((byte & (1 << bit)) != 0);
            }
        }
        let rem = len % 8;
        if rem > 0 {
            let last = packed[full_bytes];
            for bit in 0..rem {
                out.push((last & (1 << bit)) != 0);
            }
        }
        Ok(out)
    }

    pub fn read_short_array(&mut self) -> Result<Vec<i16>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let byte_len = len.checked_mul(2).ok_or(Protocol18Error::UnexpectedEof)?;
        let raw = self.take(byte_len)?;
        let mut out = Vec::with_capacity(len);
        for &chunk in raw.as_chunks::<2>().0 {
            out.push(i16::from_le_bytes(chunk));
        }
        Ok(out)
    }

    pub fn read_float_array(&mut self) -> Result<Vec<f32>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let byte_len = len.checked_mul(4).ok_or(Protocol18Error::UnexpectedEof)?;
        let raw = self.take(byte_len)?;
        let mut out = Vec::with_capacity(len);
        for &chunk in raw.as_chunks::<4>().0 {
            out.push(f32::from_le_bytes(chunk));
        }
        Ok(out)
    }

    pub fn read_double_array(&mut self) -> Result<Vec<f64>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let byte_len = len.checked_mul(8).ok_or(Protocol18Error::UnexpectedEof)?;
        let raw = self.take(byte_len)?;
        let mut out = Vec::with_capacity(len);
        for &chunk in raw.as_chunks::<8>().0 {
            out.push(f64::from_le_bytes(chunk));
        }
        Ok(out)
    }

    pub fn read_string_array(&mut self) -> Result<Vec<String>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_string()?);
        }
        Ok(out)
    }

    pub fn read_compressed_int_array(&mut self) -> Result<Vec<i32>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_compressed_i32()?);
        }
        Ok(out)
    }

    pub fn read_compressed_long_array(&mut self) -> Result<Vec<i64>, Protocol18Error> {
        let len = self.read_collection_len()?;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.read_compressed_i64()?);
        }
        Ok(out)
    }

    pub fn read_parameter_table(&mut self, depth: u8) -> Result<ParameterTable, Protocol18Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Protocol18Error::RecursionLimitExceeded);
        }
        let count = self.read_u8()? as usize;
        let mut table = ParameterTable::new();
        for _ in 0..count {
            let key = self.read_u8()?;
            let tag = self.read_u8()?;
            let val = self.read_value_with_tag(tag, depth + 1)?;
            table.insert(key, val);
        }
        Ok(table)
    }

    pub fn read_event_data(&mut self, depth: u8) -> Result<EventDataPayload, Protocol18Error> {
        let code = self.read_u8()?;
        let parameters = self.read_parameter_table(depth + 1)?;
        Ok(EventDataPayload { code, parameters })
    }

    pub fn read_operation_request(
        &mut self,
        depth: u8,
    ) -> Result<OperationRequestPayload, Protocol18Error> {
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
    ) -> Result<OperationResponsePayload, Protocol18Error> {
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

    fn read_hashtable(&mut self, depth: u8) -> Result<Protocol16Value, Protocol18Error> {
        let len = self.read_collection_len()?;
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

    fn skip_nested_dictionary_type(&mut self, depth: u8) -> Result<(), Protocol18Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Protocol18Error::RecursionLimitExceeded);
        }
        let _key_type = self.read_u8()?;
        let val_type = self.read_u8()?;
        match val_type {
            x if x == Protocol18Tag::Dictionary as u8 => {
                self.skip_nested_dictionary_type(depth + 1)?;
            }
            x if x == Protocol18Tag::Array as u8 => {
                let mut elem_tag = self.read_u8()?;
                let mut array_depth = 0u8;
                while elem_tag == Protocol18Tag::Array as u8 {
                    array_depth = array_depth
                        .checked_add(1)
                        .filter(|&d| d <= MAX_RECURSION_DEPTH)
                        .ok_or(Protocol18Error::RecursionLimitExceeded)?;
                    elem_tag = self.read_u8()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn read_dictionary_type_header(&mut self, depth: u8) -> Result<(u8, u8), Protocol18Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Protocol18Error::RecursionLimitExceeded);
        }
        let key_type = self.read_u8()?;
        let mut val_type = self.read_u8()?;
        if val_type == Protocol18Tag::Dictionary as u8 {
            self.skip_nested_dictionary_type(depth + 1)?;
        } else if val_type == Protocol18Tag::Array as u8 {
            let mut elem_tag = self.read_u8()?;
            let mut array_depth = 0u8;
            while elem_tag == Protocol18Tag::Array as u8 {
                array_depth = array_depth
                    .checked_add(1)
                    .filter(|&d| d <= MAX_RECURSION_DEPTH)
                    .ok_or(Protocol18Error::RecursionLimitExceeded)?;
                elem_tag = self.read_u8()?;
            }
            val_type = Protocol18Tag::Unknown as u8;
        }
        Ok((key_type, val_type))
    }

    fn read_dictionary_elements(
        &mut self,
        key_type: u8,
        val_type: u8,
        depth: u8,
    ) -> Result<Protocol16Value, Protocol18Error> {
        let len = self.read_collection_len()?;
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let actual_k_tag = if key_type == Protocol18Tag::Unknown as u8
                || key_type == Protocol18Tag::Null as u8
            {
                self.read_u8()?
            } else {
                key_type
            };
            let k = self.read_value_with_tag(actual_k_tag, depth + 1)?;
            let actual_v_tag = if val_type == Protocol18Tag::Unknown as u8
                || val_type == Protocol18Tag::Null as u8
            {
                self.read_u8()?
            } else {
                val_type
            };
            let v = self.read_value_with_tag(actual_v_tag, depth + 1)?;
            out.push((k, v));
        }
        Ok(Protocol16Value::Dictionary(out))
    }

    pub fn read_value_with_tag(
        &mut self,
        tag_code: u8,
        depth: u8,
    ) -> Result<Protocol16Value, Protocol18Error> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(Protocol18Error::RecursionLimitExceeded);
        }

        if (Protocol18Tag::CustomTypeSlim as u8..=MAX_SLIM_CUSTOM_TYPE_CODE).contains(&tag_code) {
            let type_code = tag_code - Protocol18Tag::CustomTypeSlim as u8;
            let data = self.read_byte_array()?;
            return Ok(Protocol16Value::Custom { type_code, data });
        }

        let tag =
            Protocol18Tag::from_u8(tag_code).ok_or(Protocol18Error::UnknownTypeTag(tag_code))?;

        match tag {
            Protocol18Tag::Unknown | Protocol18Tag::Null => Ok(Protocol16Value::Null),
            Protocol18Tag::Boolean => Ok(Protocol16Value::Boolean(self.read_bool()?)),
            Protocol18Tag::Byte => Ok(Protocol16Value::Byte(self.read_u8()?)),
            Protocol18Tag::Short => Ok(Protocol16Value::Short(self.read_i16()?)),
            Protocol18Tag::Float => Ok(Protocol16Value::Float(self.read_f32()?)),
            Protocol18Tag::Double => Ok(Protocol16Value::Double(self.read_f64()?)),
            Protocol18Tag::String => Ok(Protocol16Value::String(self.read_string()?)),
            Protocol18Tag::CompressedInt => {
                Ok(Protocol16Value::Integer(self.read_compressed_i32()?))
            }
            Protocol18Tag::CompressedLong => Ok(Protocol16Value::Long(self.read_compressed_i64()?)),
            Protocol18Tag::Int1 => Ok(Protocol16Value::Integer(self.read_u8()? as i32)),
            Protocol18Tag::Int1Negative => Ok(Protocol16Value::Integer(-(self.read_u8()? as i32))),
            Protocol18Tag::Int2 => Ok(Protocol16Value::Integer(self.read_u16()? as i32)),
            Protocol18Tag::Int2Negative => Ok(Protocol16Value::Integer(-(self.read_u16()? as i32))),
            Protocol18Tag::Long1 => Ok(Protocol16Value::Long(self.read_u8()? as i64)),
            Protocol18Tag::Long1Negative => Ok(Protocol16Value::Long(-(self.read_u8()? as i64))),
            Protocol18Tag::Long2 => Ok(Protocol16Value::Long(self.read_u16()? as i64)),
            Protocol18Tag::Long2Negative => Ok(Protocol16Value::Long(-(self.read_u16()? as i64))),
            Protocol18Tag::Custom => {
                let type_code = self.read_u8()?;
                let data = self.read_byte_array()?;
                Ok(Protocol16Value::Custom { type_code, data })
            }
            Protocol18Tag::CustomTypeSlim => {
                // Already handled by range check above; kept for exhaustive match clarity.
                let data = self.read_byte_array()?;
                Ok(Protocol16Value::Custom { type_code: 0, data })
            }
            Protocol18Tag::Dictionary => {
                let (key_type, val_type) = self.read_dictionary_type_header(depth + 1)?;
                self.read_dictionary_elements(key_type, val_type, depth)
            }
            Protocol18Tag::Hashtable => self.read_hashtable(depth),
            Protocol18Tag::ObjectArray => {
                let len = self.read_collection_len()?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    let elem_tag = self.read_u8()?;
                    out.push(self.read_value_with_tag(elem_tag, depth + 1)?);
                }
                Ok(Protocol16Value::ObjectArray(out))
            }
            Protocol18Tag::Array => {
                let len = self.read_collection_len()?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    let elem_tag = self.read_u8()?;
                    out.push(self.read_value_with_tag(elem_tag, depth + 1)?);
                }
                Ok(Protocol16Value::Array(out))
            }
            Protocol18Tag::OperationRequest => Ok(Protocol16Value::OperationRequest(Box::new(
                self.read_operation_request(depth + 1)?,
            ))),
            Protocol18Tag::OperationResponse => Ok(Protocol16Value::OperationResponse(Box::new(
                self.read_operation_response(depth + 1)?,
            ))),
            Protocol18Tag::EventData => Ok(Protocol16Value::EventData(Box::new(
                self.read_event_data(depth + 1)?,
            ))),
            Protocol18Tag::BooleanFalse => Ok(Protocol16Value::Boolean(false)),
            Protocol18Tag::BooleanTrue => Ok(Protocol16Value::Boolean(true)),
            Protocol18Tag::ShortZero => Ok(Protocol16Value::Short(0)),
            Protocol18Tag::IntZero => Ok(Protocol16Value::Integer(0)),
            Protocol18Tag::LongZero => Ok(Protocol16Value::Long(0)),
            Protocol18Tag::FloatZero => Ok(Protocol16Value::Float(0.0)),
            Protocol18Tag::DoubleZero => Ok(Protocol16Value::Double(0.0)),
            Protocol18Tag::ByteZero => Ok(Protocol16Value::Byte(0)),
            Protocol18Tag::BooleanArray => {
                Ok(Protocol16Value::BoolArray(self.read_boolean_array()?))
            }
            Protocol18Tag::ByteArray => Ok(Protocol16Value::ByteArray(self.read_byte_array()?)),
            Protocol18Tag::ShortArray => Ok(Protocol16Value::ShortArray(self.read_short_array()?)),
            Protocol18Tag::FloatArray => Ok(Protocol16Value::FloatArray(self.read_float_array()?)),
            Protocol18Tag::DoubleArray => {
                Ok(Protocol16Value::DoubleArray(self.read_double_array()?))
            }
            Protocol18Tag::StringArray => {
                Ok(Protocol16Value::StringArray(self.read_string_array()?))
            }
            Protocol18Tag::CompressedIntArray => {
                Ok(Protocol16Value::IntArray(self.read_compressed_int_array()?))
            }
            Protocol18Tag::CompressedLongArray => Ok(Protocol16Value::LongArray(
                self.read_compressed_long_array()?,
            )),
            Protocol18Tag::CustomTypeArray => {
                let len = self.read_collection_len()?;
                let type_code = self.read_u8()?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    let data = self.read_byte_array()?;
                    out.push(Protocol16Value::Custom { type_code, data });
                }
                Ok(Protocol16Value::Array(out))
            }
            Protocol18Tag::DictionaryArray => {
                let (key_type, val_type) = self.read_dictionary_type_header(depth + 1)?;
                let len = self.read_collection_len()?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    out.push(self.read_dictionary_elements(key_type, val_type, depth + 1)?);
                }
                Ok(Protocol16Value::Array(out))
            }
            Protocol18Tag::HashtableArray => {
                let len = self.read_collection_len()?;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    out.push(self.read_hashtable(depth + 1)?);
                }
                Ok(Protocol16Value::Array(out))
            }
        }
    }
}

pub fn write_compressed_u32(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

pub fn write_compressed_u64(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[inline]
pub fn write_compressed_i32(out: &mut Vec<u8>, value: i32) {
    write_compressed_u32(out, encode_zigzag32(value));
}

#[inline]
pub fn write_compressed_i64(out: &mut Vec<u8>, value: i64) {
    write_compressed_u64(out, encode_zigzag64(value));
}

/// Encodes a [`Protocol16Value`] with its leading Protocol18 type tag into `out`.
pub fn encode_protocol18_tagged_value(out: &mut Vec<u8>, value: &Protocol16Value) {
    match value {
        Protocol16Value::Null => out.push(Protocol18Tag::Null as u8),
        Protocol16Value::Byte(0) => out.push(Protocol18Tag::ByteZero as u8),
        Protocol16Value::Byte(v) => {
            out.push(Protocol18Tag::Byte as u8);
            out.push(*v);
        }
        Protocol16Value::Boolean(false) => out.push(Protocol18Tag::BooleanFalse as u8),
        Protocol16Value::Boolean(true) => out.push(Protocol18Tag::BooleanTrue as u8),
        Protocol16Value::Short(0) => out.push(Protocol18Tag::ShortZero as u8),
        Protocol16Value::Short(v) => {
            out.push(Protocol18Tag::Short as u8);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Protocol16Value::Integer(0) => out.push(Protocol18Tag::IntZero as u8),
        Protocol16Value::Integer(v) if (1..=255).contains(v) => {
            out.push(Protocol18Tag::Int1 as u8);
            out.push(*v as u8);
        }
        Protocol16Value::Integer(v) if (-255..=-1).contains(v) => {
            out.push(Protocol18Tag::Int1Negative as u8);
            out.push((-*v) as u8);
        }
        Protocol16Value::Integer(v) if (256..=65_535).contains(v) => {
            out.push(Protocol18Tag::Int2 as u8);
            out.extend_from_slice(&(*v as u16).to_le_bytes());
        }
        Protocol16Value::Integer(v) if (-65_535..=-256).contains(v) => {
            out.push(Protocol18Tag::Int2Negative as u8);
            out.extend_from_slice(&((-*v) as u16).to_le_bytes());
        }
        Protocol16Value::Integer(v) => {
            out.push(Protocol18Tag::CompressedInt as u8);
            write_compressed_i32(out, *v);
        }
        Protocol16Value::Long(0) => out.push(Protocol18Tag::LongZero as u8),
        Protocol16Value::Long(v) if (1..=255).contains(v) => {
            out.push(Protocol18Tag::Long1 as u8);
            out.push(*v as u8);
        }
        Protocol16Value::Long(v) if (-255..=-1).contains(v) => {
            out.push(Protocol18Tag::Long1Negative as u8);
            out.push((-*v) as u8);
        }
        Protocol16Value::Long(v) if (256..=65_535).contains(v) => {
            out.push(Protocol18Tag::Long2 as u8);
            out.extend_from_slice(&(*v as u16).to_le_bytes());
        }
        Protocol16Value::Long(v) if (-65_535..=-256).contains(v) => {
            out.push(Protocol18Tag::Long2Negative as u8);
            out.extend_from_slice(&((-*v) as u16).to_le_bytes());
        }
        Protocol16Value::Long(v) => {
            out.push(Protocol18Tag::CompressedLong as u8);
            write_compressed_i64(out, *v);
        }
        Protocol16Value::Float(v) if *v == 0.0 && v.is_sign_positive() => {
            out.push(Protocol18Tag::FloatZero as u8);
        }
        Protocol16Value::Float(v) => {
            out.push(Protocol18Tag::Float as u8);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Protocol16Value::Double(v) if *v == 0.0 && v.is_sign_positive() => {
            out.push(Protocol18Tag::DoubleZero as u8);
        }
        Protocol16Value::Double(v) => {
            out.push(Protocol18Tag::Double as u8);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Protocol16Value::String(s) => {
            out.push(Protocol18Tag::String as u8);
            let bytes = s.as_bytes();
            write_compressed_u32(out, bytes.len() as u32);
            out.extend_from_slice(bytes);
        }
        Protocol16Value::BoolArray(vals) => {
            out.push(Protocol18Tag::BooleanArray as u8);
            write_compressed_u32(out, vals.len() as u32);
            for chunk in vals.chunks(8) {
                let mut byte = 0u8;
                for (bit, &b) in chunk.iter().enumerate() {
                    if b {
                        byte |= 1 << bit;
                    }
                }
                out.push(byte);
            }
        }
        Protocol16Value::ByteArray(bytes) => {
            out.push(Protocol18Tag::ByteArray as u8);
            write_compressed_u32(out, bytes.len() as u32);
            out.extend_from_slice(bytes);
        }
        Protocol16Value::ShortArray(vals) => {
            out.push(Protocol18Tag::ShortArray as u8);
            write_compressed_u32(out, vals.len() as u32);
            for &x in vals {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        Protocol16Value::IntArray(vals) => {
            out.push(Protocol18Tag::CompressedIntArray as u8);
            write_compressed_u32(out, vals.len() as u32);
            for &x in vals {
                write_compressed_i32(out, x);
            }
        }
        Protocol16Value::LongArray(vals) => {
            out.push(Protocol18Tag::CompressedLongArray as u8);
            write_compressed_u32(out, vals.len() as u32);
            for &x in vals {
                write_compressed_i64(out, x);
            }
        }
        Protocol16Value::FloatArray(vals) => {
            out.push(Protocol18Tag::FloatArray as u8);
            write_compressed_u32(out, vals.len() as u32);
            for &x in vals {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        Protocol16Value::DoubleArray(vals) => {
            out.push(Protocol18Tag::DoubleArray as u8);
            write_compressed_u32(out, vals.len() as u32);
            for &x in vals {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        Protocol16Value::StringArray(strings) => {
            out.push(Protocol18Tag::StringArray as u8);
            write_compressed_u32(out, strings.len() as u32);
            for s in strings {
                let bytes = s.as_bytes();
                write_compressed_u32(out, bytes.len() as u32);
                out.extend_from_slice(bytes);
            }
        }
        Protocol16Value::Array(items) => {
            out.push(Protocol18Tag::Array as u8);
            write_compressed_u32(out, items.len() as u32);
            for item in items {
                encode_protocol18_tagged_value(out, item);
            }
        }
        Protocol16Value::ObjectArray(items) => {
            out.push(Protocol18Tag::ObjectArray as u8);
            write_compressed_u32(out, items.len() as u32);
            for item in items {
                encode_protocol18_tagged_value(out, item);
            }
        }
        Protocol16Value::Hashtable(pairs) => {
            out.push(Protocol18Tag::Hashtable as u8);
            write_compressed_u32(out, pairs.len() as u32);
            for (k, v) in pairs {
                encode_protocol18_tagged_value(out, k);
                encode_protocol18_tagged_value(out, v);
            }
        }
        Protocol16Value::Dictionary(pairs) => {
            out.push(Protocol18Tag::Dictionary as u8);
            out.push(Protocol18Tag::Unknown as u8);
            out.push(Protocol18Tag::Unknown as u8);
            write_compressed_u32(out, pairs.len() as u32);
            for (k, v) in pairs {
                encode_protocol18_tagged_value(out, k);
                encode_protocol18_tagged_value(out, v);
            }
        }
        Protocol16Value::Custom { type_code, data } => {
            if *type_code <= 100 {
                out.push(Protocol18Tag::CustomTypeSlim as u8 + *type_code);
            } else {
                out.push(Protocol18Tag::Custom as u8);
                out.push(*type_code);
            }
            write_compressed_u32(out, data.len() as u32);
            out.extend_from_slice(data);
        }
        Protocol16Value::EventData(ev) => {
            out.push(Protocol18Tag::EventData as u8);
            encode_protocol18_event_data_body(out, ev);
        }
        Protocol16Value::OperationRequest(req) => {
            out.push(Protocol18Tag::OperationRequest as u8);
            encode_protocol18_operation_request_body(out, req);
        }
        Protocol16Value::OperationResponse(resp) => {
            out.push(Protocol18Tag::OperationResponse as u8);
            encode_protocol18_operation_response_body(out, resp);
        }
    }
}

pub fn encode_protocol18_parameter_table(out: &mut Vec<u8>, table: &ParameterTable) {
    out.push(table.entries.len().min(u8::MAX as usize) as u8);
    for (&k, v) in table.entries.iter().take(u8::MAX as usize) {
        out.push(k);
        encode_protocol18_tagged_value(out, v);
    }
}

pub fn encode_protocol18_event_data_body(out: &mut Vec<u8>, ev: &EventDataPayload) {
    out.push(ev.code);
    encode_protocol18_parameter_table(out, &ev.parameters);
}

pub fn encode_protocol18_operation_request_body(out: &mut Vec<u8>, req: &OperationRequestPayload) {
    out.push(req.operation_code);
    encode_protocol18_parameter_table(out, &req.parameters);
}

pub fn encode_protocol18_operation_response_body(
    out: &mut Vec<u8>,
    resp: &OperationResponsePayload,
) {
    out.push(resp.operation_code);
    out.extend_from_slice(&resp.return_code.to_le_bytes());
    match &resp.debug_message {
        Some(msg) => encode_protocol18_tagged_value(out, &Protocol16Value::String(msg.clone())),
        None => out.push(Protocol18Tag::Null as u8),
    }
    encode_protocol18_parameter_table(out, &resp.parameters);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb128_and_zigzag_boundary_roundtrip() {
        let i32_cases = [
            0,
            1,
            -1,
            63,
            64,
            -64,
            -65,
            127,
            -128,
            16_383,
            -16_384,
            1_000_000,
            -1_000_000,
            i32::MAX,
            i32::MIN,
        ];
        for v in i32_cases {
            let mut buf = Vec::new();
            write_compressed_i32(&mut buf, v);
            let mut reader = Protocol18Reader::new(&buf);
            assert_eq!(reader.read_compressed_i32().unwrap(), v);
            assert_eq!(reader.remaining(), 0);
        }

        let i64_cases = [
            0i64,
            1,
            -1,
            63,
            -64,
            127,
            -128,
            987_654_321_000,
            -987_654_321_000,
            i64::MAX,
            i64::MIN,
        ];
        for v in i64_cases {
            let mut buf = Vec::new();
            write_compressed_i64(&mut buf, v);
            let mut reader = Protocol18Reader::new(&buf);
            assert_eq!(reader.read_compressed_i64().unwrap(), v);
            assert_eq!(reader.remaining(), 0);
        }
    }

    #[test]
    fn roundtrips_all_scalars_compact_ints_zero_tags_and_arrays() {
        let mut table = ParameterTable::new();
        table.insert(0, Protocol16Value::Byte(0));
        table.insert(1, Protocol16Value::Byte(250));
        table.insert(2, Protocol16Value::Boolean(false));
        table.insert(3, Protocol16Value::Boolean(true));
        table.insert(4, Protocol16Value::Short(0));
        table.insert(5, Protocol16Value::Short(-12345));
        table.insert(6, Protocol16Value::Integer(0));
        table.insert(7, Protocol16Value::Integer(42));
        table.insert(8, Protocol16Value::Integer(-42));
        table.insert(9, Protocol16Value::Integer(30_000));
        table.insert(10, Protocol16Value::Integer(-30_000));
        table.insert(11, Protocol16Value::Integer(i32::MIN));
        table.insert(12, Protocol16Value::Long(0));
        table.insert(13, Protocol16Value::Long(200));
        table.insert(14, Protocol16Value::Long(-200));
        table.insert(15, Protocol16Value::Long(50_000));
        table.insert(16, Protocol16Value::Long(-50_000));
        table.insert(17, Protocol16Value::Long(-987_654_321_000));
        table.insert(18, Protocol16Value::Float(0.0));
        table.insert(19, Protocol16Value::Float(-3.25));
        table.insert(20, Protocol16Value::Double(0.0));
        table.insert(21, Protocol16Value::Double(1234.5625));
        table.insert(22, Protocol16Value::String("Brumes d'Avalon".into()));
        table.insert(23, Protocol16Value::Null);
        table.insert(24, Protocol16Value::ByteArray(vec![0xAA; 32]));
        table.insert(25, Protocol16Value::ShortArray(vec![-3, 0, 777]));
        table.insert(26, Protocol16Value::IntArray(vec![-100_000, 0, 100_000]));
        table.insert(
            27,
            Protocol16Value::LongArray(vec![i64::MIN, 0, 5_000_000_000]),
        );
        table.insert(28, Protocol16Value::FloatArray(vec![1.5, -2.5]));
        table.insert(29, Protocol16Value::DoubleArray(vec![99.25, -0.125]));
        table.insert(
            30,
            Protocol16Value::StringArray(vec!["Kaelen".into(), "".into(), "Lyra".into()]),
        );
        table.insert(
            31,
            Protocol16Value::Custom {
                type_code: 42,
                data: vec![1, 2, 3, 4],
            },
        );
        table.insert(
            32,
            Protocol16Value::Custom {
                type_code: 150,
                data: vec![9, 8, 7],
            },
        );
        table.insert(
            33,
            Protocol16Value::Hashtable(vec![(
                Protocol16Value::String("k".into()),
                Protocol16Value::Integer(99),
            )]),
        );
        table.insert(
            34,
            Protocol16Value::Dictionary(vec![(
                Protocol16Value::Integer(1),
                Protocol16Value::String("one".into()),
            )]),
        );
        table.insert(
            35,
            Protocol16Value::ObjectArray(vec![
                Protocol16Value::Long(555_000),
                Protocol16Value::String("nested".into()),
            ]),
        );
        table.insert(252, Protocol16Value::Short(82));

        let ev = EventDataPayload {
            code: 1,
            parameters: table.clone(),
        };
        let mut buf = Vec::new();
        encode_protocol18_event_data_body(&mut buf, &ev);

        let mut reader = Protocol18Reader::new(&buf);
        let decoded = reader.read_event_data(0).unwrap();
        assert_eq!(reader.remaining(), 0);
        assert_eq!(decoded, ev);
    }

    #[test]
    fn boolean_array_handles_exact_and_unaligned_bit_lengths() {
        for bit_len in [0usize, 1, 7, 8, 9, 16, 17] {
            let bits: Vec<bool> = (0..bit_len).map(|i| (i % 3) == 0 || i == 7).collect();
            let val = Protocol16Value::BoolArray(bits.clone());
            let mut buf = Vec::new();
            encode_protocol18_tagged_value(&mut buf, &val);
            let mut reader = Protocol18Reader::new(&buf);
            let tag = reader.read_u8().unwrap();
            let decoded = reader.read_value_with_tag(tag, 0).unwrap();
            assert_eq!(reader.remaining(), 0);
            assert_eq!(decoded, Protocol16Value::BoolArray(bits));
        }
    }

    #[test]
    fn decodes_operation_response_le_return_code_and_custom_types_and_arrays() {
        let mut inner_params = ParameterTable::new();
        inner_params.insert(0, Protocol16Value::Long(12345));
        inner_params.insert(253, Protocol16Value::Short(2));
        let resp = OperationResponsePayload {
            operation_code: 1,
            return_code: -2,
            debug_message: Some("err_le".into()),
            parameters: inner_params,
        };
        let mut raw = Vec::new();
        encode_protocol18_operation_response_body(&mut raw, &resp);
        // Verify ReturnCode at bytes 1..=2 is little-endian -2i16 = [0xFE, 0xFF]
        assert_eq!(&raw[1..3], &[0xFE, 0xFF]);

        let mut reader = Protocol18Reader::new(&raw);
        let decoded = reader.read_operation_response(0).unwrap();
        assert_eq!(reader.remaining(), 0);
        assert_eq!(decoded, resp);

        // Hand-crafted CustomTypeArray (83), HashtableArray (85), and Dictionary with nested Array type header
        #[rustfmt::skip]
        let wire_special: Vec<u8> = vec![
            1, // EventData code
            3, // 3 parameters (u8 count)
            // param 0: CustomTypeArray (tag 83), 2 items, type_code 12
            0, Protocol18Tag::CustomTypeArray as u8, 2, 12, 2, 0xDE, 0xAD, 1, 0xBE,
            // param 1: HashtableArray (tag 85), 1 item, 1 kv ("a" => 7)
            1, Protocol18Tag::HashtableArray as u8, 1, 1,
                Protocol18Tag::String as u8, 1, b'a',
                Protocol18Tag::Int1 as u8, 7,
            // param 2: Typed Dictionary<String, CompressedInt[]> -> key=7(String), val=64(Array) + 9(CompressedInt)
            2, Protocol18Tag::Dictionary as u8,
                Protocol18Tag::String as u8,
                Protocol18Tag::Array as u8,
                Protocol18Tag::CompressedInt as u8,
                1, // 1 entry
                1, b'x', // key (untagged because key_type == String)
                Protocol18Tag::CompressedIntArray as u8, 2, 2, 4, // val (tagged because Array header forces Unknown) -> ZigZag(2)=1, ZigZag(4)=2
        ];

        let mut sp_reader = Protocol18Reader::new(&wire_special);
        let ev = sp_reader.read_event_data(0).unwrap();
        assert_eq!(sp_reader.remaining(), 0);
        assert_eq!(
            ev.parameters.get(0),
            Some(&Protocol16Value::Array(vec![
                Protocol16Value::Custom {
                    type_code: 12,
                    data: vec![0xDE, 0xAD]
                },
                Protocol16Value::Custom {
                    type_code: 12,
                    data: vec![0xBE]
                },
            ]))
        );
        assert_eq!(
            ev.parameters.get(1),
            Some(&Protocol16Value::Array(vec![Protocol16Value::Hashtable(
                vec![(
                    Protocol16Value::String("a".into()),
                    Protocol16Value::Integer(7)
                )]
            )]))
        );
        assert_eq!(
            ev.parameters.get(2),
            Some(&Protocol16Value::Dictionary(vec![(
                Protocol16Value::String("x".into()),
                Protocol16Value::IntArray(vec![1, 2])
            )]))
        );
    }

    #[test]
    fn decodes_nested_dictionary_and_operation_request_and_enforces_bounds() {
        // Nested Dictionary<Int1, Dictionary<String, BooleanTrue>>
        #[rustfmt::skip]
        let nested_dict: Vec<u8> = vec![
            Protocol18Tag::Dictionary as u8,
            Protocol18Tag::Int1 as u8,       // outer key: Int1
            Protocol18Tag::Dictionary as u8, // outer val: Dictionary
            Protocol18Tag::String as u8,     // inner key: String (consumed by outer header)
            Protocol18Tag::BooleanTrue as u8,// inner val: BooleanTrue (consumed by outer header)
            1,                               // outer len = 1
            5,                               // outer key = 5 (untagged Int1)
            // outer val: inner Dictionary repeats its type header (key=String, val=BooleanTrue)
            Protocol18Tag::String as u8,
            Protocol18Tag::BooleanTrue as u8,
            1,                               // inner len = 1
            2, b'o', b'k',                   // inner key = "ok"
            // inner val = BooleanTrue (0 bytes!)
        ];
        let mut r = Protocol18Reader::new(&nested_dict);
        let tag = r.read_u8().unwrap();
        let val = r.read_value_with_tag(tag, 0).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(
            val,
            Protocol16Value::Dictionary(vec![(
                Protocol16Value::Integer(5),
                Protocol16Value::Dictionary(vec![(
                    Protocol16Value::String("ok".into()),
                    Protocol16Value::Boolean(true),
                )]),
            )])
        );

        // Nested OperationRequest inside parameter table
        let mut req_params = ParameterTable::new();
        req_params.insert(1, Protocol16Value::Boolean(false));
        let req = OperationRequestPayload {
            operation_code: 41,
            parameters: req_params,
        };
        let mut outer = ParameterTable::new();
        outer.insert(9, Protocol16Value::OperationRequest(Box::new(req.clone())));
        let mut req_buf = Vec::new();
        encode_protocol18_parameter_table(&mut req_buf, &outer);
        let mut req_reader = Protocol18Reader::new(&req_buf);
        let dec_table = req_reader.read_parameter_table(0).unwrap();
        assert_eq!(dec_table, outer);

        // Oversized ByteArray rejected without panic/OOM
        let mut bad_bytes = vec![Protocol18Tag::ByteArray as u8];
        write_compressed_u32(&mut bad_bytes, (MAX_BYTE_ARRAY_LEN as u32) + 1);
        let mut bad_reader = Protocol18Reader::new(&bad_bytes);
        let bad_tag = bad_reader.read_u8().unwrap();
        assert_eq!(
            bad_reader.read_value_with_tag(bad_tag, 0),
            Err(Protocol18Error::CollectionTooLarge(MAX_BYTE_ARRAY_LEN + 1))
        );
    }
}
