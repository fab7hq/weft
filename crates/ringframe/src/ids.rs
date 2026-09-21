//! Opaque, time-ordered identifiers: `<prefix>_<26 Crockford base32 chars>`.

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn new_id(prefix: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is before 1970")
        .as_millis();
    let mut value = (millis << 80) | u128::from(random_u80());
    let mut chars = [0u8; 26];
    for slot in chars.iter_mut().rev() {
        *slot = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    format!("{prefix}_{}", std::str::from_utf8(&chars).expect("base32 is ASCII"))
}

/// Eighty bits of randomness below the millisecond, so two ids minted in the
/// same millisecond still differ.
fn random_u80() -> u128 {
    let mut bytes = [0u8; 10];
    let mut file = std::fs::File::open("/dev/urandom").expect("/dev/urandom");
    std::io::Read::read_exact(&mut file, &mut bytes).expect("/dev/urandom");
    bytes.iter().fold(0u128, |acc, b| (acc << 8) | u128::from(*b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_shape_and_prefix() {
        let a = new_id("ask");
        let (prefix, body) = a.split_once('_').expect("a prefix");
        assert_eq!(prefix, "ask");
        assert_eq!(body.len(), 26);
        assert!(
            body.bytes().all(|c| ALPHABET.contains(&c)),
            "not Crockford base32: {body}"
        );
    }

    #[test]
    fn ids_unique_and_time_ordered() {
        let first = new_id("evt");
        let seen: std::collections::HashSet<String> =
            (0..2000).map(|_| new_id("evt")).collect();
        assert_eq!(seen.len(), 2000);
        // The timestamp prefix never goes backwards.
        for s in &seen {
            assert!(first[..4 + 9] <= s[..4 + 9], "{first} then {s}");
        }
    }
}

/// Random hex, for a temporary name nobody else will pick.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    let mut file = std::fs::File::open("/dev/urandom").expect("/dev/urandom");
    std::io::Read::read_exact(&mut file, &mut buf).expect("/dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
