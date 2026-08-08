//! Coeur metier de Hwarang.
//!
//! Regle d'architecture : cette crate ne connait ni le reseau, ni la base de
//! donnees, ni le client. Elle expose des types immuables et des fonctions
//! pures, ce qui rend chaque regle de jeu testable sans lancer de serveur.

pub mod character;
pub mod combat;
pub mod shared;

pub use character::{Attributes, Character, CharacterId, ProgressionOutcome};
pub use combat::{AttackProfile, DefenseProfile, Resistance, resolve_attack};
pub use shared::{Experience, Level, ProgressionCurve, Vitals};
