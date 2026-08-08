//! Trames binaires echangees entre le client et le serveur.
//!
//! L'encodage est explicite plutot que derive d'une bibliotheque de
//! serialisation : le format reseau d'un jeu doit rester stable et lisible sur
//! le fil independamment des refactorings internes des structures Rust.
//!
//! Format : `[longueur: u16 BE][opcode: u8][charge utile]`, la longueur couvrant
//! l'opcode et la charge utile.

mod codec;

use codec::{Reader, Writer};

/// Version du protocole. Toute rupture de format incremente cette valeur, ce
/// qui permet au serveur de refuser proprement un client desynchronise plutot
/// que de mal interpreter ses octets.
pub const PROTOCOL_VERSION: u16 = 2;

/// Plafond d'une trame. Borne l'allocation faite sur donnee non fiable : sans
/// lui, un client hostile annonce 65535 octets et fait allouer le serveur.
pub const MAX_FRAME_LEN: usize = 8 * 1024;

/// En-tete de longueur, exclu du champ `longueur`.
const HEADER_LEN: usize = 2;

/// Identifiant d'une entite dans le monde, unique le temps d'une session serveur.
pub type EntityId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessage {
    /// Premiere trame envoyee par le client.
    Handshake { protocol_version: u16 },
    /// Maintien de session.
    Ping { nonce: u32 },
    /// Demande d'apparition dans le monde.
    EnterWorld,
    /// Position revendiquee par le client.
    ///
    /// Le client propose, le serveur dispose : cette trame est une intention,
    /// pas un fait. Voir `MoveRejected`.
    Move { x: i32, y: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMessage {
    HandshakeAccepted {
        session_id: u64,
    },
    HandshakeRejected {
        expected_version: u16,
    },
    Pong {
        nonce: u32,
    },
    /// Le joueur est entre dans le monde, a la position indiquee.
    WorldEntered {
        entity_id: EntityId,
        x: i32,
        y: i32,
    },
    /// Une entite entre dans le champ de vision.
    EntityAppeared {
        entity_id: EntityId,
        x: i32,
        y: i32,
    },
    /// Une entite deja visible s'est deplacee.
    EntityMoved {
        entity_id: EntityId,
        x: i32,
        y: i32,
    },
    /// Une entite quitte le champ de vision, ou se deconnecte.
    EntityVanished {
        entity_id: EntityId,
    },
    /// Deplacement refuse : la position faisant foi est celle-ci.
    ///
    /// Le serveur renvoie sa propre verite plutot qu'un simple refus, ce qui
    /// permet au client de se resynchroniser sans aller-retour supplementaire.
    MoveRejected {
        x: i32,
        y: i32,
    },
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
    pub const ENTER_WORLD: u8 = 0x03;
    pub const MOVE: u8 = 0x04;

    pub const HANDSHAKE_ACCEPTED: u8 = 0x81;
    pub const HANDSHAKE_REJECTED: u8 = 0x82;
    pub const PONG: u8 = 0x83;
    pub const WORLD_ENTERED: u8 = 0x84;
    pub const ENTITY_APPEARED: u8 = 0x85;
    pub const ENTITY_MOVED: u8 = 0x86;
    pub const ENTITY_VANISHED: u8 = 0x87;
    pub const MOVE_REJECTED: u8 = 0x88;
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

fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let announced = payload.len() + 1;
    let mut out = Vec::with_capacity(HEADER_LEN + announced);
    // Les charges utiles sont des entiers de taille fixe : la saturation est
    // inatteignable, mais elle evite un cast silencieux.
    out.extend_from_slice(&u16::try_from(announced).unwrap_or(u16::MAX).to_be_bytes());
    out.push(opcode);
    out.extend_from_slice(payload);
    out
}

/// Charge utile commune a toutes les trames « entite a telle position ».
fn entity_at(opcode: u8, entity_id: EntityId, x: i32, y: i32) -> Vec<u8> {
    frame(
        opcode,
        &Writer::default().u64(entity_id).i32(x).i32(y).into_bytes(),
    )
}

fn read_entity_at(payload: &[u8]) -> Result<(EntityId, i32, i32), DecodeError> {
    let mut reader = Reader::new(payload);
    let entity_id = reader.u64()?;
    let x = reader.i32()?;
    let y = reader.i32()?;
    reader.finish()?;
    Ok((entity_id, x, y))
}

fn read_point(payload: &[u8]) -> Result<(i32, i32), DecodeError> {
    let mut reader = Reader::new(payload);
    let x = reader.i32()?;
    let y = reader.i32()?;
    reader.finish()?;
    Ok((x, y))
}

impl ClientMessage {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::Handshake { protocol_version } => frame(
                opcode::HANDSHAKE,
                &Writer::default().u16(protocol_version).into_bytes(),
            ),
            Self::Ping { nonce } => frame(opcode::PING, &Writer::default().u32(nonce).into_bytes()),
            Self::EnterWorld => frame(opcode::ENTER_WORLD, &[]),
            Self::Move { x, y } => {
                frame(opcode::MOVE, &Writer::default().i32(x).i32(y).into_bytes())
            }
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
            opcode::HANDSHAKE => {
                let mut reader = Reader::new(payload);
                let protocol_version = reader.u16()?;
                reader.finish()?;
                Self::Handshake { protocol_version }
            }
            opcode::PING => {
                let mut reader = Reader::new(payload);
                let nonce = reader.u32()?;
                reader.finish()?;
                Self::Ping { nonce }
            }
            opcode::ENTER_WORLD => {
                Reader::new(payload).finish()?;
                Self::EnterWorld
            }
            opcode::MOVE => {
                let (x, y) = read_point(payload)?;
                Self::Move { x, y }
            }
            other => return Err(DecodeError::UnknownOpcode(other)),
        };
        Ok((message, consumed))
    }
}

impl ServerMessage {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::HandshakeAccepted { session_id } => frame(
                opcode::HANDSHAKE_ACCEPTED,
                &Writer::default().u64(session_id).into_bytes(),
            ),
            Self::HandshakeRejected { expected_version } => frame(
                opcode::HANDSHAKE_REJECTED,
                &Writer::default().u16(expected_version).into_bytes(),
            ),
            Self::Pong { nonce } => frame(opcode::PONG, &Writer::default().u32(nonce).into_bytes()),
            Self::WorldEntered { entity_id, x, y } => {
                entity_at(opcode::WORLD_ENTERED, entity_id, x, y)
            }
            Self::EntityAppeared { entity_id, x, y } => {
                entity_at(opcode::ENTITY_APPEARED, entity_id, x, y)
            }
            Self::EntityMoved { entity_id, x, y } => {
                entity_at(opcode::ENTITY_MOVED, entity_id, x, y)
            }
            Self::EntityVanished { entity_id } => frame(
                opcode::ENTITY_VANISHED,
                &Writer::default().u64(entity_id).into_bytes(),
            ),
            Self::MoveRejected { x, y } => frame(
                opcode::MOVE_REJECTED,
                &Writer::default().i32(x).i32(y).into_bytes(),
            ),
        }
    }

    /// # Errors
    /// Voir [`DecodeError`].
    pub fn decode(input: &[u8]) -> Result<(Self, usize), DecodeError> {
        let (opcode, payload, consumed) = split_frame(input)?;
        let message = match opcode {
            opcode::HANDSHAKE_ACCEPTED => {
                let mut reader = Reader::new(payload);
                let session_id = reader.u64()?;
                reader.finish()?;
                Self::HandshakeAccepted { session_id }
            }
            opcode::HANDSHAKE_REJECTED => {
                let mut reader = Reader::new(payload);
                let expected_version = reader.u16()?;
                reader.finish()?;
                Self::HandshakeRejected { expected_version }
            }
            opcode::PONG => {
                let mut reader = Reader::new(payload);
                let nonce = reader.u32()?;
                reader.finish()?;
                Self::Pong { nonce }
            }
            opcode::WORLD_ENTERED => {
                let (entity_id, x, y) = read_entity_at(payload)?;
                Self::WorldEntered { entity_id, x, y }
            }
            opcode::ENTITY_APPEARED => {
                let (entity_id, x, y) = read_entity_at(payload)?;
                Self::EntityAppeared { entity_id, x, y }
            }
            opcode::ENTITY_MOVED => {
                let (entity_id, x, y) = read_entity_at(payload)?;
                Self::EntityMoved { entity_id, x, y }
            }
            opcode::ENTITY_VANISHED => {
                let mut reader = Reader::new(payload);
                let entity_id = reader.u64()?;
                reader.finish()?;
                Self::EntityVanished { entity_id }
            }
            opcode::MOVE_REJECTED => {
                let (x, y) = read_point(payload)?;
                Self::MoveRejected { x, y }
            }
            other => return Err(DecodeError::UnknownOpcode(other)),
        };
        Ok((message, consumed))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Un exemplaire de chaque variante, pour que l'ajout d'un message sans son
    /// test d'aller-retour se voie.
    fn all_client_messages() -> Vec<ClientMessage> {
        vec![
            ClientMessage::Handshake {
                protocol_version: PROTOCOL_VERSION,
            },
            ClientMessage::Ping { nonce: 0xDEAD_BEEF },
            ClientMessage::EnterWorld,
            ClientMessage::Move {
                x: i32::MIN,
                y: i32::MAX,
            },
        ]
    }

    fn all_server_messages() -> Vec<ServerMessage> {
        vec![
            ServerMessage::HandshakeAccepted { session_id: 42 },
            ServerMessage::HandshakeRejected {
                expected_version: PROTOCOL_VERSION,
            },
            ServerMessage::Pong { nonce: 7 },
            ServerMessage::WorldEntered {
                entity_id: 1,
                x: -1_000,
                y: 2_000,
            },
            ServerMessage::EntityAppeared {
                entity_id: u64::MAX,
                x: 0,
                y: 0,
            },
            ServerMessage::EntityMoved {
                entity_id: 3,
                x: i32::MIN,
                y: i32::MAX,
            },
            ServerMessage::EntityVanished { entity_id: 9 },
            ServerMessage::MoveRejected { x: 5, y: -5 },
        ]
    }

    #[test]
    fn les_messages_client_font_un_aller_retour_fidele() {
        for message in all_client_messages() {
            let bytes = message.encode();
            let (decoded, consumed) = ClientMessage::decode(&bytes).unwrap();
            assert_eq!(decoded, message);
            assert_eq!(consumed, bytes.len());
        }
    }

    #[test]
    fn les_messages_serveur_font_un_aller_retour_fidele() {
        for message in all_server_messages() {
            let bytes = message.encode();
            let (decoded, consumed) = ServerMessage::decode(&bytes).unwrap();
            assert_eq!(decoded, message);
            assert_eq!(consumed, bytes.len());
        }
    }

    #[test]
    fn les_opcodes_client_et_serveur_ne_se_recouvrent_pas() {
        // Un client ne doit jamais decoder par erreur une trame serveur, et
        // reciproquement : la separation par bit de poids fort est la garantie.
        for message in all_client_messages() {
            assert!(ServerMessage::decode(&message.encode()).is_err());
        }
        for message in all_server_messages() {
            assert!(ClientMessage::decode(&message.encode()).is_err());
        }
    }

    #[test]
    fn toute_troncature_est_signalee_incomplete() {
        for message in all_client_messages() {
            let bytes = message.encode();
            for cut in 0..bytes.len() {
                assert_eq!(
                    ClientMessage::decode(&bytes[..cut]),
                    Err(DecodeError::Incomplete),
                    "{message:?} coupe a {cut}"
                );
            }
        }
    }

    #[test]
    fn le_decodage_isole_une_seule_trame_dans_un_flux() {
        let mut stream = ClientMessage::Ping { nonce: 1 }.encode();
        let first_len = stream.len();
        stream.extend(ClientMessage::Move { x: 10, y: 20 }.encode());

        let (first, consumed) = ClientMessage::decode(&stream).unwrap();
        assert_eq!(first, ClientMessage::Ping { nonce: 1 });
        assert_eq!(consumed, first_len);

        let (second, _) = ClientMessage::decode(&stream[consumed..]).unwrap();
        assert_eq!(second, ClientMessage::Move { x: 10, y: 20 });
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
        let bytes = frame(0x7F, &[0, 0]);
        assert_eq!(
            ClientMessage::decode(&bytes),
            Err(DecodeError::UnknownOpcode(0x7F))
        );
    }

    #[test]
    fn une_charge_utile_de_mauvaise_taille_est_rejetee() {
        // Trop courte pour un Move, trop longue pour un Ping.
        assert_eq!(
            ClientMessage::decode(&frame(opcode::MOVE, &[1, 2])),
            Err(DecodeError::MalformedPayload)
        );
        assert_eq!(
            ClientMessage::decode(&frame(opcode::PING, &[1, 2, 3, 4, 5])),
            Err(DecodeError::MalformedPayload)
        );
        assert_eq!(
            ClientMessage::decode(&frame(opcode::ENTER_WORLD, &[0])),
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
