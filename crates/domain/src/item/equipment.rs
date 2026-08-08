use super::catalog::{Catalog, ItemId, Slot};
use crate::shared::Level;

/// Ce qu'un personnage porte sur lui.
///
/// Un emplacement par nature d'objet. Les bonus ne sont pas stockes ici : ils se
/// relisent dans le catalogue a chaque calcul, pour qu'un reequilibrage
/// s'applique aux objets deja portes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Equipment {
    weapon: Option<ItemId>,
    armor: Option<ItemId>,
}

impl Equipment {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            weapon: None,
            armor: None,
        }
    }

    #[must_use]
    pub const fn at(self, slot: Slot) -> Option<ItemId> {
        match slot {
            Slot::Weapon => self.weapon,
            Slot::Armor => self.armor,
        }
    }

    /// Equipe un objet et rend celui qu'il remplace.
    ///
    /// `None` si l'objet est inconnu du catalogue, ne s'equipe pas, ou demande
    /// un palier superieur. Renvoyer l'ancien objet plutot que de le detruire
    /// est ce qui permet a l'appelant de le remettre au sac — sinon un
    /// changement d'arme fait disparaitre la precedente.
    #[must_use]
    pub fn equip(
        self,
        item: ItemId,
        catalog: &Catalog,
        level: Level,
    ) -> Option<(Self, Option<ItemId>)> {
        let definition = catalog.definition(item)?;
        let slot = definition.slot?;
        if level.get() < definition.required_level {
            return None;
        }

        let previous = self.at(slot);
        Some((self.set(slot, Some(item)), previous))
    }

    /// Retire l'objet d'un emplacement et le rend.
    #[must_use]
    pub fn unequip(self, slot: Slot) -> (Self, Option<ItemId>) {
        (self.set(slot, None), self.at(slot))
    }

    /// Impose le contenu d'un emplacement, sans verifier le catalogue.
    ///
    /// Reservee a la restitution depuis la persistance : le joueur portait deja
    /// cet objet, le lui retirer parce qu'une definition a bouge serait plus
    /// brutal que de le laisser porter un objet devenu sans effet — et
    /// `attack_bonus` traite deja ce cas en ne comptant rien.
    #[must_use]
    pub const fn forced(self, slot: Slot, item: Option<ItemId>) -> Self {
        self.set(slot, item)
    }

    const fn set(self, slot: Slot, item: Option<ItemId>) -> Self {
        match slot {
            Slot::Weapon => Self {
                weapon: item,
                ..self
            },
            Slot::Armor => Self {
                armor: item,
                ..self
            },
        }
    }

    /// Bonus d'attaque cumule de l'equipement porte.
    ///
    /// Un objet devenu inconnu du catalogue ne rapporte rien, plutot que de
    /// faire echouer le calcul : le personnage doit rester jouable meme si une
    /// definition a disparu entre deux versions.
    #[must_use]
    pub fn attack_bonus(self, catalog: &Catalog) -> u32 {
        self.total(catalog, |definition| definition.attack_bonus)
    }

    /// Bonus de defense cumule de l'equipement porte.
    #[must_use]
    pub fn defense_bonus(self, catalog: &Catalog) -> u32 {
        self.total(catalog, |definition| definition.defense_bonus)
    }

    fn total(self, catalog: &Catalog, pick: impl Fn(super::ItemDefinition) -> u32) -> u32 {
        Slot::ALL
            .into_iter()
            .filter_map(|slot| self.at(slot))
            .filter_map(|item| catalog.definition(item))
            .map(pick)
            .fold(0_u32, u32::saturating_add)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::ItemDefinition;
    use super::*;

    const SWORD: ItemId = ItemId::new(1);
    const ARMOR: ItemId = ItemId::new(2);
    const POTION: ItemId = ItemId::new(3);
    const GREAT_SWORD: ItemId = ItemId::new(4);

    fn catalog() -> Catalog {
        Catalog::new()
            .with(
                SWORD,
                ItemDefinition {
                    slot: Some(Slot::Weapon),
                    attack_bonus: 40,
                    ..ItemDefinition::default()
                },
            )
            .with(
                ARMOR,
                ItemDefinition {
                    slot: Some(Slot::Armor),
                    defense_bonus: 25,
                    ..ItemDefinition::default()
                },
            )
            .with(POTION, ItemDefinition::default())
            .with(
                GREAT_SWORD,
                ItemDefinition {
                    slot: Some(Slot::Weapon),
                    attack_bonus: 120,
                    required_level: 20,
                    ..ItemDefinition::default()
                },
            )
    }

    fn level(value: u8) -> Level {
        Level::new(value).unwrap()
    }

    #[test]
    fn un_equipement_neuf_n_apporte_aucun_bonus() {
        let empty = Equipment::empty();
        assert_eq!(empty.attack_bonus(&catalog()), 0);
        assert_eq!(empty.defense_bonus(&catalog()), 0);
    }

    #[test]
    fn une_arme_equipee_apporte_son_bonus() {
        let (equipment, previous) = Equipment::empty()
            .equip(SWORD, &catalog(), level(1))
            .unwrap();

        assert_eq!(previous, None);
        assert_eq!(equipment.at(Slot::Weapon), Some(SWORD));
        assert_eq!(equipment.attack_bonus(&catalog()), 40);
    }

    #[test]
    fn les_emplacements_se_cumulent_sans_se_gener() {
        let (equipment, _) = Equipment::empty()
            .equip(SWORD, &catalog(), level(1))
            .unwrap();
        let (equipment, _) = equipment.equip(ARMOR, &catalog(), level(1)).unwrap();

        assert_eq!(equipment.attack_bonus(&catalog()), 40);
        assert_eq!(equipment.defense_bonus(&catalog()), 25);
    }

    #[test]
    fn remplacer_une_arme_rend_la_precedente() {
        // Sans cela, changer d'arme detruit silencieusement l'ancienne.
        let (equipment, _) = Equipment::empty()
            .equip(SWORD, &catalog(), level(1))
            .unwrap();
        let (equipment, previous) = equipment.equip(GREAT_SWORD, &catalog(), level(20)).unwrap();

        assert_eq!(previous, Some(SWORD));
        assert_eq!(equipment.at(Slot::Weapon), Some(GREAT_SWORD));
    }

    #[test]
    fn un_objet_non_equipable_est_refuse() {
        assert_eq!(Equipment::empty().equip(POTION, &catalog(), level(1)), None);
    }

    #[test]
    fn un_objet_inconnu_du_catalogue_est_refuse() {
        assert_eq!(
            Equipment::empty().equip(ItemId::new(999), &catalog(), level(1)),
            None
        );
    }

    #[test]
    fn un_objet_hors_de_portee_du_palier_est_refuse() {
        assert_eq!(
            Equipment::empty().equip(GREAT_SWORD, &catalog(), level(19)),
            None
        );
        assert!(
            Equipment::empty()
                .equip(GREAT_SWORD, &catalog(), level(20))
                .is_some(),
            "le palier exact doit suffire"
        );
    }

    #[test]
    fn retirer_rend_l_objet_et_supprime_son_bonus() {
        let (equipment, _) = Equipment::empty()
            .equip(SWORD, &catalog(), level(1))
            .unwrap();
        let (equipment, removed) = equipment.unequip(Slot::Weapon);

        assert_eq!(removed, Some(SWORD));
        assert_eq!(equipment.at(Slot::Weapon), None);
        assert_eq!(equipment.attack_bonus(&catalog()), 0);
    }

    #[test]
    fn retirer_un_emplacement_vide_est_sans_effet() {
        let (equipment, removed) = Equipment::empty().unequip(Slot::Armor);
        assert_eq!(removed, None);
        assert_eq!(equipment, Equipment::empty());
    }

    #[test]
    fn un_objet_disparu_du_catalogue_ne_rapporte_rien_sans_casser() {
        // Une definition retiree entre deux versions ne doit pas rendre le
        // personnage injouable.
        let (equipment, _) = Equipment::empty()
            .equip(SWORD, &catalog(), level(1))
            .unwrap();

        assert_eq!(equipment.attack_bonus(&Catalog::new()), 0);
    }

    #[test]
    fn un_reequilibrage_s_applique_aux_objets_deja_portes() {
        // Les bonus sont relus, pas recopies au moment de l'equipement.
        let (equipment, _) = Equipment::empty()
            .equip(SWORD, &catalog(), level(1))
            .unwrap();

        let rebalanced = catalog().with(
            SWORD,
            ItemDefinition {
                slot: Some(Slot::Weapon),
                attack_bonus: 5,
                ..ItemDefinition::default()
            },
        );
        assert_eq!(equipment.attack_bonus(&rebalanced), 5);
    }

    #[test]
    fn les_bonus_saturent_au_lieu_de_deborder() {
        let extreme = Catalog::new()
            .with(
                SWORD,
                ItemDefinition {
                    slot: Some(Slot::Weapon),
                    attack_bonus: u32::MAX,
                    ..ItemDefinition::default()
                },
            )
            .with(
                ARMOR,
                ItemDefinition {
                    slot: Some(Slot::Armor),
                    attack_bonus: u32::MAX,
                    ..ItemDefinition::default()
                },
            );

        let (equipment, _) = Equipment::empty().equip(SWORD, &extreme, level(1)).unwrap();
        let (equipment, _) = equipment.equip(ARMOR, &extreme, level(1)).unwrap();

        assert_eq!(equipment.attack_bonus(&extreme), u32::MAX);
    }
}
