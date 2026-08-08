//! Registre des entites presentes et diffusion aux voisins.
//!
//! Toutes les regles (validation d'un deplacement, portee de vue, decoupage en
//! cellules) viennent de `hwarang_domain`. Ce module ne fait que tenir l'etat
//! partage et router les notifications.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use hwarang_domain::{
    AttackProfile, AttackRejection, Attributes, CellCoord, Character, CharacterId, CombatRule,
    DefenseProfile, Engagement, Grid, MoveVerdict, MovementRule, Position, ProgressionCurve,
    Resistance, experience_reward, resolve_attack,
};
use hwarang_protocol::{AttackRefusal, EntityId, ServerMessage};
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
    combat: CombatRule,
    character: Character,
    outbox: Outbox,
    /// Entites actuellement percues. Maintenu symetriquement.
    visible: HashSet<EntityId>,
}

impl Entity {
    fn is_alive(&self) -> bool {
        self.character.is_alive()
    }

    fn attack_profile(&self) -> AttackProfile {
        // Statistique derivee, jamais stockee : elle ne peut pas se
        // desynchroniser des attributs qui la produisent.
        AttackProfile::new(BASE_ATTACK + u32::from(self.character.attributes().strength) * 5)
    }

    fn defense_profile(&self) -> DefenseProfile {
        DefenseProfile::new(u32::from(self.character.attributes().dexterity) * 10)
    }
}

/// Puissance de base, avant contribution des attributs.
const BASE_ATTACK: u32 = 120;

/// Attributs de depart d'un joueur.
///
/// En dur pour l'instant : la creation de personnage arrive avec la persistance.
fn starting_attributes() -> Attributes {
    Attributes {
        strength: 10,
        dexterity: 8,
        vitality: 10,
        intellect: 5,
    }
}

const fn refusal_of(rejection: AttackRejection) -> AttackRefusal {
    match rejection {
        AttackRejection::OutOfRange { .. } => AttackRefusal::OutOfRange,
        AttackRejection::TooSoon { .. } => AttackRefusal::TooSoon,
        AttackRejection::AttackerDown => AttackRefusal::AttackerDown,
        AttackRejection::TargetDown => AttackRefusal::TargetDown,
        AttackRejection::SelfTarget => AttackRefusal::SelfTarget,
    }
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
                combat: CombatRule::melee(),
                character: Character::create(
                    CharacterId::new(id),
                    starting_attributes(),
                    ProgressionCurve::DEFAULT,
                ),
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

        // Un mort ne se deplace pas. Le refus rappelle sa position, sinon un
        // client qui continue d'envoyer des deplacements derive silencieusement
        // et reapparait ailleurs.
        if !entity.is_alive() {
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

        self.reindex(&mut state, id, from, target);
        self.refresh_visibility(&mut state, id);
    }

    /// Deplace une entite d'une cellule a l'autre dans l'index spatial.
    fn reindex(&self, state: &mut State, id: EntityId, from: Position, to: Position) {
        if !self.grid.crosses_cell(from, to) {
            return;
        }
        let (before, after) = (self.grid.cell_of(from), self.grid.cell_of(to));
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

    /// Resout une attaque revendiquee par un client.
    ///
    /// `elapsed_ms` est mesure par le serveur depuis la derniere attaque
    /// *retenue*, jamais annonce par le client — meme principe que pour le
    /// deplacement.
    pub fn request_attack(&self, attacker_id: EntityId, target_id: EntityId, elapsed_ms: u64) {
        let mut state = self.lock();

        let Some(attacker) = state.entities.get(&attacker_id) else {
            return;
        };
        let Some(target) = state.entities.get(&target_id) else {
            // Cible absente : le refus est explicite plutot que silencieux, sinon
            // un client desynchronise frappe dans le vide sans jamais le savoir.
            send(
                &state,
                attacker_id,
                ServerMessage::AttackRefused {
                    reason: AttackRefusal::NoSuchTarget,
                },
            );
            return;
        };

        let engagement = Engagement {
            attacker_at: attacker.position,
            target_at: target.position,
            attacker_alive: attacker.is_alive(),
            target_alive: target.is_alive(),
            same_entity: attacker_id == target_id,
        };

        if let Err(rejection) = attacker.combat.authorize(engagement, elapsed_ms) {
            send(
                &state,
                attacker_id,
                ServerMessage::AttackRefused {
                    reason: refusal_of(rejection),
                },
            );
            return;
        }

        let damage = resolve_attack(
            attacker.attack_profile(),
            target.defense_profile(),
            Resistance::NONE,
            target.character.level(),
        );

        let Some(target) = state.entities.get_mut(&target_id) else {
            return;
        };
        target.character = target.character.take_damage(damage);
        let remaining_health = target.character.vitals().current();
        let died = !target.character.is_alive();

        broadcast_around(
            &state,
            attacker_id,
            target_id,
            ServerMessage::DamageDealt {
                attacker: attacker_id,
                target: target_id,
                damage,
                remaining_health,
            },
        );

        if died {
            Self::on_death(&mut state, attacker_id, target_id);
        }
    }

    /// Remet une entite en jeu a son point d'apparition.
    pub fn request_respawn(&self, id: EntityId) {
        let mut state = self.lock();
        let Some(entity) = state.entities.get(&id) else {
            return;
        };
        // Reapparaitre vivant remettrait les jauges a plein a volonte : le
        // soin gratuit serait a un message d'intervalle.
        if entity.is_alive() {
            return;
        }

        let position = spawn_position(id);
        let from = entity.position;

        let Some(entity) = state.entities.get_mut(&id) else {
            return;
        };
        entity.character = entity.character.respawn();
        entity.position = position;
        let health = entity.character.vitals().current();

        self.reindex(&mut state, id, from, position);

        // `broadcast_around` inclut deja l'entite dans ses destinataires : un
        // `send` supplementaire lui livrerait l'evenement en double.
        broadcast_around(
            &state,
            id,
            id,
            ServerMessage::EntityRespawned {
                entity: id,
                x: position.x,
                y: position.y,
                health,
            },
        );
        self.refresh_visibility(&mut state, id);
    }

    /// Applique les consequences d'une mort : annonce et recompense.
    fn on_death(state: &mut State, killer_id: EntityId, victim_id: EntityId) {
        let Some(victim) = state.entities.get(&victim_id) else {
            return;
        };
        let reward = experience_reward(victim.character.level());

        broadcast_around(
            state,
            killer_id,
            victim_id,
            ServerMessage::EntityDied {
                entity: victim_id,
                killer: killer_id,
            },
        );

        let Some(killer) = state.entities.get_mut(&killer_id) else {
            return;
        };
        let (grown, _) = killer.character.gain_experience(reward);
        killer.character = grown;

        send(
            state,
            killer_id,
            ServerMessage::ExperienceGained {
                amount: reward.get(),
                level: grown.level().get(),
            },
        );
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

/// Diffuse a deux entites et a tous ceux qui percoivent l'une ou l'autre.
///
/// L'union des deux voisinages, et non le seul voisinage de l'attaquant : un
/// temoin place pres de la cible mais loin de l'attaquant doit voir le coup
/// porter.
fn broadcast_around(state: &State, first: EntityId, second: EntityId, message: ServerMessage) {
    let mut recipients: HashSet<EntityId> = HashSet::new();
    for id in [first, second] {
        if let Some(entity) = state.entities.get(&id) {
            recipients.insert(id);
            recipients.extend(entity.visible.iter().copied());
        }
    }
    for id in recipients {
        send(state, id, message);
    }
}

/// Un envoi qui echoue signifie que la connexion est deja fermee ; le retrait
/// de l'entite viendra de la tache de lecture.
fn send(state: &State, id: EntityId, message: ServerMessage) {
    if let Some(entity) = state.entities.get(&id) {
        let _ = entity.outbox.send(message);
    }
}

/// Cote de la grille de points d'apparition.
const SPAWN_COLUMNS: i32 = 8;
/// Ecart entre deux points d'apparition.
///
/// Choisi pour que la **diagonale** de la grille reste sous [`VIEW_RADIUS_CM`] :
/// deux arrivants doivent toujours se percevoir, sinon la zone de depart se
/// comporte differemment selon les identifiants distribues. Un ecart de 500
/// donnerait 4950 cm de diagonale, au-dela des 4000 cm de portee.
const SPAWN_STEP_CM: i32 = 300;

/// Repartit les apparitions sur une grille autour de l'origine.
///
/// Faire apparaitre tout le monde au meme point rendrait invisible toute erreur
/// de calcul de visibilite : chacun verrait chacun, quelle que soit la grille.
fn spawn_position(id: EntityId) -> Position {
    let slots = SPAWN_COLUMNS * SPAWN_COLUMNS;
    let index = i32::try_from(id % u64::try_from(slots).unwrap_or(1)).unwrap_or(0);
    Position::new(
        (index % SPAWN_COLUMNS) * SPAWN_STEP_CM,
        (index / SPAWN_COLUMNS) * SPAWN_STEP_CM,
    )
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
    fn tous_les_points_d_apparition_sont_mutuellement_visibles() {
        // Sans cette garantie, deux joueurs qui se connectent coup sur coup se
        // voient ou non selon les identifiants qu'ils ont recus.
        let slots = SPAWN_COLUMNS * SPAWN_COLUMNS;
        for a in 0..slots {
            for b in 0..slots {
                let (first, second) = (
                    spawn_position(u64::try_from(a).unwrap_or(0)),
                    spawn_position(u64::try_from(b).unwrap_or(0)),
                );
                assert!(
                    first.is_within(second, VIEW_RADIUS_CM),
                    "les emplacements {a} et {b} ne se voient pas"
                );
            }
        }
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

    // --- Combat ---

    /// Cadence largement satisfaite, pour isoler ce qui est teste.
    const AT_EASE: u64 = 60_000;

    fn refusal(messages: &[ServerMessage]) -> Option<AttackRefusal> {
        messages.iter().find_map(|m| match m {
            ServerMessage::AttackRefused { reason } => Some(*reason),
            _ => None,
        })
    }

    fn health_of(world: &World, id: EntityId) -> u32 {
        world.lock().entities[&id].character.vitals().current()
    }

    /// Deux combattants au contact, journaux vides.
    fn duel(
        world: &World,
    ) -> (
        UnboundedReceiver<ServerMessage>,
        UnboundedReceiver<ServerMessage>,
    ) {
        let mut first = join(world, 1);
        let mut second = join(world, 2);
        teleport(world, 2, spawn_position(1).x + 100, spawn_position(1).y);
        drain(&mut first);
        drain(&mut second);
        (first, second)
    }

    #[test]
    fn une_attaque_au_contact_retire_des_points_de_vie() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);
        let before = health_of(&world, 2);

        world.request_attack(1, 2, AT_EASE);

        let after = health_of(&world, 2);
        assert!(after < before, "aucun degat applique");
        assert!(drain(&mut attacker).iter().any(|m| matches!(
            m,
            ServerMessage::DamageDealt {
                attacker: 1,
                target: 2,
                ..
            }
        )));
    }

    #[test]
    fn la_cible_est_prevenue_du_coup_qu_elle_recoit() {
        let world = World::new();
        let (_attacker, mut target) = duel(&world);

        world.request_attack(1, 2, AT_EASE);

        assert!(
            drain(&mut target)
                .iter()
                .any(|m| matches!(m, ServerMessage::DamageDealt { target: 2, .. }))
        );
    }

    #[test]
    fn une_cible_hors_d_allonge_est_refusee() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);
        teleport(&world, 2, 3_000, 0);
        drain(&mut attacker);

        world.request_attack(1, 2, AT_EASE);

        assert_eq!(
            refusal(&drain(&mut attacker)),
            Some(AttackRefusal::OutOfRange)
        );
    }

    #[test]
    fn la_cadence_bloque_les_attaques_en_rafale() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);

        world.request_attack(1, 2, AT_EASE);
        drain(&mut attacker);
        world.request_attack(1, 2, 5);

        assert_eq!(refusal(&drain(&mut attacker)), Some(AttackRefusal::TooSoon));
    }

    #[test]
    fn une_rafale_refusee_n_inflige_aucun_degat() {
        let world = World::new();
        let (_attacker, _target) = duel(&world);

        world.request_attack(1, 2, AT_EASE);
        let after_first = health_of(&world, 2);
        for _ in 0..50 {
            world.request_attack(1, 2, 1);
        }

        assert_eq!(
            health_of(&world, 2),
            after_first,
            "la rafale a traverse la cadence"
        );
    }

    #[test]
    fn se_prendre_pour_cible_est_refuse() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);

        world.request_attack(1, 1, AT_EASE);

        assert_eq!(
            refusal(&drain(&mut attacker)),
            Some(AttackRefusal::SelfTarget)
        );
    }

    #[test]
    fn attaquer_une_entite_absente_est_refuse_explicitement() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);

        world.request_attack(1, 999, AT_EASE);

        assert_eq!(
            refusal(&drain(&mut attacker)),
            Some(AttackRefusal::NoSuchTarget)
        );
    }

    /// Frappe jusqu'a la mort de la cible, en respectant la cadence.
    fn strike_until_dead(world: &World, attacker: EntityId, target: EntityId) -> usize {
        for blow in 1..500 {
            world.request_attack(attacker, target, AT_EASE);
            if !world.lock().entities[&target].is_alive() {
                return blow;
            }
        }
        panic!("la cible n'est jamais tombee");
    }

    #[test]
    fn la_cible_finit_par_tomber_et_la_mort_est_annoncee() {
        let world = World::new();
        let (mut attacker, mut target) = duel(&world);

        strike_until_dead(&world, 1, 2);

        let seen_by_target = drain(&mut target);
        assert!(seen_by_target.iter().any(|m| matches!(
            m,
            ServerMessage::EntityDied {
                entity: 2,
                killer: 1
            }
        )));
        assert!(
            drain(&mut attacker)
                .iter()
                .any(|m| matches!(m, ServerMessage::EntityDied { entity: 2, .. }))
        );
    }

    #[test]
    fn eliminer_une_cible_rapporte_de_l_experience() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);

        strike_until_dead(&world, 1, 2);

        assert!(
            drain(&mut attacker).iter().any(
                |m| matches!(m, ServerMessage::ExperienceGained { amount, .. } if *amount > 0)
            )
        );
    }

    #[test]
    fn s_acharner_sur_un_cadavre_est_refuse() {
        let world = World::new();
        let (mut attacker, _target) = duel(&world);
        strike_until_dead(&world, 1, 2);
        drain(&mut attacker);

        world.request_attack(1, 2, AT_EASE);

        assert_eq!(
            refusal(&drain(&mut attacker)),
            Some(AttackRefusal::TargetDown)
        );
    }

    #[test]
    fn un_mort_ne_riposte_pas() {
        let world = World::new();
        let (_attacker, mut target) = duel(&world);
        strike_until_dead(&world, 1, 2);
        drain(&mut target);

        world.request_attack(2, 1, AT_EASE);

        assert_eq!(
            refusal(&drain(&mut target)),
            Some(AttackRefusal::AttackerDown)
        );
    }

    #[test]
    fn un_mort_ne_se_deplace_pas() {
        let world = World::new();
        let (_attacker, mut target) = duel(&world);
        strike_until_dead(&world, 1, 2);
        let position = world.lock().entities[&2].position;
        drain(&mut target);

        teleport(&world, 2, 9_000, 9_000);

        assert_eq!(world.lock().entities[&2].position, position);
        assert!(
            drain(&mut target)
                .iter()
                .any(|m| matches!(m, ServerMessage::MoveRejected { .. }))
        );
    }

    #[test]
    fn reapparaitre_restaure_les_points_de_vie_et_previent_les_temoins() {
        let world = World::new();
        let (mut attacker, mut target) = duel(&world);
        strike_until_dead(&world, 1, 2);
        drain(&mut attacker);
        drain(&mut target);

        world.request_respawn(2);

        let entity = &world.lock().entities[&2];
        assert!(entity.is_alive());
        assert_eq!(
            entity.character.vitals().current(),
            entity.character.vitals().max()
        );
        assert!(
            drain(&mut attacker)
                .iter()
                .any(|m| matches!(m, ServerMessage::EntityRespawned { entity: 2, .. })),
            "le temoin n'a pas ete prevenu du retour"
        );
    }

    #[test]
    fn la_reapparition_n_est_annoncee_qu_une_fois_a_l_interesse() {
        let world = World::new();
        let (_attacker, mut target) = duel(&world);
        strike_until_dead(&world, 1, 2);
        drain(&mut target);

        world.request_respawn(2);

        let announcements = drain(&mut target)
            .iter()
            .filter(|m| matches!(m, ServerMessage::EntityRespawned { .. }))
            .count();
        assert_eq!(announcements, 1, "evenement livre en double");
    }

    #[test]
    fn reapparaitre_vivant_ne_soigne_pas() {
        let world = World::new();
        let (_attacker, _target) = duel(&world);
        world.request_attack(1, 2, AT_EASE);
        let wounded = health_of(&world, 2);

        world.request_respawn(2);

        assert_eq!(
            health_of(&world, 2),
            wounded,
            "la reapparition sert de soin gratuit"
        );
    }
}
