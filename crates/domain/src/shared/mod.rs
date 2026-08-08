//! Objets valeur partages par les contextes metier.

mod experience;
mod level;
mod vitals;

pub use experience::{Experience, ProgressionCurve};
pub use level::{Level, MAX_LEVEL};
pub use vitals::Vitals;
