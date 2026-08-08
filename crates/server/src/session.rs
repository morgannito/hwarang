//! Politique de session, sans I/O.
//!
//! Separer la decision de la lecture socket rend le cycle de vie d'une
//! connexion testable a froid : pas de port a ouvrir pour verifier qu'un client
//! desynchronise est bien ejecte.

use hwarang_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage};

/// Action a executer sur l'etat partage du monde.
///
/// La session ne touche pas au monde elle-meme : elle decrit ce qu'il faut
/// faire, l'appelant l'applique. C'est ce qui la garde testable sans monde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldCommand {
    Enter,
    Move { x: i32, y: i32 },
}

/// Suite a donner apres traitement d'une trame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// Repondre et garder la connexion.
    Reply(ServerMessage),
    /// Repondre puis fermer.
    ReplyAndClose(ServerMessage),
    /// Fermer sans repondre : le client a viole le protocole.
    Close,
    /// Agir sur le monde ; les notifications partiront par le canal de sortie.
    Perform(WorldCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingHandshake,
    Authenticated,
    InWorld,
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

    /// L'entite du joueur porte l'identifiant de sa session : une connexion
    /// pilote un seul personnage, donc un second identifiant n'apporterait
    /// qu'une table de correspondance a tenir a jour.
    #[must_use]
    pub const fn entity_id(&self) -> u64 {
        self.session_id
    }

    /// Traite une trame entrante.
    ///
    /// Toute trame hors sequence ferme la connexion sans reponse : une machine
    /// a etats permissive est la porte d'entree classique des exploits sur les
    /// serveurs de jeu.
    pub fn on_message(&mut self, message: ClientMessage) -> Reaction {
        match (self.state, message) {
            (State::AwaitingHandshake, ClientMessage::Handshake { protocol_version }) => {
                if protocol_version == PROTOCOL_VERSION {
                    self.state = State::Authenticated;
                    Reaction::Reply(ServerMessage::HandshakeAccepted {
                        session_id: self.session_id,
                    })
                } else {
                    Reaction::ReplyAndClose(ServerMessage::HandshakeRejected {
                        expected_version: PROTOCOL_VERSION,
                    })
                }
            }

            // Le ping n'a pas besoin du monde : il sert a maintenir la
            // connexion pendant la selection de personnage.
            (State::Authenticated | State::InWorld, ClientMessage::Ping { nonce }) => {
                Reaction::Reply(ServerMessage::Pong { nonce })
            }

            (State::Authenticated, ClientMessage::EnterWorld) => {
                self.state = State::InWorld;
                Reaction::Perform(WorldCommand::Enter)
            }

            (State::InWorld, ClientMessage::Move { x, y }) => {
                Reaction::Perform(WorldCommand::Move { x, y })
            }

            // Reste : trame avant le handshake, deplacement avant l'entree dans
            // le monde, seconde entree, handshake rejoue.
            _ => Reaction::Close,
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

    fn authenticated() -> Session {
        let mut session = Session::new(7);
        session.on_message(handshake(PROTOCOL_VERSION));
        session
    }

    fn in_world() -> Session {
        let mut session = authenticated();
        session.on_message(ClientMessage::EnterWorld);
        session
    }

    #[test]
    fn un_handshake_valide_authentifie() {
        let mut session = Session::new(7);
        assert_eq!(
            session.on_message(handshake(PROTOCOL_VERSION)),
            Reaction::Reply(ServerMessage::HandshakeAccepted { session_id: 7 })
        );
    }

    #[test]
    fn une_version_incompatible_est_refusee_explicitement() {
        let mut session = Session::new(7);
        assert_eq!(
            session.on_message(handshake(PROTOCOL_VERSION + 1)),
            Reaction::ReplyAndClose(ServerMessage::HandshakeRejected {
                expected_version: PROTOCOL_VERSION,
            })
        );
        // Le refus n'authentifie rien : la trame suivante est hors sequence.
        assert_eq!(
            session.on_message(ClientMessage::Ping { nonce: 1 }),
            Reaction::Close
        );
    }

    #[test]
    fn toute_trame_avant_le_handshake_ferme_la_connexion() {
        for message in [
            ClientMessage::Ping { nonce: 1 },
            ClientMessage::EnterWorld,
            ClientMessage::Move { x: 0, y: 0 },
        ] {
            let mut session = Session::new(7);
            assert_eq!(session.on_message(message), Reaction::Close, "{message:?}");
        }
    }

    #[test]
    fn le_ping_est_servi_avant_et_apres_l_entree_dans_le_monde() {
        for mut session in [authenticated(), in_world()] {
            assert_eq!(
                session.on_message(ClientMessage::Ping { nonce: 99 }),
                Reaction::Reply(ServerMessage::Pong { nonce: 99 })
            );
        }
    }

    #[test]
    fn entrer_dans_le_monde_demande_l_action_correspondante() {
        let mut session = authenticated();
        assert_eq!(
            session.on_message(ClientMessage::EnterWorld),
            Reaction::Perform(WorldCommand::Enter)
        );
    }

    #[test]
    fn se_deplacer_avant_d_entrer_ferme_la_connexion() {
        let mut session = authenticated();
        assert_eq!(
            session.on_message(ClientMessage::Move { x: 1, y: 1 }),
            Reaction::Close
        );
    }

    #[test]
    fn entrer_deux_fois_ferme_la_connexion() {
        let mut session = in_world();
        assert_eq!(
            session.on_message(ClientMessage::EnterWorld),
            Reaction::Close
        );
    }

    #[test]
    fn un_second_handshake_ferme_la_connexion() {
        for mut session in [authenticated(), in_world()] {
            assert_eq!(
                session.on_message(handshake(PROTOCOL_VERSION)),
                Reaction::Close
            );
        }
    }

    #[test]
    fn le_deplacement_est_transmis_tel_quel_au_monde() {
        let mut session = in_world();
        assert_eq!(
            session.on_message(ClientMessage::Move { x: -42, y: 7 }),
            Reaction::Perform(WorldCommand::Move { x: -42, y: 7 })
        );
    }
}
