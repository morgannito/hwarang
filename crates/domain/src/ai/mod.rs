//! Comportement des entites non joueuses.
//!
//! La regle decide d'une **intention** a partir d'une situation ; elle ne
//! manipule aucun identifiant et ne deplace rien. L'appelant sait qui est la
//! cible et applique le mouvement ou l'attaque.
//!
//! Cette separation garde la decision rejouable : rejouer un combat conteste
//! demande de savoir ce que la creature a decide, pas comment le serveur l'a
//! execute.

use crate::world::Position;

/// Ce que la creature poursuit a cet instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stance {
    /// A son poste, sans cible.
    #[default]
    Idle,
    /// Engagee sur une cible.
    Engaged,
    /// Rentre a son poste.
    Returning,
}

/// Ce qu'une creature percoit d'une cible potentielle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threat {
    pub position: Position,
    pub alive: bool,
}

/// Situation soumise a la regle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Situation {
    /// Ou se trouve la creature.
    pub creature: Position,
    /// Son poste d'origine, ou elle est apparue.
    pub anchor: Position,
    /// La cible la plus proche, si elle en percoit une.
    pub nearest: Option<Threat>,
    pub stance: Stance,
}

/// Ce que la creature veut faire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Ne rien faire.
    Hold,
    /// Se rapprocher de ce point.
    Approach(Position),
    /// Frapper la cible : elle est a portee.
    Strike,
    /// Regagner ce point, sans cible.
    ReturnTo(Position),
}

/// Politique d'agressivite d'une creature.
///
/// Trois distances, toutes en centimetres et toutes mesurees depuis la creature
/// — d'ou le suffixe commun, qui les rend comparables d'un coup d'oeil.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggroRule {
    aggro_radius_cm: u32,
    leash_radius_cm: u32,
    strike_radius_cm: u32,
}

impl AggroRule {
    /// Distance a laquelle une creature remarque un joueur : 15 m.
    pub const DEFAULT_AGGRO_CM: u32 = 1_500;
    /// Distance maximale a son poste avant abandon : 30 m.
    pub const DEFAULT_LEASH_CM: u32 = 3_000;

    /// `None` si la laisse est plus courte que le rayon d'agressivite : la
    /// creature partirait vers une cible qu'elle abandonnerait au pas suivant,
    /// et oscillerait indefiniment entre poursuite et retour.
    #[must_use]
    pub const fn new(
        aggro_radius_cm: u32,
        leash_radius_cm: u32,
        strike_radius_cm: u32,
    ) -> Option<Self> {
        if leash_radius_cm < aggro_radius_cm {
            None
        } else {
            Some(Self {
                aggro_radius_cm,
                leash_radius_cm,
                strike_radius_cm,
            })
        }
    }

    /// Politique de reference, calee sur l'allonge du corps a corps.
    #[must_use]
    pub const fn standard(strike_radius_cm: u32) -> Self {
        Self {
            aggro_radius_cm: Self::DEFAULT_AGGRO_CM,
            leash_radius_cm: Self::DEFAULT_LEASH_CM,
            strike_radius_cm,
        }
    }

    #[must_use]
    pub const fn aggro_radius_cm(self) -> u32 {
        self.aggro_radius_cm
    }

    /// Decide de l'intention, et de la posture qui en decoule.
    ///
    /// La laisse se mesure depuis le **poste**, jamais depuis la cible : sinon
    /// un joueur peut trainer une creature a travers toute la carte en restant
    /// devant elle, exploit classique consistant a amasser un troupeau pour le
    /// deposer sur quelqu'un d'autre.
    #[must_use]
    pub fn decide(self, situation: Situation) -> (Intent, Stance) {
        let too_far_from_post = !situation
            .creature
            .is_within(situation.anchor, self.leash_radius_cm);

        if too_far_from_post {
            return Self::go_home(situation);
        }

        let Some(threat) = situation.nearest.filter(|threat| threat.alive) else {
            return Self::go_home(situation);
        };

        // Une creature deja engagee poursuit au-dela de son rayon de detection,
        // dans la limite de sa laisse. Sans cela elle lacherait prise des que le
        // joueur recule d'un pas, et aucun combat ne s'engagerait jamais.
        let noticed = match situation.stance {
            Stance::Engaged => true,
            Stance::Idle | Stance::Returning => situation
                .creature
                .is_within(threat.position, self.aggro_radius_cm),
        };

        if !noticed {
            return Self::go_home(situation);
        }

        if situation
            .creature
            .is_within(threat.position, self.strike_radius_cm)
        {
            (Intent::Strike, Stance::Engaged)
        } else {
            (Intent::Approach(threat.position), Stance::Engaged)
        }
    }

    /// Rentre au poste, ou attend si l'on y est deja.
    fn go_home(situation: Situation) -> (Intent, Stance) {
        if situation.creature == situation.anchor {
            (Intent::Hold, Stance::Idle)
        } else {
            (Intent::ReturnTo(situation.anchor), Stance::Returning)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIKE: u32 = 200;

    fn rule() -> AggroRule {
        AggroRule::standard(STRIKE)
    }

    fn at_post(nearest: Option<Threat>, stance: Stance) -> Situation {
        Situation {
            creature: Position::ORIGIN,
            anchor: Position::ORIGIN,
            nearest,
            stance,
        }
    }

    fn threat_at(x: i32) -> Threat {
        Threat {
            position: Position::new(x, 0),
            alive: true,
        }
    }

    #[test]
    fn une_laisse_plus_courte_que_la_detection_est_refusee() {
        // Elle produirait une oscillation : partir vers une cible, puis
        // l'abandonner au pas suivant, indefiniment.
        assert_eq!(AggroRule::new(1_500, 1_000, STRIKE), None);
        assert!(AggroRule::new(1_000, 1_000, STRIKE).is_some());
    }

    #[test]
    fn sans_cible_la_creature_reste_a_son_poste() {
        assert_eq!(
            rule().decide(at_post(None, Stance::Idle)),
            (Intent::Hold, Stance::Idle)
        );
    }

    #[test]
    fn une_cible_lointaine_passe_inapercue() {
        let far = threat_at(i32::try_from(AggroRule::DEFAULT_AGGRO_CM).unwrap_or(0) + 1);
        assert_eq!(
            rule().decide(at_post(Some(far), Stance::Idle)),
            (Intent::Hold, Stance::Idle)
        );
    }

    #[test]
    fn une_cible_dans_le_rayon_declenche_la_poursuite() {
        let (intent, stance) = rule().decide(at_post(Some(threat_at(800)), Stance::Idle));
        assert_eq!(intent, Intent::Approach(Position::new(800, 0)));
        assert_eq!(stance, Stance::Engaged);
    }

    #[test]
    fn une_cible_au_contact_est_frappee() {
        assert_eq!(
            rule().decide(at_post(Some(threat_at(100)), Stance::Idle)),
            (Intent::Strike, Stance::Engaged)
        );
    }

    #[test]
    fn une_cible_morte_n_est_plus_poursuivie() {
        let dead = Some(Threat {
            position: Position::new(300, 0),
            alive: false,
        });
        let engaged = Situation {
            creature: Position::new(500, 0),
            ..at_post(dead, Stance::Engaged)
        };
        assert_eq!(
            rule().decide(engaged),
            (Intent::ReturnTo(Position::ORIGIN), Stance::Returning)
        );
    }

    #[test]
    fn une_creature_engagee_poursuit_au_dela_de_son_rayon_de_detection() {
        // Sans cela elle lacherait des que le joueur recule d'un pas, et aucun
        // combat ne s'engagerait.
        let beyond = threat_at(i32::try_from(AggroRule::DEFAULT_AGGRO_CM).unwrap_or(0) + 500);
        let (intent, stance) = rule().decide(at_post(Some(beyond), Stance::Engaged));
        assert!(matches!(intent, Intent::Approach(_)));
        assert_eq!(stance, Stance::Engaged);
    }

    #[test]
    fn la_laisse_se_mesure_depuis_le_poste_et_non_depuis_la_cible() {
        // Le joueur est colle a la creature, mais celle-ci a trop derive : elle
        // rentre. C'est ce qui empeche de trainer un troupeau a travers la carte.
        let dragged = Situation {
            creature: Position::new(
                i32::try_from(AggroRule::DEFAULT_LEASH_CM).unwrap_or(0) + 1,
                0,
            ),
            anchor: Position::ORIGIN,
            nearest: Some(Threat {
                position: Position::new(
                    i32::try_from(AggroRule::DEFAULT_LEASH_CM).unwrap_or(0) + 50,
                    0,
                ),
                alive: true,
            }),
            stance: Stance::Engaged,
        };

        assert_eq!(
            rule().decide(dragged),
            (Intent::ReturnTo(Position::ORIGIN), Stance::Returning)
        );
    }

    #[test]
    fn la_laisse_est_inclusive_a_sa_borne() {
        let at_limit = Situation {
            creature: Position::new(i32::try_from(AggroRule::DEFAULT_LEASH_CM).unwrap_or(0), 0),
            anchor: Position::ORIGIN,
            nearest: None,
            stance: Stance::Engaged,
        };
        // Dans la laisse, mais sans cible : retour au poste.
        assert_eq!(
            rule().decide(at_limit),
            (Intent::ReturnTo(Position::ORIGIN), Stance::Returning)
        );
    }

    #[test]
    fn une_fois_rentree_la_creature_se_remet_en_attente() {
        let home = Situation {
            creature: Position::ORIGIN,
            anchor: Position::ORIGIN,
            nearest: None,
            stance: Stance::Returning,
        };
        assert_eq!(rule().decide(home), (Intent::Hold, Stance::Idle));
    }

    #[test]
    fn une_creature_qui_rentre_peut_etre_reengagee_a_portee_de_detection() {
        let intercepted = Situation {
            creature: Position::new(500, 0),
            anchor: Position::ORIGIN,
            nearest: Some(threat_at(1_200)),
            stance: Stance::Returning,
        };
        assert_eq!(
            rule().decide(intercepted),
            (Intent::Approach(Position::new(1_200, 0)), Stance::Engaged)
        );
    }

    #[test]
    fn la_decision_est_stable_pour_une_situation_identique() {
        // Fonction pure : deux appels sur la meme situation donnent le meme
        // resultat, ce qui rend un comportement conteste rejouable.
        let situation = at_post(Some(threat_at(900)), Stance::Idle);
        assert_eq!(rule().decide(situation), rule().decide(situation));
    }
}
