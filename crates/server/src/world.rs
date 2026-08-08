//! Registre des entites presentes et diffusion aux voisins.
//!
//! Toutes les regles (validation d'un deplacement, portee de vue, decoupage en
//! cellules) viennent de `hwarang_domain`. Ce module ne fait que tenir l'etat
//! partage et router les notifications.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use hwarang_domain::{
    AggroRule, AttackProfile, AttackRejection, Attributes, Catalog, CellCoord, Character,
    CharacterId, CombatRule, DefenseProfile, Engagement, Equipment, Grid, Intent, Inventory,
    ItemId, MoveVerdict, MovementRule, Position, ProgressionCurve, RegenerationRule, Resistance,
    Situation, Slot, Stance, Threat, experience_reward, resolve_attack,
};
use hwarang_protocol::{AttackRefusal, EntityId, EntityKind, ServerMessage};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

/// Rayon de perception, identique pour toutes les entites.
///
/// L'uniformite rend la visibilite **symetrique** : si A voit B alors B voit A.
/// C'est ce qui permet de ne stocker la relation qu'une fois par cote et de
/// n'examiner, apres un deplacement, que le voisinage du seul mobile.
const VIEW_RADIUS_CM: u32 = Grid::DEFAULT_VIEW_RADIUS_CM;

/// Canal de sortie vers une connexion.
///
/// **Borne.** Un canal non borne permettrait a un client d'entrer dans le monde
/// puis de cesser de lire sa socket : l'ecriture reseau le concernant reste en
/// attente, mais les autres joueurs continuent de produire des evenements a son
/// intention, et la file croit sans limite jusqu'a epuiser la memoire du
/// serveur — un deni de service pour tout le monde, declenche par un seul
/// client.
type Outbox = Sender<ServerMessage>;

/// Profondeur de la file de sortie par connexion.
///
/// Large de quoi absorber une rafale legitime (arrivee dans une zone peuplee,
/// melee), etroite de quoi que le retard devienne visible avant de couter cher.
pub const OUTBOX_CAPACITY: usize = 256;

/// Ce qui pilote une creature : sa politique, son poste et sa posture.
///
/// Absent des entites joueuses : c'est ce qui distingue une entite mue par un
/// client d'une entite mue par la simulation.
#[derive(Debug, Clone, Copy)]
struct Brain {
    rule: AggroRule,
    anchor: Position,
    stance: Stance,
    /// Instant de la mort, pour la reapparition differee.
    died_at: Option<Instant>,
    /// Objet laisse a la mort, s'il y en a un.
    ///
    /// Propriete de la creature, fixee a sa creation. Le deriver de
    /// l'identifiant le rendrait tributaire d'un detail d'implementation :
    /// changer la plage des identifiants changerait le butin de tout le monde,
    /// sans que rien ne le signale.
    loot: Option<ItemId>,
    /// Instant de la derniere attaque **portee**, pas de la derniere tentative.
    ///
    /// La creature tente sa chance a chaque pas de simulation : compter les
    /// tentatives remettrait son horloge a zero toutes les 200 ms, et sa cadence
    /// d une seconde ne serait jamais atteinte.
    last_attack: Option<Instant>,
}

struct Entity {
    position: Position,
    rule: MovementRule,
    combat: CombatRule,
    character: Character,
    inventory: Inventory,
    equipment: Equipment,
    /// `None` pour un joueur : la connexion attend ses messages.
    outbox: Option<Outbox>,
    brain: Option<Brain>,
    /// Dernier instant ou l'entite a subi des degats.
    ///
    /// `None` signifie « jamais touchee » : elle recupere donc sans attendre.
    last_damaged: Option<Instant>,
    /// Entites actuellement percues. Maintenu symetriquement.
    visible: HashSet<EntityId>,
}

impl Entity {
    fn is_alive(&self) -> bool {
        self.character.is_alive()
    }

    const fn kind(&self) -> EntityKind {
        if self.brain.is_some() {
            EntityKind::Creature
        } else {
            EntityKind::Player
        }
    }

    /// Statistiques derivees des attributs **et** de l'equipement porte.
    ///
    /// Jamais stockees : elles ne peuvent pas se desynchroniser de ce qui les
    /// produit. Changer d'arme suffit a changer les degats au coup suivant, sans
    /// qu'aucun recalcul n'ait a etre declenche.
    fn attack_profile(&self, catalog: &Catalog) -> AttackProfile {
        AttackProfile::new(
            (BASE_ATTACK + u32::from(self.character.attributes().strength) * 5)
                .saturating_add(self.equipment.attack_bonus(catalog)),
        )
    }

    fn defense_profile(&self, catalog: &Catalog) -> DefenseProfile {
        DefenseProfile::new(
            (u32::from(self.character.attributes().dexterity) * 10)
                .saturating_add(self.equipment.defense_bonus(catalog)),
        )
    }
}

/// Puissance de base, avant contribution des attributs.
const BASE_ATTACK: u32 = 120;

/// Recuperation hors combat, identique pour tous.
const REGENERATION: RegenerationRule = RegenerationRule::standard();

/// Delai avant qu'une creature abattue ne revienne a son poste.
///
/// Assez long pour que le joueur constate sa victoire et ramasse ce qu'elle
/// laissera un jour ; assez court pour qu'une zone ne se vide pas.
pub const CREATURE_RESPAWN_DELAY: Duration = Duration::from_secs(10);

/// Debut de la plage d'identifiants reservee aux creatures.
///
/// Les sessions montent depuis 1, les creatures depuis ce seuil : les deux
/// suites ne peuvent pas se croiser avant des milliards de connexions.
///
/// Volontairement **sous** `i64::MAX` et non descendant depuis `u64::MAX` :
/// beaucoup de langages clients n'ont que des entiers signes 64 bits — `GDScript`
/// et JavaScript notamment. Un identifiant au-dela de `i64::MAX` y apparait
/// negatif. L'aller-retour reste correct, les bits etant les memes, mais tout
/// affichage ou journal devient illisible et la moindre comparaison arithmetique
/// se comporte de travers.
pub const CREATURE_ID_BASE: EntityId = 1 << 62;

/// Premier poste de la zone de depart, a l'ecart du point d'apparition des
/// joueurs : on doit aller chercher les creatures, pas naitre au milieu.
const STARTING_AREA_ORIGIN: Position = Position::new(6_000, 1_500);

/// Ecart entre deux postes de creatures.
///
/// Au moins deux fois le rayon d'agressivite, pour qu'aucune position ne
/// permette d'en reveiller deux a la fois.
const CREATURE_SPACING_CM: i32 = 3_200;

/// Attributs d'une creature de base.
///
/// Plus faible qu'un joueur : le premier adversaire rencontre doit pouvoir etre
/// battu par un personnage neuf, sinon la zone de depart est infranchissable.
fn creature_attributes() -> Attributes {
    Attributes {
        strength: 6,
        dexterity: 4,
        vitality: 4,
        intellect: 1,
    }
}

/// Point situe a au plus `allowance_cm` de `from`, en direction de `target`.
///
/// Arithmetique entiere : la trajectoire d'une creature doit etre identique
/// d'une machine a l'autre, comme tout le reste de la simulation.
fn advance(from: Position, target: Position, allowance_cm: u64) -> Position {
    let distance = from.distance_squared(target).isqrt();
    if distance == 0 || distance <= allowance_cm {
        return target;
    }

    let dx = i64::from(target.x) - i64::from(from.x);
    let dy = i64::from(target.y) - i64::from(from.y);
    let reach = i64::try_from(allowance_cm).unwrap_or(i64::MAX);
    let span = i64::try_from(distance).unwrap_or(i64::MAX).max(1);

    Position::new(
        i32::try_from(i64::from(from.x) + dx * reach / span).unwrap_or(from.x),
        i32::try_from(i64::from(from.y) + dy * reach / span).unwrap_or(from.y),
    )
}

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

/// Ce que laisse une creature de la zone de depart.
///
/// L'arme de base : le premier butin d'un joueur doit lui servir tout de suite,
/// sinon la recompense d'un premier combat ne se voit pas.
const DEFAULT_LOOT: ItemId = ItemId::new(1);

const fn slot_from_code(code: u8) -> Option<Slot> {
    match code {
        1 => Some(Slot::Weapon),
        2 => Some(Slot::Armor),
        _ => None,
    }
}

/// Catalogue de la zone de depart.
///
/// En dur pour l'instant, mais deja une **donnee** passee au monde : la sortir
/// vers un fichier ne demandera pas de toucher aux regles.
#[must_use]
pub fn starting_catalog() -> Catalog {
    Catalog::new()
        .with(
            ItemId::new(1),
            hwarang_domain::ItemDefinition {
                slot: Some(Slot::Weapon),
                attack_bonus: 45,
                ..hwarang_domain::ItemDefinition::default()
            },
        )
        .with(
            ItemId::new(2),
            hwarang_domain::ItemDefinition {
                slot: Some(Slot::Armor),
                defense_bonus: 30,
                ..hwarang_domain::ItemDefinition::default()
            },
        )
        .with(
            ItemId::new(3),
            hwarang_domain::ItemDefinition {
                slot: Some(Slot::Weapon),
                attack_bonus: 150,
                required_level: 15,
                ..hwarang_domain::ItemDefinition::default()
            },
        )
        // Seconde arme accessible des le depart : sans elle, le premier
        // changement d'arme n'arrive qu'au palier 15, et l'echange avec le sac
        // ne serait exerce qu'a ce moment-la.
        .with(
            ItemId::new(4),
            hwarang_domain::ItemDefinition {
                slot: Some(Slot::Weapon),
                attack_bonus: 20,
                ..hwarang_domain::ItemDefinition::default()
            },
        )
}

const fn equipment_changed(slot: Slot, item: Option<ItemId>) -> ServerMessage {
    ServerMessage::EquipmentChanged {
        slot: match slot {
            Slot::Weapon => 1,
            Slot::Armor => 2,
        },
        // 0 signale un emplacement vide : aucun objet ne porte cet identifiant.
        item: match item {
            Some(id) => id.get(),
            None => 0,
        },
    }
}

/// Etat d'un joueur recharge depuis la persistance.
#[derive(Debug, Clone)]
pub struct RestoredPlayer {
    pub character: Character,
    pub position: Position,
    pub inventory: Inventory,
    pub equipment: Equipment,
}

/// Ce qu'une creature percoit a un instant donne.
struct Observation {
    situation: Situation,
    /// Identifiant de la cible retenue, s'il y en a une.
    target: Option<EntityId>,
    alive: bool,
    died_at: Option<Instant>,
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
    /// Definitions des objets. Donnee, pas code : fournie a la construction.
    catalog: Catalog,
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
        Self::with_catalog(Catalog::new())
    }

    #[must_use]
    pub fn with_catalog(catalog: Catalog) -> Self {
        Self {
            grid: Grid::with_default_view(),
            catalog,
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
    /// `restored` porte l'etat rechargé depuis la persistance. `None` cree un
    /// personnage neuf : c'est la premiere connexion d'un compte.
    ///
    /// Retourne la position d'apparition.
    pub fn enter(
        &self,
        id: EntityId,
        outbox: Outbox,
        restored: Option<RestoredPlayer>,
    ) -> Position {
        let restored_inventory = restored.as_ref().map(|r| r.inventory.clone());
        let restored_equipment = restored.as_ref().map(|r| r.equipment);
        let (character, position) = restored.map_or_else(
            || {
                (
                    Character::create(
                        CharacterId::new(id),
                        starting_attributes(),
                        ProgressionCurve::DEFAULT,
                    ),
                    spawn_position(id),
                )
            },
            |r| (r.character, r.position),
        );
        let mut state = self.lock();

        state.entities.insert(
            id,
            Entity {
                position,
                rule: MovementRule::running(),
                combat: CombatRule::melee(),
                character,
                inventory: restored_inventory.unwrap_or_default(),
                equipment: restored_equipment.unwrap_or_default(),
                outbox: Some(outbox),
                brain: None,
                last_damaged: None,
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
    /// Retourne `true` si le coup a porte.
    ///
    /// `elapsed_ms` est mesure par le serveur, jamais annonce par le client.
    ///
    /// Compter depuis la derniere attaque *aboutie* laisserait un client
    /// accumuler du temps a coups de tentatives refusees, puis le depenser en
    /// salve. Le prix a payer est qu'une tentative hors de portee decale la
    /// premiere frappe recevable d'un cycle de cadence.
    pub fn request_attack(
        &self,
        attacker_id: EntityId,
        target_id: EntityId,
        elapsed_ms: u64,
    ) -> bool {
        let mut state = self.lock();

        let Some(attacker) = state.entities.get(&attacker_id) else {
            return false;
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
            return false;
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
            return false;
        }

        let damage = resolve_attack(
            attacker.attack_profile(&self.catalog),
            target.defense_profile(&self.catalog),
            Resistance::NONE,
            target.character.level(),
        );

        let Some(target) = state.entities.get_mut(&target_id) else {
            return false;
        };
        target.character = target.character.take_damage(damage);
        target.last_damaged = Some(Instant::now());
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
        true
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
        let Some(victim) = state.entities.get_mut(&victim_id) else {
            return;
        };
        // Une creature abattue demarre son compte a rebours de reapparition ; un
        // joueur attend sa propre demande.
        if let Some(brain) = victim.brain.as_mut() {
            brain.died_at = Some(Instant::now());
            brain.stance = Stance::Idle;
        }
        let reward = experience_reward(victim.character.level());
        // Seules les creatures laissent du butin : depouiller un joueur vaincu
        // est une decision de jeu lourde de consequences, pas un effet de bord.
        let loot = victim.brain.and_then(|brain| brain.loot);

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

        Self::award_loot(state, killer_id, loot);
    }

    /// Remet le butin au vainqueur.
    ///
    /// L'objet va directement au sac plutot qu'au sol : un objet au sol est une
    /// entite du monde a part entiere — visible, ramassable, expirable — et ce
    /// contexte n'existe pas encore. Le sac plein fait perdre le butin, ce que
    /// le joueur apprend explicitement.
    fn award_loot(state: &mut State, winner: EntityId, loot: Option<ItemId>) {
        let Some(item) = loot else { return };
        let Some(entity) = state.entities.get(&winner) else {
            return;
        };

        match entity.inventory.add(item) {
            Ok((filled, index)) => {
                if let Some(entity) = state.entities.get_mut(&winner) {
                    entity.inventory = filled;
                }
                send(
                    state,
                    winner,
                    ServerMessage::ItemReceived {
                        item: item.get(),
                        slot_index: u16::try_from(index).unwrap_or(u16::MAX),
                    },
                );
            }
            Err(_) => send(state, winner, ServerMessage::InventoryFull),
        }
    }

    /// Equipe un objet du sac.
    ///
    /// L'objet quitte le sac et l'objet remplace y retourne : l'echange est
    /// atomique, sinon un sac plein ferait disparaitre l'ancien equipement.
    pub fn request_equip(&self, id: EntityId, slot_index: u16) {
        let mut state = self.lock();
        let Some(entity) = state.entities.get(&id) else {
            return;
        };

        let index = usize::from(slot_index);
        let Ok((emptied, item)) = entity.inventory.remove(index) else {
            send(&state, id, ServerMessage::EquipRefused);
            return;
        };

        let Some((equipped, replaced)) =
            entity
                .equipment
                .equip(item, &self.catalog, entity.character.level())
        else {
            send(&state, id, ServerMessage::EquipRefused);
            return;
        };

        // L'objet remplace reprend l'emplacement libere : il y a forcement la
        // place, puisqu'on vient de l'y retirer.
        let restored = match replaced {
            Some(previous) => emptied
                .add(previous)
                .map_or(emptied.clone(), |(bag, _)| bag),
            None => emptied,
        };

        let Some(entity) = state.entities.get_mut(&id) else {
            return;
        };
        entity.inventory = restored;
        entity.equipment = equipped;
        let slot = self
            .catalog
            .definition(item)
            .and_then(|definition| definition.slot);

        if let Some(slot) = slot {
            send(&state, id, equipment_changed(slot, Some(item)));
        }
    }

    /// Retire un objet equipe et le remet au sac.
    pub fn request_unequip(&self, id: EntityId, slot: Slot) {
        let mut state = self.lock();
        let Some(entity) = state.entities.get(&id) else {
            return;
        };

        let (stripped, removed) = entity.equipment.unequip(slot);
        let Some(item) = removed else {
            send(&state, id, ServerMessage::EquipRefused);
            return;
        };

        // Sac plein : l'objet reste equipe. Le detruire serait pire que de
        // refuser l'operation.
        let Ok((bag, _)) = entity.inventory.add(item) else {
            send(&state, id, ServerMessage::InventoryFull);
            return;
        };

        if let Some(entity) = state.entities.get_mut(&id) {
            entity.inventory = bag;
            entity.equipment = stripped;
        }
        send(&state, id, equipment_changed(slot, None));
    }

    /// Variante prenant le code d'emplacement du protocole.
    pub fn request_unequip_code(&self, id: EntityId, code: u8) {
        match slot_from_code(code) {
            Some(slot) => self.request_unequip(id, slot),
            // Code inconnu : le client parle d'un emplacement qui n'existe pas.
            None => send(&self.lock(), id, ServerMessage::EquipRefused),
        }
    }

    /// Sac et equipement d'une entite, pour la sauvegarde.
    #[must_use]
    pub fn belongings(&self, id: EntityId) -> Option<(Inventory, Equipment)> {
        let state = self.lock();
        let entity = state.entities.get(&id)?;
        Some((entity.inventory.clone(), entity.equipment))
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
            }
            // Passe par `send` et non par l'`outbox` directement : sur un canal
            // borne, `Sender::send` est asynchrone et un `let _ =` sur le futur
            // n'envoie rien du tout, silencieusement.
            send(
                &state,
                other,
                ServerMessage::EntityVanished { entity_id: id },
            );
        }
    }

    #[must_use]
    pub fn population(&self) -> usize {
        self.lock().entities.len()
    }

    /// Fait apparaitre une creature a son poste.
    ///
    /// Les identifiants de creatures sont distincts de ceux des sessions : ils
    /// viennent d'un compteur descendant, ce qui rend impossible qu'une creature
    /// et un joueur partagent un identifiant, meme apres des milliers de
    /// connexions.
    pub fn spawn_creature(&self, id: EntityId, anchor: Position) {
        self.spawn_creature_with_loot(id, anchor, Some(DEFAULT_LOOT));
    }

    /// Fait apparaitre une creature en choisissant ce qu'elle laissera.
    pub fn spawn_creature_with_loot(&self, id: EntityId, anchor: Position, loot: Option<ItemId>) {
        let mut state = self.lock();
        state.entities.insert(
            id,
            Entity {
                position: anchor,
                rule: MovementRule::walking(),
                combat: CombatRule::melee(),
                character: Character::create(
                    CharacterId::new(id),
                    creature_attributes(),
                    ProgressionCurve::DEFAULT,
                ),
                inventory: Inventory::default(),
                equipment: Equipment::empty(),
                outbox: None,
                last_damaged: None,
                brain: Some(Brain {
                    rule: AggroRule::standard(CombatRule::MELEE_RANGE_CM),
                    anchor,
                    stance: Stance::Idle,
                    died_at: None,
                    loot,
                    last_attack: None,
                }),
                visible: HashSet::new(),
            },
        );
        state
            .cells
            .entry(self.grid.cell_of(anchor))
            .or_default()
            .insert(id);
        self.refresh_visibility(&mut state, id);
    }

    /// Peuple la zone de depart.
    ///
    /// Les creatures sont espacees d'au moins **deux fois** leur rayon
    /// d'agressivite : sinon en approcher une reveille toutes ses voisines, et un
    /// personnage neuf se fait submerger sans avoir rien fait de maladroit. Ce
    /// « reveil de groupe » involontaire est ce qui rend une zone de depart
    /// injouable.
    ///
    /// Les identifiants partent de [`CREATURE_ID_BASE`] et montent, ceux des
    /// sessions montent depuis 1 : les deux suites ne peuvent pas se croiser.
    pub fn populate_starting_area(&self, count: u64) {
        for index in 0..count {
            let anchor = Position::new(
                STARTING_AREA_ORIGIN.x + i32::try_from(index).unwrap_or(0) * CREATURE_SPACING_CM,
                STARTING_AREA_ORIGIN.y,
            );
            self.spawn_creature(CREATURE_ID_BASE + index, anchor);
        }
    }

    /// Avance la simulation d'un pas.
    ///
    /// Chaque creature percoit, decide et agit. Le pas de temps est passe en
    /// parametre plutot que mesure ici : la simulation reste ainsi rejouable a
    /// l'identique, et testable sans attendre.
    pub fn tick(&self, step: Duration, now: Instant) {
        self.regenerate(step, now);

        let creatures: Vec<EntityId> = {
            let state = self.lock();
            state
                .entities
                .iter()
                .filter(|(_, entity)| entity.brain.is_some())
                .map(|(id, _)| *id)
                .collect()
        };

        for id in creatures {
            self.step_creature(id, step, now);
        }
    }

    /// Rend des points de vie a tout ce qui est vivant et au calme.
    ///
    /// Joueurs comme creatures : une creature qui a survecu a un assaut doit se
    /// remettre, sinon la deuxieme tentative d'un joueur se joue toujours sur un
    /// adversaire deja entame, et la difficulte d'une zone depend de l'historique
    /// plutot que de ce qu'elle est.
    fn regenerate(&self, step: Duration, now: Instant) {
        let elapsed_ms = u64::try_from(step.as_millis()).unwrap_or(u64::MAX);
        let mut state = self.lock();

        for entity in state.entities.values_mut() {
            if !entity.is_alive() {
                continue;
            }
            let idle_ms = entity.last_damaged.map_or(u64::MAX, |at| {
                u64::try_from(now.saturating_duration_since(at).as_millis()).unwrap_or(u64::MAX)
            });
            let healed = REGENERATION.amount(idle_ms, elapsed_ms);
            if healed > 0 {
                entity.character = entity.character.regenerate(healed);
            }
        }
    }

    /// Fait agir une creature : reapparition, perception, decision, action.
    fn step_creature(&self, id: EntityId, step: Duration, now: Instant) {
        let elapsed_ms = u64::try_from(step.as_millis()).unwrap_or(u64::MAX);

        let Some(Observation {
            situation,
            target,
            alive,
            died_at,
        }) = self.observe(id)
        else {
            return;
        };

        if !alive {
            // Reapparition differee : la creature reste au sol le temps que le
            // joueur constate sa victoire, puis revient a son poste.
            if died_at.is_some_and(|at| now.duration_since(at) >= CREATURE_RESPAWN_DELAY) {
                self.revive_creature(id);
            }
            return;
        }

        let Some((intent, stance)) = self.decide(id, situation) else {
            return;
        };
        if std::env::var("HWARANG_TRACE_AI").is_ok() {
            eprintln!(
                "[ia] {id} en ({},{}) poste ({},{}) cible={:?} posture={:?} -> {intent:?}",
                situation.creature.x,
                situation.creature.y,
                situation.anchor.x,
                situation.anchor.y,
                situation.nearest.map(|t| (t.position.x, t.position.y)),
                situation.stance,
            );
        }
        self.remember_stance(id, stance);

        match intent {
            Intent::Hold => {}
            Intent::Approach(target) | Intent::ReturnTo(target) => {
                self.step_towards(id, target, elapsed_ms);
            }
            Intent::Strike => {
                if let Some(target) = target {
                    // Temps ecoule depuis la derniere attaque portee, et non
                    // depuis le dernier pas de simulation : les deux different
                    // d'un facteur cinq.
                    let since = self.attack_clock(id, now);
                    if self.request_attack(id, target, since) {
                        self.mark_attack(id, now);
                    }
                }
            }
        }
    }

    /// Rassemble ce que la creature percoit.
    fn observe(&self, id: EntityId) -> Option<Observation> {
        let state = self.lock();
        let entity = state.entities.get(&id)?;
        let brain = entity.brain?;

        // La cible est cherchee parmi les entites deja percues : la relation de
        // visibilite est maintenue a chaque deplacement, la reparcourir ici
        // reviendrait a refaire le travail de la grille.
        //
        // Son **identifiant** est retenu, pas seulement sa position : le joueur
        // continue de bouger pendant que la creature reflechit, et le retrouver
        // ensuite par comparaison de coordonnees echouerait des qu'il a fait un
        // pas — la creature ne frapperait alors que des cibles immobiles.
        let target = entity
            .visible
            .iter()
            .filter(|other| {
                state
                    .entities
                    .get(other)
                    .is_some_and(|o| o.brain.is_none() && o.is_alive())
            })
            .min_by_key(|other| {
                state
                    .entities
                    .get(other)
                    .map_or(u64::MAX, |o| entity.position.distance_squared(o.position))
            })
            .copied();

        let nearest = target
            .and_then(|other| state.entities.get(&other))
            .map(|other| Threat {
                position: other.position,
                alive: true,
            });

        Some(Observation {
            situation: Situation {
                creature: entity.position,
                anchor: brain.anchor,
                nearest,
                stance: brain.stance,
            },
            target,
            alive: entity.is_alive(),
            died_at: brain.died_at,
        })
    }

    fn decide(&self, id: EntityId, situation: Situation) -> Option<(Intent, Stance)> {
        let state = self.lock();
        let brain = state.entities.get(&id)?.brain?;
        Some(brain.rule.decide(situation))
    }

    /// Temps depuis la derniere attaque **portee** de cette creature.
    ///
    /// Lecture seule, contrairement a l'horloge des connexions. Cote client, la
    /// remise a zero est inconditionnelle pour qu'un joueur ne puisse pas
    /// accumuler du temps a coups de tentatives refusees. Une creature tente sa
    /// chance a chaque pas de simulation : lui appliquer la meme regle
    /// remettrait son horloge a zero toutes les 200 ms, elle n'atteindrait
    /// jamais sa cadence d'une seconde et resterait paralysee apres son premier
    /// coup. Il n'y a rien a s'y proteger — c'est le serveur qui decide quand
    /// elle frappe.
    fn attack_clock(&self, id: EntityId, now: Instant) -> u64 {
        let state = self.lock();
        state
            .entities
            .get(&id)
            .and_then(|entity| entity.brain)
            .map_or(0, |brain| {
                brain.last_attack.map_or(u64::MAX, |at| {
                    u64::try_from(now.duration_since(at).as_millis()).unwrap_or(u64::MAX)
                })
            })
    }

    /// Enregistre qu'une creature vient de frapper.
    fn mark_attack(&self, id: EntityId, now: Instant) {
        let mut state = self.lock();
        if let Some(brain) = state.entities.get_mut(&id).and_then(|e| e.brain.as_mut()) {
            brain.last_attack = Some(now);
        }
    }

    fn remember_stance(&self, id: EntityId, stance: Stance) {
        let mut state = self.lock();
        if let Some(brain) = state.entities.get_mut(&id).and_then(|e| e.brain.as_mut()) {
            brain.stance = stance;
        }
    }

    /// Avance d'un pas vers un point, sans jamais depasser sa propre vitesse.
    ///
    /// Passe par `request_move`, donc par la meme validation que les joueurs :
    /// une creature qui se teleporterait serait un bug invisible en test unitaire
    /// mais flagrant a l'ecran.
    fn step_towards(&self, id: EntityId, target: Position, elapsed_ms: u64) {
        let Some((from, allowance)) = ({
            let state = self.lock();
            state
                .entities
                .get(&id)
                .map(|entity| (entity.position, entity.rule.allowance_cm(elapsed_ms)))
        }) else {
            return;
        };

        let step = advance(from, target, allowance);
        if step != from {
            self.request_move(id, step.x, step.y, elapsed_ms);
        }
    }

    /// Remet une creature morte a son poste, en pleine sante.
    fn revive_creature(&self, id: EntityId) {
        let mut state = self.lock();
        let Some(entity) = state.entities.get(&id) else {
            return;
        };
        let Some(brain) = entity.brain else { return };
        let from = entity.position;
        let anchor = brain.anchor;

        let Some(entity) = state.entities.get_mut(&id) else {
            return;
        };
        entity.character = entity.character.respawn();
        entity.position = anchor;
        if let Some(brain) = entity.brain.as_mut() {
            brain.stance = Stance::Idle;
            brain.died_at = None;
        }
        let health = entity.character.vitals().current();

        self.reindex(&mut state, id, from, anchor);
        broadcast_around(
            &state,
            id,
            id,
            ServerMessage::EntityRespawned {
                entity: id,
                x: anchor.x,
                y: anchor.y,
                health,
            },
        );
        self.refresh_visibility(&mut state, id);
    }

    /// Etat courant d'une entite, pour la sauvegarde.
    ///
    /// Lecture instantanee sous verrou : l'ecriture en base se fait ensuite,
    /// hors verrou, pour ne pas retenir le monde pendant une entree-sortie.
    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<(Character, Position)> {
        let state = self.lock();
        let entity = state.entities.get(&id)?;
        Some((entity.character, entity.position))
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
        let subject_kind = subject.kind();
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
                    let (other_kind, own_kind) = (candidate.kind(), subject_kind);
                    link(state, id, other);
                    send(state, id, appeared(other, other_kind, other_position));
                    send(state, other, appeared(id, own_kind, position));
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

const fn appeared(entity_id: EntityId, kind: EntityKind, position: Position) -> ServerMessage {
    ServerMessage::EntityAppeared {
        entity_id,
        kind,
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

/// Depose un message dans la file d'une connexion.
///
/// `try_send` et non `send().await` : la diffusion s'execute sous le verrou du
/// monde, ou attendre un client lent bloquerait tous les autres.
///
/// Une file pleine designe un client qui ne draine plus sa socket. Le message
/// est perdu et la memoire reste bornee, ce qui suffit a fermer la voie du deni
/// de service ; la connexion, elle, mourra d'elle-meme quand son ecriture
/// reseau expirera.
///
/// **Limite connue** : perdre un `EntityVanished` laisse un fantome a l'ecran du
/// client en retard. Distinguer les messages qu'on peut perdre (`EntityMoved`,
/// remplace par le suivant) de ceux qui portent une transition unique
/// (`EntityVanished`, `EntityDied`) demandera de fermer la session au lieu
/// d'ecreter — a traiter quand le client existera et qu'on pourra l'observer.
///
/// Un envoi qui echoue parce que le recepteur a disparu signifie que la
/// connexion est deja fermee ; le retrait viendra de la tache de lecture.
fn send(state: &State, id: EntityId, message: ServerMessage) {
    // Une creature n'a pas de canal : elle n'a personne a prevenir. Les
    // notifications la concernant partent vers ses temoins, pas vers elle.
    let Some(outbox) = state
        .entities
        .get(&id)
        .and_then(|entity| entity.outbox.as_ref())
    else {
        return;
    };
    if let Err(TrySendError::Full(dropped)) = outbox.try_send(message) {
        eprintln!("entite {id} ne draine plus sa file, message perdu : {dropped:?}");
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
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{Receiver, channel};

    fn join(world: &World, id: EntityId) -> Receiver<ServerMessage> {
        let (tx, rx) = channel(OUTBOX_CAPACITY);
        world.enter(id, tx, None);
        rx
    }

    fn drain(rx: &mut Receiver<ServerMessage>) -> Vec<ServerMessage> {
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
    fn un_client_qui_ne_draine_jamais_ne_fait_pas_enfler_la_memoire() {
        // Un client entre dans le monde puis cesse de lire sa socket. Les autres
        // joueurs continuent de produire des evenements a son intention. Avec un
        // canal non borne, sa file croissait sans limite : un seul client
        // suffisait a epuiser la memoire du serveur pour tout le monde.
        let world = World::new();
        let mut idle = join(&world, 1);
        let _neighbour = join(&world, 2);

        for step in 0..10_000 {
            teleport(&world, 2, 100 + step % 200, 0);
        }

        let queued = drain(&mut idle).len();
        assert!(
            queued <= OUTBOX_CAPACITY,
            "{queued} messages en file pour un plafond de {OUTBOX_CAPACITY}"
        );
    }

    // --- Creatures ---

    const CREATURE: EntityId = CREATURE_ID_BASE;
    const STEP: Duration = Duration::from_millis(200);

    fn creature_position(world: &World) -> Position {
        world.lock().entities[&CREATURE].position
    }

    fn creature_stance(world: &World) -> Stance {
        world.lock().entities[&CREATURE]
            .brain
            .map_or(Stance::Idle, |brain| brain.stance)
    }

    /// Avance la simulation de `rounds` pas.
    fn run_ticks(world: &World, rounds: usize) {
        for _ in 0..rounds {
            world.tick(STEP, Instant::now());
        }
    }

    #[test]
    fn une_creature_immobile_reste_a_son_poste() {
        let world = World::new();
        let post = Position::new(5_000, 5_000);
        world.spawn_creature(CREATURE, post);

        run_ticks(&world, 10);

        assert_eq!(creature_position(&world), post);
        assert_eq!(creature_stance(&world), Stance::Idle);
    }

    #[test]
    fn une_creature_est_annoncee_comme_telle_et_non_comme_un_joueur() {
        let world = World::new();
        world.spawn_creature(CREATURE, spawn_position(1));
        let mut player = join(&world, 1);

        let kinds: Vec<EntityKind> = drain(&mut player)
            .iter()
            .filter_map(|m| match m {
                ServerMessage::EntityAppeared { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, vec![EntityKind::Creature]);
    }

    #[test]
    fn une_creature_poursuit_un_joueur_a_portee_de_detection() {
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, Position::new(post.x + 1_000, post.y));
        let _player = join(&world, 1);

        let before = creature_position(&world);
        run_ticks(&world, 3);
        let after = creature_position(&world);

        assert_ne!(after, before, "la creature n'a pas bouge");
        assert!(
            after.distance_squared(post) < before.distance_squared(post),
            "elle ne s'est pas rapprochee du joueur"
        );
        assert_eq!(creature_stance(&world), Stance::Engaged);
    }

    #[test]
    fn une_creature_ignore_un_joueur_hors_de_portee() {
        let world = World::new();
        let far = Position::new(500_000, 500_000);
        world.spawn_creature(CREATURE, far);
        let _player = join(&world, 1);

        run_ticks(&world, 5);

        assert_eq!(creature_position(&world), far);
        assert_eq!(creature_stance(&world), Stance::Idle);
    }

    #[test]
    fn une_creature_ne_se_teleporte_pas_vers_sa_cible() {
        // Elle passe par la meme validation de vitesse que les joueurs : un pas
        // de creature ne peut pas depasser ce qu'un pas de joueur pourrait faire.
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, Position::new(post.x + 1_200, post.y));
        let _player = join(&world, 1);

        let before = creature_position(&world);
        world.tick(STEP, Instant::now());
        let travelled = before.distance_squared(creature_position(&world)).isqrt();

        let allowance = MovementRule::walking().allowance_cm(200);
        assert!(
            travelled <= allowance,
            "{travelled} cm parcourus pour {allowance} autorises"
        );
    }

    #[test]
    fn une_creature_attaque_le_joueur_qu_elle_a_rejoint() {
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, Position::new(post.x + 150, post.y));
        let mut player = join(&world, 1);
        drain(&mut player);

        run_ticks(&world, 3);

        assert!(
            drain(&mut player).iter().any(|m| matches!(
                m,
                ServerMessage::DamageDealt {
                    attacker: CREATURE,
                    ..
                }
            )),
            "la creature au contact n'a jamais frappe"
        );
    }

    #[test]
    fn une_creature_au_contact_frappe_a_sa_cadence_et_pas_une_seule_fois() {
        // Regression. La creature tente sa chance a chaque pas de 200 ms ; si son
        // horloge repartait a chaque tentative — comme celle d'une connexion, ou
        // c'est une protection contre l'accumulation — l'ecoule ne depasserait
        // jamais la cadence d'une seconde, et elle resterait paralysee apres son
        // premier coup. Le symptome est une creature qui suit le joueur sans
        // jamais le toucher.
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, Position::new(post.x + 150, post.y));
        let mut player = join(&world, 1);
        drain(&mut player);

        // Cinq secondes de simulation, horodatees explicitement : le test ne
        // dure pas cinq secondes pour autant.
        let start = Instant::now();
        for step in 0..25 {
            world.tick(STEP, start + STEP * step);
        }

        let blows = drain(&mut player)
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    ServerMessage::DamageDealt {
                        attacker: CREATURE,
                        ..
                    }
                )
            })
            .count();
        assert!(
            blows >= 3,
            "{blows} coup(s) en 5 s simulees pour une cadence d'une seconde"
        );
    }

    // --- Regeneration ---

    #[test]
    fn un_joueur_au_calme_recupere_ses_points_de_vie() {
        // Sans cela, les degats s'accumulent d'un combat au suivant et le seul
        // moyen de repartir en pleine sante est de mourir.
        let world = World::new();
        let mut player = join(&world, 1);
        drain(&mut player);

        let wounded = {
            let mut state = world.lock();
            let entity = state.entities.get_mut(&1).expect("entre");
            entity.character = entity.character.take_damage(200);
            entity.character.vitals().current()
        };

        let start = Instant::now();
        for step in 0..40 {
            world.tick(STEP, start + STEP * step);
        }

        let healed = world.lock().entities[&1].character.vitals().current();
        assert!(
            healed > wounded,
            "aucune recuperation : {wounded} -> {healed}"
        );
    }

    #[test]
    fn un_joueur_frappe_a_l_instant_ne_recupere_pas() {
        // Se soigner en encaissant rendrait la fuite inutile.
        let world = World::new();
        let (_attacker, _target) = duel(&world);
        world.request_attack(1, 2, AT_EASE);
        let wounded = world.lock().entities[&2].character.vitals().current();

        // Un seul pas, immediatement apres le coup.
        world.tick(STEP, Instant::now());

        assert_eq!(
            world.lock().entities[&2].character.vitals().current(),
            wounded
        );
    }

    #[test]
    fn la_recuperation_ne_depasse_pas_le_maximum() {
        let world = World::new();
        let _player = join(&world, 1);

        let start = Instant::now();
        for step in 0..200 {
            world.tick(STEP, start + STEP * step);
        }

        let vitals = world.lock().entities[&1].character.vitals();
        assert_eq!(vitals.current(), vitals.max());
    }

    #[test]
    fn un_mort_ne_recupere_pas() {
        let world = World::new();
        let (_attacker, _target) = duel(&world);
        strike_until_dead(&world, 1, 2);

        let start = Instant::now();
        for step in 0..60 {
            world.tick(STEP, start + STEP * step);
        }

        assert!(
            !world.lock().entities[&2].is_alive(),
            "un mort s'est releve tout seul"
        );
    }

    #[test]
    fn une_creature_survivante_recupere_aussi() {
        // Sinon la difficulte d'une zone depend de l'historique des tentatives
        // plutot que de ce qu'elle est.
        let world = World::new();
        world.spawn_creature(CREATURE, Position::new(500_000, 500_000));
        let wounded = {
            let mut state = world.lock();
            let entity = state.entities.get_mut(&CREATURE).expect("apparue");
            entity.character = entity.character.take_damage(50);
            entity.character.vitals().current()
        };

        let start = Instant::now();
        for step in 0..40 {
            world.tick(STEP, start + STEP * step);
        }

        assert!(
            world.lock().entities[&CREATURE]
                .character
                .vitals()
                .current()
                > wounded
        );
    }

    // --- Objets ---

    fn armed_world() -> World {
        World::with_catalog(starting_catalog())
    }

    fn received_items(messages: &[ServerMessage]) -> Vec<u32> {
        messages
            .iter()
            .filter_map(|m| match m {
                ServerMessage::ItemReceived { item, .. } => Some(*item),
                _ => None,
            })
            .collect()
    }

    fn inventory_of(world: &World, id: EntityId) -> Inventory {
        world.lock().entities[&id].inventory.clone()
    }

    #[test]
    fn abattre_une_creature_rapporte_du_butin() {
        let world = armed_world();
        world.spawn_creature(CREATURE, spawn_position(1));
        let mut player = join(&world, 1);
        drain(&mut player);

        strike_until_dead(&world, 1, CREATURE);

        let received = received_items(&drain(&mut player));
        assert_eq!(received.len(), 1, "aucun butin, ou plusieurs");
        assert!(!inventory_of(&world, 1).is_empty());
    }

    #[test]
    fn le_butin_ne_depend_pas_de_l_identifiant_de_la_creature() {
        // Regression : le butin etait derive de `id % 3`. Changer la plage des
        // identifiants changeait donc ce que laissait chaque creature, sans que
        // rien ne le signale — une arme devenait une armure.
        let world = armed_world();
        let mut received = vec![];

        for offset in [0_u64, 1, 2, 7, 1_000] {
            let creature = CREATURE_ID_BASE + offset;
            let player = 1 + offset;
            world.spawn_creature(creature, spawn_position(player));
            let mut inbox = join(&world, player);
            drain(&mut inbox);

            strike_until_dead(&world, player, creature);
            received.extend(received_items(&drain(&mut inbox)));
            world.leave(player);
        }

        assert_eq!(received.len(), 5, "un butin par creature abattue");
        assert!(
            received.iter().all(|item| *item == received[0]),
            "le butin varie selon l'identifiant : {received:?}"
        );
    }

    #[test]
    fn le_butin_de_base_est_equipable_et_renforce_le_porteur() {
        // La recompense d'un premier combat doit servir tout de suite, sinon
        // elle ne se voit pas.
        let world = armed_world();
        let definition = world
            .catalog
            .definition(DEFAULT_LOOT)
            .expect("le butin de base est au catalogue");

        assert!(
            definition.slot.is_some(),
            "le butin de base ne s'equipe pas"
        );
        assert_eq!(definition.required_level, 1.min(definition.required_level));
        assert!(
            definition.attack_bonus > 0 || definition.defense_bonus > 0,
            "le butin de base n'apporte rien"
        );
    }

    #[test]
    fn abattre_un_joueur_ne_rapporte_aucun_butin() {
        // Depouiller un vaincu est une decision de jeu lourde, pas un effet de
        // bord de la mecanique de mort.
        let world = armed_world();
        let (mut attacker, _target) = duel(&world);

        strike_until_dead(&world, 1, 2);

        assert!(received_items(&drain(&mut attacker)).is_empty());
    }

    #[test]
    fn un_sac_plein_fait_perdre_le_butin_et_le_joueur_l_apprend() {
        let world = armed_world();
        world.spawn_creature(CREATURE, spawn_position(1));
        let mut player = join(&world, 1);

        // Sac rempli jusqu'a la derniere place.
        {
            let mut state = world.lock();
            let entity = state.entities.get_mut(&1).expect("le joueur est entre");
            let mut bag = Inventory::default();
            while let Ok((filled, _)) = bag.add(ItemId::new(1)) {
                bag = filled;
            }
            entity.inventory = bag;
        }
        drain(&mut player);

        strike_until_dead(&world, 1, CREATURE);

        let messages = drain(&mut player);
        assert!(
            messages
                .iter()
                .any(|m| matches!(m, ServerMessage::InventoryFull)),
            "le joueur n'a pas ete prevenu"
        );
        assert!(received_items(&messages).is_empty());
    }

    #[test]
    fn equiper_une_arme_augmente_les_degats() {
        let world = armed_world();
        let (_attacker, _target) = duel(&world);

        let before = {
            let state = world.lock();
            state.entities[&1].attack_profile(&world.catalog).power()
        };

        {
            let mut state = world.lock();
            let entity = state.entities.get_mut(&1).expect("le joueur est entre");
            entity.inventory = Inventory::default().placed(0, ItemId::new(1));
        }
        world.request_equip(1, 0);

        let after = {
            let state = world.lock();
            state.entities[&1].attack_profile(&world.catalog).power()
        };
        assert!(
            after > before,
            "l'arme n'a rien change : {before} -> {after}"
        );
    }

    #[test]
    fn equiper_retire_l_objet_du_sac_et_previent_le_joueur() {
        let world = armed_world();
        let mut player = join(&world, 1);
        {
            let mut state = world.lock();
            state.entities.get_mut(&1).expect("entre").inventory =
                Inventory::default().placed(0, ItemId::new(1));
        }
        drain(&mut player);

        world.request_equip(1, 0);

        assert_eq!(inventory_of(&world, 1).at(0), None);
        assert!(
            drain(&mut player)
                .iter()
                .any(|m| matches!(m, ServerMessage::EquipmentChanged { slot: 1, item: 1 }))
        );
    }

    #[test]
    fn remplacer_une_arme_remet_la_precedente_au_sac() {
        // Sans cet echange, changer d'arme detruit silencieusement l'ancienne.
        let world = armed_world();
        let _player = join(&world, 1);
        {
            let mut state = world.lock();
            // Deux armes : la seconde doit chasser la premiere vers le sac.
            state.entities.get_mut(&1).expect("entre").inventory = Inventory::default()
                .placed(0, ItemId::new(1))
                .placed(1, ItemId::new(4));
        }

        world.request_equip(1, 0);
        world.request_equip(1, 1);

        let bag = inventory_of(&world, 1);
        assert_eq!(bag.count(), 1, "l'arme remplacee a disparu");
        assert_eq!(bag.at(0), Some(ItemId::new(1)));
        assert_eq!(
            world.lock().entities[&1].equipment.at(Slot::Weapon),
            Some(ItemId::new(4))
        );
    }

    #[test]
    fn une_arme_et_une_armure_occupent_des_emplacements_distincts() {
        let world = armed_world();
        let _player = join(&world, 1);
        {
            let mut state = world.lock();
            state.entities.get_mut(&1).expect("entre").inventory = Inventory::default()
                .placed(0, ItemId::new(1))
                .placed(1, ItemId::new(2));
        }

        world.request_equip(1, 0);
        world.request_equip(1, 1);

        let equipment = world.lock().entities[&1].equipment;
        assert_eq!(equipment.at(Slot::Weapon), Some(ItemId::new(1)));
        assert_eq!(equipment.at(Slot::Armor), Some(ItemId::new(2)));
        assert!(inventory_of(&world, 1).is_empty());
    }

    #[test]
    fn equiper_un_emplacement_vide_est_refuse() {
        let world = armed_world();
        let mut player = join(&world, 1);
        drain(&mut player);

        world.request_equip(1, 3);

        assert!(
            drain(&mut player)
                .iter()
                .any(|m| matches!(m, ServerMessage::EquipRefused))
        );
    }

    #[test]
    fn un_objet_hors_de_portee_du_palier_est_refuse_et_reste_au_sac() {
        let world = armed_world();
        let mut player = join(&world, 1);
        {
            let mut state = world.lock();
            // L'objet 3 demande le palier 15 ; le joueur est au palier 1.
            state.entities.get_mut(&1).expect("entre").inventory =
                Inventory::default().placed(0, ItemId::new(3));
        }
        drain(&mut player);

        world.request_equip(1, 0);

        assert_eq!(
            inventory_of(&world, 1).at(0),
            Some(ItemId::new(3)),
            "l'objet refuse a quitte le sac"
        );
        assert!(
            drain(&mut player)
                .iter()
                .any(|m| matches!(m, ServerMessage::EquipRefused))
        );
    }

    #[test]
    fn retirer_un_objet_le_remet_au_sac() {
        let world = armed_world();
        let mut player = join(&world, 1);
        {
            let mut state = world.lock();
            state.entities.get_mut(&1).expect("entre").inventory =
                Inventory::default().placed(0, ItemId::new(1));
        }
        world.request_equip(1, 0);
        drain(&mut player);

        world.request_unequip_code(1, 1);

        assert_eq!(inventory_of(&world, 1).count(), 1);
        assert!(
            drain(&mut player)
                .iter()
                .any(|m| matches!(m, ServerMessage::EquipmentChanged { slot: 1, item: 0 }))
        );
    }

    #[test]
    fn un_emplacement_d_equipement_inconnu_est_refuse() {
        let world = armed_world();
        let mut player = join(&world, 1);
        drain(&mut player);

        world.request_unequip_code(1, 99);

        assert!(
            drain(&mut player)
                .iter()
                .any(|m| matches!(m, ServerMessage::EquipRefused))
        );
    }

    #[test]
    fn une_creature_ne_depasse_pas_sa_cadence_malgre_les_pas_rapides() {
        // L'autre bord du meme reglage : cinq tentatives par seconde ne doivent
        // pas produire cinq coups.
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, Position::new(post.x + 150, post.y));
        let mut player = join(&world, 1);
        drain(&mut player);

        let start = Instant::now();
        for step in 0..15 {
            world.tick(STEP, start + STEP * step);
        }

        let blows = drain(&mut player)
            .iter()
            .filter(|m| {
                matches!(
                    m,
                    ServerMessage::DamageDealt {
                        attacker: CREATURE,
                        ..
                    }
                )
            })
            .count();
        assert!(
            blows <= 4,
            "{blows} coups en 3 s simulees : la cadence n'est pas respectee"
        );
    }

    #[test]
    fn une_creature_rentre_a_son_poste_quand_le_joueur_disparait() {
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, Position::new(post.x + 1_000, post.y));
        let _player = join(&world, 1);

        run_ticks(&world, 3);
        assert_eq!(creature_stance(&world), Stance::Engaged);

        world.leave(1);
        run_ticks(&world, 40);

        assert_eq!(
            creature_position(&world),
            Position::new(post.x + 1_000, post.y),
            "la creature n'est pas rentree"
        );
        assert_eq!(creature_stance(&world), Stance::Idle);
    }

    #[test]
    fn une_creature_abattue_ne_revient_pas_immediatement() {
        let world = World::new();
        world.spawn_creature(CREATURE, spawn_position(1));
        let _player = join(&world, 1);
        strike_until_dead(&world, 1, CREATURE);

        run_ticks(&world, 5);

        assert!(
            !world.lock().entities[&CREATURE].is_alive(),
            "elle est revenue avant son delai"
        );
    }

    #[test]
    fn une_creature_abattue_revient_a_son_poste_le_delai_passe() {
        let world = World::new();
        let post = spawn_position(1);
        world.spawn_creature(CREATURE, post);
        let mut player = join(&world, 1);
        strike_until_dead(&world, 1, CREATURE);
        drain(&mut player);

        // Le temps est fourni a `tick`, jamais lu par lui : la reapparition se
        // teste sans attendre dix secondes.
        let later = Instant::now() + CREATURE_RESPAWN_DELAY;
        world.tick(STEP, later);

        let entity = &world.lock().entities[&CREATURE];
        assert!(entity.is_alive(), "elle n'est pas revenue");
        assert_eq!(entity.position, post);
        assert!(
            drain(&mut player).iter().any(|m| matches!(
                m,
                ServerMessage::EntityRespawned {
                    entity: CREATURE,
                    ..
                }
            )),
            "le joueur n'a pas ete prevenu du retour"
        );
    }

    #[test]
    fn une_creature_n_attaque_pas_une_autre_creature() {
        let world = World::new();
        world.spawn_creature(CREATURE, Position::new(1_000, 0));
        world.spawn_creature(CREATURE + 1, Position::new(1_100, 0));

        run_ticks(&world, 5);

        for id in [CREATURE, CREATURE + 1] {
            let entity = &world.lock().entities[&id];
            assert_eq!(
                entity.character.vitals().current(),
                entity.character.vitals().max(),
                "l'entite {id} a ete blessee"
            );
        }
    }

    #[test]
    fn aucune_position_ne_reveille_deux_creatures_a_la_fois() {
        // La contrainte qui rend la zone de depart jouable : un personnage neuf
        // ne doit jamais se retrouver a affronter deux creatures pour s'etre
        // approche d'une seule.
        let world = World::new();
        world.populate_starting_area(6);

        let posts: Vec<Position> = world
            .lock()
            .entities
            .values()
            .filter_map(|entity| entity.brain.map(|brain| brain.anchor))
            .collect();
        let aggro = AggroRule::standard(CombatRule::MELEE_RANGE_CM).aggro_radius_cm();

        for (index, first) in posts.iter().enumerate() {
            for second in posts.iter().skip(index + 1) {
                let gap = first.distance_squared(*second).isqrt();
                assert!(
                    gap > u64::from(aggro) * 2,
                    "deux postes distants de {gap} cm pour un rayon de {aggro}"
                );
            }
        }
    }

    #[test]
    fn les_creatures_apparaissent_loin_du_point_d_apparition_des_joueurs() {
        // Elles doivent etre cherchees, pas rencontrees a la connexion.
        let world = World::new();
        world.populate_starting_area(6);
        let aggro = AggroRule::standard(CombatRule::MELEE_RANGE_CM).aggro_radius_cm();

        let posts: Vec<Position> = world
            .lock()
            .entities
            .values()
            .filter_map(|entity| entity.brain.map(|brain| brain.anchor))
            .collect();

        for slot in 0..64 {
            let spawn = spawn_position(slot);
            for post in &posts {
                assert!(
                    !spawn.is_within(*post, aggro),
                    "l'emplacement {slot} nait dans le rayon d'une creature"
                );
            }
        }
    }

    #[test]
    fn les_identifiants_de_creature_tiennent_dans_un_entier_signe() {
        // Beaucoup de langages clients — GDScript, JavaScript — n'ont que des
        // entiers signes 64 bits. Un identifiant au-dela de `i64::MAX` y
        // apparait negatif : l'aller-retour reste juste, mais les journaux
        // deviennent illisibles et les comparaisons se comportent de travers.
        let world = World::new();
        world.populate_starting_area(64);

        for id in world.lock().entities.keys() {
            assert!(
                i64::try_from(*id).is_ok(),
                "l'identifiant {id} deborde un entier signe"
            );
        }
    }

    #[test]
    fn les_plages_d_identifiants_ne_se_croisent_pas() {
        // Les sessions montent depuis 1, les creatures depuis leur seuil : il
        // faudrait des milliards de connexions pour que les deux se rejoignent.
        let world = World::new();
        world.populate_starting_area(8);
        let _first_session = join(&world, 1);

        for id in world.lock().entities.keys() {
            assert!(
                *id == 1 || *id >= CREATURE_ID_BASE,
                "identifiant {id} hors plage"
            );
        }
    }

    #[test]
    fn les_creatures_ne_comptent_pas_comme_des_joueurs_connectes() {
        let world = World::new();
        world.spawn_creature(CREATURE, Position::ORIGIN);
        // `population` sert au journal d'exploitation : y melanger les creatures
        // rendrait le nombre de connectes illisible.
        assert_eq!(world.population(), 1);
    }

    #[test]
    fn un_pas_vers_une_cible_proche_l_atteint_sans_la_depasser() {
        let from = Position::new(100, 100);
        let target = Position::new(150, 100);
        assert_eq!(advance(from, target, 1_000), target);
    }

    #[test]
    fn un_pas_vers_une_cible_lointaine_respecte_l_allocation() {
        let from = Position::ORIGIN;
        let target = Position::new(10_000, 0);
        let step = advance(from, target, 300);
        assert_eq!(step, Position::new(300, 0));
    }

    #[test]
    fn un_pas_en_diagonale_ne_depasse_pas_l_allocation() {
        let from = Position::ORIGIN;
        let target = Position::new(10_000, 10_000);
        let step = advance(from, target, 300);
        assert!(
            from.distance_squared(step).isqrt() <= 300,
            "{step:?} depasse l'allocation"
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
    fn duel(world: &World) -> (Receiver<ServerMessage>, Receiver<ServerMessage>) {
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
