//! Persistance des comptes et des personnages.
//!
//! Cette crate connait `SQLite` et `Argon2` ; elle ne connait aucune regle de jeu.
//! Elle traduit un `Character` du domaine en lignes et inversement, sans jamais
//! decider de ce qui est jouable — cette responsabilite reste dans
//! `hwarang-domain`, qui revalide tout ce qui remonte d'ici.

mod account;
mod schema;

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use hwarang_domain::{
    Attributes, Character, CharacterId, Experience, Level, Position, ProgressionCurve,
};
use rusqlite::{Connection, OptionalExtension, params};

pub use account::{AccountId, CredentialError, validate_credentials};

/// Ce qui peut mal se passer en parlant au stockage.
#[derive(Debug)]
pub enum StorageError {
    /// Nom deja pris.
    UsernameTaken,
    /// Identifiants incorrects, ou compte inexistant.
    ///
    /// Les deux cas sont volontairement confondus : les distinguer permettrait
    /// d'enumerer les comptes existants.
    InvalidCredentials,
    /// Nom ou mot de passe hors des bornes acceptees.
    Credentials(CredentialError),
    /// La base a refuse l'operation.
    Database(rusqlite::Error),
    /// Le hachage a echoue.
    Hashing(argon2::password_hash::Error),
    /// Une ligne existe mais decrit un personnage impossible.
    ///
    /// Distinct d'une erreur de base : la lecture a reussi, c'est le contenu qui
    /// ne passe pas les invariants du domaine.
    CorruptCharacter,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UsernameTaken => write!(f, "nom deja pris"),
            Self::InvalidCredentials => write!(f, "identifiants incorrects"),
            Self::Credentials(error) => write!(f, "identifiants invalides : {error}"),
            Self::Database(error) => write!(f, "base de donnees : {error}"),
            Self::Hashing(error) => write!(f, "hachage : {error}"),
            Self::CorruptCharacter => write!(f, "personnage sauvegarde incoherent"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<CredentialError> for StorageError {
    fn from(error: CredentialError) -> Self {
        Self::Credentials(error)
    }
}

type Result<T> = std::result::Result<T, StorageError>;

/// Etat d'un personnage tel qu'il traverse la persistance.
///
/// Type distinct de `Character` : la sauvegarde porte la position, que le
/// domaine ne connait pas (elle appartient au contexte monde), et elle doit
/// pouvoir representer un etat qui ne passera l'invariant qu'a la relecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedCharacter {
    pub level: u8,
    pub experience: u64,
    pub attributes: Attributes,
    pub current_health: u32,
    pub position: Position,
}

impl SavedCharacter {
    /// Capture l'etat d'un personnage en jeu.
    #[must_use]
    pub fn of(character: &Character, position: Position) -> Self {
        Self {
            level: character.level().get(),
            experience: character.experience().get(),
            attributes: character.attributes(),
            current_health: character.vitals().current(),
            position,
        }
    }

    /// Reconstitue un personnage jouable.
    ///
    /// # Errors
    /// [`StorageError::CorruptCharacter`] si l'etat sauvegarde ne satisfait pas
    /// les invariants du domaine — palier hors bornes, attributs impossibles.
    pub fn into_character(self, id: CharacterId, curve: ProgressionCurve) -> Result<Character> {
        let level = Level::new(self.level).ok_or(StorageError::CorruptCharacter)?;
        Character::restore(
            id,
            level,
            Experience::new(self.experience),
            self.attributes,
            self.current_health,
            curve,
        )
        .ok_or(StorageError::CorruptCharacter)
    }
}

/// Depot des comptes et des personnages.
pub struct Storage {
    // Une seule connexion sous verrou plutot qu'un pool : les acces sont rares
    // (connexion, deconnexion, sauvegarde) et courts. Un pool serait de la
    // complexite sans contrepartie mesurable a cette echelle.
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage").finish_non_exhaustive()
    }
}

impl Storage {
    /// Ouvre ou cree une base sur disque.
    ///
    /// # Errors
    /// Si le fichier ne peut pas etre ouvert ou le schema applique.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    /// Base en memoire, pour les tests.
    ///
    /// # Errors
    /// Si le schema ne peut pas etre applique.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        schema::apply(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Cree un compte et retourne son identifiant.
    ///
    /// # Errors
    /// [`StorageError::UsernameTaken`] si le nom existe deja,
    /// [`StorageError::Credentials`] s'il est hors bornes.
    pub fn register(&self, username: &str, password: &str) -> Result<AccountId> {
        validate_credentials(username, password)?;
        let hash = account::hash_password(password)?;

        let connection = self.lock();
        // `INSERT OR IGNORE` plutot qu'un `SELECT` prealable : verifier puis
        // inserer laisserait une fenetre ou deux inscriptions simultanees
        // passent le controle avant que l'une n'ecrive.
        let affected = connection.execute(
            "INSERT OR IGNORE INTO accounts (username, password_hash) VALUES (?1, ?2)",
            params![username, hash],
        )?;

        if affected == 0 {
            return Err(StorageError::UsernameTaken);
        }
        Ok(AccountId::new(connection.last_insert_rowid()))
    }

    /// Verifie des identifiants.
    ///
    /// # Errors
    /// [`StorageError::InvalidCredentials`] si le compte n'existe pas ou que le
    /// mot de passe ne correspond pas.
    pub fn authenticate(&self, username: &str, password: &str) -> Result<AccountId> {
        let found: Option<(i64, String)> = self
            .lock()
            .query_row(
                "SELECT id, password_hash FROM accounts WHERE username = ?1",
                params![username],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((id, hash)) = found else {
            // Compte inconnu : on verifie quand meme le mot de passe contre un
            // hachage factice. Sans cela, la reponse arrive bien plus vite pour
            // un nom inexistant, et cet ecart de temps suffit a enumerer les
            // comptes du serveur.
            account::verify_against_dummy(password);
            return Err(StorageError::InvalidCredentials);
        };

        if account::verify_password(password, &hash) {
            Ok(AccountId::new(id))
        } else {
            Err(StorageError::InvalidCredentials)
        }
    }

    /// Charge le personnage d'un compte, s'il en a un.
    ///
    /// # Errors
    /// Si la lecture echoue.
    pub fn load_character(&self, account: AccountId) -> Result<Option<SavedCharacter>> {
        self.lock()
            .query_row(
                "SELECT level, experience, strength, dexterity, vitality, intellect,
                        current_health, x, y
                 FROM characters WHERE account_id = ?1",
                params![account.get()],
                |row| {
                    Ok(SavedCharacter {
                        level: row.get(0)?,
                        // SQLite stocke des entiers signes : l'experience fait
                        // l'aller-retour par `i64`, sans perte tant qu'elle
                        // reste sous 2^63 — la courbe plafonne bien en deca.
                        experience: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        attributes: Attributes {
                            strength: row.get(2)?,
                            dexterity: row.get(3)?,
                            vitality: row.get(4)?,
                            intellect: row.get(5)?,
                        },
                        current_health: row.get(6)?,
                        position: Position::new(row.get(7)?, row.get(8)?),
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// Ecrit l'etat d'un personnage, en creant la ligne au besoin.
    ///
    /// # Errors
    /// Si l'ecriture echoue.
    pub fn save_character(&self, account: AccountId, saved: &SavedCharacter) -> Result<()> {
        self.lock().execute(
            "INSERT INTO characters
                (account_id, level, experience, strength, dexterity, vitality,
                 intellect, current_health, x, y, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, unixepoch())
             ON CONFLICT(account_id) DO UPDATE SET
                level = excluded.level,
                experience = excluded.experience,
                strength = excluded.strength,
                dexterity = excluded.dexterity,
                vitality = excluded.vitality,
                intellect = excluded.intellect,
                current_health = excluded.current_health,
                x = excluded.x,
                y = excluded.y,
                updated_at = excluded.updated_at",
            params![
                account.get(),
                saved.level,
                i64::try_from(saved.experience).unwrap_or(i64::MAX),
                saved.attributes.strength,
                saved.attributes.dexterity,
                saved.attributes.vitality,
                saved.attributes.intellect,
                saved.current_health,
                saved.position.x,
                saved.position.y,
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn storage() -> Storage {
        Storage::in_memory().unwrap()
    }

    fn sample() -> SavedCharacter {
        SavedCharacter {
            level: 12,
            experience: 3_456,
            attributes: Attributes {
                strength: 14,
                dexterity: 9,
                vitality: 11,
                intellect: 6,
            },
            current_health: 250,
            position: Position::new(-1_500, 2_400),
        }
    }

    #[test]
    fn un_compte_cree_permet_de_s_authentifier() {
        let storage = storage();
        let created = storage.register("morgann", "mot-de-passe-solide").unwrap();
        let signed_in = storage
            .authenticate("morgann", "mot-de-passe-solide")
            .unwrap();
        assert_eq!(created, signed_in);
    }

    #[test]
    fn le_mot_de_passe_n_est_jamais_stocke_en_clair() {
        let storage = storage();
        storage.register("morgann", "mot-de-passe-solide").unwrap();

        let hash: String = storage
            .lock()
            .query_row("SELECT password_hash FROM accounts", [], |row| row.get(0))
            .unwrap();

        assert!(!hash.contains("mot-de-passe-solide"));
        assert!(hash.starts_with("$argon2"), "hachage inattendu : {hash}");
    }

    #[test]
    fn deux_comptes_ne_peuvent_pas_porter_le_meme_nom() {
        let storage = storage();
        storage.register("morgann", "mot-de-passe-solide").unwrap();
        assert!(matches!(
            storage.register("morgann", "un-autre-mot-de-passe"),
            Err(StorageError::UsernameTaken)
        ));
    }

    #[test]
    fn le_nom_est_insensible_a_la_casse() {
        // Sans cela, « Morgann » et « morgann » sont deux comptes distincts, ce
        // qui ouvre l'usurpation d'identite par difference de casse.
        let storage = storage();
        storage.register("morgann", "mot-de-passe-solide").unwrap();
        assert!(matches!(
            storage.register("MorGann", "un-autre-mot-de-passe"),
            Err(StorageError::UsernameTaken)
        ));
        assert!(
            storage
                .authenticate("MORGANN", "mot-de-passe-solide")
                .is_ok()
        );
    }

    #[test]
    fn un_mauvais_mot_de_passe_est_refuse() {
        let storage = storage();
        storage.register("morgann", "mot-de-passe-solide").unwrap();
        assert!(matches!(
            storage.authenticate("morgann", "mauvais-mot-de-passe"),
            Err(StorageError::InvalidCredentials)
        ));
    }

    #[test]
    fn un_compte_inexistant_donne_la_meme_erreur_qu_un_mauvais_mot_de_passe() {
        // Distinguer les deux permettrait d'enumerer les comptes du serveur.
        let storage = storage();
        storage.register("morgann", "mot-de-passe-solide").unwrap();
        assert!(matches!(
            storage.authenticate("inconnu", "peu-importe-vraiment"),
            Err(StorageError::InvalidCredentials)
        ));
    }

    #[test]
    fn des_identifiants_hors_bornes_sont_refuses_avant_ecriture() {
        let storage = storage();
        assert!(matches!(
            storage.register("ab", "mot-de-passe-solide"),
            Err(StorageError::Credentials(_))
        ));

        let rows: i64 = storage
            .lock()
            .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0, "un compte invalide a ete ecrit");
    }

    #[test]
    fn un_personnage_absent_se_signale_par_none() {
        let storage = storage();
        let account = storage.register("morgann", "mot-de-passe-solide").unwrap();
        assert_eq!(storage.load_character(account).unwrap(), None);
    }

    #[test]
    fn un_personnage_sauvegarde_se_recharge_a_l_identique() {
        let storage = storage();
        let account = storage.register("morgann", "mot-de-passe-solide").unwrap();
        storage.save_character(account, &sample()).unwrap();

        assert_eq!(storage.load_character(account).unwrap(), Some(sample()));
    }

    #[test]
    fn sauvegarder_deux_fois_met_a_jour_sans_dupliquer() {
        let storage = storage();
        let account = storage.register("morgann", "mot-de-passe-solide").unwrap();
        storage.save_character(account, &sample()).unwrap();

        let progressed = SavedCharacter {
            level: 13,
            ..sample()
        };
        storage.save_character(account, &progressed).unwrap();

        assert_eq!(storage.load_character(account).unwrap(), Some(progressed));
        let rows: i64 = storage
            .lock()
            .query_row("SELECT COUNT(*) FROM characters", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn les_personnages_de_deux_comptes_ne_se_melangent_pas() {
        let storage = storage();
        let first = storage.register("morgann", "mot-de-passe-solide").unwrap();
        let second = storage.register("helene", "un-autre-mot-de-passe").unwrap();

        storage.save_character(first, &sample()).unwrap();
        assert_eq!(storage.load_character(second).unwrap(), None);
    }

    #[test]
    fn les_positions_negatives_traversent_la_base() {
        let storage = storage();
        let account = storage.register("morgann", "mot-de-passe-solide").unwrap();
        let far = SavedCharacter {
            position: Position::new(i32::MIN, i32::MAX),
            ..sample()
        };
        storage.save_character(account, &far).unwrap();

        assert_eq!(storage.load_character(account).unwrap(), Some(far));
    }

    #[test]
    fn un_personnage_recharge_redevient_jouable() {
        let restored = sample()
            .into_character(CharacterId::new(1), ProgressionCurve::DEFAULT)
            .unwrap();

        assert_eq!(restored.level().get(), 12);
        assert_eq!(restored.experience(), Experience::new(3_456));
        assert_eq!(restored.vitals().current(), 250);
        assert!(restored.is_alive());
    }

    #[test]
    fn une_ligne_incoherente_est_refusee_plutot_que_chargee() {
        let corrupt = SavedCharacter {
            level: 0, // hors de 1..=120
            ..sample()
        };
        assert!(matches!(
            corrupt.into_character(CharacterId::new(1), ProgressionCurve::DEFAULT),
            Err(StorageError::CorruptCharacter)
        ));
    }

    #[test]
    fn la_capture_puis_le_rechargement_conservent_l_etat() {
        let character = sample()
            .into_character(CharacterId::new(1), ProgressionCurve::DEFAULT)
            .unwrap();
        let position = Position::new(700, -300);

        let captured = SavedCharacter::of(&character, position);
        let round_tripped = captured
            .into_character(CharacterId::new(1), ProgressionCurve::DEFAULT)
            .unwrap();

        assert_eq!(round_tripped, character);
        assert_eq!(captured.position, position);
    }

    #[test]
    fn la_persistance_survit_a_la_fermeture() {
        let path = std::env::temp_dir().join("hwarang-test-persistance.sqlite");
        let _ = std::fs::remove_file(&path);

        let account = {
            let storage = Storage::open(&path).unwrap();
            let account = storage.register("morgann", "mot-de-passe-solide").unwrap();
            storage.save_character(account, &sample()).unwrap();
            account
        };

        let reopened = Storage::open(&path).unwrap();
        assert_eq!(reopened.load_character(account).unwrap(), Some(sample()));
        assert!(
            reopened
                .authenticate("morgann", "mot-de-passe-solide")
                .is_ok()
        );

        let _ = std::fs::remove_file(&path);
    }
}
