//! SRP-6a, как в HAP: группа 3072 бит из RFC 5054, g = 5, SHA-512,
//! имя пользователя `Pair-Setup`. Код общий для любой группы и хеша,
//! чтобы проверять его на эталонных векторах.

use num_bigint::BigUint;
use sha2::Digest;
use sha2::Sha512;

use crate::Error;

const N_3072_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB",
    "9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF695581718",
    "3995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7",
    "ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E2",
    "08E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF",
);

/// Группа SRP: модуль `n` и генератор `g`.
#[derive(Clone)]
pub struct Group {
    n: BigUint,
    g: BigUint,
}

impl Group {
    /// Группа 3072 бит из RFC 5054, которую использует HAP.
    pub fn rfc5054_3072() -> Self {
        Self::new(N_3072_HEX, 5)
    }

    pub fn new(n_hex: &str, g: u32) -> Self {
        Self {
            n: BigUint::parse_bytes(n_hex.as_bytes(), 16).expect("корректный модуль"),
            g: BigUint::from(g),
        }
    }

    fn len(&self) -> usize {
        self.n.to_bytes_be().len()
    }

    /// Число в big-endian, дополненное нулями слева до длины модуля.
    fn pad(&self, v: &BigUint) -> Vec<u8> {
        let bytes = v.to_bytes_be();
        let mut out = vec![0u8; self.len().saturating_sub(bytes.len())];
        out.extend_from_slice(&bytes);
        out
    }
}

fn hash<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
    let mut d = D::new();
    for p in parts {
        d.update(p);
    }
    d.finalize().to_vec()
}

fn num(bytes: &[u8]) -> BigUint {
    BigUint::from_bytes_be(bytes)
}

/// Общие вычисления для обеих сторон.
struct Params<D> {
    group: Group,
    _d: std::marker::PhantomData<D>,
}

impl<D: Digest> Params<D> {
    fn k(&self) -> BigUint {
        num(&hash::<D>(&[
            &self.group.n.to_bytes_be(),
            &self.group.pad(&self.group.g),
        ]))
    }

    fn x(&self, username: &[u8], password: &[u8], salt: &[u8]) -> BigUint {
        let inner = hash::<D>(&[username, b":", password]);
        num(&hash::<D>(&[salt, &inner]))
    }

    fn u(&self, a: &BigUint, b: &BigUint) -> BigUint {
        num(&hash::<D>(&[&self.group.pad(a), &self.group.pad(b)]))
    }

    /// M1 = H(H(N) xor H(g) | H(I) | s | A | B | K)
    fn m1(&self, username: &[u8], salt: &[u8], a: &BigUint, b: &BigUint, key: &[u8]) -> Vec<u8> {
        let hn = hash::<D>(&[&self.group.n.to_bytes_be()]);
        let hg = hash::<D>(&[&self.group.g.to_bytes_be()]);
        let xor: Vec<u8> = hn.iter().zip(&hg).map(|(x, y)| x ^ y).collect();
        hash::<D>(&[
            &xor,
            &hash::<D>(&[username]),
            salt,
            &a.to_bytes_be(),
            &b.to_bytes_be(),
            key,
        ])
    }

    /// M2 = H(A | M1 | K)
    fn m2(&self, a: &BigUint, m1: &[u8], key: &[u8]) -> Vec<u8> {
        hash::<D>(&[&a.to_bytes_be(), m1, key])
    }
}

/// Результат успешного обмена: общий ключ сессии `K` и доказательство M2.
pub struct Verified {
    pub session_key: Vec<u8>,
    pub server_proof: Vec<u8>,
}

/// Сторона сервера (приёмника).
pub struct Server<D = Sha512> {
    params: Params<D>,
    username: Vec<u8>,
    salt: Vec<u8>,
    v: BigUint,
    b: BigUint,
    pub_b: BigUint,
}

impl<D: Digest> Server<D> {
    /// `private` — случайные байты секретного `b` (рекомендуется 32–64 байта).
    pub fn new(
        group: Group,
        username: &[u8],
        password: &[u8],
        salt: &[u8],
        private: &[u8],
    ) -> Self {
        let params = Params::<D> {
            group,
            _d: std::marker::PhantomData,
        };
        let n = &params.group.n;
        let x = params.x(username, password, salt);
        let v = params.group.g.modpow(&x, n);
        let b = num(private);
        let pub_b = (params.k() * &v + params.group.g.modpow(&b, n)) % n;
        Self {
            params,
            username: username.to_vec(),
            salt: salt.to_vec(),
            v,
            b,
            pub_b,
        }
    }

    pub fn salt(&self) -> &[u8] {
        &self.salt
    }

    /// Публичный ключ B, дополненный до длины модуля.
    pub fn public_key(&self) -> Vec<u8> {
        self.params.group.pad(&self.pub_b)
    }

    /// Проверяет A и доказательство клиента M1.
    pub fn verify(&self, client_public: &[u8], client_proof: &[u8]) -> Result<Verified, Error> {
        let n = &self.params.group.n;
        let a = num(client_public);
        if (&a % n) == BigUint::ZERO {
            return Err(Error::Srp("A mod N = 0"));
        }
        let u = self.params.u(&a, &self.pub_b);
        if u == BigUint::ZERO {
            return Err(Error::Srp("u = 0"));
        }
        let s = (&a * self.v.modpow(&u, n)).modpow(&self.b, n);
        let key = hash::<D>(&[&s.to_bytes_be()]);
        let m1 = self
            .params
            .m1(&self.username, &self.salt, &a, &self.pub_b, &key);
        if m1 != client_proof {
            return Err(Error::Srp("неверное доказательство клиента (PIN)"));
        }
        let m2 = self.params.m2(&a, &m1, &key);
        Ok(Verified {
            session_key: key,
            server_proof: m2,
        })
    }
}

/// Результат клиентского шага: A, M1 и ожидаемое M2.
pub struct ClientProof {
    pub public_key: Vec<u8>,
    pub proof: Vec<u8>,
    pub session_key: Vec<u8>,
    expected_server_proof: Vec<u8>,
}

impl ClientProof {
    pub fn verify_server(&self, server_proof: &[u8]) -> Result<(), Error> {
        if server_proof == self.expected_server_proof {
            Ok(())
        } else {
            Err(Error::Srp("неверное доказательство сервера"))
        }
    }
}

/// Сторона клиента (отправителя).
pub struct Client<D = Sha512> {
    params: Params<D>,
    username: Vec<u8>,
    password: Vec<u8>,
    a: BigUint,
}

impl<D: Digest> Client<D> {
    pub fn new(group: Group, username: &[u8], password: &[u8], private: &[u8]) -> Self {
        Self {
            params: Params {
                group,
                _d: std::marker::PhantomData,
            },
            username: username.to_vec(),
            password: password.to_vec(),
            a: num(private),
        }
    }

    pub fn process(&self, salt: &[u8], server_public: &[u8]) -> Result<ClientProof, Error> {
        let n = &self.params.group.n;
        let g = &self.params.group.g;
        let b = num(server_public);
        if (&b % n) == BigUint::ZERO {
            return Err(Error::Srp("B mod N = 0"));
        }
        let pub_a = g.modpow(&self.a, n);
        let u = self.params.u(&pub_a, &b);
        let x = self.params.x(&self.username, &self.password, salt);
        let kgx = (self.params.k() * g.modpow(&x, n)) % n;
        let base = (b + n - kgx) % n;
        let s = base.modpow(&(&self.a + u * x), n);
        let key = hash::<D>(&[&s.to_bytes_be()]);
        let m1 = self
            .params
            .m1(&self.username, salt, &pub_a, &num(server_public), &key);
        let m2 = self.params.m2(&pub_a, &m1, &key);
        Ok(ClientProof {
            public_key: self.params.group.pad(&pub_a),
            proof: m1,
            session_key: key,
            expected_server_proof: m2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_and_server_agree() {
        let salt = [0x5au8; 16];
        let server = Server::<Sha512>::new(
            Group::rfc5054_3072(),
            b"Pair-Setup",
            b"3939",
            &salt,
            &[0x42; 64],
        );
        let client =
            Client::<Sha512>::new(Group::rfc5054_3072(), b"Pair-Setup", b"3939", &[0x24; 64]);
        let proof = client.process(server.salt(), &server.public_key()).unwrap();
        let ok = server.verify(&proof.public_key, &proof.proof).unwrap();
        assert_eq!(ok.session_key, proof.session_key);
        proof.verify_server(&ok.server_proof).unwrap();
    }

    #[test]
    fn wrong_pin_is_rejected() {
        let salt = [0x5au8; 16];
        let server = Server::<Sha512>::new(
            Group::rfc5054_3072(),
            b"Pair-Setup",
            b"1234",
            &salt,
            &[0x42; 64],
        );
        let client =
            Client::<Sha512>::new(Group::rfc5054_3072(), b"Pair-Setup", b"1111", &[0x24; 64]);
        let proof = client.process(server.salt(), &server.public_key()).unwrap();
        assert!(server.verify(&proof.public_key, &proof.proof).is_err());
    }

    /// Вектор, посчитанный независимой реализацией (Python srptools 1.0.1,
    /// её же использует pyatv): соль 0x5a×16, b = 0x42×64, a = 0x24×64, PIN 3939.
    #[test]
    fn matches_srptools_vector() {
        let salt = [0x5au8; 16];
        let server = Server::<Sha512>::new(
            Group::rfc5054_3072(),
            b"Pair-Setup",
            b"3939",
            &salt,
            &[0x42; 64],
        );
        let client =
            Client::<Sha512>::new(Group::rfc5054_3072(), b"Pair-Setup", b"3939", &[0x24; 64]);
        let proof = client.process(&salt, &server.public_key()).unwrap();
        let ok = server.verify(&proof.public_key, &proof.proof).unwrap();
        let h = |s: &str| hex::decode(s).unwrap();
        assert_eq!(
            server.public_key(),
            h(concat!(
                "1ab6e13000a176cc8de5099aed7c70237cf631f0d42f4d1456cce3a29daf79dd9e5a2b37d5e0b528bd03ec1a187ecb61",
                "1d65e9b926aa8f7f305de10eeef1c84e33731f000d88b5758785352b81f7f1042d8e63488a0fd24042a73e477c6f71ff",
                "a752799bd4dc9d5041fccbdc2e94be6052354a1ae885740d57839dd965ee6e123e684114f970f50ecb8403a2984e042d",
                "6bcb8b92b52341c095c0c87c10707ac31ce7711fb6c92e4e2920917118ca4db3e507b23ee50b8d0bbac6caaa643e1128",
                "87dfa615a22170a8eac1be86fe01c4b31f82461d7042ccbe903679c6638fb666313e5f77933a432b79fa7a3975db39e7",
                "80c8eb7678bf5f1abab648454c8a4f3dfb994a5b6609c230a219fa8bf529b78c9f31ae6c839f62563462f4eddc14054d",
                "27d45bca3dc0521b81760ef4bf4ae7d86a2dab8551d1e19c7b5da6e9728bf61245dc94b3fe213b1cd4b590a7369beaf1",
                "4734f4c49500822ec172d53210f584cfacdbf65279321e07f5bbf48a3f9dba7ff9457053e508da008fb011b75ca24029",
            ))
        );
        assert_eq!(
            proof.public_key,
            h(concat!(
                "5176a3c3ec98100b9a18fbe82e346c5460b67d9766e91534cd6cd9ad92cf25ddc2cf0b163cf710667ebad8b8ddfd80ca",
                "2fa20fd9c6c4d32aed78edc6c1c94ac94d87c9de34df34dec7fb7772b8e3717aac5d03ec5ee0e60480a6189afbc8c081",
                "ece09703888514f042c0c5b56683ab2d6c7cd4a1fd317fa151a15a6721f780c75ab3b2969c81b27e59625b842bae29f8",
                "d0f04038cb0b91d018067f343751402c287b55fc5a0880305abc5cbe88951f2db97c944c00ffd52a65323edd69d4e2f3",
                "6cd575d3be3f9fcb18002e20c4738f68656d91e2cbdbfe55863c39b50b3967042ff4dda749bd6b08325f74c35b83982a",
                "5fa773bf7ac709463d6448a5727d9a3b367cbe07900f5a14212082388d463451e27acab34d8fa237c3ea62ec050885fb",
                "0ccea0033e15395b428ee3f44b470b3d7a5f19bbbd555bcab23f70e0149f65c69dc69518180e4616b15ac3603aad52c5",
                "33eba99903a3f9f2d35d8dbf80c3b7ecb7d65c645f859aab564c2048831cabc2cca3567e09a54e0b2111292ef5722b07",
            ))
        );
        assert_eq!(
            ok.session_key,
            h(concat!(
                "8dcaae0edde5a5fe76aa7d8207d67a20323c73040b2e1c56f96e21e691a72bd52d2c43e7761b18a98c5c1f9cd939c8f2",
                "cd59a3340d7fb90d855803f472ecfc0b",
            ))
        );
        assert_eq!(
            proof.proof,
            h(concat!(
                "109e3cbe71526f49b62cf3f894ee5f670483793bfc8997c1ff45e2070c94efa1345d95af3d52d73e34669050d42aa8ca",
                "3b6b80eea511cf722d5bff2ba6498e64",
            ))
        );
        assert_eq!(
            ok.server_proof,
            h(concat!(
                "f87dc67e42957d0a925ae8752b3eba21a854a89afe98906d3aa229b6a4c4f0da9958d46b2b78f8a3433dffb68d40ce5a",
                "627015971c059fa76c8071c02dadcb59",
            ))
        );
    }
}
