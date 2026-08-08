//! Adaptateur reseau : traduit un flux TCP en trames, delegue la decision a
//! [`session::Session`], l'etat partage a [`world::World`] et la durabilite a
//! `hwarang_storage`. Aucune regle de jeu ici.

mod session;
mod world;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hwarang_domain::ProgressionCurve;
use hwarang_protocol::{AuthRefusal, ClientMessage, DecodeError, MAX_FRAME_LEN, ServerMessage};
use hwarang_storage::{AccountId, SavedCharacter, Storage, StorageError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{Sender, channel};

use session::{AuthCommand, Reaction, Session, WorldCommand};
use world::World;

const DEFAULT_BIND: &str = "127.0.0.1:13000";
const DEFAULT_DATABASE: &str = "hwarang.sqlite";

/// Compteur de sessions. Suffisant pour correler les journaux ; un identifiant
/// non devinable sera necessaire le jour ou il servira a l'authentification.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Ce que partagent toutes les connexions.
struct Shared {
    world: World,
    storage: Storage,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("HWARANG_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let database =
        PathBuf::from(std::env::var("HWARANG_DB").unwrap_or_else(|_| DEFAULT_DATABASE.to_owned()));

    let storage = Storage::open(&database).map_err(|error| {
        std::io::Error::other(format!(
            "base {} inutilisable : {error}",
            database.display()
        ))
    })?;
    let shared = Arc::new(Shared {
        world: World::new(),
        storage,
    });

    let listener = TcpListener::bind(&bind).await?;
    println!(
        "hwarang-server ecoute sur {bind}, base {}",
        database.display()
    );

    loop {
        tokio::select! {
            incoming = listener.accept() => {
                let (stream, peer) = incoming?;
                let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    let session = Session::new(id);
                    if let Err(error) = serve(stream, session, &shared).await {
                        eprintln!("session {id} ({peer}) interrompue : {error}");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => {
                println!("arret demande, plus de nouvelle connexion");
                return Ok(());
            }
        }
    }
}

/// Boucle d'une connexion : lit les trames du client et ecrit celles que le
/// monde destine a ce joueur.
///
/// Les deux sens vivent dans la meme tache via `select!` : une notification de
/// deplacement d'un voisin doit partir sans attendre que ce client parle.
async fn serve(
    stream: TcpStream,
    mut session: Session,
    shared: &Arc<Shared>,
) -> std::io::Result<()> {
    let entity = session.entity_id();
    let outcome = run(stream, &mut session, shared).await;

    // Quelle que soit la cause de la sortie — deconnexion propre, violation de
    // protocole, erreur reseau — l'etat doit etre sauvegarde puis l'entite
    // retiree. La sauvegarde vient d'abord : retirer l'entite effacerait
    // justement ce qu'il faut ecrire.
    persist(shared, &session, entity).await;
    shared.world.leave(entity);
    println!(
        "session {entity} terminee, {} en jeu",
        shared.world.population()
    );

    outcome
}

async fn run(
    stream: TcpStream,
    session: &mut Session,
    shared: &Arc<Shared>,
) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let (outbox, mut inbox) = channel::<ServerMessage>(world::OUTBOX_CAPACITY);

    let mut buffer = Vec::with_capacity(MAX_FRAME_LEN);
    let mut chunk = [0_u8; 1024];
    // Horodatages serveur des dernieres actions retenues : le client n'a aucune
    // prise sur la mesure du temps qui borne sa vitesse et sa cadence.
    let mut clock = ActionClock::new();

    loop {
        tokio::select! {
            outgoing = inbox.recv() => {
                let Some(message) = outgoing else { return Ok(()) };
                writer.write_all(&message.encode()).await?;
            }

            read = reader.read(&mut chunk) => {
                let read = read?;
                if read == 0 {
                    return Ok(());
                }
                buffer.extend_from_slice(&chunk[..read]);

                // Un client qui emet sans jamais completer de trame est ejecte
                // plutot que de faire enfler la memoire du serveur.
                if buffer.len() > MAX_FRAME_LEN {
                    return Ok(());
                }

                while let Some(reaction) = next_reaction(&mut buffer, session) {
                    match reaction {
                        Reaction::Reply(message) => {
                            writer.write_all(&message.encode()).await?;
                        }
                        Reaction::ReplyAndClose(message) => {
                            writer.write_all(&message.encode()).await?;
                            return Ok(());
                        }
                        Reaction::Close => return Ok(()),
                        Reaction::Perform(command) => {
                            perform(command, session, shared, &outbox, &mut clock).await;
                        }
                        Reaction::Authenticate(command) => {
                            let verdict = authenticate(shared, session, command).await;
                            match verdict {
                                Reaction::Reply(message) => {
                                    writer.write_all(&message.encode()).await?;
                                }
                                // `authenticate` ne produit que des reponses.
                                other => {
                                    debug_assert!(false, "verdict inattendu : {other:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Verifie des identifiants et met la session a jour.
///
/// Le hachage Argon2 est deliberement couteux en temps processeur. L'executer
/// sur le fil asynchrone bloquerait toutes les autres connexions servies par le
/// meme fil pendant la verification : `spawn_blocking` le confine a un fil dedie.
async fn authenticate(
    shared: &Arc<Shared>,
    session: &mut Session,
    command: AuthCommand,
) -> Reaction {
    let shared = Arc::clone(shared);
    let outcome = tokio::task::spawn_blocking(move || match command {
        AuthCommand::Register { username, password } => {
            shared.storage.register(&username, &password)
        }
        AuthCommand::Login { username, password } => {
            shared.storage.authenticate(&username, &password)
        }
    })
    .await;

    match outcome {
        Ok(Ok(account)) => session.authenticated_as(account.as_u64()),
        Ok(Err(error)) => Session::authentication_refused(refusal_of(&error)),
        Err(error) => {
            eprintln!("verification interrompue : {error}");
            Session::authentication_refused(AuthRefusal::Unavailable)
        }
    }
}

const fn refusal_of(error: &StorageError) -> AuthRefusal {
    match error {
        StorageError::UsernameTaken => AuthRefusal::UsernameTaken,
        StorageError::InvalidCredentials => AuthRefusal::InvalidCredentials,
        StorageError::Credentials(_) => AuthRefusal::MalformedCredentials,
        // Une panne de stockage ne doit rien reveler de plus qu'elle-meme.
        StorageError::Database(_) | StorageError::Hashing(_) | StorageError::CorruptCharacter => {
            AuthRefusal::Unavailable
        }
    }
}

/// Ecrit l'etat du personnage, si la session en avait un en jeu.
async fn persist(shared: &Arc<Shared>, session: &Session, entity: u64) {
    let (Some(account), Some((character, position))) =
        (session.account_id(), shared.world.snapshot(entity))
    else {
        return;
    };

    let shared = Arc::clone(shared);
    let saved = SavedCharacter::of(&character, position);
    let account = AccountId::from_u64(account);

    // L'ecriture disque est bloquante, comme le hachage.
    if let Err(error) =
        tokio::task::spawn_blocking(move || shared.storage.save_character(account, &saved)).await
    {
        eprintln!("sauvegarde interrompue pour l'entite {entity} : {error}");
    }
}

/// Horodatages des dernieres actions d'une connexion.
///
/// Un compteur par nature d'action : un joueur qui court ne doit pas voir sa
/// cadence d'attaque remise a zero, et inversement.
struct ActionClock {
    last_move: Instant,
    last_attack: Instant,
}

impl ActionClock {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            last_move: now,
            last_attack: now,
        }
    }

    /// Temps ecoule depuis `marker`, puis remise a zero.
    ///
    /// La remise a zero est inconditionnelle, y compris quand l'action est
    /// refusee : sinon un client rejete accumulerait du temps et s'offrirait
    /// ensuite un grand saut ou une salve d'attaques.
    fn take(marker: &mut Instant) -> u64 {
        let now = Instant::now();
        let elapsed = u64::try_from(now.duration_since(*marker).as_millis()).unwrap_or(u64::MAX);
        *marker = now;
        elapsed
    }
}

async fn perform(
    command: WorldCommand,
    session: &Session,
    shared: &Arc<Shared>,
    outbox: &Sender<ServerMessage>,
    clock: &mut ActionClock,
) {
    let id = session.entity_id();
    match command {
        WorldCommand::Enter => {
            let restored = load_character(shared, session, id).await;
            let known = restored.is_some();
            let at = shared.world.enter(id, outbox.clone(), restored);
            *clock = ActionClock::new();
            println!(
                "entite {id} {} en ({}, {}), {} en jeu",
                if known { "revient" } else { "apparait" },
                at.x,
                at.y,
                shared.world.population()
            );
        }
        WorldCommand::Move { x, y } => {
            let elapsed = ActionClock::take(&mut clock.last_move);
            shared.world.request_move(id, x, y, elapsed);
        }
        WorldCommand::Attack { target } => {
            let elapsed = ActionClock::take(&mut clock.last_attack);
            shared.world.request_attack(id, target, elapsed);
        }
        WorldCommand::Respawn => {
            shared.world.request_respawn(id);
            *clock = ActionClock::new();
        }
    }
}

/// Recharge le personnage du compte authentifie, s'il en a deja un.
///
/// Une sauvegarde illisible n'empeche pas de jouer : le personnage repart neuf.
/// Refuser la connexion punirait le joueur d'un defaut qui n'est pas le sien, et
/// un compte devenu injouable ne se repare pas tout seul.
async fn load_character(
    shared: &Arc<Shared>,
    session: &Session,
    entity: u64,
) -> Option<(hwarang_domain::Character, hwarang_domain::Position)> {
    let account = AccountId::from_u64(session.account_id()?);
    let shared_for_task = Arc::clone(shared);

    let loaded =
        tokio::task::spawn_blocking(move || shared_for_task.storage.load_character(account))
            .await
            .ok()?;

    match loaded {
        Ok(Some(saved)) => {
            let position = saved.position;
            match saved.into_character(
                hwarang_domain::CharacterId::new(entity),
                ProgressionCurve::DEFAULT,
            ) {
                Ok(character) => Some((character, position)),
                Err(error) => {
                    eprintln!(
                        "sauvegarde illisible pour l'entite {entity}, reprise a neuf : {error}"
                    );
                    None
                }
            }
        }
        Ok(None) => None,
        Err(error) => {
            eprintln!("lecture impossible pour l'entite {entity}, reprise a neuf : {error}");
            None
        }
    }
}

/// Consomme une trame complete du tampon, si elle est disponible.
///
/// `None` signifie « il manque des octets », pas « rien a faire » : c'est ce qui
/// distingue une trame fragmentee par TCP d'un client silencieux.
fn next_reaction(buffer: &mut Vec<u8>, session: &mut Session) -> Option<Reaction> {
    match ClientMessage::decode(buffer) {
        Ok((message, consumed)) => {
            buffer.drain(..consumed);
            Some(session.on_message(message))
        }
        Err(DecodeError::Incomplete) => None,
        Err(error) => {
            // Trame invalide : on coupe. Tenter de resynchroniser sur un flux
            // deja corrompu ouvre plus de surface d'attaque que ca n'en ferme.
            eprintln!("trame invalide, fermeture : {error:?}");
            Some(Reaction::Close)
        }
    }
}
