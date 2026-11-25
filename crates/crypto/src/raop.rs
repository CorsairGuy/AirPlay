//! AirPlay 1 public-key wrapping. The public modulus is a protocol constant;
//! its bytes match OwnTone's RAOP public key at commit 612d6b3cf34ae38bb71b6edfcd69fd7afe6ab25c.
use rand::rngs::OsRng;
use rsa::{BigUint, Oaep, RsaPublicKey};
use sha1::Sha1;

const MODULUS: &[u8; 256] = include_bytes!("raop_public_modulus.bin");

/// Wrap a 128-bit session key with RSA-OAEP/SHA-1 for classic RAOP receivers.
pub fn wrap_key(key: &[u8; 16]) -> Result<Vec<u8>, rsa::errors::Error> {
    let public = RsaPublicKey::new(BigUint::from_bytes_be(MODULUS), BigUint::from(65537u32))?;
    public.encrypt(&mut OsRng, Oaep::new::<Sha1>(), key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;
    use sha2::{Digest, Sha256};

    #[test]
    fn protocol_modulus_matches_reference() {
        assert_eq!(
            hex::encode(Sha256::digest(MODULUS)),
            "bcab8f192eb454017f0cad661cb3da4b58ead76d09737eb8ad9dcd9a565791d0"
        );
    }

    #[test]
    fn oaep_sha1_round_trip_and_protocol_key_size() {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let key = [0x5au8; 16];
        let ciphertext = RsaPublicKey::from(&private)
            .encrypt(&mut OsRng, Oaep::new::<Sha1>(), &key)
            .unwrap();
        assert_eq!(
            private.decrypt(Oaep::new::<Sha1>(), &ciphertext).unwrap(),
            key
        );
        assert_eq!(wrap_key(&key).unwrap().len(), 256);
    }
}
