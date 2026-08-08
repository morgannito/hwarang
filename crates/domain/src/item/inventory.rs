use super::catalog::ItemId;

/// Pourquoi une operation d'inventaire echoue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryError {
    /// Plus de place.
    Full,
    /// Emplacement inexistant ou vide.
    NoSuchSlot,
}

/// Sac d'un personnage, a capacite fixe.
///
/// Les emplacements sont **stables** : retirer un objet laisse un trou plutot
/// que de tasser le reste. Un client qui affiche une grille verrait sinon tout
/// son contenu se decaler a chaque retrait, et un joueur cliquerait sur l'objet
/// voisin de celui qu'il visait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    slots: Vec<Option<ItemId>>,
}

impl Inventory {
    /// Capacite de reference.
    pub const DEFAULT_CAPACITY: usize = 24;

    /// `None` si la capacite est nulle : un sac sans emplacement rendrait tout
    /// butin impossible a recevoir, sans que rien ne le signale.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Option<Self> {
        if capacity == 0 {
            None
        } else {
            Some(Self {
                slots: vec![None; capacity],
            })
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Nombre d'objets portes.
    #[must_use]
    pub fn count(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.count() == self.capacity()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    #[must_use]
    pub fn at(&self, index: usize) -> Option<ItemId> {
        self.slots.get(index).copied().flatten()
    }

    /// Contenu, emplacement par emplacement.
    pub fn slots(&self) -> impl Iterator<Item = (usize, Option<ItemId>)> + '_ {
        self.slots.iter().copied().enumerate()
    }

    /// Range un objet dans le premier emplacement libre.
    ///
    /// Retourne l'inventaire modifie et l'emplacement occupe.
    ///
    /// # Errors
    /// [`InventoryError::Full`] si le sac est plein.
    pub fn add(&self, item: ItemId) -> Result<(Self, usize), InventoryError> {
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(InventoryError::Full)?;

        let mut next = self.clone();
        next.slots[index] = Some(item);
        Ok((next, index))
    }

    /// Place un objet a un emplacement precis, en ecrasant ce qui s'y trouve.
    ///
    /// Reservee a la restitution depuis la persistance : c'est le seul cas ou
    /// l'emplacement est impose de l'exterieur. Le jeu passe par [`Self::add`],
    /// qui respecte la capacite et signale un sac plein.
    ///
    /// Un indice hors du sac est ignore : une sauvegarde ecrite quand le sac
    /// etait plus grand ne doit pas empecher de se connecter.
    #[must_use]
    pub fn placed(&self, index: usize, item: ItemId) -> Self {
        let mut next = self.clone();
        if let Some(slot) = next.slots.get_mut(index) {
            *slot = Some(item);
        }
        next
    }

    /// Retire l'objet d'un emplacement.
    ///
    /// # Errors
    /// [`InventoryError::NoSuchSlot`] si l'emplacement n'existe pas ou est vide.
    pub fn remove(&self, index: usize) -> Result<(Self, ItemId), InventoryError> {
        let item = self
            .slots
            .get(index)
            .copied()
            .flatten()
            .ok_or(InventoryError::NoSuchSlot)?;

        let mut next = self.clone();
        next.slots[index] = None;
        Ok((next, item))
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: vec![None; Self::DEFAULT_CAPACITY],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn item(id: u32) -> ItemId {
        ItemId::new(id)
    }

    #[test]
    fn un_sac_de_capacite_nulle_est_refuse() {
        assert_eq!(Inventory::with_capacity(0), None);
    }

    #[test]
    fn un_sac_neuf_est_vide_mais_pas_plein() {
        let bag = Inventory::with_capacity(3).unwrap();
        assert!(bag.is_empty());
        assert!(!bag.is_full());
        assert_eq!(bag.count(), 0);
        assert_eq!(bag.capacity(), 3);
    }

    #[test]
    fn un_objet_range_se_retrouve_a_son_emplacement() {
        let bag = Inventory::with_capacity(3).unwrap();
        let (bag, index) = bag.add(item(7)).unwrap();

        assert_eq!(index, 0);
        assert_eq!(bag.at(0), Some(item(7)));
        assert_eq!(bag.count(), 1);
    }

    #[test]
    fn les_objets_occupent_les_emplacements_dans_l_ordre() {
        let bag = Inventory::with_capacity(3).unwrap();
        let (bag, _) = bag.add(item(1)).unwrap();
        let (bag, second) = bag.add(item(2)).unwrap();

        assert_eq!(second, 1);
        assert_eq!(bag.at(0), Some(item(1)));
        assert_eq!(bag.at(1), Some(item(2)));
    }

    #[test]
    fn un_sac_plein_refuse_tout_ajout() {
        let mut bag = Inventory::with_capacity(2).unwrap();
        for id in 1..=2 {
            bag = bag.add(item(id)).unwrap().0;
        }

        assert!(bag.is_full());
        assert_eq!(bag.add(item(3)), Err(InventoryError::Full));
    }

    #[test]
    fn retirer_laisse_un_trou_sans_decaler_le_reste() {
        // La stabilite des emplacements : un client qui affiche une grille ne
        // doit pas voir son contenu se reorganiser sous le curseur.
        let mut bag = Inventory::with_capacity(3).unwrap();
        for id in 1..=3 {
            bag = bag.add(item(id)).unwrap().0;
        }

        let (bag, removed) = bag.remove(1).unwrap();

        assert_eq!(removed, item(2));
        assert_eq!(bag.at(0), Some(item(1)));
        assert_eq!(bag.at(1), None);
        assert_eq!(bag.at(2), Some(item(3)), "l'objet suivant a ete decale");
    }

    #[test]
    fn le_trou_laisse_est_reutilise_avant_les_emplacements_suivants() {
        let mut bag = Inventory::with_capacity(3).unwrap();
        for id in 1..=3 {
            bag = bag.add(item(id)).unwrap().0;
        }
        let (bag, _) = bag.remove(0).unwrap();

        let (bag, index) = bag.add(item(9)).unwrap();
        assert_eq!(index, 0);
        assert_eq!(bag.at(0), Some(item(9)));
    }

    #[test]
    fn retirer_d_un_emplacement_vide_ou_inexistant_echoue() {
        let bag = Inventory::with_capacity(2).unwrap();
        assert_eq!(bag.remove(0), Err(InventoryError::NoSuchSlot));
        assert_eq!(bag.remove(99), Err(InventoryError::NoSuchSlot));
    }

    #[test]
    fn une_operation_refusee_ne_modifie_rien() {
        // Les operations retournent un nouvel inventaire : l'original ne peut
        // pas se retrouver a moitie modifie.
        let bag = Inventory::with_capacity(1).unwrap();
        let (full, _) = bag.add(item(1)).unwrap();

        assert!(full.add(item(2)).is_err());
        assert_eq!(full.at(0), Some(item(1)));
        assert_eq!(full.count(), 1);
    }

    #[test]
    fn le_meme_objet_peut_etre_porte_en_plusieurs_exemplaires() {
        let bag = Inventory::with_capacity(3).unwrap();
        let (bag, _) = bag.add(item(5)).unwrap();
        let (bag, _) = bag.add(item(5)).unwrap();

        assert_eq!(bag.count(), 2);
        assert_eq!(bag.at(0), bag.at(1));
    }

    #[test]
    fn le_parcours_expose_les_trous() {
        let bag = Inventory::with_capacity(2).unwrap();
        let (bag, _) = bag.add(item(1)).unwrap();

        let listing: Vec<(usize, Option<ItemId>)> = bag.slots().collect();
        assert_eq!(listing, vec![(0, Some(item(1))), (1, None)]);
    }
}
