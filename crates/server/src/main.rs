//! Adaptateur reseau : traduit un flux TCP en trames, delegue la decision a
//! [`session::Session`] et l'etat partage a [`world::World`]. Aucune regle de
//! jeu ici.

mod session;
mod world;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hwarang_protocol::{ClientMessage, DecodeError, MAX_FRAME_LEN, ServerMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use session::{Reaction, Session, WorldCommand};
use world::World;

const DEFAULT_BIND: &str = "127.0.0.1:13000";

/// Compteur de sessions. Suffisant pour correler les journaux ; un identifiant
/// non devinable sera necessaire le jour ou il servira a l'authentification.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("HWARANG_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let listener = TcpListener::bind(&bind).await?;
    let world = Arc::new(World::new());
    println!("hwarang-server ecoute sur {bind}");

    loop {
        tokio::select! {
            incoming = listener.accept() => {
                let (stream, peer) = incoming?;
                let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
                let world = Arc::clone(&world);
                tokio::spawn(async move {
                    let session = Session::new(id);
                    let entity = session.entity_id();
                    if let Err(error) = serve(stream, session, &world).await {
                        eprintln!("session {id} ({peer}) interrompue : {error}");
                    }
                    // Quelle que soit la cause de la sortie — deconnexion propre,
                    // violation de protocole, erreur reseau — l'entite doit
                    // disparaitre pour les autres joueurs.
                    world.leave(entity);
                    println!("session {id} terminee, {} en jeu", world.population());
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
async fn serve(stream: TcpStream, mut session: Session, world: &World) -> std::io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let (outbox, mut inbox) = unbounded_channel::<ServerMessage>();

    let mut buffer = Vec::with_capacity(MAX_FRAME_LEN);
    let mut chunk = [0_u8; 1024];
    // Horodatage serveur du dernier deplacement retenu : le client n'a aucune
    // prise sur la mesure du temps qui borne sa vitesse.
    let mut last_move = Instant::now();

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

                while let Some(reaction) = next_reaction(&mut buffer, &mut session) {
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
                            perform(command, &session, world, &outbox, &mut last_move);
                        }
                    }
                }
            }
        }
    }
}

fn perform(
    command: WorldCommand,
    session: &Session,
    world: &World,
    outbox: &UnboundedSender<ServerMessage>,
    last_move: &mut Instant,
) {
    match command {
        WorldCommand::Enter => {
            let at = world.enter(session.entity_id(), outbox.clone());
            *last_move = Instant::now();
            println!(
                "entite {} apparait en ({}, {}), {} en jeu",
                session.entity_id(),
                at.x,
                at.y,
                world.population()
            );
        }
        WorldCommand::Move { x, y } => {
            let now = Instant::now();
            let elapsed_ms =
                u64::try_from(now.duration_since(*last_move).as_millis()).unwrap_or(u64::MAX);
            world.request_move(session.entity_id(), x, y, elapsed_ms);
            // L'horloge repart meme si le deplacement est refuse : sinon un
            // client rejete accumulerait du temps et s'offrirait un grand saut.
            *last_move = now;
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
