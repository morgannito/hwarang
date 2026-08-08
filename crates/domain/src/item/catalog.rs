use std::collections::HashMap;

/// Reference d'un objet dans le catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(u32);

impl ItemId {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Emplacement d'equipement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    Weapon,
    Armor,
}

impl Slot {
    /// Tous les emplacements existants.
    ///
    /// Enumerer ici plutot qu'a chaque appelant : ajouter un emplacement ne doit
    /// pas obliger a retrouver toutes les boucles qui les parcourent.
    pub const ALL: [Self; 2] = [Self::Weapon, Self::Armor];
}

/// Ce qu'un objet apporte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemDefinition {
    /// `None` pour un objet qui ne s'equipe pas.
    pub slot: Option<Slot>,
    pub attack_bonus: u32,
    pub defense_bonus: u32,
    /// Palier minimum pour l'equiper.
    pub required_level: u8,
}

/// Catalogue des objets connus.
///
/// Donnee du domaine, fournie par l'exterieur : un objet inconnu du catalogue
/// est un objet qui n'existe pas, et le domaine le traite comme tel plutot que
/// de supposer des caracteristiques par defaut.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    definitions: HashMap<ItemId, ItemDefinition>,
}

impl Catalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute ou remplace une definition.
    #[must_use]
    pub fn with(mut self, id: ItemId, definition: ItemDefinition) -> Self {
        self.definitions.insert(id, definition);
        self
    }

    #[must_use]
    pub fn definition(&self, id: ItemId) -> Option<ItemDefinition> {
        self.definitions.get(&id).copied()
    }

    #[must_use]
    pub fn contains(&self, id: ItemId) -> bool {
        self.definitions.contains_key(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_objet_absent_du_catalogue_n_a_pas_de_definition() {
        // Ni bonus par defaut, ni valeur nulle silencieuse : l'appelant doit
        // decider quoi faire d'un objet qui n'existe pas.
        assert_eq!(Catalog::new().definition(ItemId::new(1)), None);
    }

    #[test]
    fn une_definition_ajoutee_se_relit() {
        let sword = ItemDefinition {
            slot: Some(Slot::Weapon),
            attack_bonus: 40,
            required_level: 5,
            ..ItemDefinition::default()
        };
        let catalog = Catalog::new().with(ItemId::new(1), sword);

        assert_eq!(catalog.definition(ItemId::new(1)), Some(sword));
        assert!(catalog.contains(ItemId::new(1)));
        assert_eq!(catalog.len(), 1);
    }

    #[test]
    fn une_seconde_definition_remplace_la_premiere() {
        // Un reequilibrage doit pouvoir redefinir un objet sans en creer un
        // second portant le meme identifiant.
        let catalog = Catalog::new()
            .with(ItemId::new(1), ItemDefinition::default())
            .with(
                ItemId::new(1),
                ItemDefinition {
                    attack_bonus: 99,
                    ..ItemDefinition::default()
                },
            );

        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog.definition(ItemId::new(1)).map(|d| d.attack_bonus),
            Some(99)
        );
    }
}
