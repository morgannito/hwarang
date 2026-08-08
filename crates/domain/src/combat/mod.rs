//! Contexte combat : qui peut frapper qui, quand, et pour combien.
//!
//! Deux responsabilites distinctes, volontairement separees :
//! - [`engagement`] decide si une attaque est **recevable** (portee, cadence,
//!   etat du defenseur). C'est le pendant offensif de l'autorite serveur deja
//!   appliquee au deplacement.
//! - [`damage`] calcule **combien** elle retire, une fois recevable.
//!
//! Melanger les deux produirait une fonction qui renvoie zero degat aussi bien
//! pour « hors de portee » que pour « armure enorme » — deux situations qui
//! n'appellent ni la meme reponse reseau, ni le meme retour au joueur.

mod damage;
mod engagement;

pub use damage::{
    ARMOR_SCALING, AttackProfile, DefenseProfile, MIN_DAMAGE, Resistance, resolve_attack,
};
pub use engagement::{AttackRejection, CombatRule, Engagement, experience_reward};
