//! Trames binaires echangees entre le client et le serveur.
//!
//! L'encodage est explicite plutot que derive d'une bibliotheque de
//! serialisation : le format reseau d'un jeu doit rester stable et lisible sur
//! le fil independamment des refactorings internes des structures Rust.
//!
//! Format : `[longueur: u16 BE][opcode: u8][charge utile]`, la longueur couvrant
//! l'opcode et la charge utile.

/// Version du protocole. Toute rupture de format incremente cette valeur, ce
/// qui permet au serveur de refuser proprement un client desynchronise plutot
/// que de mal interpreter ses octets.
pub const PROTOCOL_VERSION: u16 = 1;

/// Plafond d'une trame. Borne l'allocation faite sur donnee non fiable : sans
/// lui, un client hostile annonce 65535 octets et fait allouer le serveur.
pub const MAX_FRAME_LEN: usize = 8 * 1024;

/// En-tete de longueur, exclu du champ `longueur`.
const HEADER_LEN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessage {
    /// Premiere trame envoyee par le client.
    Handshake { protocol_version: u16 },
    /// Maintien de session.
    Ping { nonce: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMessage {
    HandshakeAccepted { session_id: u64 },
    HandshakeRejected { expected_version: u16 },
    Pong { nonce: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Trame incomplete : il manque des octets, rappeler plus tard.
    Incomplete,
    /// Longueur annoncee au-dela de [`MAX_FRAME_LEN`].
    FrameTooLarge { announced: usize },
    /// Opcode inconnu.
    UnknownOpcode(u8),
    /// Charge utile incoherente avec l'opcode.
    MalformedPayload,
}

mod opcode {
    pub const HANDSHAKE: u8 = 0x01;
    pub const PING: u8 = 0x02;
    pub const HANDSHAKE_ACCEPTED: u8 = 0x81;
    pub const HANDSHAKE_REJECTED: u8 = 0x82;
    pub const PONG: u8 = 0x83;
}

/// Isole une trame du flux : retourne `(opcode, charge utile, octets consommes)`.
fn split_frame(input: &[u8]) -> Result<(u8, &[u8], usize), DecodeError> {
    let header: [u8; HEADER_LEN] = input
        .get(..HEADER_LEN)
        .ok_or(DecodeError::Incomplete)?
        .try_into()
        .map_err(|_| DecodeError::Incomplete)?;

    let announced = usize::from(u16::from_be_bytes(header));
    if announced > MAX_FRAME_LEN {
        return Err(DecodeError::FrameTooLarge { announced });
    }
    if announced == 0 {
        return Err(DecodeError::MalformedPayload);
    }

    let total = HEADER_LEN + announced;
    let body = input
        .get(HEADER_LEN..total)
        .ok_or(DecodeError::Incomplete)?;
    let (opcode, payload) = body.split_first().ok_or(DecodeError::MalformedPayload)?;

    Ok((*opcode, payload, total))
}

fn encode(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let announced = payload.len() + 1;
    let mut out = Vec::with_capacity(HEADER_LEN + announced);
    // Les charges utiles sont des entiers de taille fixe : la saturation est
    // inatteignable, mais elle evite un cast silencieux.
    out.extend_from_slice(&u16::try_from(announced).unwrap_or(u16::MAX).to_be_bytes());
    out.push(opcode);
    out.extend_from_slice(payload);
    out
}

fn payload_u16(payload: &[u8]) -> Result<u16, DecodeError> {
    payload
        .try_into()
        .map(u16::from_be_bytes)
        .map_err(|_| DecodeError::MalformedPayload)
}

fn payload_u32(payload: &[u8]) -> Result<u32, DecodeError> {
    payload
        .try_into()
        .map(u32::from_be_bytes)
        .map_err(|_| DecodeError::MalformedPayload)
}

fn payload_u64(payload: &[u8]) -> Result<u64, DecodeError> {
    payload
        .try_into()
        .map(u64::from_be_bytes)
        .map_err(|_| DecodeError::MalformedPayload)
}

impl ClientMessage {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::Handshake { protocol_version } => {
                encode(opcode::HANDSHAKE, &protocol_version.to_be_bytes())
            }
            Self::Ping { nonce } => encode(opcode::PING, &nonce.to_be_bytes()),
        }
    }

    /// Decode la premiere trame de `input` et indique combien d'octets ont ete
    /// consommes, pour que l'appelant purge son tampon sans le reparcourir.
    ///
    /// # Errors
    /// Voir [`DecodeError`].
    pub fn decode(input: &[u8]) -> Result<(Self, usize), DecodeError> {
        let (opcode, payload, consumed) = split_frame(input)?;
        let message = match opcode {
            opcode::HANDSHAKE => Self::Handshake {
                protocol_version: payload_u16(payload)?,
            },
            opcode::PING => Self::Ping {
                nonce: payload_u32(payload)?,
            },
            other => return Err(DecodeError::UnknownOpcode(other)),
        };
        Ok((message, consumed))
    }
}

impl ServerMessage {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::HandshakeAccepted { session_id } => {
                encode(opcode::HANDSHAKE_ACCEPTED, &session_id.to_be_bytes())
            }
            Self::HandshakeRejected { expected_version } => {
                encode(opcode::HANDSHAKE_REJECTED, &expected_version.to_be_bytes())
            }
            Self::Pong { nonce } => encode(opcode::PONG, &nonce.to_be_bytes()),
        }
    }

    /// # Errors
    /// Voir [`DecodeError`].
    pub fn decode(input: &[u8]) -> Result<(Self, usize), DecodeError> {
        let (opcode, payload, consumed) = split_frame(input)?;
        let message = match opcode {
            opcode::HANDSHAKE_ACCEPTED => Self::HandshakeAccepted {
                session_id: payload_u64(payload)?,
            },
            opcode::HANDSHAKE_REJECTED => Self::HandshakeRejected {
                expected_version: payload_u16(payload)?,
            },
            opcode::PONG => Self::Pong {
                nonce: payload_u32(payload)?,
            },
            other => return Err(DecodeError::UnknownOpcode(other)),
        };
        Ok((message, consumed))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn les_messages_client_font_un_aller_retour_fidele() {
        for message in [
            ClientMessage::Handshake {
                protocol_version: PROTOCOL_VERSION,
            },
            ClientMessage::Ping { nonce: 0xDEAD_BEEF },
        ] {
            let bytes = message.encode();
            let (decoded, consumed) = ClientMessage::decode(&bytes).unwrap();
            assert_eq!(decoded, message);
            assert_eq!(consumed, bytes.len());
        }
    }

    #[test]
    fn les_messages_serveur_font_un_aller_retour_fidele() {
        for message in [
            ServerMessage::HandshakeAccepted { session_id: 42 },
            ServerMessage::HandshakeRejected {
                expected_version: PROTOCOL_VERSION,
            },
            ServerMessage::Pong { nonce: 7 },
        ] {
            let bytes = message.encode();
            let (decoded, consumed) = ServerMessage::decode(&bytes).unwrap();
            assert_eq!(decoded, message);
            assert_eq!(consumed, bytes.len());
        }
    }

    #[test]
    fn une_trame_tronquee_est_incomplete_et_non_invalide() {
        let bytes = ClientMessage::Ping { nonce: 1 }.encode();
        for cut in 0..bytes.len() {
            assert_eq!(
                ClientMessage::decode(&bytes[..cut]),
                Err(DecodeError::Incomplete),
                "coupe a {cut}"
            );
        }
    }

    #[test]
    fn le_decodage_isole_une_seule_trame_dans_un_flux() {
        let mut stream = ClientMessage::Ping { nonce: 1 }.encode();
        let first_len = stream.len();
        stream.extend(ClientMessage::Ping { nonce: 2 }.encode());

        let (first, consumed) = ClientMessage::decode(&stream).unwrap();
        assert_eq!(first, ClientMessage::Ping { nonce: 1 });
        assert_eq!(consumed, first_len);

        let (second, _) = ClientMessage::decode(&stream[consumed..]).unwrap();
        assert_eq!(second, ClientMessage::Ping { nonce: 2 });
    }

    #[test]
    fn une_longueur_hostile_est_rejetee_sans_allocation() {
        let oversized = u16::try_from(MAX_FRAME_LEN).unwrap_or(u16::MAX) + 1;
        let mut bytes = oversized.to_be_bytes().to_vec();
        bytes.push(opcode::PING);
        assert!(matches!(
            ClientMessage::decode(&bytes),
            Err(DecodeError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn un_opcode_inconnu_est_signale() {
        let bytes = encode(0x7F, &[0, 0]);
        assert_eq!(
            ClientMessage::decode(&bytes),
            Err(DecodeError::UnknownOpcode(0x7F))
        );
    }

    #[test]
    fn une_charge_utile_de_mauvaise_taille_est_rejetee() {
        let bytes = encode(opcode::PING, &[1, 2]);
        assert_eq!(
            ClientMessage::decode(&bytes),
            Err(DecodeError::MalformedPayload)
        );
    }

    #[test]
    fn une_trame_vide_est_rejetee() {
        let bytes = 0u16.to_be_bytes();
        assert_eq!(
            ClientMessage::decode(&bytes),
            Err(DecodeError::MalformedPayload)
        );
    }
}
