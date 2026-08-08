use super::position::Position;

/// Coordonnee de cellule dans la grille d'interet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellCoord {
    pub cx: i32,
    pub cy: i32,
}

/// Decoupage du plan en cellules, pour repondre a « qui voit qui ».
///
/// Sans grille, diffuser un deplacement impose de comparer l'emetteur a tous
/// les autres joueurs : le cout est quadratique et s'effondre bien avant
/// d'atteindre une population interessante. Avec la grille, seules les neuf
/// cellules voisines sont consultees, quel que soit le nombre de connectes.
///
/// La cellule doit etre au moins aussi large que le rayon de vue, sinon une
/// entite visible pourrait se trouver en dehors du bloc de neuf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    cell_size_cm: u32,
}

impl Grid {
    /// Rayon de vue de reference : 40 m.
    pub const DEFAULT_VIEW_RADIUS_CM: u32 = 4_000;

    /// `None` si la cellule est plus petite que le rayon de vue : la grille ne
    /// pourrait plus garantir que tout ce qui est visible est atteignable dans
    /// le bloc de neuf cellules.
    #[must_use]
    pub const fn new(cell_size_cm: u32, view_radius_cm: u32) -> Option<Self> {
        if cell_size_cm == 0 || cell_size_cm < view_radius_cm {
            None
        } else {
            Some(Self { cell_size_cm })
        }
    }

    #[must_use]
    pub const fn with_default_view() -> Self {
        Self {
            cell_size_cm: Self::DEFAULT_VIEW_RADIUS_CM,
        }
    }

    #[must_use]
    pub const fn cell_size_cm(self) -> u32 {
        self.cell_size_cm
    }

    /// Cellule contenant `position`.
    ///
    /// `div_euclid` et non la division entiere : celle-ci arrondit vers zero,
    /// ce qui donnerait une cellule deux fois plus large a cheval sur l'origine.
    ///
    /// Le calcul passe par `i64` pour accepter une taille de cellule au-dela de
    /// `i32::MAX` sans conversion hasardeuse. Le quotient, lui, ne depasse
    /// jamais la coordonnee divisee : il rentre toujours dans un `i32`.
    #[must_use]
    pub fn cell_of(self, position: Position) -> CellCoord {
        let size = i64::from(self.cell_size_cm);
        let divide =
            |value: i32| i32::try_from(i64::from(value).div_euclid(size)).unwrap_or(i32::MIN);
        CellCoord {
            cx: divide(position.x),
            cy: divide(position.y),
        }
    }

    /// Le bloc de neuf cellules centre sur `cell`, la cellule elle-meme incluse.
    ///
    /// Les cellules qui deborderaient du plan sont omises : aucun joueur ne peut
    /// s'y trouver, puisque leur existence supposerait une position hors `i32`.
    #[must_use]
    pub fn neighbourhood(self, cell: CellCoord) -> Vec<CellCoord> {
        let mut cells = Vec::with_capacity(9);
        for dy in -1..=1 {
            for dx in -1..=1 {
                if let (Some(cx), Some(cy)) = (cell.cx.checked_add(dx), cell.cy.checked_add(dy)) {
                    cells.push(CellCoord { cx, cy });
                }
            }
        }
        cells
    }

    /// Vrai si un changement de cellule impose de recalculer le voisinage.
    #[must_use]
    pub fn crosses_cell(self, from: Position, to: Position) -> bool {
        let before = self.cell_of(from);
        let after = self.cell_of(to);
        before.cx != after.cx || before.cy != after.cy
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn une_cellule_plus_petite_que_la_vue_est_refusee() {
        assert_eq!(Grid::new(1_000, 4_000), None);
        assert_eq!(Grid::new(0, 0), None);
        assert!(Grid::new(4_000, 4_000).is_some());
    }

    #[test]
    fn l_origine_ne_cree_pas_de_cellule_double() {
        let grid = Grid::with_default_view();
        let size = i32::try_from(grid.cell_size_cm()).unwrap();

        // Sans div_euclid, -1 et 0 tomberaient dans la meme cellule.
        assert_eq!(
            grid.cell_of(Position::new(0, 0)),
            CellCoord { cx: 0, cy: 0 }
        );
        assert_eq!(
            grid.cell_of(Position::new(-1, -1)),
            CellCoord { cx: -1, cy: -1 }
        );
        assert_eq!(
            grid.cell_of(Position::new(-size, -size)),
            CellCoord { cx: -1, cy: -1 }
        );
        assert_eq!(
            grid.cell_of(Position::new(-size - 1, 0)),
            CellCoord { cx: -2, cy: 0 }
        );
    }

    #[test]
    fn le_voisinage_courant_compte_neuf_cellules_dont_la_sienne() {
        let grid = Grid::with_default_view();
        let cell = CellCoord { cx: 5, cy: -3 };
        let neighbours = grid.neighbourhood(cell);

        assert_eq!(neighbours.len(), 9);
        assert!(neighbours.contains(&cell));
    }

    #[test]
    fn le_voisinage_est_tronque_aux_bords_du_plan() {
        let grid = Grid::with_default_view();
        let corner = CellCoord {
            cx: i32::MAX,
            cy: i32::MAX,
        };
        assert_eq!(grid.neighbourhood(corner).len(), 4);
    }

    #[test]
    fn tout_ce_qui_est_visible_tient_dans_le_bloc_de_neuf() {
        // Pas de balayage volontairement non aligne sur la taille de cellule,
        // pour ne pas tester uniquement les bords.
        const STEP: usize = 137;

        let grid = Grid::with_default_view();
        let radius = Grid::DEFAULT_VIEW_RADIUS_CM;
        let observer = Position::new(1_234, 5_678);
        let block = grid.neighbourhood(grid.cell_of(observer));

        // Balayage du disque de vue : aucune position visible ne doit tomber
        // hors du bloc, sinon la diffusion oublierait une entite.
        let reach = i32::try_from(radius).unwrap();
        for dx in (-reach..=reach).step_by(STEP) {
            for dy in (-reach..=reach).step_by(STEP) {
                let other = Position::new(observer.x + dx, observer.y + dy);
                if observer.is_within(other, radius) {
                    assert!(
                        block.contains(&grid.cell_of(other)),
                        "{other:?} est visible mais hors du bloc"
                    );
                }
            }
        }
    }

    #[test]
    fn le_franchissement_de_cellule_est_detecte() {
        let grid = Grid::with_default_view();
        let size = i32::try_from(grid.cell_size_cm()).unwrap();

        assert!(!grid.crosses_cell(Position::new(10, 10), Position::new(20, 20)));
        assert!(grid.crosses_cell(Position::new(size - 1, 0), Position::new(size, 0)));
    }
}
