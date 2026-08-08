use super::level::Level;

/// Points d'experience accumules depuis le dernier passage de palier.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Experience(u64);

impl Experience {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(amount: u64) -> Self {
        Self(amount)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Addition saturante : un gain aberrant plafonne au lieu de reboucler a
    /// zero, ce qui transformerait un bug d'equilibrage en perte de progression.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// Courbe de progression, exprimee comme une formule parametree.
///
/// Metin2 embarque une table de 120 constantes compilees dans le binaire :
/// rejouer l'equilibrage impose de recompiler le serveur. Ici la courbe est une
/// donnee du domaine, injectable et donc modifiable a chaud ou par configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressionCurve {
    base: u64,
}

impl ProgressionCurve {
    /// Courbe de reference : `base * niveau^2 * (niveau + 10) / 10`.
    ///
    /// Cubique, donc l'ecart entre paliers s'accroit continument, sans les
    /// plateaux ni les sauts brutaux d'une table saisie a la main.
    pub const DEFAULT: Self = Self { base: 100 };

    /// `None` si `base` est nul : une courbe plate rendrait la progression
    /// instantanee et casserait l'invariant de monotonie.
    #[must_use]
    pub const fn new(base: u64) -> Option<Self> {
        if base == 0 { None } else { Some(Self { base }) }
    }

    /// Experience a accumuler pour quitter `level`.
    ///
    /// Le calcul passe par `u128` puis sature : aucune combinaison de `base` et
    /// de niveau ne peut produire un rebouclage silencieux.
    #[must_use]
    pub fn required_to_leave(self, level: Level) -> Experience {
        let l = u128::from(level.get());
        let raw = u128::from(self.base) * l * l * (l + 10) / 10;
        Experience(u64::try_from(raw).unwrap_or(u64::MAX))
    }
}

impl Default for ProgressionCurve {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::shared::level::MAX_LEVEL;

    #[test]
    fn le_cout_croit_strictement_sur_toute_la_plage() {
        let curve = ProgressionCurve::DEFAULT;
        let mut previous = Experience::ZERO;
        for value in 1..=MAX_LEVEL {
            let cost = curve.required_to_leave(Level::new(value).unwrap());
            assert!(
                cost > previous,
                "palier {value} n'est pas plus couteux que le precedent"
            );
            previous = cost;
        }
    }

    #[test]
    fn refuse_une_courbe_plate() {
        assert_eq!(ProgressionCurve::new(0), None);
    }

    #[test]
    fn sature_au_lieu_de_reboucler() {
        let curve = ProgressionCurve::new(u64::MAX).unwrap();
        assert_eq!(curve.required_to_leave(Level::LAST).get(), u64::MAX);
    }

    #[test]
    fn l_addition_sature() {
        let almost_full = Experience::new(u64::MAX - 1);
        assert_eq!(
            almost_full.saturating_add(Experience::new(100)).get(),
            u64::MAX
        );
    }
}
