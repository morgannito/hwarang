//! Resolution d'une attaque.
//!
//! Le modele corrige les deux defauts structurels du calcul historique de
//! Metin2 : la mitigation y est additive, donc l'empilement d'armure et de
//! resistances finit par annuler tout degat (invincibilite de fait), et les
//! serveurs prives compensent au cas par cas avec des rustines. Ici les deux
//! reductions sont multiplicatives et bornees par construction : aucun jeu de
//! statistiques, aussi extreme soit-il, ne peut ramener les degats a zero.

use crate::shared::Level;

/// Degats minimaux garantis. Une attaque qui touche fait toujours quelque chose,
/// sinon un defenseur sur-equipe devient une cible impossible a tuer.
pub const MIN_DAMAGE: u32 = 1;

/// Facteur de calibrage de l'armure, par niveau du defenseur.
///
/// A armure egale a `ARMOR_SCALING * niveau`, l'attaque est reduite de moitie.
/// Indexer sur le niveau evite qu'une armure de bas niveau reste efficace en
/// fin de progression.
pub const ARMOR_SCALING: u32 = 40;

/// Puissance offensive consolidee (arme, attributs, bonus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackProfile {
    power: u32,
}

impl AttackProfile {
    #[must_use]
    pub const fn new(power: u32) -> Self {
        Self { power }
    }

    #[must_use]
    pub const fn power(self) -> u32 {
        self.power
    }
}

/// Armure consolidee du defenseur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DefenseProfile {
    armor: u32,
}

impl DefenseProfile {
    #[must_use]
    pub const fn new(armor: u32) -> Self {
        Self { armor }
    }

    #[must_use]
    pub const fn armor(self) -> u32 {
        self.armor
    }
}

/// Resistance elementaire en pour mille, plafonnee a `MAX_PERMILLE`.
///
/// Le plafond est dans le type et non dans la formule : il est impossible
/// d'introduire ailleurs un chemin de code qui produirait une immunite totale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resistance(u16);

impl Resistance {
    /// 90 % : au-dela, l'equilibrage `PvP` s'effondre.
    pub const MAX_PERMILLE: u16 = 900;
    pub const NONE: Self = Self(0);

    /// Ecrete silencieusement plutot que d'echouer : les sources de resistance
    /// s'additionnent en amont et depasser le plafond est un cas nominal.
    #[must_use]
    pub const fn clamped(permille: u16) -> Self {
        Self(if permille > Self::MAX_PERMILLE {
            Self::MAX_PERMILLE
        } else {
            permille
        })
    }

    #[must_use]
    pub const fn permille(self) -> u16 {
        self.0
    }
}

/// Calcule les degats subis.
///
/// `degats = puissance * K / (armure + K) * (1000 - resistance) / 1000`,
/// avec `K = ARMOR_SCALING * niveau_defenseur`, plancher a [`MIN_DAMAGE`].
///
/// Fonction pure et deterministe : la variance eventuelle (critiques, esquive)
/// releve d'une couche superieure, ce qui garde ce calcul rejouable a l'identique
/// pour l'audit d'un combat.
#[must_use]
pub fn resolve_attack(
    attack: AttackProfile,
    defense: DefenseProfile,
    resistance: Resistance,
    defender_level: Level,
) -> u32 {
    let scaling = u64::from(ARMOR_SCALING) * u64::from(defender_level.get());
    let after_armor = u64::from(attack.power()) * scaling / (u64::from(defense.armor()) + scaling);
    let after_resistance = after_armor * u64::from(1000 - resistance.permille()) / 1000;

    u32::try_from(after_resistance)
        .unwrap_or(u32::MAX)
        .max(MIN_DAMAGE)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn level(value: u8) -> Level {
        Level::new(value).unwrap()
    }

    #[test]
    fn sans_armure_ni_resistance_les_degats_valent_la_puissance() {
        let damage = resolve_attack(
            AttackProfile::new(500),
            DefenseProfile::default(),
            Resistance::NONE,
            level(10),
        );
        assert_eq!(damage, 500);
    }

    #[test]
    fn une_armure_egale_au_facteur_de_calibrage_reduit_de_moitie() {
        let defender = level(10);
        let armor = ARMOR_SCALING * u32::from(defender.get());
        let damage = resolve_attack(
            AttackProfile::new(1000),
            DefenseProfile::new(armor),
            Resistance::NONE,
            defender,
        );
        assert_eq!(damage, 500);
    }

    #[test]
    fn aucune_armure_ne_rend_invincible() {
        let damage = resolve_attack(
            AttackProfile::new(100),
            DefenseProfile::new(u32::MAX),
            Resistance::clamped(Resistance::MAX_PERMILLE),
            level(1),
        );
        assert!(damage >= MIN_DAMAGE);
    }

    #[test]
    fn la_resistance_est_plafonnee_a_90_pour_cent() {
        assert_eq!(
            Resistance::clamped(u16::MAX).permille(),
            Resistance::MAX_PERMILLE
        );
    }

    #[test]
    fn la_mitigation_est_monotone_decroissante() {
        let defender = level(50);
        let attack = AttackProfile::new(10_000);
        let mut previous = u32::MAX;
        for armor in (0..20_000).step_by(500) {
            let damage = resolve_attack(
                attack,
                DefenseProfile::new(armor),
                Resistance::NONE,
                defender,
            );
            assert!(damage <= previous, "l'armure {armor} a augmente les degats");
            previous = damage;
        }
    }

    #[test]
    fn l_armure_a_un_rendement_decroissant() {
        let defender = level(50);
        let attack = AttackProfile::new(10_000);
        let hit = |armor| {
            resolve_attack(
                attack,
                DefenseProfile::new(armor),
                Resistance::NONE,
                defender,
            )
        };

        let first_gain = hit(0) - hit(2_000);
        let second_gain = hit(2_000) - hit(4_000);
        assert!(
            second_gain < first_gain,
            "les 2000 points suivants doivent rapporter moins ({second_gain} vs {first_gain})"
        );
    }

    #[test]
    fn une_armure_de_bas_niveau_perd_son_efficacite_en_fin_de_progression() {
        let armor = DefenseProfile::new(400);
        let attack = AttackProfile::new(10_000);

        let low = resolve_attack(attack, armor, Resistance::NONE, level(10));
        let high = resolve_attack(attack, armor, Resistance::NONE, level(100));
        assert!(high > low);
    }

    #[test]
    fn ne_deborde_pas_sur_les_valeurs_extremes() {
        let damage = resolve_attack(
            AttackProfile::new(u32::MAX),
            DefenseProfile::new(0),
            Resistance::NONE,
            Level::LAST,
        );
        assert_eq!(damage, u32::MAX);
    }
}
