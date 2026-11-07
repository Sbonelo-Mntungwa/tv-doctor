//! Just enough protobuf to speak Cast and the Android TV remote protocol.
//!
//! Both protocols use a handful of small messages, so encoding them by hand is
//! shorter than any dependency would be.

pub fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        out.push(if value > 0 { byte | 0x80 } else { byte });
        if value == 0 {
            return out;
        }
    }
}

pub fn field_varint(number: u64, value: u64) -> Vec<u8> {
    let mut out = varint(number << 3);
    out.extend(varint(value));
    out
}

pub fn field_bytes(number: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = varint((number << 3) | 2);
    out.extend(varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

pub fn field_string(number: u64, text: &str) -> Vec<u8> {
    field_bytes(number, text.as_bytes())
}

pub fn read_varint(buffer: &[u8], index: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = *buffer.get(*index)?;
        *index += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// One decoded field. Fixed 32 and 64 bit values are kept as integers.
pub enum Value {
    Int(u64),
    Bytes(Vec<u8>),
}

/// Every field of one message, in order. None when the bytes do not parse.
pub fn fields(body: &[u8]) -> Option<Vec<(u64, Value)>> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < body.len() {
        let key = read_varint(body, &mut index)?;
        let number = key >> 3;
        match key & 7 {
            0 => out.push((number, Value::Int(read_varint(body, &mut index)?))),
            1 => {
                let chunk = body.get(index..index + 8)?;
                index += 8;
                let mut raw = [0u8; 8];
                raw.copy_from_slice(chunk);
                out.push((number, Value::Int(u64::from_le_bytes(raw))));
            }
            2 => {
                let length = read_varint(body, &mut index)? as usize;
                let chunk = body.get(index..index.checked_add(length)?)?;
                index += length;
                out.push((number, Value::Bytes(chunk.to_vec())));
            }
            5 => {
                let chunk = body.get(index..index + 4)?;
                index += 4;
                let mut raw = [0u8; 4];
                raw.copy_from_slice(chunk);
                out.push((number, Value::Int(u32::from_le_bytes(raw) as u64)));
            }
            _ => return None,
        }
    }
    Some(out)
}

pub fn get_int(fields: &[(u64, Value)], number: u64) -> Option<u64> {
    fields.iter().find_map(|(n, v)| match v {
        Value::Int(i) if *n == number => Some(*i),
        _ => None,
    })
}

pub fn get_bytes(fields: &[(u64, Value)], number: u64) -> Option<&[u8]> {
    fields.iter().find_map(|(n, v)| match v {
        Value::Bytes(b) if *n == number => Some(b.as_slice()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut message = field_varint(1, 300);
        message.extend(field_string(2, "hi"));
        let parsed = fields(&message).unwrap();
        assert_eq!(get_int(&parsed, 1), Some(300));
        assert_eq!(get_bytes(&parsed, 2), Some(&b"hi"[..]));
        assert_eq!(get_int(&parsed, 3), None);
    }

    #[test]
    fn truncated_is_rejected() {
        let message = field_string(2, "hello");
        assert!(fields(&message[..message.len() - 1]).is_none());
    }
}
