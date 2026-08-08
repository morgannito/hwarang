//! Politique de session, sans I/O.
//!
//! Separer la decision de la lecture socket rend le cycle de vie d'une
//! connexion testable a froid : pas de port a ouvrir pour verifier qu'un client
//! desynchronise est bien ejecte.

use hwarang_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

/// Suite a donner apres traitement d'une trame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// Repondre et garder la connexion.
    Reply(ServerMessage),
    /// Repondre puis fermer.
    ReplyAndClose(ServerMessage),
    /// Fermer sans repondre : le client a viole le protocole.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingHandshake,
    Established { session_id: u64 },
}

#[derive(Debug)]
pub struct Session {
    state: State,
    session_id: u64,
}

impl Session {
    #[must_use]
    pub const fn new(session_id: u64) -> Self {
        Self {
            state: State::AwaitingHandshake,
            session_id,
        }
    }

    /// Traite une trame entrante.
    ///
    /// Toute trame recue avant le handshake ferme la connexion sans reponse :
    /// une machine a etats permissive est la porte d'entree classique des
    /// exploits sur les serveurs de jeu.
    pub fn on_message(&mut self, message: ClientMessage) -> Reaction {
        match (self.state, message) {
            (State::AwaitingHandshake, ClientMessage::Handshake { protocol_version }) => {
                if protocol_version == PROTOCOL_VERSION {
                    self.state = State::Established {
                        session_id: self.session_id,
                    };
                    Reaction::Reply(ServerMessage::HandshakeAccepted {
                        session_id: self.session_id,
                    })
                } else {
                    Reaction::ReplyAndClose(ServerMessage::HandshakeRejected {
                        expected_version: PROTOCOL_VERSION,
                    })
                }
            }
            // Trame hors sequence : soit avant le handshake, soit un handshake
            // rejoue sur une session deja etablie (client detourne).
            (State::AwaitingHandshake, _)
            | (State::Established { .. }, ClientMessage::Handshake { .. }) => Reaction::Close,
            (State::Established { .. }, ClientMessage::Ping { nonce }) => {
                Reaction::Reply(ServerMessage::Pong { nonce })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handshake(version: u16) -> ClientMessage {
        ClientMessage::Handshake {
            protocol_version: version,
        }
    }

    #[test]
    fn un_handshake_valide_etablit_la_session() {
        let mut session = Session::new(7);
        let reaction = session.on_message(handshake(PROTOCOL_VERSION));

        assert_eq!(
            reaction,
            Reaction::Reply(ServerMessage::HandshakeAccepted { session_id: 7 })
        );
    }

    #[test]
    fn une_version_incompatible_est_refusee_explicitement() {
        let mut session = Session::new(7);
        let reaction = session.on_message(handshake(PROTOCOL_VERSION + 1));

        assert_eq!(
            reaction,
            Reaction::ReplyAndClose(ServerMessage::HandshakeRejected {
                expected_version: PROTOCOL_VERSION,
            })
        );
        // Le refus n'etablit rien : la trame suivante est traitee comme
        // hors sequence.
        assert_eq!(
            session.on_message(ClientMessage::Ping { nonce: 1 }),
            Reaction::Close
        );
    }

    #[test]
    fn toute_trame_avant_le_handshake_ferme_la_connexion() {
        let mut session = Session::new(7);
        assert_eq!(
            session.on_message(ClientMessage::Ping { nonce: 1 }),
            Reaction::Close
        );
    }

    #[test]
    fn le_ping_est_servi_une_fois_la_session_etablie() {
        let mut session = Session::new(7);
        session.on_message(handshake(PROTOCOL_VERSION));

        assert_eq!(
            session.on_message(ClientMessage::Ping { nonce: 99 }),
            Reaction::Reply(ServerMessage::Pong { nonce: 99 })
        );
    }

    #[test]
    fn un_second_handshake_ferme_la_connexion() {
        let mut session = Session::new(7);
        session.on_message(handshake(PROTOCOL_VERSION));

        assert_eq!(
            session.on_message(handshake(PROTOCOL_VERSION)),
            Reaction::Close
        );
    }
}
