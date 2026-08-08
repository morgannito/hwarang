/// Jauge bornee (points de vie, energie), invariant `current <= max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vitals {
    current: u32,
    max: u32,
}

impl Vitals {
    /// Cree une jauge pleine. `None` si `max` est nul : une entite sans point
    /// de vie maximum serait morte des sa creation.
    #[must_use]
    pub const fn full(max: u32) -> Option<Self> {
        if max == 0 {
            None
        } else {
            Some(Self { current: max, max })
        }
    }

    #[must_use]
    pub const fn current(self) -> u32 {
        self.current
    }

    #[must_use]
    pub const fn max(self) -> u32 {
        self.max
    }

    #[must_use]
    pub const fn is_depleted(self) -> bool {
        self.current == 0
    }

    /// Retranche `amount`, plancher a zero.
    #[must_use]
    pub const fn damaged_by(self, amount: u32) -> Self {
        Self {
            current: self.current.saturating_sub(amount),
            max: self.max,
        }
    }

    /// Ajoute `amount`, plafond a `max`.
    ///
    /// Un mort ne remonte pas : la resurrection est une transition d'etat du
    /// contexte personnage, pas un simple soin.
    #[must_use]
    pub const fn healed_by(self, amount: u32) -> Self {
        if self.is_depleted() {
            return self;
        }
        let raised = self.current.saturating_add(amount);
        Self {
            current: if raised > self.max { self.max } else { raised },
            max: self.max,
        }
    }

    /// Redimensionne le plafond en conservant la proportion courante.
    ///
    /// Sans cela, un gain de vitalite en plein combat rendrait le personnage
    /// proportionnellement plus fragile qu'avant.
    #[must_use]
    pub fn with_max(self, new_max: u32) -> Option<Self> {
        if new_max == 0 {
            return None;
        }
        let ratio = u64::from(self.current) * u64::from(new_max) / u64::from(self.max);
        let scaled = u32::try_from(ratio).unwrap_or(new_max).min(new_max);
        Some(Self {
            current: if scaled == 0 && self.current > 0 {
                1
            } else {
                scaled
            },
            max: new_max,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn refuse_un_maximum_nul() {
        assert_eq!(Vitals::full(0), None);
    }

    #[test]
    fn les_degats_ont_un_plancher_a_zero() {
        let vitals = Vitals::full(100).unwrap().damaged_by(500);
        assert_eq!(vitals.current(), 0);
        assert!(vitals.is_depleted());
    }

    #[test]
    fn les_soins_ont_un_plafond_au_maximum() {
        let vitals = Vitals::full(100).unwrap().damaged_by(30).healed_by(500);
        assert_eq!(vitals.current(), 100);
    }

    #[test]
    fn un_mort_ne_se_soigne_pas() {
        let dead = Vitals::full(100).unwrap().damaged_by(100);
        assert!(dead.healed_by(50).is_depleted());
    }

    #[test]
    fn le_redimensionnement_conserve_la_proportion() {
        let half = Vitals::full(100).unwrap().damaged_by(50);
        let grown = half.with_max(200).unwrap();
        assert_eq!(grown.current(), 100);
        assert_eq!(grown.max(), 200);
    }

    #[test]
    fn le_redimensionnement_ne_tue_pas_un_survivant() {
        let sliver = Vitals::full(1_000_000).unwrap().damaged_by(999_999);
        let shrunk = sliver.with_max(10).unwrap();
        assert!(!shrunk.is_depleted());
    }
}
