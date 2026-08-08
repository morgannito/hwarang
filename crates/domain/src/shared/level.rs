/// Palier maximum atteignable. Constante de politique de jeu, pas une limite
/// technique : l'elargir n'impose aucune migration de format.
pub const MAX_LEVEL: u8 = 120;

/// Niveau d'un personnage, garanti dans `1..=MAX_LEVEL` par construction.
///
/// L'invariant est porte par le type : aucun appelant ne peut fabriquer un
/// niveau 0 ou 255, donc aucune regle en aval n'a besoin de s'en premunir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Level(u8);

impl Level {
    /// Niveau de depart de tout personnage.
    pub const FIRST: Self = Self(1);
    /// Palier terminal.
    pub const LAST: Self = Self(MAX_LEVEL);

    /// Construit un niveau, ou `None` s'il sort de `1..=MAX_LEVEL`.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value >= 1 && value <= MAX_LEVEL {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Palier suivant, ou `None` au niveau maximum.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        Self::new(self.0 + 1)
    }

    #[must_use]
    pub const fn is_max(self) -> bool {
        self.0 == MAX_LEVEL
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rejette_les_niveaux_hors_bornes() {
        assert_eq!(Level::new(0), None);
        assert_eq!(Level::new(MAX_LEVEL + 1), None);
    }

    #[test]
    fn accepte_les_bornes_inclusives() {
        assert_eq!(Level::new(1).unwrap(), Level::FIRST);
        assert_eq!(Level::new(MAX_LEVEL).unwrap(), Level::LAST);
    }

    #[test]
    fn le_palier_terminal_n_a_pas_de_suivant() {
        assert_eq!(Level::LAST.next(), None);
        assert!(Level::LAST.is_max());
    }

    #[test]
    fn progresse_d_un_palier() {
        assert_eq!(Level::FIRST.next().unwrap().get(), 2);
    }
}
