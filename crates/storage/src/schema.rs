//! Schema de la base et sa mise en place.

use rusqlite::Connection;

/// Applique le schema et les reglages de connexion.
///
/// Idempotent : chaque ouverture peut l'appeler sans condition.
pub(crate) fn apply(connection: &Connection) -> rusqlite::Result<()> {
    // `foreign_keys` est desactive par defaut dans SQLite, et doit etre remis a
    // chaque connexion : sans lui, `REFERENCES` n'est qu'un commentaire et un
    // personnage peut survivre a la suppression de son compte.
    connection.pragma_update(None, "foreign_keys", "ON")?;
    // Journalisation en ecriture anticipee : les lectures ne bloquent plus les
    // ecritures, ce qui compte des que les sauvegardes deviennent periodiques.
    connection.pragma_update(None, "journal_mode", "WAL")?;

    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts (
             id            INTEGER PRIMARY KEY,
             -- NOCASE : « Morgann » et « morgann » designent le meme compte,
             -- sinon l'usurpation par difference de casse est triviale.
             username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
             password_hash TEXT NOT NULL,
             created_at    INTEGER NOT NULL DEFAULT (unixepoch())
         );

         CREATE TABLE IF NOT EXISTS characters (
             -- Cle primaire et non simple reference : un compte porte au plus
             -- un personnage, et la contrainte le garantit plutot que le code.
             account_id     INTEGER PRIMARY KEY
                            REFERENCES accounts(id) ON DELETE CASCADE,
             level          INTEGER NOT NULL,
             experience     INTEGER NOT NULL,
             strength       INTEGER NOT NULL,
             dexterity      INTEGER NOT NULL,
             vitality       INTEGER NOT NULL,
             intellect      INTEGER NOT NULL,
             current_health INTEGER NOT NULL,
             x              INTEGER NOT NULL,
             y              INTEGER NOT NULL,
             updated_at     INTEGER NOT NULL DEFAULT (unixepoch())
         );",
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn le_schema_peut_etre_applique_deux_fois() {
        let connection = Connection::open_in_memory().unwrap();
        apply(&connection).unwrap();
        apply(&connection).unwrap();
    }

    #[test]
    fn supprimer_un_compte_emporte_son_personnage() {
        let connection = Connection::open_in_memory().unwrap();
        apply(&connection).unwrap();

        connection
            .execute(
                "INSERT INTO accounts (username, password_hash) VALUES ('morgann', 'x')",
                [],
            )
            .unwrap();
        let account = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO characters
                 (account_id, level, experience, strength, dexterity, vitality,
                  intellect, current_health, x, y)
                 VALUES (?1, 1, 0, 1, 1, 1, 1, 100, 0, 0)",
                [account],
            )
            .unwrap();

        connection
            .execute("DELETE FROM accounts WHERE id = ?1", [account])
            .unwrap();

        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM characters", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "le personnage a survecu a son compte");
    }

    #[test]
    fn un_personnage_ne_peut_pas_referencer_un_compte_absent() {
        let connection = Connection::open_in_memory().unwrap();
        apply(&connection).unwrap();

        let orphan = connection.execute(
            "INSERT INTO characters
             (account_id, level, experience, strength, dexterity, vitality,
              intellect, current_health, x, y)
             VALUES (999, 1, 0, 1, 1, 1, 1, 100, 0, 0)",
            [],
        );
        assert!(orphan.is_err(), "une ligne orpheline a ete acceptee");
    }
}
