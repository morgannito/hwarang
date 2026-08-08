//! Adaptateur reseau : traduit un flux TCP en trames, delegue la decision a
//! [`session::Session`], et n'embarque aucune regle de jeu.

mod session;

use std::sync::atomic::{AtomicU64, Ordering};

use hwarang_protocol::{ClientMessage, DecodeError, MAX_FRAME_LEN, ServerMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use session::{Reaction, Session};

const DEFAULT_BIND: &str = "127.0.0.1:13000";

/// Compteur de sessions. Suffisant pour correler les journaux ; un identifiant
/// non devinable sera necessaire le jour ou il servira a l'authentification.
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let bind = std::env::var("HWARANG_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let listener = TcpListener::bind(&bind).await?;
    println!("hwarang-server ecoute sur {bind}");

    loop {
        tokio::select! {
            incoming = listener.accept() => {
                let (stream, peer) = incoming?;
                let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    if let Err(error) = serve(stream, Session::new(id)).await {
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

/// Boucle de lecture d'une connexion.
///
/// Le tampon est borne par [`MAX_FRAME_LEN`] : un client qui envoie un flux
/// continu sans jamais completer de trame est ejecte au lieu de faire enfler la
/// memoire du serveur.
async fn serve(mut stream: TcpStream, mut session: Session) -> std::io::Result<()> {
    let mut buffer = Vec::with_capacity(MAX_FRAME_LEN);
    let mut chunk = [0_u8; 1024];

    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);

        if buffer.len() > MAX_FRAME_LEN {
            return Ok(());
        }

        while let Some(reaction) = next_reaction(&mut buffer, &mut session) {
            match reaction {
                Reaction::Reply(message) => write_message(&mut stream, message).await?,
                Reaction::ReplyAndClose(message) => {
                    write_message(&mut stream, message).await?;
                    return Ok(());
                }
                Reaction::Close => return Ok(()),
            }
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

async fn write_message(stream: &mut TcpStream, message: ServerMessage) -> std::io::Result<()> {
    stream.write_all(&message.encode()).await
}
