//! Decoding of the PostgreSQL binary `numeric` wire format into its canonical
//! text form (what the text protocol would have sent), so that decimals keep
//! their scale exactly like in the Node.js driver (`'1.00'`).

use std::error::Error;

const NUMERIC_POS: u16 = 0x0000;
const NUMERIC_NEG: u16 = 0x4000;
const NUMERIC_NAN: u16 = 0xC000;
const NUMERIC_PINF: u16 = 0xD000;
const NUMERIC_NINF: u16 = 0xF000;

/// Converts a binary `numeric` into its text representation.
pub fn numeric_to_string(raw: &[u8]) -> Result<String, Box<dyn Error + Sync + Send>> {
    if raw.len() < 8 {
        return Err("invalid numeric: header too short".into());
    }
    let ndigits = u16::from_be_bytes([raw[0], raw[1]]) as usize;
    let weight = i16::from_be_bytes([raw[2], raw[3]]) as i32;
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let dscale = u16::from_be_bytes([raw[6], raw[7]]) as usize;

    if raw.len() < 8 + ndigits * 2 {
        return Err("invalid numeric: digits truncated".into());
    }
    let digits: Vec<u16> = (0..ndigits)
        .map(|i| u16::from_be_bytes([raw[8 + i * 2], raw[9 + i * 2]]))
        .collect();

    match sign {
        NUMERIC_NAN => return Ok("NaN".to_string()),
        NUMERIC_PINF => return Ok("Infinity".to_string()),
        NUMERIC_NINF => return Ok("-Infinity".to_string()),
        NUMERIC_POS | NUMERIC_NEG => {}
        other => return Err(format!("invalid numeric sign: {other:#x}").into()),
    }

    let digit_at = |d: i32| -> u16 {
        if d >= 0 && (d as usize) < ndigits {
            digits[d as usize]
        } else {
            0
        }
    };

    let mut out = String::new();
    if sign == NUMERIC_NEG {
        out.push('-');
    }

    if weight < 0 {
        out.push('0');
    } else {
        for d in 0..=weight {
            let dig = digit_at(d);
            if d == 0 {
                out.push_str(&dig.to_string());
            } else {
                out.push_str(&format!("{dig:04}"));
            }
        }
    }

    if dscale > 0 {
        out.push('.');
        let mut frac = String::new();
        let mut d = weight + 1;
        while frac.len() < dscale {
            frac.push_str(&format!("{:04}", digit_at(d)));
            d += 1;
        }
        frac.truncate(dscale);
        out.push_str(&frac);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(digits: &[u16], weight: i16, sign: u16, dscale: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(digits.len() as u16).to_be_bytes());
        out.extend_from_slice(&weight.to_be_bytes());
        out.extend_from_slice(&sign.to_be_bytes());
        out.extend_from_slice(&dscale.to_be_bytes());
        for d in digits {
            out.extend_from_slice(&d.to_be_bytes());
        }
        out
    }

    #[test]
    fn keeps_scale() {
        assert_eq!(
            numeric_to_string(&encode(&[1], 0, NUMERIC_POS, 2)).unwrap(),
            "1.00"
        );
        assert_eq!(
            numeric_to_string(&encode(&[1], 0, NUMERIC_POS, 0)).unwrap(),
            "1"
        );
        assert_eq!(
            numeric_to_string(&encode(&[], 0, NUMERIC_POS, 0)).unwrap(),
            "0"
        );
        assert_eq!(
            numeric_to_string(&encode(&[], 0, NUMERIC_POS, 3)).unwrap(),
            "0.000"
        );
    }

    #[test]
    fn multi_group_values() {
        // 12345.678 = groups [1][2345].[6780], weight 1
        assert_eq!(
            numeric_to_string(&encode(&[1, 2345, 6780], 1, NUMERIC_POS, 3)).unwrap(),
            "12345.678"
        );
        // 10000 = [1][0], weight 1, trailing zero group stripped by PG (ndigits = 1)
        assert_eq!(
            numeric_to_string(&encode(&[1], 1, NUMERIC_POS, 0)).unwrap(),
            "10000"
        );
        // -0.5 = [5000], weight -1
        assert_eq!(
            numeric_to_string(&encode(&[5000], -1, NUMERIC_NEG, 1)).unwrap(),
            "-0.5"
        );
        // 0.001 = [10], weight -1, dscale 3
        assert_eq!(
            numeric_to_string(&encode(&[10], -1, NUMERIC_POS, 3)).unwrap(),
            "0.001"
        );
        // 0.00001 = [1000], weight -2, dscale 5
        assert_eq!(
            numeric_to_string(&encode(&[1000], -2, NUMERIC_POS, 5)).unwrap(),
            "0.00001"
        );
        // 1234567.89 = [123][4567][8900] weight 1 dscale 2
        assert_eq!(
            numeric_to_string(&encode(&[123, 4567, 8900], 1, NUMERIC_POS, 2)).unwrap(),
            "1234567.89"
        );
    }

    #[test]
    fn specials() {
        assert_eq!(
            numeric_to_string(&encode(&[], 0, NUMERIC_NAN, 0)).unwrap(),
            "NaN"
        );
        assert_eq!(
            numeric_to_string(&encode(&[], 0, NUMERIC_PINF, 0)).unwrap(),
            "Infinity"
        );
        assert_eq!(
            numeric_to_string(&encode(&[], 0, NUMERIC_NINF, 0)).unwrap(),
            "-Infinity"
        );
        assert!(numeric_to_string(&[0, 0, 0]).is_err());
    }
}
