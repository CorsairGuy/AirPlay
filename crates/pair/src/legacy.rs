//! Сопряжение AirPlay 1, как в UxPlay: `/pair-setup` обменивается ключами
//! Ed25519 (по 32 байта), `/pair-verify` — X25519 + подписи под AES-128-CTR.

use crypto::ctr;
use rand::rngs::OsRng;
use sha2::{Digest, Sha512};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::Error;
use crate::identity::{Identity, verify_signature};

fn aes_params(shared: &[u8; 32]) -> ([u8; 16], [u8; 16]) {
    let derive = |label: &[u8]| -> [u8; 16] {
        let h = Sha512::new()
            .chain_update(label)
            .chain_update(shared)
            .finalize();
        h[..16].try_into().unwrap()
    };
    (
        derive(b"Pair-Verify-AES-Key"),
        derive(b"Pair-Verify-AES-IV"),
    )
}

pub enum LegacyOutcome {
    Continue(Vec<u8>),
    /// Проверка пройдена; общий секрет нужен позже для ключей зеркалирования.
    Verified {
        shared: [u8; 32],
    },
}

/// Серверная сторона. Один экземпляр на соединение.
pub struct LegacyServer {
    identity: Identity,
    client_ed: Option<[u8; 32]>,
    pending: Option<([u8; 32], [u8; 32], [u8; 32])>,
}

impl LegacyServer {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            client_ed: None,
            pending: None,
        }
    }

    /// `/pair-setup`: 32 байта ключа клиента → 32 байта нашего ключа.
    pub fn pair_setup(&mut self, body: &[u8]) -> Result<Vec<u8>, Error> {
        let key: [u8; 32] = body
            .try_into()
            .map_err(|_| Error::Malformed("pair-setup: ожидалось 32 байта"))?;
        self.client_ed = Some(key);
        Ok(self.identity.public_key().to_vec())
    }

    /// `/pair-verify`: 4 байта заголовка (первый — флаг шага) + данные.
    pub fn pair_verify(&mut self, body: &[u8]) -> Result<LegacyOutcome, Error> {
        if body.len() != 68 {
            return Err(Error::Malformed("pair-verify: ожидалось 68 байт"));
        }
        match body[0] {
            1 => {
                let client_eph: [u8; 32] = body[4..36].try_into().unwrap();
                self.client_ed = Some(body[36..68].try_into().unwrap());
                let secret = StaticSecret::random_from_rng(OsRng);
                let server_eph = PublicKey::from(&secret).to_bytes();
                let shared = secret
                    .diffie_hellman(&PublicKey::from(client_eph))
                    .to_bytes();
                let (key, iv) = aes_params(&shared);
                let mut sig = self.identity.sign(&[server_eph, client_eph].concat());
                ctr::apply(&mut ctr::aes128_ctr(&key, &iv), &mut sig);
                self.pending = Some((shared, client_eph, server_eph));
                Ok(LegacyOutcome::Continue(
                    [server_eph.as_slice(), &sig].concat(),
                ))
            }
            0 => {
                let (shared, client_eph, server_eph) =
                    self.pending.take().ok_or(Error::UnexpectedState(0))?;
                let client_ed = self.client_ed.ok_or(Error::UnexpectedState(0))?;
                let (key, iv) = aes_params(&shared);
                let mut cipher = ctr::aes128_ctr(&key, &iv);
                // Первые 64 байта потока ушли на нашу подпись в шаге 1.
                ctr::apply(&mut cipher, &mut [0u8; 64]);
                let mut sig = [0u8; 64];
                sig.copy_from_slice(&body[4..68]);
                ctr::apply(&mut cipher, &mut sig);
                verify_signature(&client_ed, &[client_eph, server_eph].concat(), &sig)?;
                Ok(LegacyOutcome::Verified { shared })
            }
            f => Err(Error::UnexpectedState(f)),
        }
    }
}

/// Клиентская сторона (для отправки и тестов).
pub struct LegacyClient {
    identity: Identity,
    secret: StaticSecret,
}

impl LegacyClient {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            secret: StaticSecret::random_from_rng(OsRng),
        }
    }

    pub fn pair_setup(&self) -> Vec<u8> {
        self.identity.public_key().to_vec()
    }

    pub fn verify1(&self) -> Vec<u8> {
        let eph = PublicKey::from(&self.secret).to_bytes();
        [&[1, 0, 0, 0][..], &eph, &self.identity.public_key()].concat()
    }

    /// Проверяет ответ сервера (ключ сервера из pair-setup) и возвращает второе
    /// сообщение и общий секрет.
    pub fn verify2(
        &self,
        server_ed: &[u8; 32],
        reply: &[u8],
    ) -> Result<(Vec<u8>, [u8; 32]), Error> {
        if reply.len() != 96 {
            return Err(Error::Malformed("pair-verify: ожидалось 96 байт"));
        }
        let server_eph: [u8; 32] = reply[..32].try_into().unwrap();
        let shared = self
            .secret
            .diffie_hellman(&PublicKey::from(server_eph))
            .to_bytes();
        let (key, iv) = aes_params(&shared);
        let mut cipher = ctr::aes128_ctr(&key, &iv);
        let mut server_sig = [0u8; 64];
        server_sig.copy_from_slice(&reply[32..]);
        ctr::apply(&mut cipher, &mut server_sig);
        let client_eph = PublicKey::from(&self.secret).to_bytes();
        verify_signature(server_ed, &[server_eph, client_eph].concat(), &server_sig)?;
        let mut sig = self.identity.sign(&[client_eph, server_eph].concat());
        ctr::apply(&mut cipher, &mut sig);
        Ok(([&[0, 0, 0, 0][..], &sig].concat(), shared))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_and_verify() {
        let mut server = LegacyServer::new(Identity::from_seed("acc", &[3; 32]));
        let client = LegacyClient::new(Identity::from_seed("ctl", &[4; 32]));
        let server_ed: [u8; 32] = server
            .pair_setup(&client.pair_setup())
            .unwrap()
            .try_into()
            .unwrap();
        let LegacyOutcome::Continue(reply) = server.pair_verify(&client.verify1()).unwrap() else {
            panic!()
        };
        let (msg2, client_shared) = client.verify2(&server_ed, &reply).unwrap();
        let LegacyOutcome::Verified { shared } = server.pair_verify(&msg2).unwrap() else {
            panic!()
        };
        assert_eq!(shared, client_shared);
    }

    #[test]
    fn foreign_signature_is_rejected() {
        let mut server = LegacyServer::new(Identity::from_seed("acc", &[3; 32]));
        let client = LegacyClient::new(Identity::from_seed("ctl", &[4; 32]));
        let server_ed: [u8; 32] = server
            .pair_setup(&client.pair_setup())
            .unwrap()
            .try_into()
            .unwrap();
        let LegacyOutcome::Continue(reply) = server.pair_verify(&client.verify1()).unwrap() else {
            panic!()
        };
        let (mut msg2, _) = client.verify2(&server_ed, &reply).unwrap();
        msg2[10] ^= 0xff;
        assert!(server.pair_verify(&msg2).is_err());
    }
}
