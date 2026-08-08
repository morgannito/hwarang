//! Coeur metier de Hwarang.
//!
//! Regle d'architecture : cette crate ne connait ni le reseau, ni la base de
//! donnees, ni le client. Elle expose des types immuables et des fonctions
//! pures, ce qui rend chaque regle de jeu testable sans lancer de serveur.

pub mod ai;
pub mod character;
pub mod combat;
pub mod shared;
pub mod world;

pub use ai::{AggroRule, Intent, Situation, Stance, Threat};
pub use character::{Attributes, Character, CharacterId, ProgressionOutcome};
pub use combat::{
    AttackProfile, AttackRejection, CombatRule, DefenseProfile, Engagement, Resistance,
    experience_reward, resolve_attack,
};
pub use shared::{Experience, Level, ProgressionCurve, Vitals};
pub use world::{CellCoord, Grid, MoveVerdict, MovementRule, MovementSpeed, Position};
