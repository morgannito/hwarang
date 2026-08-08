//! Agregat personnage : identite, attributs, progression.

use crate::shared::{Experience, Level, ProgressionCurve, Vitals};

/// Identifiant opaque, attribue par la couche de persistance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharacterId(u64);

impl CharacterId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Attributs primaires. Toute statistique derivee se recalcule a partir d'eux :
/// il n'existe pas de valeur stockee qui puisse desynchroniser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attributes {
    pub strength: u16,
    pub dexterity: u16,
    pub vitality: u16,
    pub intellect: u16,
}

impl Attributes {
    const BASE_HEALTH: u32 = 100;
    const HEALTH_PER_VITALITY: u32 = 30;
    const HEALTH_PER_LEVEL: u32 = 40;

    /// Points de vie maximum pour ces attributs a ce niveau.
    #[must_use]
    pub const fn max_health(self, level: Level) -> u32 {
        Self::BASE_HEALTH
            + self.vitality as u32 * Self::HEALTH_PER_VITALITY
            + (level.get() as u32 - 1) * Self::HEALTH_PER_LEVEL
    }
}

/// Resultat d'un gain d'experience.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressionOutcome {
    /// Experience accumulee, palier inchange.
    Accumulated,
    /// Un ou plusieurs paliers franchis d'un coup.
    LeveledUp { from: Level, to: Level },
    /// Palier maximum : le gain est ignore.
    AtMaxLevel,
}

/// Personnage jouable.
///
/// Les transitions retournent une nouvelle valeur au lieu de muter sur place :
/// un etat invalide ne peut pas etre observe a mi-chemin d'une regle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Character {
    id: CharacterId,
    level: Level,
    experience: Experience,
    attributes: Attributes,
    vitals: Vitals,
    curve: ProgressionCurve,
}

impl Character {
    /// Cree un personnage de niveau 1, en pleine sante.
    #[must_use]
    pub fn create(id: CharacterId, attributes: Attributes, curve: ProgressionCurve) -> Self {
        let level = Level::FIRST;
        let vitals = Vitals::full(attributes.max_health(level))
            .unwrap_or_else(|| unreachable!("max_health inclut une base non nulle"));

        Self {
            id,
            level,
            experience: Experience::ZERO,
            attributes,
            vitals,
            curve,
        }
    }

    #[must_use]
    pub const fn id(self) -> CharacterId {
        self.id
    }

    #[must_use]
    pub const fn level(self) -> Level {
        self.level
    }

    #[must_use]
    pub const fn experience(self) -> Experience {
        self.experience
    }

    #[must_use]
    pub const fn attributes(self) -> Attributes {
        self.attributes
    }

    #[must_use]
    pub const fn vitals(self) -> Vitals {
        self.vitals
    }

    #[must_use]
    pub const fn is_alive(self) -> bool {
        !self.vitals.is_depleted()
    }

    /// Applique un gain d'experience et franchit autant de paliers que le
    /// montant le permet.
    ///
    /// Le franchissement multiple est traite en une passe : un gain massif
    /// (quete de fin, evenement) ne doit pas etre tronque a un seul palier ni
    /// obliger a rejouer la regle en boucle depuis l'appelant.
    #[must_use]
    pub fn gain_experience(self, amount: Experience) -> (Self, ProgressionOutcome) {
        if self.level.is_max() {
            return (self, ProgressionOutcome::AtMaxLevel);
        }

        let mut level = self.level;
        let mut pool = self.experience.saturating_add(amount);

        loop {
            let needed = self.curve.required_to_leave(level);
            if pool < needed {
                break;
            }
            let Some(next) = level.next() else {
                // Palier terminal atteint avec de quoi le franchir : le surplus
                // est ecrete. Le conserver laisserait une experience superieure
                // au seuil de son propre palier, et toute barre de progression
                // calculee comme `experience / seuil` afficherait plus de 100 %.
                pool = needed.saturating_sub(Experience::new(1));
                break;
            };
            pool = pool.saturating_sub(needed);
            level = next;
        }

        if level == self.level {
            return (
                Self {
                    experience: pool,
                    ..self
                },
                ProgressionOutcome::Accumulated,
            );
        }

        // La jauge suit le nouveau plafond en conservant la proportion : monter
        // de palier ne soigne pas, mais ne penalise pas non plus.
        let vitals = self
            .vitals
            .with_max(self.attributes.max_health(level))
            .unwrap_or(self.vitals);

        (
            Self {
                level,
                experience: pool,
                vitals,
                ..self
            },
            ProgressionOutcome::LeveledUp {
                from: self.level,
                to: level,
            },
        )
    }

    /// Applique des degats deja resolus par le contexte combat.
    #[must_use]
    pub fn take_damage(self, amount: u32) -> Self {
        Self {
            vitals: self.vitals.damaged_by(amount),
            ..self
        }
    }

    /// Remet le personnage en jeu, jauges pleines.
    ///
    /// Ni palier ni experience ne sont retires : une penalite de mort est une
    /// decision d'equilibrage, pas une consequence mecanique — et l'introduire
    /// ici la rendrait invisible depuis les regles de jeu.
    #[must_use]
    pub fn respawn(self) -> Self {
        Self {
            vitals: Vitals::full(self.attributes.max_health(self.level)).unwrap_or(self.vitals),
            ..self
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn hero() -> Character {
        Character::create(
            CharacterId::new(1),
            Attributes {
                vitality: 10,
                ..Attributes::default()
            },
            ProgressionCurve::DEFAULT,
        )
    }

    #[test]
    fn un_nouveau_personnage_demarre_au_premier_palier_en_pleine_sante() {
        let character = hero();
        assert_eq!(character.level(), Level::FIRST);
        assert_eq!(character.experience(), Experience::ZERO);
        assert_eq!(character.vitals().current(), character.vitals().max());
        assert!(character.is_alive());
    }

    #[test]
    fn un_gain_insuffisant_accumule_sans_changer_de_palier() {
        let (character, outcome) = hero().gain_experience(Experience::new(10));
        assert_eq!(outcome, ProgressionOutcome::Accumulated);
        assert_eq!(character.level(), Level::FIRST);
        assert_eq!(character.experience().get(), 10);
    }

    #[test]
    fn le_franchissement_conserve_le_reliquat() {
        let curve = ProgressionCurve::DEFAULT;
        let needed = curve.required_to_leave(Level::FIRST).get();

        let (character, outcome) = hero().gain_experience(Experience::new(needed + 7));
        assert_eq!(
            outcome,
            ProgressionOutcome::LeveledUp {
                from: Level::FIRST,
                to: Level::new(2).unwrap(),
            }
        );
        assert_eq!(character.experience().get(), 7);
    }

    #[test]
    fn un_gain_massif_franchit_plusieurs_paliers_en_une_passe() {
        let (character, outcome) = hero().gain_experience(Experience::new(1_000_000));
        assert!(character.level().get() > 2, "un seul palier a ete franchi");
        assert!(matches!(outcome, ProgressionOutcome::LeveledUp { .. }));
    }

    #[test]
    fn l_experience_reste_toujours_sous_le_seuil_de_son_palier() {
        // Invariant transverse : `experience` est un reliquat, jamais un cumul.
        // Il doit le rester au palier terminal, ou la boucle de franchissement
        // s'arrete sans consommer le seuil.
        let curve = ProgressionCurve::DEFAULT;
        for gain in [1, 1_000, 100_000, u64::MAX / 2, u64::MAX] {
            let (character, _) = hero().gain_experience(Experience::new(gain));
            assert!(
                character.experience() < curve.required_to_leave(character.level()),
                "gain {gain} : reliquat {:?} au-dela du seuil du palier {}",
                character.experience(),
                character.level().get()
            );
        }
    }

    #[test]
    fn le_palier_maximum_ignore_les_gains() {
        let mut character = hero().gain_experience(Experience::new(u64::MAX)).0;
        assert_eq!(character.level(), Level::LAST);

        let outcome;
        (character, outcome) = character.gain_experience(Experience::new(1_000));
        assert_eq!(outcome, ProgressionOutcome::AtMaxLevel);
        assert_eq!(character.level(), Level::LAST);
    }

    #[test]
    fn monter_de_palier_ne_soigne_pas() {
        let wounded = hero().take_damage(100);
        let ratio_before =
            f64::from(wounded.vitals().current()) / f64::from(wounded.vitals().max());

        let (grown, _) = wounded.gain_experience(Experience::new(100_000));
        let ratio_after = f64::from(grown.vitals().current()) / f64::from(grown.vitals().max());

        assert!(grown.level() > wounded.level());
        assert!((ratio_before - ratio_after).abs() < 0.01);
        assert!(grown.vitals().max() > wounded.vitals().max());
    }

    #[test]
    fn les_degats_letaux_marquent_le_personnage_hors_combat() {
        let character = hero();
        let dead = character.take_damage(character.vitals().max());
        assert!(!dead.is_alive());
    }

    #[test]
    fn reapparaitre_restaure_les_jauges_sans_toucher_a_la_progression() {
        let (grown, _) = hero().gain_experience(Experience::new(100_000));
        let dead = grown.take_damage(u32::MAX);
        assert!(!dead.is_alive());

        let revived = dead.respawn();
        assert!(revived.is_alive());
        assert_eq!(revived.vitals().current(), revived.vitals().max());
        assert_eq!(revived.level(), grown.level());
        assert_eq!(revived.experience(), grown.experience());
    }

    #[test]
    fn la_vitalite_augmente_les_points_de_vie() {
        let frail = Character::create(
            CharacterId::new(2),
            Attributes::default(),
            ProgressionCurve::DEFAULT,
        );
        assert!(hero().vitals().max() > frail.vitals().max());
    }
}
