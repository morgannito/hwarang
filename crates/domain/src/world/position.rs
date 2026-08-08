/// Position sur le plan de jeu, en centimetres.
///
/// Entiers et non flottants : deux serveurs, ou un serveur et un client, doivent
/// obtenir exactement le meme resultat pour un meme calcul. Un `f32` ne le
/// garantit pas d'une plateforme a l'autre, et une desynchronisation de
/// position est indebogable une fois en production.
///
/// La portee d'un `i32` en centimetres couvre +/- 21 000 km : aucune carte
/// realiste ne s'en approche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    pub const ORIGIN: Self = Self { x: 0, y: 0 };

    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Distance au carre, en cm².
    ///
    /// Le carre evite la racine : comparer des distances ne demande jamais leur
    /// valeur reelle, et la racine entiere introduirait un arrondi.
    ///
    /// Le calcul passe par `i128` : aux extremites du plan, l'ecart des `x`
    /// approche 2^32 et son carre deborde un `i64`. Le resultat sature a
    /// `u64::MAX`, ce qui se lit « hors de toute portee » — la seule
    /// interpretation utile a cette distance.
    #[must_use]
    pub fn distance_squared(self, other: Self) -> u64 {
        let dx = i128::from(self.x) - i128::from(other.x);
        let dy = i128::from(self.y) - i128::from(other.y);
        u64::try_from(dx * dx + dy * dy).unwrap_or(u64::MAX)
    }

    /// Vrai si `other` est dans un rayon de `radius_cm`.
    #[must_use]
    pub fn is_within(self, other: Self, radius_cm: u32) -> bool {
        let radius = u64::from(radius_cm);
        self.distance_squared(other) <= radius.saturating_mul(radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_distance_a_soi_meme_est_nulle() {
        let position = Position::new(1_234, -5_678);
        assert_eq!(position.distance_squared(position), 0);
    }

    #[test]
    fn la_distance_est_symetrique() {
        let a = Position::new(100, 200);
        let b = Position::new(-300, 50);
        assert_eq!(a.distance_squared(b), b.distance_squared(a));
    }

    #[test]
    fn le_triangle_3_4_5_donne_bien_25() {
        let a = Position::ORIGIN;
        let b = Position::new(3, 4);
        assert_eq!(a.distance_squared(b), 25);
    }

    #[test]
    fn le_rayon_est_inclusif_a_sa_borne() {
        let a = Position::ORIGIN;
        assert!(a.is_within(Position::new(3, 4), 5));
        assert!(!a.is_within(Position::new(3, 5), 5));
    }

    #[test]
    fn ne_deborde_pas_aux_extremites_du_plan() {
        let a = Position::new(i32::MIN, i32::MIN);
        let b = Position::new(i32::MAX, i32::MAX);
        // 2 * (2^32 - 1)^2 tient dans un u64, donc pas de rebouclage.
        assert!(a.distance_squared(b) > 0);
        assert!(!a.is_within(b, u32::MAX));
    }
}
