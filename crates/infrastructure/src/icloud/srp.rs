//! Apple's SRP-6a variant (RFC 5054 2048-bit group, SHA-256), as used by
//! idmsa.apple.com's `/signin/init` + `/signin/complete`.
//!
//! Differences from textbook SRP-6a, all matching rclone/pyicloud:
//! * the password is first run through `derive_password` (SHA-256, then
//!   PBKDF2-HMAC-SHA256 with the server's salt and iteration count);
//! * `x = H(salt | H(":" | derived))` — the username is NOT in the inner hash;
//! * `M1 = H(H(g) ^ H(N) | H(username) | salt | A | B | K)` with `A`, `B`
//!   padded to the group size and `H(g)` over the padded `g`.

use num_bigint::BigUint;
use num_traits::Zero;
use sha2::{Digest, Sha256};
use thiserror::Error;

const N_HEX: &str = "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050\
A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50\
E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8\
55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B\
CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748\
544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB37861602790\
04E57AE6AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8\
E9DBFBB694B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F\
9E4AFF73";
const N_LEN_BYTES: usize = 2048 / 8;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SrpError {
    #[error("unsupported SRP protocol {0:?}")]
    UnsupportedProtocol(String),
    #[error("server public value B out of range")]
    InvalidServerB,
    #[error("scrambling parameter u is zero")]
    ZeroU,
}

fn n() -> BigUint {
    // The constant is a well-formed hex literal; a parse failure is a build
    // defect, so degrade to zero and let the range checks fail loudly.
    BigUint::parse_bytes(N_HEX.as_bytes(), 16).unwrap_or_default()
}

fn g() -> BigUint {
    BigUint::from(2u8)
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Big-endian bytes left-padded to the 256-byte group size.
fn pad_to_n(v: &BigUint) -> Vec<u8> {
    let b = v.to_bytes_be();
    if b.len() >= N_LEN_BYTES {
        return b;
    }
    let mut out = vec![0u8; N_LEN_BYTES - b.len()];
    out.extend_from_slice(&b);
    out
}

/// `k = H(N | pad(g))` where `g` is padded to the byte length of `N`.
fn multiplier() -> BigUint {
    let n_bytes = n().to_bytes_be();
    let mut g_bytes = g().to_bytes_be();
    while g_bytes.len() < n_bytes.len() {
        g_bytes.insert(0, 0);
    }
    BigUint::from_bytes_be(&sha256(&[&n_bytes, &g_bytes]))
}

/// Apple's password pre-processing before the SRP proof.
/// `s2k`: PBKDF2(SHA256(password) as raw bytes); `s2k_fo`: PBKDF2(hex of it).
pub fn derive_password(
    password: &str,
    salt: &[u8],
    iterations: u32,
    protocol: &str,
) -> Result<[u8; 32], SrpError> {
    let hashed = sha256(&[password.as_bytes()]);
    let input: Vec<u8> = match protocol {
        "s2k" => hashed.to_vec(),
        "s2k_fo" => hex(&hashed).into_bytes(),
        other => return Err(SrpError::UnsupportedProtocol(other.to_string())),
    };
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(&input, salt, iterations, &mut out);
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Client proofs to send in `/signin/complete`.
#[derive(Debug, PartialEq, Eq)]
pub struct SrpProof {
    pub m1: Vec<u8>,
    pub m2: Vec<u8>,
}

/// One SRP exchange: holds the client secret `a` and public `A`.
pub struct SrpClient {
    a: BigUint,
    big_a: BigUint,
}

impl SrpClient {
    /// Fresh random 32-byte secret.
    pub fn new() -> Self {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        Self::from_secret(&secret)
    }

    /// Deterministic construction (tests and vectors).
    pub fn from_secret(secret: &[u8]) -> Self {
        let a = BigUint::from_bytes_be(secret);
        let big_a = g().modpow(&a, &n());
        Self { a, big_a }
    }

    /// `A`, padded to the group size — the `a` field of `/signin/init`.
    pub fn public_a(&self) -> Vec<u8> {
        pad_to_n(&self.big_a)
    }

    /// Run the challenge: `username` is the lower-cased Apple ID, `derived`
    /// the output of [`derive_password`], `salt` and `server_b` the decoded
    /// values from `/signin/init`.
    pub fn process_challenge(
        &self,
        username: &[u8],
        derived: &[u8],
        salt: &[u8],
        server_b: &[u8],
    ) -> Result<SrpProof, SrpError> {
        let n = n();
        let big_b = BigUint::from_bytes_be(server_b);
        if big_b.is_zero() || big_b >= n {
            return Err(SrpError::InvalidServerB);
        }

        // x = H(salt | H(":" | derived))  — no username (Apple's NoUserNameInX)
        let inner = sha256(&[b":", derived]);
        let x = BigUint::from_bytes_be(&sha256(&[salt, &inner]));

        let a_pad = pad_to_n(&self.big_a);
        let b_pad = pad_to_n(&big_b);
        let u = BigUint::from_bytes_be(&sha256(&[&a_pad, &b_pad]));
        if u.is_zero() {
            return Err(SrpError::ZeroU);
        }

        // S = (B - k * g^x) ^ (a + u * x) mod N, computed without going negative.
        let k = multiplier();
        let kgx = (k * g().modpow(&x, &n)) % &n;
        let base = (&big_b + &n - kgx) % &n;
        let exp = &self.a + u * x;
        let s = base.modpow(&exp, &n);
        let big_k = sha256(&[&pad_to_n(&s)]);

        // M1 = H(H(pad(g)) ^ H(N) | H(username) | salt | A | B | K)
        let hg = sha256(&[&pad_to_n(&g())]);
        let hn = sha256(&[&n.to_bytes_be()]);
        let hxor: Vec<u8> = hg.iter().zip(hn.iter()).map(|(p, q)| p ^ q).collect();
        let hi = sha256(&[username]);
        let m1 = sha256(&[&hxor, &hi, salt, &a_pad, &b_pad, &big_k]).to_vec();
        let m2 = sha256(&[&a_pad, &m1, &big_k]).to_vec();
        Ok(SrpProof { m1, m2 })
    }
}

impl Default for SrpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from an independent Python reference of the same algorithm
    // (hashlib + pow), fixed a = 0x11*32, salt = 0..16, B = g^(0x22*32) mod N,
    // username "someone@example.com", password "correct horse battery staple",
    // 20000 iterations.
    fn fixture() -> (SrpClient, Vec<u8>, Vec<u8>) {
        let client = SrpClient::from_secret(&[0x11u8; 32]);
        let salt: Vec<u8> = (0u8..16).collect();
        let b = g().modpow(&BigUint::from_bytes_be(&[0x22u8; 32]), &n());
        (client, salt, pad_to_n(&b))
    }

    #[test]
    fn multiplier_matches_reference() {
        assert_eq!(
            multiplier().to_str_radix(16),
            "5b9e8ef059c6b32ea59fc1d322d37f04aa30bae5aa9003b8321e21ddb04e300"
        );
    }

    #[test]
    fn public_a_is_padded_and_matches_reference() {
        let (client, _, _) = fixture();
        let a = client.public_a();
        assert_eq!(a.len(), 256);
        let h = hex(&a);
        assert!(h.starts_with("24d1e3e550122e1dc571bcefd01f494d"), "{h}");
        assert!(h.ends_with("d4662155f89b5d0f"), "{h}");
    }

    #[test]
    fn derive_password_both_protocols_match_reference() {
        let salt: Vec<u8> = (0u8..16).collect();
        let s2k = derive_password("correct horse battery staple", &salt, 20000, "s2k").unwrap();
        assert_eq!(
            hex(&s2k),
            "acadb60b74c60e9faee03393f08584f322e7c0a456db31c7f11276f736599b27"
        );
        let fo = derive_password("correct horse battery staple", &salt, 20000, "s2k_fo").unwrap();
        assert_eq!(
            hex(&fo),
            "272cfa7988c03b2100d3dcd40767c3d48df3b58ea2a9af2b2043e1ab88656c2b"
        );
        assert_eq!(
            derive_password("x", &salt, 1, "bogus"),
            Err(SrpError::UnsupportedProtocol("bogus".into()))
        );
    }

    #[test]
    fn proofs_match_reference_for_both_protocols() {
        let (client, salt, b) = fixture();
        let user = b"someone@example.com";
        let dk = derive_password("correct horse battery staple", &salt, 20000, "s2k").unwrap();
        let proof = client.process_challenge(user, &dk, &salt, &b).unwrap();
        assert_eq!(
            hex(&proof.m1),
            "6d45dba6b6207a3be93c147527f8f736d2a73b3832010a5eee8f7fe16088fd2d"
        );
        assert_eq!(
            hex(&proof.m2),
            "bb6d40ec972f14018e082c14c7610dc4c308d733b42452fade46e8cb2bc6cb43"
        );
        let dk = derive_password("correct horse battery staple", &salt, 20000, "s2k_fo").unwrap();
        let proof = client.process_challenge(user, &dk, &salt, &b).unwrap();
        assert_eq!(
            hex(&proof.m1),
            "0a350fa7083a4a1e05e22dff4acd2d0e9f19ec13093b60d3d150a5e25efff454"
        );
        assert_eq!(
            hex(&proof.m2),
            "cb14cb4d7d6f49e5074f8ff9035cc6283428974d9bb81bf34c95cfeb0f101adf"
        );
    }

    #[test]
    fn rejects_out_of_range_server_values() {
        let (client, salt, _) = fixture();
        let dk = [0u8; 32];
        assert_eq!(
            client.process_challenge(b"u", &dk, &salt, &[0u8; 256]),
            Err(SrpError::InvalidServerB)
        );
        assert_eq!(
            client.process_challenge(b"u", &dk, &salt, &n().to_bytes_be()),
            Err(SrpError::InvalidServerB)
        );
    }

    #[test]
    fn a_wrong_password_changes_the_proof() {
        let (client, salt, b) = fixture();
        let right = derive_password("correct horse battery staple", &salt, 20000, "s2k").unwrap();
        let wrong = derive_password("correct horse battery stable", &salt, 20000, "s2k").unwrap();
        let p1 = client.process_challenge(b"u", &right, &salt, &b).unwrap();
        let p2 = client.process_challenge(b"u", &wrong, &salt, &b).unwrap();
        assert_ne!(p1, p2);
    }
}
