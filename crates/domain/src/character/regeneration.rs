/// Recuperation des points de vie hors combat.
///
/// Sans elle, chaque combat entame definitivement le personnage : les degats
/// s'accumulent d'un adversaire au suivant, et la seule facon de repartir en
/// pleine sante est de mourir. Un joueur ne peut alors enchainer que deux ou
/// trois affrontements avant d'etre bloque.
///
/// La regeneration s'interrompt pendant le combat : se soigner en encaissant
/// rendrait la fuite inutile et supprimerait tout enjeu aux echanges longs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegenerationRule {
    per_second: u32,
    idle_delay_ms: u64,
}

impl RegenerationRule {
    /// Points de vie rendus par seconde au repos.
    pub const DEFAULT_PER_SECOND: u32 = 20;

    /// Delai de calme exige avant de commencer a recuperer.
    pub const DEFAULT_IDLE_DELAY_MS: u64 = 5_000;

    #[must_use]
    pub const fn new(per_second: u32, idle_delay_ms: u64) -> Self {
        Self {
            per_second,
            idle_delay_ms,
        }
    }

    #[must_use]
    pub const fn standard() -> Self {
        Self::new(Self::DEFAULT_PER_SECOND, Self::DEFAULT_IDLE_DELAY_MS)
    }

    /// Points rendus sur `elapsed_ms`, apres `idle_ms` sans avoir ete touche.
    ///
    /// Retourne 0 tant que le delai de calme n'est pas atteint, ce qui rend la
    /// regle utilisable a chaque pas de simulation sans condition a l'appel.
    #[must_use]
    pub fn amount(self, idle_ms: u64, elapsed_ms: u64) -> u32 {
        if idle_ms < self.idle_delay_ms {
            return 0;
        }
        let healed = u64::from(self.per_second).saturating_mul(elapsed_ms) / 1000;
        u32::try_from(healed).unwrap_or(u32::MAX)
    }
}

impl Default for RegenerationRule {
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rien_ne_se_regenere_pendant_le_combat() {
        let rule = RegenerationRule::standard();
        assert_eq!(rule.amount(0, 1_000), 0);
        assert_eq!(
            rule.amount(RegenerationRule::DEFAULT_IDLE_DELAY_MS - 1, 1_000),
            0
        );
    }

    #[test]
    fn le_calme_atteint_declenche_la_recuperation() {
        let rule = RegenerationRule::standard();
        assert_eq!(
            rule.amount(RegenerationRule::DEFAULT_IDLE_DELAY_MS, 1_000),
            RegenerationRule::DEFAULT_PER_SECOND
        );
    }

    #[test]
    fn la_quantite_suit_le_temps_ecoule() {
        let rule = RegenerationRule::new(20, 0);
        assert_eq!(rule.amount(0, 1_000), 20);
        assert_eq!(rule.amount(0, 500), 10);
        assert_eq!(rule.amount(0, 5_000), 100);
    }

    #[test]
    fn un_pas_trop_court_ne_rend_rien_plutot_que_d_arrondir_en_haut() {
        // 200 ms a 20 PV/s valent 4 points ; 10 ms n'en valent aucun. Arrondir
        // au superieur ferait recuperer d'autant plus vite que le pas est fin.
        let rule = RegenerationRule::new(20, 0);
        assert_eq!(rule.amount(0, 200), 4);
        assert_eq!(rule.amount(0, 10), 0);
    }

    #[test]
    fn ne_deborde_pas_sur_des_valeurs_extremes() {
        let rule = RegenerationRule::new(u32::MAX, 0);
        assert_eq!(rule.amount(u64::MAX, u64::MAX), u32::MAX);
    }

    #[test]
    fn une_regeneration_nulle_ne_rend_jamais_rien() {
        let rule = RegenerationRule::new(0, 0);
        assert_eq!(rule.amount(u64::MAX, 10_000), 0);
    }
}
