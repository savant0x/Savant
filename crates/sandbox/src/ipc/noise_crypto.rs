use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use snow::{Builder, HandshakeState, TransportState};

#[derive(Debug, thiserror::Error)]
pub enum NoiseError {
    #[error("invalid protocol: {0}")]
    InvalidProtocol(String),
    #[error("handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("encryption failed: {0}")]
    EncryptFailed(String),
    #[error("decryption failed: {0}")]
    DecryptFailed(String),
}

/// Initiator state machine for Noise_XX_25519_ChaChaPoly_SHA256.
pub struct NoiseInitiator {
    handshake: HandshakeState,
}

impl NoiseInitiator {
    pub fn new(local_private: &SigningKey) -> Result<Self, NoiseError> {
        let proto: snow::params::NoiseParams = "Noise_XX_25519_ChaChaPoly_SHA256"
            .parse()
            .map_err(|e| NoiseError::InvalidProtocol(format!("{:?}", e)))?;
        let handshake = Builder::new(proto)
            .local_private_key(&local_private.to_bytes())
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?
            .build_initiator()
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        Ok(Self { handshake })
    }

    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; 65535];
        let len = self
            .handshake
            .write_message(payload, &mut buf)
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn read_message(&mut self, incoming: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; 65535];
        let len = self
            .handshake
            .read_message(incoming, &mut buf)
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn into_transport(self) -> Result<NoiseTransport, NoiseError> {
        let state = self
            .handshake
            .into_transport_mode()
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        Ok(NoiseTransport { state })
    }
}

/// Responder state machine for Noise_XX.
pub struct NoiseResponder {
    handshake: HandshakeState,
}

impl NoiseResponder {
    pub fn new(local_private: &SigningKey) -> Result<Self, NoiseError> {
        let proto: snow::params::NoiseParams = "Noise_XX_25519_ChaChaPoly_SHA256"
            .parse()
            .map_err(|e| NoiseError::InvalidProtocol(format!("{:?}", e)))?;
        let handshake = Builder::new(proto)
            .local_private_key(&local_private.to_bytes())
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?
            .build_responder()
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        Ok(Self { handshake })
    }

    pub fn read_message(&mut self, incoming: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; 65535];
        let len = self
            .handshake
            .read_message(incoming, &mut buf)
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; 65535];
        let len = self
            .handshake
            .write_message(payload, &mut buf)
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn into_transport(self) -> Result<NoiseTransport, NoiseError> {
        let state = self
            .handshake
            .into_transport_mode()
            .map_err(|e| NoiseError::HandshakeFailed(e.to_string()))?;
        Ok(NoiseTransport { state })
    }
}

/// Transport mode: encrypted message exchange.
pub struct NoiseTransport {
    state: TransportState,
}

impl NoiseTransport {
    pub fn send(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; plaintext.len() + 16];
        let len = self
            .state
            .write_message(plaintext, &mut buf)
            .map_err(|e| NoiseError::EncryptFailed(e.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn receive(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; ciphertext.len()];
        let len = self
            .state
            .read_message(ciphertext, &mut buf)
            .map_err(|e| NoiseError::DecryptFailed(e.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }
}

/// Generate an ephemeral keypair for a single session.
pub fn generate_session_keypair() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

/// Length-prefixed message framing.
pub mod framing {
    use super::*;

    pub fn encode(payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    pub fn decode(data: &[u8]) -> Result<(&[u8], &[u8]), NoiseError> {
        if data.len() < 4 {
            return Err(NoiseError::DecryptFailed("frame too short".into()));
        }
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if data.len() < 4 + len {
            return Err(NoiseError::DecryptFailed("incomplete frame".into()));
        }
        Ok((&data[4..4 + len], &data[4 + len..]))
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_full_handshake_success() {
        let host_key = generate_session_keypair();
        let guest_key = generate_session_keypair();

        let mut initiator = NoiseInitiator::new(&host_key).expect("initiator creation");
        let mut responder = NoiseResponder::new(&guest_key).expect("responder creation");

        let msg1 = initiator.write_message(&[]).expect("initiator msg1");
        assert!(!msg1.is_empty());

        let payload1 = responder.read_message(&msg1).expect("responder read msg1");
        assert!(payload1.is_empty()); // empty payload in handshake
        let msg2 = responder.write_message(&[]).expect("responder msg2");
        assert!(!msg2.is_empty());

        let payload2 = initiator.read_message(&msg2).expect("initiator read msg2");
        assert!(payload2.is_empty());
        let msg3 = initiator.write_message(&[]).expect("initiator msg3");
        let payload3 = responder.read_message(&msg3).expect("responder read msg3");
        assert!(payload3.is_empty());

        let mut tx = initiator.into_transport().expect("initiator transport");
        let mut rx = responder.into_transport().expect("responder transport");

        let ct = tx.send(b"hello microvm").expect("encrypt");
        let pt = rx.receive(&ct).expect("decrypt");
        assert_eq!(&pt, b"hello microvm");
    }

    #[test]
    fn test_tampered_message_rejected() {
        let host_key = generate_session_keypair();
        let guest_key = generate_session_keypair();

        let mut init = NoiseInitiator::new(&host_key).expect("initiator creation");
        let mut resp = NoiseResponder::new(&guest_key).expect("responder creation");

        let msg1 = init.write_message(&[]).expect("initiator msg1");
        let _payload = resp.read_message(&msg1).expect("responder read msg1");
        let msg2 = resp.write_message(&[]).expect("responder msg2");

        let mut tampered = msg2;
        if let Some(b) = tampered.last_mut() {
            *b ^= 0xFF;
        }

        let result = init.read_message(&tampered);
        assert!(result.is_err());
    }

    #[test]
    fn test_replay_detection() {
        let host_key = generate_session_keypair();
        let guest_key = generate_session_keypair();

        let mut init = NoiseInitiator::new(&host_key).expect("initiator creation");
        let mut resp = NoiseResponder::new(&guest_key).expect("responder creation");

        let msg1 = init.write_message(&[]).expect("initiator msg1");
        let _p1 = resp.read_message(&msg1).expect("responder read msg1");
        let msg2 = resp.write_message(&[]).expect("responder msg2");
        let _p2 = init.read_message(&msg2).expect("initiator read msg2");
        let msg3 = init.write_message(&[]).expect("initiator msg3");
        let _p3 = resp.read_message(&msg3).expect("responder read msg3");

        let mut tx = init.into_transport().expect("initiator transport");
        let mut rx = resp.into_transport().expect("responder transport");

        let ct = tx.send(b"valid").expect("encrypt");
        let _r1 = rx.receive(&ct).expect("first decrypt");
        let r2 = rx.receive(&ct);
        assert!(r2.is_err());
    }

    #[test]
    fn test_cbor_framing() {
        let payload = b"hello world";
        let frame = framing::encode(payload);
        let (decoded, rest) = framing::decode(&frame).unwrap();
        assert_eq!(decoded, payload);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_framing_rejects_short_data() {
        let result = framing::decode(&[0x00, 0x01]);
        assert!(result.is_err());
    }
}
