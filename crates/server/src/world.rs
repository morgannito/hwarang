//! Registre des entites presentes et diffusion aux voisins.
//!
//! Toutes les regles (validation d'un deplacement, portee de vue, decoupage en
//! cellules) viennent de `hwarang_domain`. Ce module ne fait que tenir l'etat
//! partage et router les notifications.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use hwarang_domain::{CellCoord, Grid, MoveVerdict, MovementRule, Position};
use hwarang_protocol::{EntityId, ServerMessage};
use tokio::sync::mpsc::UnboundedSender;

/// Rayon de perception, identique pour toutes les entites.
///
/// L'uniformite rend la visibilite **symetrique** : si A voit B alors B voit A.
/// C'est ce qui permet de ne stocker la relation qu'une fois par cote et de
/// n'examiner, apres un deplacement, que le voisinage du seul mobile.
const VIEW_RADIUS_CM: u32 = Grid::DEFAULT_VIEW_RADIUS_CM;

/// Canal de sortie vers une connexion.
type Outbox = UnboundedSender<ServerMessage>;

struct Entity {
    position: Position,
    rule: MovementRule,
    outbox: Outbox,
    /// Entites actuellement percues. Maintenu symetriquement.
    visible: HashSet<EntityId>,
}

#[derive(Default)]
struct State {
    entities: HashMap<EntityId, Entity>,
    /// Index spatial : evite de comparer chaque mobile a toute la population.
    cells: HashMap<CellCoord, HashSet<EntityId>>,
}

/// Etat partage du monde.
///
/// Le verrou est volontairement synchrone et jamais tenu au travers d'un
/// `await` : les sections critiques se limitent a des operations sur des tables
/// en memoire, et l'ecriture reseau se fait apres relachement, via les canaux.
pub struct World {
    grid: Grid,
    state: Mutex<State>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    #[must_use]
    pub fn new() -> Self {
        Self {
            grid: Grid::with_default_view(),
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // Un thread qui panique en section critique laisse l'etat coherent :
        // toutes les mutations sont des insertions ou retraits atomiques sur des
        // tables. Empoisonner le monde entier serait une reaction disproportionnee.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fait apparaitre une entite et lui envoie le voisinage deja present.
    ///
    /// Retourne la position d'apparition.
    pub fn enter(&self, id: EntityId, outbox: Outbox) -> Position {
        let position = spawn_position(id);
        let mut state = self.lock();

        state.entities.insert(
            id,
            Entity {
                position,
                rule: MovementRule::running(),
                outbox,
                visible: HashSet::new(),
            },
        );
        state
            .cells
            .entry(self.grid.cell_of(position))
            .or_default()
            .insert(id);

        send(
            &state,
            id,
            ServerMessage::WorldEntered {
                entity_id: id,
                x: position.x,
                y: position.y,
            },
        );
        self.refresh_visibility(&mut state, id);

        position
    }

    /// Traite un deplacement revendique par le client.
    ///
    /// `elapsed_ms` est mesure par le serveur, jamais annonce par le client :
    /// laisser ce dernier declarer le temps ecoule reviendrait a lui confier la
    /// cle de sa propre limite de vitesse.
    pub fn request_move(&self, id: EntityId, x: i32, y: i32, elapsed_ms: u64) {
        let target = Position::new(x, y);
        let mut state = self.lock();

        let Some(entity) = state.entities.get(&id) else {
            return;
        };
        let from = entity.position;

        if let MoveVerdict::TooFast { .. } = entity.rule.verify(from, target, elapsed_ms) {
            // Le serveur reaffirme sa position : le client se resynchronise
            // sans aller-retour supplementaire.
            send(
                &state,
                id,
                ServerMessage::MoveRejected {
                    x: from.x,
                    y: from.y,
                },
            );
            return;
        }

        if let Some(entity) = state.entities.get_mut(&id) {
            entity.position = target;
        }

        if self.grid.crosses_cell(from, target) {
            let (before, after) = (self.grid.cell_of(from), self.grid.cell_of(target));
            if let Some(cell) = state.cells.get_mut(&before) {
                cell.remove(&id);
                if cell.is_empty() {
                    // Sans ce retrait, une carte parcourue longtemps accumule
                    // indefiniment des cellules vides.
                    state.cells.remove(&before);
                }
            }
            state.cells.entry(after).or_default().insert(id);
        }

        self.refresh_visibility(&mut state, id);
    }

    /// Retire une entite et previent ceux qui la percevaient.
    pub fn leave(&self, id: EntityId) {
        let mut state = self.lock();
        let Some(entity) = state.entities.remove(&id) else {
            return;
        };

        let cell = self.grid.cell_of(entity.position);
        if let Some(occupants) = state.cells.get_mut(&cell) {
            occupants.remove(&id);
            if occupants.is_empty() {
                state.cells.remove(&cell);
            }
        }

        for other in entity.visible {
            if let Some(watcher) = state.entities.get_mut(&other) {
                watcher.visible.remove(&id);
                let _ = watcher
                    .outbox
                    .send(ServerMessage::EntityVanished { entity_id: id });
            }
        }
    }

    #[must_use]
    pub fn population(&self) -> usize {
        self.lock().entities.len()
    }

    /// Recalcule ce que `id` percoit, et symetriquement ce que les autres
    /// percoivent de lui.
    ///
    /// L'ensemble examine reunit les candidats du bloc de neuf cellules et les
    /// entites deja percues : sans ces dernieres, une entite sortie du bloc ne
    /// recevrait jamais sa disparition.
    fn refresh_visibility(&self, state: &mut State, id: EntityId) {
        let Some(subject) = state.entities.get(&id) else {
            return;
        };
        let position = subject.position;
        let previously_visible = subject.visible.clone();

        let mut examined: HashSet<EntityId> = previously_visible.clone();
        for cell in self.grid.neighbourhood(self.grid.cell_of(position)) {
            if let Some(occupants) = state.cells.get(&cell) {
                examined.extend(occupants.iter().copied());
            }
        }
        examined.remove(&id);

        for other in examined {
            let Some(candidate) = state.entities.get(&other) else {
                continue;
            };
            let other_position = candidate.position;
            let visible_now = position.is_within(other_position, VIEW_RADIUS_CM);
            let visible_before = previously_visible.contains(&other);

            match (visible_before, visible_now) {
                (false, true) => {
                    link(state, id, other);
                    send(state, id, appeared(other, other_position));
                    send(state, other, appeared(id, position));
                }
                (true, false) => {
                    unlink(state, id, other);
                    send(
                        state,
                        id,
                        ServerMessage::EntityVanished { entity_id: other },
                    );
                    send(
                        state,
                        other,
                        ServerMessage::EntityVanished { entity_id: id },
                    );
                }
                (true, true) => send(state, other, moved(id, position)),
                (false, false) => {}
            }
        }
    }
}

const fn appeared(entity_id: EntityId, position: Position) -> ServerMessage {
    ServerMessage::EntityAppeared {
        entity_id,
        x: position.x,
        y: position.y,
    }
}

const fn moved(entity_id: EntityId, position: Position) -> ServerMessage {
    ServerMessage::EntityMoved {
        entity_id,
        x: position.x,
        y: position.y,
    }
}

fn link(state: &mut State, a: EntityId, b: EntityId) {
    if let Some(entity) = state.entities.get_mut(&a) {
        entity.visible.insert(b);
    }
    if let Some(entity) = state.entities.get_mut(&b) {
        entity.visible.insert(a);
    }
}

fn unlink(state: &mut State, a: EntityId, b: EntityId) {
    if let Some(entity) = state.entities.get_mut(&a) {
        entity.visible.remove(&b);
    }
    if let Some(entity) = state.entities.get_mut(&b) {
        entity.visible.remove(&a);
    }
}

/// Un envoi qui echoue signifie que la connexion est deja fermee ; le retrait
/// de l'entite viendra de la tache de lecture.
fn send(state: &State, id: EntityId, message: ServerMessage) {
    if let Some(entity) = state.entities.get(&id) {
        let _ = entity.outbox.send(message);
    }
}

/// Repartit les apparitions sur une spirale carree autour de l'origine.
///
/// Faire apparaitre tout le monde au meme point rendrait invisible toute erreur
/// de calcul de visibilite : chacun verrait chacun, quelle que soit la grille.
fn spawn_position(id: EntityId) -> Position {
    let step = 500;
    let index = i32::try_from(id % 64).unwrap_or(0);
    Position::new((index % 8) * step, (index / 8) * step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    fn join(world: &World, id: EntityId) -> UnboundedReceiver<ServerMessage> {
        let (tx, rx) = unbounded_channel();
        world.enter(id, tx);
        rx
    }

    fn drain(rx: &mut UnboundedReceiver<ServerMessage>) -> Vec<ServerMessage> {
        let mut messages = Vec::new();
        while let Ok(message) = rx.try_recv() {
            messages.push(message);
        }
        messages
    }

    /// Deplace sans contrainte de vitesse, pour isoler la visibilite.
    fn teleport(world: &World, id: EntityId, x: i32, y: i32) {
        world.request_move(id, x, y, u64::MAX / 4);
    }

    #[test]
    fn entrer_dans_le_monde_annonce_sa_propre_position() {
        let world = World::new();
        let mut rx = join(&world, 1);

        assert!(matches!(
            drain(&mut rx).first(),
            Some(ServerMessage::WorldEntered { entity_id: 1, .. })
        ));
        assert_eq!(world.population(), 1);
    }

    #[test]
    fn deux_entites_proches_se_decouvrent_mutuellement() {
        let world = World::new();
        let mut first = join(&world, 1);
        let mut second = join(&world, 2);

        assert!(
            drain(&mut first)
                .iter()
                .any(|m| matches!(m, ServerMessage::EntityAppeared { entity_id: 2, .. }))
        );
        assert!(
            drain(&mut second)
                .iter()
                .any(|m| matches!(m, ServerMessage::EntityAppeared { entity_id: 1, .. }))
        );
    }

    #[test]
    fn une_entite_hors_de_portee_reste_invisible() {
        let world = World::new();
        let mut first = join(&world, 1);
        let _second = join(&world, 2);
        drain(&mut first);

        teleport(&world, 2, 500_000, 500_000);

        assert!(
            drain(&mut first)
                .iter()
                .any(|m| matches!(m, ServerMessage::EntityVanished { entity_id: 2 }))
        );
    }

    #[test]
    fn s_eloigner_puis_revenir_redeclenche_l_apparition() {
        let world = World::new();
        let mut first = join(&world, 1);
        let _second = join(&world, 2);

        teleport(&world, 2, 500_000, 500_000);
        drain(&mut first);
        teleport(&world, 2, 0, 0);

        assert!(
            drain(&mut first)
                .iter()
                .any(|m| matches!(m, ServerMessage::EntityAppeared { entity_id: 2, .. })),
            "le retour dans la vue n'a pas ete annonce"
        );
    }

    #[test]
    fn un_deplacement_visible_est_diffuse_aux_voisins() {
        let world = World::new();
        let mut first = join(&world, 1);
        let _second = join(&world, 2);
        drain(&mut first);

        teleport(&world, 2, 100, 100);

        assert!(drain(&mut first).iter().any(|m| matches!(
            m,
            ServerMessage::EntityMoved {
                entity_id: 2,
                x: 100,
                y: 100
            }
        )));
    }

    #[test]
    fn un_deplacement_trop_rapide_est_refuse_et_la_position_reaffirmee() {
        let world = World::new();
        let mut rx = join(&world, 1);
        let spawn = spawn_position(1);
        drain(&mut rx);

        world.request_move(1, spawn.x + 1_000_000, spawn.y, 100);

        assert_eq!(
            drain(&mut rx),
            vec![ServerMessage::MoveRejected {
                x: spawn.x,
                y: spawn.y
            }]
        );
    }

    #[test]
    fn un_refus_ne_deplace_pas_l_entite() {
        let world = World::new();
        let mut first = join(&world, 1);
        let mut second = join(&world, 2);
        drain(&mut first);
        drain(&mut second);

        world.request_move(2, 999_999, 999_999, 10);

        // Le voisin ne doit voir ni deplacement, ni disparition.
        assert!(drain(&mut first).is_empty());
    }

    #[test]
    fn quitter_previent_ceux_qui_percevaient_l_entite() {
        let world = World::new();
        let mut first = join(&world, 1);
        let _second = join(&world, 2);
        drain(&mut first);

        world.leave(2);

        assert_eq!(
            drain(&mut first),
            vec![ServerMessage::EntityVanished { entity_id: 2 }]
        );
        assert_eq!(world.population(), 1);
    }

    #[test]
    fn quitter_deux_fois_est_sans_effet() {
        let world = World::new();
        let _rx = join(&world, 1);
        world.leave(1);
        world.leave(1);
        assert_eq!(world.population(), 0);
    }

    #[test]
    fn les_cellules_vides_ne_s_accumulent_pas() {
        let world = World::new();
        let _rx = join(&world, 1);

        for step in 1..20 {
            teleport(&world, 1, step * 100_000, 0);
        }

        assert_eq!(
            world.lock().cells.len(),
            1,
            "des cellules vides sont restees indexees"
        );
    }

    #[test]
    fn deplacer_une_entite_absente_est_sans_effet() {
        let world = World::new();
        world.request_move(404, 10, 10, 1_000);
        assert_eq!(world.population(), 0);
    }
}
