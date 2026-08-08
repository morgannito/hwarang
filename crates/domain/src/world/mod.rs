//! Contexte monde : ou sont les entites, comment elles se deplacent, et qui
//! percoit qui.
//!
//! Le temps n'est pas lu ici : les durees arrivent en parametre. Une regle qui
//! consulterait l'horloge ne serait ni rejouable, ni testable sans attendre.

mod grid;
mod movement;
mod position;

pub use grid::{CellCoord, Grid};
pub use movement::{MoveVerdict, MovementRule, MovementSpeed};
pub use position::Position;
