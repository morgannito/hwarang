use crate::shared::{Experience, Level};
use crate::world::Position;

/// Raison pour laquelle une attaque n'est pas recevable.
///
/// Chaque variante porte les valeurs constatees : le client doit pouvoir
/// afficher « trop loin de 3 m » plutot qu'un refus opaque, et l'exploitant doit
/// pouvoir distinguer un joueur mal synchronise d'un client modifie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackRejection {
    /// Cible au-dela de l'allonge.
    OutOfRange { distance_cm: u64, range_cm: u32 },
    /// Attaque plus rapide que la cadence autorisee.
    TooSoon { elapsed_ms: u64, cooldown_ms: u64 },
    /// L'attaquant est mort.
    AttackerDown,
    /// La cible est deja morte : evite l'acharnement et le vol d'experience sur
    /// un cadavre.
    TargetDown,
    /// Se prendre soi-meme pour cible.
    SelfTarget,
}

/// Regle d'engagement appliquee a une entite.
///
/// Sans cadence serveur, un client modifie envoie mille attaques par seconde et
/// vide n'importe quelle barre de vie instantanement. C'est exactement la meme
/// faiblesse que le deplacement non valide, appliquee a l'offensive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombatRule {
    range_cm: u32,
    cooldown_ms: u64,
}

impl CombatRule {
    /// Allonge au corps a corps : 2 m.
    pub const MELEE_RANGE_CM: u32 = 200;
    /// Une attaque par seconde.
    pub const DEFAULT_COOLDOWN_MS: u64 = 1_000;

    #[must_use]
    pub const fn new(range_cm: u32, cooldown_ms: u64) -> Self {
        Self {
            range_cm,
            cooldown_ms,
        }
    }

    #[must_use]
    pub const fn melee() -> Self {
        Self::new(Self::MELEE_RANGE_CM, Self::DEFAULT_COOLDOWN_MS)
    }

    #[must_use]
    pub const fn range_cm(self) -> u32 {
        self.range_cm
    }

    /// Verifie qu'une attaque est recevable.
    ///
    /// L'ordre des controles determine ce que le joueur lit quand plusieurs
    /// causes s'appliquent, et il est choisi sur **ce qu'il peut en faire** :
    ///
    /// 1. causes structurelles (cible invalide, morte) — rien ne les corrige ;
    /// 2. portee — le joueur peut avancer ;
    /// 3. cadence — se resout seule en patientant.
    ///
    /// Annoncer `TooSoon` a quelqu'un qui est aussi hors d'allonge l'enverrait
    /// attendre au lieu de se rapprocher.
    ///
    /// # Errors
    /// Voir [`AttackRejection`].
    pub fn authorize(self, engagement: Engagement, elapsed_ms: u64) -> Result<(), AttackRejection> {
        if engagement.same_entity {
            return Err(AttackRejection::SelfTarget);
        }
        if !engagement.attacker_alive {
            return Err(AttackRejection::AttackerDown);
        }
        if !engagement.target_alive {
            return Err(AttackRejection::TargetDown);
        }
        if !engagement
            .attacker_at
            .is_within(engagement.target_at, self.range_cm)
        {
            return Err(AttackRejection::OutOfRange {
                distance_cm: engagement
                    .attacker_at
                    .distance_squared(engagement.target_at)
                    .isqrt(),
                range_cm: self.range_cm,
            });
        }
        if elapsed_ms < self.cooldown_ms {
            return Err(AttackRejection::TooSoon {
                elapsed_ms,
                cooldown_ms: self.cooldown_ms,
            });
        }
        Ok(())
    }
}

/// Situation soumise a [`CombatRule::authorize`].
///
/// Regroupee en un type plutot qu'en six parametres : l'ordre de deux `bool` et
/// de deux `Position` consecutifs s'inverse silencieusement a l'appel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Engagement {
    pub attacker_at: Position,
    pub target_at: Position,
    pub attacker_alive: bool,
    pub target_alive: bool,
    pub same_entity: bool,
}

/// Experience accordee pour l'elimination d'une entite de ce niveau.
///
/// Croissance lineaire et non calquee sur la courbe de progression : sinon le
/// nombre d'eliminations necessaires pour monter d'un palier resterait constant
/// sur toute la partie, et la progression n'aurait plus de relief.
#[must_use]
pub fn experience_reward(victim_level: Level) -> Experience {
    const BASE: u64 = 50;
    Experience::new(BASE * u64::from(victim_level.get()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engagement(attacker: Position, target: Position) -> Engagement {
        Engagement {
            attacker_at: attacker,
            target_at: target,
            attacker_alive: true,
            target_alive: true,
            same_entity: false,
        }
    }

    fn adjacent() -> Engagement {
        engagement(Position::ORIGIN, Position::new(100, 0))
    }

    #[test]
    fn une_attaque_a_portee_et_a_cadence_est_recevable() {
        assert_eq!(CombatRule::melee().authorize(adjacent(), 1_000), Ok(()));
    }

    #[test]
    fn une_cible_hors_d_allonge_est_refusee_avec_la_distance() {
        let far = engagement(Position::ORIGIN, Position::new(1_000, 0));
        assert_eq!(
            CombatRule::melee().authorize(far, 5_000),
            Err(AttackRejection::OutOfRange {
                distance_cm: 1_000,
                range_cm: CombatRule::MELEE_RANGE_CM,
            })
        );
    }

    #[test]
    fn la_cadence_borne_le_rythme_des_attaques() {
        assert_eq!(
            CombatRule::melee().authorize(adjacent(), 10),
            Err(AttackRejection::TooSoon {
                elapsed_ms: 10,
                cooldown_ms: CombatRule::DEFAULT_COOLDOWN_MS,
            })
        );
    }

    #[test]
    fn la_cadence_est_atteinte_a_sa_borne_exacte() {
        assert_eq!(
            CombatRule::melee().authorize(adjacent(), CombatRule::DEFAULT_COOLDOWN_MS),
            Ok(())
        );
    }

    #[test]
    fn frapper_un_cadavre_est_refuse_pour_ce_motif() {
        let dead_target = Engagement {
            target_alive: false,
            ..adjacent()
        };
        assert_eq!(
            CombatRule::melee().authorize(dead_target, 5_000),
            Err(AttackRejection::TargetDown)
        );
    }

    #[test]
    fn un_mort_ne_frappe_pas() {
        let dead_attacker = Engagement {
            attacker_alive: false,
            ..adjacent()
        };
        assert_eq!(
            CombatRule::melee().authorize(dead_attacker, 5_000),
            Err(AttackRejection::AttackerDown)
        );
    }

    #[test]
    fn se_prendre_pour_cible_est_refuse() {
        let oneself = Engagement {
            same_entity: true,
            ..adjacent()
        };
        assert_eq!(
            CombatRule::melee().authorize(oneself, 5_000),
            Err(AttackRejection::SelfTarget)
        );
    }

    #[test]
    fn la_portee_prime_sur_la_cadence() {
        // Trop loin ET trop tot : le joueur doit lire ce qu'il peut corriger en
        // agissant, pas ce qui se resout en patientant.
        let far = engagement(Position::ORIGIN, Position::new(1_000, 0));
        assert!(matches!(
            CombatRule::melee().authorize(far, 0),
            Err(AttackRejection::OutOfRange { .. })
        ));
    }

    #[test]
    fn le_motif_structurel_prime_sur_le_motif_circonstanciel() {
        // Cible morte ET hors de portee ET trop tot : le joueur doit lire la
        // cause qu'il ne peut pas corriger en attendant ou en avancant.
        let hopeless = Engagement {
            target_alive: false,
            ..engagement(Position::ORIGIN, Position::new(100_000, 0))
        };
        assert_eq!(
            CombatRule::melee().authorize(hopeless, 0),
            Err(AttackRejection::TargetDown)
        );
    }

    #[test]
    fn la_recompense_croit_avec_le_niveau_de_la_victime() {
        let low = experience_reward(Level::FIRST);
        let high = experience_reward(Level::LAST);
        assert!(high > low);
        assert!(low > Experience::ZERO);
    }
}
