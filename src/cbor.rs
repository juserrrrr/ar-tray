//! The small part of CBOR (RFC 8949) the unit's local resources use, read into and written from
//! JSON values. Byte strings become base64 text, tags are skipped.

use base64::Engine;
use serde_json::{Map, Number, Value};

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn head(major: u8, length: u64, out: &mut Vec<u8>) {
    let major = major << 5;
    match length {
        0..=23 => out.push(major | length as u8),
        24..=0xff => out.extend([major | 24, length as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend((length as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend((length as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend(length.to_be_bytes());
        }
    }
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(0xf6),
        Value::Bool(flag) => out.push(if *flag { 0xf5 } else { 0xf4 }),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                head(0, unsigned, out);
            } else if let Some(signed) = number.as_i64() {
                head(1, (-1 - signed) as u64, out);
            } else {
                out.push(0xfb);
                out.extend(number.as_f64().unwrap_or(0.0).to_be_bytes());
            }
        }
        Value::String(text) => {
            head(3, text.len() as u64, out);
            out.extend(text.as_bytes());
        }
        Value::Array(items) => {
            head(4, items.len() as u64, out);
            items.iter().for_each(|item| write(item, out));
        }
        Value::Object(map) => {
            head(5, map.len() as u64, out);
            for (key, item) in map {
                head(3, key.len() as u64, out);
                out.extend(key.as_bytes());
                write(item, out);
            }
        }
    }
}

pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.item(0)?;
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

const BREAK: u8 = 0xff;

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(count).filter(|end| *end <= self.bytes.len()).ok_or("CBOR cortado")?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn argument(&mut self, info: u8) -> Result<Option<u64>, String> {
        Ok(Some(match info {
            0..=23 => info as u64,
            24 => self.take(1)?[0] as u64,
            25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
            26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            31 => return Ok(None),
            _ => return Err(format!("CBOR inválido: {info}")),
        }))
    }

    fn at_break(&mut self) -> bool {
        if self.bytes.get(self.at) == Some(&BREAK) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn text_chunks(&mut self, major: u8, length: Option<u64>) -> Result<Vec<u8>, String> {
        match length {
            Some(length) => Ok(self.take(length as usize)?.to_vec()),
            None => {
                let mut joined = Vec::new();
                while !self.at_break() {
                    let initial = self.take(1)?[0];
                    if initial >> 5 != major {
                        return Err("CBOR inválido: pedaço de texto".into());
                    }
                    let length = self.argument(initial & 31)?.ok_or("CBOR inválido: pedaço sem tamanho")?;
                    joined.extend(self.take(length as usize)?);
                }
                Ok(joined)
            }
        }
    }

    fn item(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 32 {
            return Err("CBOR fundo demais".into());
        }
        let initial = self.take(1)?[0];
        let (major, info) = (initial >> 5, initial & 31);
        Ok(match major {
            0 => Value::from(self.argument(info)?.ok_or("CBOR inválido")?),
            1 => Value::from(-1 - self.argument(info)?.ok_or("CBOR inválido")? as i128 as i64),
            2 => {
                let length = self.argument(info)?;
                Value::String(base64::engine::general_purpose::STANDARD.encode(self.text_chunks(2, length)?))
            }
            3 => {
                let length = self.argument(info)?;
                Value::String(String::from_utf8_lossy(&self.text_chunks(3, length)?).into_owned())
            }
            4 => {
                let mut items = Vec::new();
                match self.argument(info)? {
                    Some(count) => {
                        for _ in 0..count {
                            items.push(self.item(depth + 1)?);
                        }
                    }
                    None => {
                        while !self.at_break() {
                            items.push(self.item(depth + 1)?);
                        }
                    }
                }
                Value::Array(items)
            }
            5 => {
                let mut map = Map::new();
                let count = self.argument(info)?;
                let mut index = 0u64;
                loop {
                    match count {
                        Some(count) if index >= count => break,
                        None if self.at_break() => break,
                        _ => {}
                    }
                    let key = match self.item(depth + 1)? {
                        Value::String(text) => text,
                        other => other.to_string(),
                    };
                    map.insert(key, self.item(depth + 1)?);
                    index += 1;
                }
                Value::Object(map)
            }
            6 => {
                self.argument(info)?;
                self.item(depth + 1)?
            }
            _ => match info {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 | 23 => Value::Null,
                25 => float(half(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))),
                26 => float(f32::from_be_bytes(self.take(4)?.try_into().unwrap()) as f64),
                27 => float(f64::from_be_bytes(self.take(8)?.try_into().unwrap())),
                _ => Value::Null,
            },
        })
    }
}

fn float(number: f64) -> Value {
    Number::from_f64(number).map_or(Value::Null, Value::Number)
}

fn half(bits: u16) -> f64 {
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = (bits & 0x3ff) as f64;
    let magnitude = match exponent {
        0 => mantissa * 2f64.powi(-24),
        31 => f64::INFINITY,
        _ => (1.0 + mantissa / 1024.0) * 2f64.powi(exponent as i32 - 15),
    };
    if bits & 0x8000 != 0 { -magnitude } else { magnitude }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_the_shapes_the_unit_uses() {
        let value = json!({
            "x.com.samsung.da.items": [{ "x.com.samsung.da.id": "0", "x.com.samsung.da.desired": "25.0" }],
            "range": [16, 30], "temperature": 22.5, "negative": -3, "flag": true, "nothing": null,
        });
        assert_eq!(decode(&encode(&value)).unwrap(), value);
    }

    #[test]
    fn reads_halves_bytes_and_indefinite_lengths() {
        // {"t": 22.0 as a half float, "b": h'0102', "a": [_ 1, 2]}
        let bytes = [0xa3, 0x61, b't', 0xf9, 0x4d, 0x80, 0x61, b'b', 0x42, 1, 2, 0x61, b'a', 0x9f, 1, 2, 0xff];
        assert_eq!(decode(&bytes).unwrap(), json!({ "t": 22.0, "b": "AQI=", "a": [1, 2] }));
        assert!(decode(&[0x82, 1]).is_err());
    }
}
