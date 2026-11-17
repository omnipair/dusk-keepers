//! Read-only transaction decoding, including v0 loaded account addresses.
use serde_json::Value;

#[derive(Clone)]
pub(super) struct Instruction {
    pub program: String,
    pub accounts: Vec<String>,
    pub data: Vec<u8>,
    pub path: String,
}

pub(super) struct Transaction {
    pub keys: Vec<String>,
    pub instructions: Vec<Instruction>,
}

struct Cursor<'a>(&'a [u8]);
impl Cursor<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], String> {
        if len > self.0.len() {
            return Err("truncated transaction receipt".into());
        }
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> Result<usize, String> {
        let mut value = 0;
        for shift in [0, 7, 14] {
            let b = self.byte()?;
            if shift == 14 && b > 3 {
                return Err("invalid compact length".into());
            }
            value |= usize::from(b & 127) << shift;
            if b & 128 == 0 {
                return Ok(value);
            }
        }
        Err("invalid compact length".into())
    }
}

pub(super) fn decode(bytes: &[u8], meta: &Value) -> Result<Transaction, String> {
    let mut cursor = Cursor(bytes);
    let signatures = cursor.short()?;
    cursor.take(signatures * 64)?;
    let first = cursor.byte()?;
    let versioned = first & 128 != 0;
    if versioned {
        if first != 128 {
            return Err("unsupported transaction version".into());
        }
        cursor.take(3)?;
    } else {
        cursor.take(2)?;
    }
    let key_count = cursor.short()?;
    if key_count == 0 || key_count > 256 {
        return Err("invalid receipt account count".into());
    }
    let mut keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        keys.push(bs58::encode(cursor.take(32)?).into_string());
    }
    cursor.take(32)?;
    let count = cursor.short()?;
    let mut compiled = Vec::new();
    for _ in 0..count {
        let program = cursor.byte()?;
        let len = cursor.short()?;
        let accounts = cursor.take(len)?.to_vec();
        let len = cursor.short()?;
        let data = cursor.take(len)?.to_vec();
        compiled.push((program, accounts, data));
    }
    if versioned {
        let tables = cursor.short()?;
        let mut writable = 0;
        let mut readonly = 0;
        for _ in 0..tables {
            cursor.take(32)?;
            let count = cursor.short()?;
            cursor.take(count)?;
            writable += count;
            let count = cursor.short()?;
            cursor.take(count)?;
            readonly += count;
        }
        for (name, expected) in [("writable", writable), ("readonly", readonly)] {
            let addresses = meta["loadedAddresses"][name].as_array();
            if addresses.map_or(0, Vec::len) != expected {
                return Err("receipt omits loaded lookup addresses".into());
            }
            for value in addresses.into_iter().flatten() {
                let key = value.as_str().ok_or("invalid loaded address")?;
                if bs58::decode(key)
                    .into_vec()
                    .map_err(|e| e.to_string())?
                    .len()
                    != 32
                {
                    return Err("invalid loaded address".into());
                }
                keys.push(key.to_owned());
            }
        }
    }
    if !cursor.0.is_empty() || keys.len() > 256 {
        return Err("invalid transaction receipt suffix".into());
    }
    let key = |index: usize| {
        keys.get(index)
            .cloned()
            .ok_or_else(|| "receipt account index out of range".to_owned())
    };
    let mut instructions = Vec::new();
    for (index, (program, accounts, data)) in compiled.into_iter().enumerate() {
        instructions.push(Instruction {
            program: key(usize::from(program))?,
            accounts: accounts
                .into_iter()
                .map(|i| key(usize::from(i)))
                .collect::<Result<_, _>>()?,
            data,
            path: index.to_string(),
        });
        for group in meta["innerInstructions"].as_array().into_iter().flatten() {
            if group["index"].as_u64() != Some(index as u64) {
                continue;
            }
            for (inner, ix) in group["instructions"]
                .as_array()
                .ok_or("invalid inner instructions")?
                .iter()
                .enumerate()
            {
                instructions.push(Instruction {
                    program: key(ix["programIdIndex"]
                        .as_u64()
                        .ok_or("missing inner program")?
                        as usize)?,
                    accounts: ix["accounts"]
                        .as_array()
                        .ok_or("missing inner accounts")?
                        .iter()
                        .map(|v| key(v.as_u64().ok_or("invalid inner account")? as usize))
                        .collect::<Result<_, _>>()?,
                    data: bs58::decode(ix["data"].as_str().ok_or("missing inner data")?)
                        .into_vec()
                        .map_err(|e| e.to_string())?,
                    path: format!("{index}.{inner}"),
                });
            }
        }
    }
    Ok(Transaction { keys, instructions })
}
