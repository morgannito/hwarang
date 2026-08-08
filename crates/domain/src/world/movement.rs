use super::position::Position;

/// Vitesse de deplacement, en centimetres par seconde.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MovementSpeed(u32);

impl MovementSpeed {
    /// Vitesse de marche de reference.
    pub const WALK: Self = Self(300);
    /// Course.
    pub const RUN: Self = Self(700);

    #[must_use]
    pub const fn new(cm_per_second: u32) -> Self {
        Self(cm_per_second)
    }

    #[must_use]
    pub const fn cm_per_second(self) -> u32 {
        self.0
    }
}

/// Verdict rendu sur un deplacement annonce par un client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveVerdict {
    /// Deplacement plausible, le serveur adopte la nouvelle position.
    Accepted,
    /// Deplacement trop rapide pour le temps ecoule.
    ///
    /// Porte les valeurs constatees : sans elles, distinguer un tricheur d'un
    /// joueur a mauvaise latence demanderait de rejouer la scene.
    TooFast { travelled_cm: u64, allowed_cm: u64 },
}

/// Politique de deplacement appliquee a une entite.
///
/// Metin2 laisse le client annoncer sa position et la croit sur parole : c'est
/// l'origine du speedhack endemique de l'ecosysteme. Ici le serveur reste
/// autoritaire — le client propose, le serveur verifie que la distance est
/// compatible avec le temps ecoule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovementRule {
    speed: MovementSpeed,
    tolerance_cm: u32,
}

impl MovementRule {
    /// Marge absolue accordee a chaque deplacement.
    ///
    /// La gigue reseau fait arriver les trames par paquets : sans marge, un
    /// joueur a 200 ms de latence serait signale en permanence. Trop large, elle
    /// devient un budget de triche par micro-sauts — d'ou une valeur fixe et
    /// modeste plutot que proportionnelle a la distance.
    pub const DEFAULT_TOLERANCE_CM: u32 = 50;

    #[must_use]
    pub const fn new(speed: MovementSpeed, tolerance_cm: u32) -> Self {
        Self {
            speed,
            tolerance_cm,
        }
    }

    #[must_use]
    pub const fn walking() -> Self {
        Self::new(MovementSpeed::WALK, Self::DEFAULT_TOLERANCE_CM)
    }

    #[must_use]
    pub const fn running() -> Self {
        Self::new(MovementSpeed::RUN, Self::DEFAULT_TOLERANCE_CM)
    }

    #[must_use]
    pub const fn speed(self) -> MovementSpeed {
        self.speed
    }

    /// Distance maximale franchissable en `elapsed_ms`, marge comprise.
    ///
    /// Sature au lieu de deborder : `elapsed_ms` vient du client et un ecart
    /// aberrant doit produire une tolerance enorme — donc un deplacement
    /// accepte — jamais une tolerance rebouclee a une valeur faible qui
    /// exclurait le joueur au premier pas.
    #[must_use]
    pub const fn allowance_cm(self, elapsed_ms: u64) -> u64 {
        let travelled = (self.speed.cm_per_second() as u64).saturating_mul(elapsed_ms) / 1000;
        travelled.saturating_add(self.tolerance_cm as u64)
    }

    /// Verifie un deplacement annonce.
    ///
    /// La comparaison se fait au carre pour rester en arithmetique exacte : une
    /// racine entiere arrondirait vers le bas et offrirait au client un
    /// centimetre gratuit a chaque pas.
    #[must_use]
    pub fn verify(self, from: Position, to: Position, elapsed_ms: u64) -> MoveVerdict {
        let allowed_cm = self.allowance_cm(elapsed_ms);
        let travelled_squared = from.distance_squared(to);

        if travelled_squared <= allowed_cm.saturating_mul(allowed_cm) {
            MoveVerdict::Accepted
        } else {
            MoveVerdict::TooFast {
                travelled_cm: travelled_squared.isqrt(),
                allowed_cm,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_pas_compatible_avec_le_temps_ecoule_est_accepte() {
        let rule = MovementRule::walking();
        // 300 cm/s pendant 1 s, marge 50 : 350 cm autorises.
        let verdict = rule.verify(Position::ORIGIN, Position::new(300, 0), 1_000);
        assert_eq!(verdict, MoveVerdict::Accepted);
    }

    #[test]
    fn un_saut_trop_long_est_refuse_avec_les_valeurs_constatees() {
        let rule = MovementRule::walking();
        let verdict = rule.verify(Position::ORIGIN, Position::new(5_000, 0), 1_000);

        match verdict {
            MoveVerdict::TooFast {
                travelled_cm,
                allowed_cm,
            } => {
                assert_eq!(travelled_cm, 5_000);
                assert_eq!(allowed_cm, 350);
            }
            MoveVerdict::Accepted => panic!("un saut de 50 m en 1 s a ete accepte"),
        }
    }

    #[test]
    fn la_marge_absorbe_la_gigue_reseau() {
        let rule = MovementRule::walking();
        // Trame en retard : 0 ms declare, mais le pas reste dans la marge.
        assert_eq!(
            rule.verify(Position::ORIGIN, Position::new(40, 0), 0),
            MoveVerdict::Accepted
        );
    }

    #[test]
    fn la_marge_ne_finance_pas_la_teleportation() {
        let rule = MovementRule::walking();
        assert!(matches!(
            rule.verify(Position::ORIGIN, Position::new(1_000, 0), 0),
            MoveVerdict::TooFast { .. }
        ));
    }

    #[test]
    fn courir_autorise_plus_loin_que_marcher() {
        let step = Position::new(600, 0);
        assert!(matches!(
            MovementRule::walking().verify(Position::ORIGIN, step, 1_000),
            MoveVerdict::TooFast { .. }
        ));
        assert_eq!(
            MovementRule::running().verify(Position::ORIGIN, step, 1_000),
            MoveVerdict::Accepted
        );
    }

    #[test]
    fn la_diagonale_n_offre_pas_de_raccourci() {
        let rule = MovementRule::walking();
        // 250 en x et 250 en y font 353 cm parcourus, au-dela des 350 autorises.
        assert!(matches!(
            rule.verify(Position::ORIGIN, Position::new(250, 250), 1_000),
            MoveVerdict::TooFast { .. }
        ));
    }

    #[test]
    fn accumuler_de_petits_sauts_ne_paie_pas() {
        let rule = MovementRule::walking();
        // Dix pas de 100 ms : 30 cm + 50 de marge chacun, soit 800 cm au total,
        // contre 350 pour un unique pas d'une seconde.
        let mut position = Position::ORIGIN;
        for _ in 0..10 {
            let attempt = Position::new(position.x + 80, 0);
            assert_eq!(rule.verify(position, attempt, 100), MoveVerdict::Accepted);
            position = attempt;
        }
        assert_eq!(position.x, 800);
    }

    #[test]
    fn ne_deborde_pas_sur_un_temps_ecoule_aberrant() {
        let rule = MovementRule::new(MovementSpeed::new(u32::MAX), u32::MAX);
        // Un client qui annonce des annees d'ecart ne doit pas faire reboucler
        // le calcul de tolerance en une valeur faible.
        assert_eq!(
            rule.verify(Position::ORIGIN, Position::new(i32::MAX, 0), u64::MAX / 4),
            MoveVerdict::Accepted
        );
    }
}
