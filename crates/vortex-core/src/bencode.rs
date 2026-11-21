//! vortex-core/src/bencode.rs
//! Pure Rust Bencode (BEP-0003) decoder and encoder.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub enum Value {
    Integer(i64),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Integer(n) => write!(f, "Int({})", n),
            Value::Bytes(b) => {
                if let Ok(s) = std::str::from_utf8(b) {
                    write!(f, "Str({:?})", s)
                } else {
                    write!(f, "Bytes([len={}])", b.len())
                }
            }
            Value::List(l) => f.debug_list().entries(l).finish(),
            Value::Dict(d) => {
                let mut map = f.debug_map();
                for (k, v) in d {
                    let k_str = std::str::from_utf8(k).unwrap_or("<binary_key>");
                    map.entry(&k_str, v);
                }
                map.finish()
            }
        }
    }
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Integer(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Bytes(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l.as_slice()),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn dict_get(&self, key: &str) -> Option<&Value> {
        self.as_dict()?.get(key.as_bytes())
    }

    pub fn dict_get_str(&self, key: &str) -> Option<&str> {
        self.dict_get(key)?.as_str()
    }

    pub fn dict_get_int(&self, key: &str) -> Option<i64> {
        self.dict_get(key)?.as_int()
    }

    /// Serializes value into canonical Bencoded byte representation.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Value::Integer(n) => {
                out.push(b'i');
                out.extend_from_slice(n.to_string().as_bytes());
                out.push(b'e');
            }
            Value::Bytes(b) => {
                out.extend_from_slice(b.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(b);
            }
            Value::List(l) => {
                out.push(b'l');
                for item in l {
                    item.encode_into(out);
                }
                out.push(b'e');
            }
            Value::Dict(d) => {
                out.push(b'd');
                // BTreeMap iterates keys in sorted order, as required by BitTorrent specification
                for (k, v) in d {
                    out.extend_from_slice(k.len().to_string().as_bytes());
                    out.push(b':');
                    out.extend_from_slice(k);
                    v.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }

    /// Parses Bencoded byte slice into Value tree.
    pub fn decode(bytes: &[u8]) -> Result<Self, BencodeError> {
        let (val, rest) = Parser::parse_value(bytes)?;
        if !rest.is_empty() {
            return Err(BencodeError::TrailingData(rest.len()));
        }
        Ok(val)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BencodeError {
    UnexpectedEnd,
    InvalidPrefix(u8),
    InvalidInteger,
    InvalidStringLength,
    InvalidDictionaryKey,
    UnsortedDictionaryKeys,
    TrailingData(usize),
}

impl fmt::Display for BencodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BencodeError::UnexpectedEnd => write!(f, "Unexpected end of bencoded input"),
            BencodeError::InvalidPrefix(b) => write!(f, "Invalid bencode prefix byte: 0x{:02x}", b),
            BencodeError::InvalidInteger => write!(f, "Malformed integer"),
            BencodeError::InvalidStringLength => write!(f, "Malformed byte string length"),
            BencodeError::InvalidDictionaryKey => write!(f, "Dictionary key must be byte string"),
            BencodeError::UnsortedDictionaryKeys => write!(f, "Dictionary keys are not sorted"),
            BencodeError::TrailingData(n) => write!(f, "Trailing data remaining: {} bytes", n),
        }
    }
}

impl std::error::Error for BencodeError {}

struct Parser;

impl Parser {
    fn parse_value(input: &[u8]) -> Result<(Value, &[u8]), BencodeError> {
        if input.is_empty() {
            return Err(BencodeError::UnexpectedEnd);
        }

        match input[0] {
            b'i' => Self::parse_integer(input),
            b'l' => Self::parse_list(input),
            b'd' => Self::parse_dict(input),
            b'0'..=b'9' => Self::parse_bytes(input),
            b => Err(BencodeError::InvalidPrefix(b)),
        }
    }

    fn parse_integer(input: &[u8]) -> Result<(Value, &[u8]), BencodeError> {
        let end = input.iter().position(|&b| b == b'e').ok_or(BencodeError::UnexpectedEnd)?;
        let num_str = std::str::from_utf8(&input[1..end]).map_err(|_| BencodeError::InvalidInteger)?;
        if (num_str.starts_with('0') && num_str.len() > 1) || num_str == "-0" {
            return Err(BencodeError::InvalidInteger);
        }
        let num = num_str.parse::<i64>().map_err(|_| BencodeError::InvalidInteger)?;
        Ok((Value::Integer(num), &input[end + 1..]))
    }

    fn parse_bytes(input: &[u8]) -> Result<(Value, &[u8]), BencodeError> {
        let colon = input.iter().position(|&b| b == b':').ok_or(BencodeError::UnexpectedEnd)?;
        let len_str = std::str::from_utf8(&input[..colon]).map_err(|_| BencodeError::InvalidStringLength)?;
        let len = len_str.parse::<usize>().map_err(|_| BencodeError::InvalidStringLength)?;

        let start = colon + 1;
        if input.len() < start + len {
            return Err(BencodeError::UnexpectedEnd);
        }

        let slice = input[start..start + len].to_vec();
        Ok((Value::Bytes(slice), &input[start + len..]))
    }

    fn parse_list(input: &[u8]) -> Result<(Value, &[u8]), BencodeError> {
        let mut cur = &input[1..];
        let mut list = Vec::new();

        while !cur.is_empty() && cur[0] != b'e' {
            let (item, rest) = Self::parse_value(cur)?;
            list.push(item);
            cur = rest;
        }

        if cur.is_empty() {
            return Err(BencodeError::UnexpectedEnd);
        }

        Ok((Value::List(list), &cur[1..]))
    }

    fn parse_dict(input: &[u8]) -> Result<(Value, &[u8]), BencodeError> {
        let mut cur = &input[1..];
        let mut dict = BTreeMap::new();
        let mut prev_key: Option<Vec<u8>> = None;

        while !cur.is_empty() && cur[0] != b'e' {
            let (key_val, rest_after_key) = Self::parse_value(cur)?;
            let key = match key_val {
                Value::Bytes(k) => k,
                _ => return Err(BencodeError::InvalidDictionaryKey),
            };

            if let Some(prev) = prev_key {
                if key <= prev {
                    return Err(BencodeError::UnsortedDictionaryKeys);
                }
            }

            let (val, rest_after_val) = Self::parse_value(rest_after_key)?;
            prev_key = Some(key.clone());
            dict.insert(key, val);
            cur = rest_after_val;
        }

        if cur.is_empty() {
            return Err(BencodeError::UnexpectedEnd);
        }

        Ok((Value::Dict(dict), &cur[1..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bencode_primitives() {
        // Integer
        let encoded_int = Value::Integer(42).encode();
        assert_eq!(encoded_int, b"i42e");
        let decoded_int = Value::decode(&encoded_int).unwrap();
        assert_eq!(decoded_int, Value::Integer(42));

        // Bytes
        let encoded_str = Value::Bytes(b"spam".to_vec()).encode();
        assert_eq!(encoded_str, b"4:spam");
        let decoded_str = Value::decode(&encoded_str).unwrap();
        assert_eq!(decoded_str, Value::Bytes(b"spam".to_vec()));

        // List
        let list = Value::List(vec![Value::Bytes(b"spam".to_vec()), Value::Integer(42)]);
        let encoded_list = list.encode();
        assert_eq!(encoded_list, b"l4:spami42ee");
        assert_eq!(Value::decode(&encoded_list).unwrap(), list);

        // Dict
        let mut map = BTreeMap::new();
        map.insert(b"bar".to_vec(), Value::Bytes(b"spam".to_vec()));
        map.insert(b"foo".to_vec(), Value::Integer(42));
        let dict = Value::Dict(map);
        let encoded_dict = dict.encode();
        assert_eq!(encoded_dict, b"d3:bar4:spam3:fooi42ee");
        assert_eq!(Value::decode(&encoded_dict).unwrap(), dict);
    }
}
