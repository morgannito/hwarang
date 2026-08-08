//! Politique de session, sans I/O.
//!
//! Separer la decision de l'execution rend le cycle de vie d'une connexion
//! testable a froid : pas de port a ouvrir ni de base a peupler pour verifier
//! qu'un client desynchronise est bien ejecte.

use hwarang_protocol::{AuthRefusal, ClientMessage, PROTOCOL_VERSION, ServerMessage};

/// Action a executer sur l'etat partage du monde.
///
/// La session ne touche pas au monde elle-meme : elle decrit ce qu'il faut
/// faire, l'appelant l'applique. C'est ce qui la garde testable sans monde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldCommand {
    Enter,
    Move { x: i32, y: i32 },
    Attack { target: u64 },
    Respawn,
}

/// Action a executer sur le stockage des comptes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthCommand {
    Register { username: String, password: String },
    Login { username: String, password: String },
}

/// Suite a donner apres traitement d'une trame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// Repondre et garder la connexion.
    Reply(ServerMessage),
    /// Repondre puis fermer.
    ReplyAndClose(ServerMessage),
    /// Fermer sans repondre : le client a viole le protocole.
    Close,
    /// Agir sur le monde ; les notifications partiront par le canal de sortie.
    Perform(WorldCommand),
    /// Verifier des identifiants ; la session attend le verdict.
    Authenticate(AuthCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingHandshake,
    AwaitingAuth,
    Authenticated { account_id: u64 },
    InWorld { account_id: u64 },
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

    /// Compte authentifie sur cette connexion, s'il y en a un.
    #[must_use]
    pub const fn account_id(&self) -> Option<u64> {
        match self.state {
            State::Authenticated { account_id } | State::InWorld { account_id } => Some(account_id),
            State::AwaitingHandshake | State::AwaitingAuth => None,
        }
    }

    /// Enregistre le succes d'une authentification.
    ///
    /// Le verdict vient de l'exterieur : la session ne sait pas verifier un mot
    /// de passe, elle sait seulement ce que cela autorise ensuite.
    pub fn authenticated_as(&mut self, account_id: u64) -> Reaction {
        self.state = State::Authenticated { account_id };
        Reaction::Reply(ServerMessage::Authenticated { account_id })
    }

    /// Enregistre l'echec d'une authentification.
    ///
    /// La connexion reste ouverte : un utilisateur qui se trompe de mot de passe
    /// doit pouvoir reessayer sans rouvrir une socket. La limitation du nombre
    /// d'essais releve d'une couche superieure, qui voit toutes les connexions.
    #[must_use]
    pub const fn authentication_refused(reason: AuthRefusal) -> Reaction {
        Reaction::Reply(ServerMessage::AuthRefused { reason })
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
                    self.state = State::AwaitingAuth;
                    Reaction::Reply(ServerMessage::HandshakeAccepted {
                        session_id: self.session_id,
                    })
                } else {
                    Reaction::ReplyAndClose(ServerMessage::HandshakeRejected {
                        expected_version: PROTOCOL_VERSION,
                    })
                }
            }

            // Le ping n'a pas besoin du monde : il maintient la connexion
            // pendant l'authentification comme pendant le jeu.
            (
                State::AwaitingAuth | State::Authenticated { .. } | State::InWorld { .. },
                ClientMessage::Ping { nonce },
            ) => Reaction::Reply(ServerMessage::Pong { nonce }),

            (State::AwaitingAuth, ClientMessage::Register { username, password }) => {
                Reaction::Authenticate(AuthCommand::Register { username, password })
            }
            (State::AwaitingAuth, ClientMessage::Login { username, password }) => {
                Reaction::Authenticate(AuthCommand::Login { username, password })
            }

            // S'authentifier deux fois changerait de compte en cours de session,
            // avec une entite deja liee au premier. Refuse sans fermer : le
            // client peut simplement avoir rejoue sa trame.
            (
                State::Authenticated { .. } | State::InWorld { .. },
                ClientMessage::Register { .. } | ClientMessage::Login { .. },
            ) => Reaction::Reply(ServerMessage::AuthRefused {
                reason: AuthRefusal::AlreadyAuthenticated,
            }),

            (State::Authenticated { account_id }, ClientMessage::EnterWorld) => {
                self.state = State::InWorld { account_id };
                Reaction::Perform(WorldCommand::Enter)
            }

            (State::InWorld { .. }, ClientMessage::Move { x, y }) => {
                Reaction::Perform(WorldCommand::Move { x, y })
            }

            // Attaque et reapparition sont refusees par le monde, pas ici : leur
            // recevabilite depend de l'etat du jeu (portee, cadence, mort), que
            // la session ne connait pas et n'a pas a dupliquer.
            (State::InWorld { .. }, ClientMessage::Attack { target }) => {
                Reaction::Perform(WorldCommand::Attack { target })
            }
            (State::InWorld { .. }, ClientMessage::Respawn) => {
                Reaction::Perform(WorldCommand::Respawn)
            }

            // Reste : trame avant le handshake, action avant authentification,
            // deplacement avant l'entree dans le monde, seconde entree.
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

    fn login() -> ClientMessage {
        ClientMessage::Login {
            username: "morgann".to_owned(),
            password: "secret".to_owned(),
        }
    }

    fn awaiting_auth() -> Session {
        let mut session = Session::new(7);
        session.on_message(handshake(PROTOCOL_VERSION));
        session
    }

    fn authenticated() -> Session {
        let mut session = awaiting_auth();
        session.authenticated_as(42);
        session
    }

    fn in_world() -> Session {
        let mut session = authenticated();
        session.on_message(ClientMessage::EnterWorld);
        session
    }

    #[test]
    fn un_handshake_valide_ouvre_l_authentification() {
        let mut session = Session::new(7);
        assert_eq!(
            session.on_message(handshake(PROTOCOL_VERSION)),
            Reaction::Reply(ServerMessage::HandshakeAccepted { session_id: 7 })
        );
        assert_eq!(session.account_id(), None);
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
            ClientMessage::Attack { target: 1 },
            ClientMessage::Respawn,
            login(),
        ] {
            let mut session = Session::new(7);
            assert_eq!(
                session.on_message(message.clone()),
                Reaction::Close,
                "{message:?}"
            );
        }
    }

    #[test]
    fn les_identifiants_sont_transmis_a_la_verification() {
        let mut session = awaiting_auth();
        assert_eq!(
            session.on_message(login()),
            Reaction::Authenticate(AuthCommand::Login {
                username: "morgann".to_owned(),
                password: "secret".to_owned(),
            })
        );
    }

    #[test]
    fn la_session_ne_retient_le_compte_qu_apres_verdict() {
        let mut session = awaiting_auth();
        session.on_message(login());
        // La demande seule n'authentifie rien.
        assert_eq!(session.account_id(), None);

        session.authenticated_as(42);
        assert_eq!(session.account_id(), Some(42));
    }

    #[test]
    fn un_echec_d_authentification_laisse_la_connexion_ouverte() {
        let mut session = awaiting_auth();
        assert_eq!(
            Session::authentication_refused(AuthRefusal::InvalidCredentials),
            Reaction::Reply(ServerMessage::AuthRefused {
                reason: AuthRefusal::InvalidCredentials
            })
        );
        // On peut reessayer sans rouvrir de socket.
        assert!(matches!(
            session.on_message(login()),
            Reaction::Authenticate(_)
        ));
    }

    #[test]
    fn agir_avant_authentification_ferme_la_connexion() {
        for message in [
            ClientMessage::EnterWorld,
            ClientMessage::Move { x: 1, y: 1 },
            ClientMessage::Attack { target: 2 },
            ClientMessage::Respawn,
        ] {
            let mut session = awaiting_auth();
            assert_eq!(
                session.on_message(message.clone()),
                Reaction::Close,
                "{message:?}"
            );
        }
    }

    #[test]
    fn se_reauthentifier_est_refuse_sans_fermer() {
        for mut session in [authenticated(), in_world()] {
            assert_eq!(
                session.on_message(login()),
                Reaction::Reply(ServerMessage::AuthRefused {
                    reason: AuthRefusal::AlreadyAuthenticated
                })
            );
            // Le compte d'origine reste celui de la session.
            assert_eq!(session.account_id(), Some(42));
        }
    }

    #[test]
    fn le_ping_est_servi_a_tous_les_stades_posterieurs_au_handshake() {
        for mut session in [awaiting_auth(), authenticated(), in_world()] {
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
        for mut session in [awaiting_auth(), authenticated(), in_world()] {
            assert_eq!(
                session.on_message(handshake(PROTOCOL_VERSION)),
                Reaction::Close
            );
        }
    }

    #[test]
    fn les_actions_de_jeu_sont_transmises_telles_quelles() {
        let mut session = in_world();
        assert_eq!(
            session.on_message(ClientMessage::Move { x: -42, y: 7 }),
            Reaction::Perform(WorldCommand::Move { x: -42, y: 7 })
        );
        assert_eq!(
            session.on_message(ClientMessage::Attack { target: 5 }),
            Reaction::Perform(WorldCommand::Attack { target: 5 })
        );
        assert_eq!(
            session.on_message(ClientMessage::Respawn),
            Reaction::Perform(WorldCommand::Respawn)
        );
    }
}
