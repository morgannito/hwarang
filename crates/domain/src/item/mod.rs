//! Objets, inventaire et equipement.
//!
//! Les **definitions** d'objets sont des donnees, pas du code : le catalogue est
//! fourni de l'exterieur. Ajouter une arme ne doit pas demander de recompiler le
//! serveur, exactement comme la courbe de progression.
//!
//! Un inventaire et un equipement ne contiennent que des identifiants. Les
//! caracteristiques se lisent dans le catalogue au moment ou l'on en a besoin :
//! recopier les bonus dans l'inventaire les figerait au jour du ramassage, et un
//! reequilibrage ne toucherait jamais les objets deja distribues.

mod catalog;
mod equipment;
mod inventory;

pub use catalog::{Catalog, ItemDefinition, ItemId, Slot};
pub use equipment::Equipment;
pub use inventory::{Inventory, InventoryError};
