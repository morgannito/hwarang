//! Identifiants de compte : validation et hachage.

use std::sync::LazyLock;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, password_hash::rand_core::OsRng};

use crate::StorageError;

/// Identifiant interne d'un compte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(i64);

impl AccountId {
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Vue non signee, pour les trames reseau.
    ///
    /// `SQLite` numerote ses lignes en entiers signes, le protocole transporte
    /// des `u64` : la conversion est une reinterpretation des memes bits, sans
    /// perte, et [`AccountId::from_u64`] la refait dans l'autre sens.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0.cast_unsigned()
    }

    /// Inverse de [`AccountId::as_u64`].
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value.cast_signed())
    }
}

/// Bornes acceptees pour un nom de compte.
pub const MIN_USERNAME_LEN: usize = 3;
pub const MAX_USERNAME_LEN: usize = 32;

/// Longueur minimale d'un mot de passe.
///
/// La longueur est le seul critere impose. Exiger des classes de caracteres
/// produit des mots de passe plus previsibles, pas plus solides.
pub const MIN_PASSWORD_LEN: usize = 10;

/// Plafond, pour borner le cout de verification.
///
/// Argon2 est deliberement lent ; sans plafond, la taille du mot de passe
/// deviendrait un levier d'amplification entre les mains d'un attaquant.
pub const MAX_PASSWORD_LEN: usize = 128;

/// Pourquoi des identifiants sont refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    UsernameTooShort,
    UsernameTooLong,
    UsernameHasInvalidCharacters,
    PasswordTooShort,
    PasswordTooLong,
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::UsernameTooShort => "nom trop court",
            Self::UsernameTooLong => "nom trop long",
            Self::UsernameHasInvalidCharacters => "le nom contient des caracteres interdits",
            Self::PasswordTooShort => "mot de passe trop court",
            Self::PasswordTooLong => "mot de passe trop long",
        };
        f.write_str(text)
    }
}

impl std::error::Error for CredentialError {}

/// Verifie que des identifiants sont acceptables avant tout travail couteux.
///
/// Le jeu de caracteres autorise est volontairement etroit : lettres ASCII,
/// chiffres, tiret et souligne. Accepter l'Unicode entier ouvrirait
/// l'usurpation par homographie — un « а » cyrillique se lit comme un « a »
/// latin, et deux comptes visuellement identiques cohabiteraient.
///
/// # Errors
/// Voir [`CredentialError`].
pub fn validate_credentials(username: &str, password: &str) -> Result<(), CredentialError> {
    if username.len() < MIN_USERNAME_LEN {
        return Err(CredentialError::UsernameTooShort);
    }
    if username.len() > MAX_USERNAME_LEN {
        return Err(CredentialError::UsernameTooLong);
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(CredentialError::UsernameHasInvalidCharacters);
    }
    if password.len() < MIN_PASSWORD_LEN {
        return Err(CredentialError::PasswordTooShort);
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(CredentialError::PasswordTooLong);
    }
    Ok(())
}

/// Hachage de reference, utilise quand le compte n'existe pas.
///
/// Calcule une fois : le construire a chaque tentative couterait le prix d'un
/// hachage supplementaire sans rien apporter.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password("mot-de-passe-inexistant").unwrap_or_else(|_| String::from("$argon2id$invalide"))
});

/// Hache un mot de passe avec un sel aleatoire.
///
/// # Errors
/// Si le hachage echoue.
pub(crate) fn hash_password(password: &str) -> Result<String, StorageError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(StorageError::Hashing)
}

/// Verifie un mot de passe contre un hachage stocke.
///
/// Retourne `false` plutot qu'une erreur si le hachage stocke est illisible : du
/// point de vue de l'appelant, un enregistrement corrompu et un mauvais mot de
/// passe autorisent la meme chose — rien.
pub(crate) fn verify_password(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// Consomme le meme temps qu'une verification reelle, pour un compte inexistant.
///
/// Sans cela, la reponse revient bien plus vite quand le nom n'existe pas, et
/// cet ecart mesurable suffit a enumerer les comptes du serveur.
pub(crate) fn verify_against_dummy(password: &str) {
    let _ = verify_password(password, &DUMMY_HASH);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn l_identifiant_de_compte_traverse_le_reseau_sans_perte() {
        for value in [0, 1, i64::MAX, i64::MIN, -1] {
            let account = AccountId::new(value);
            assert_eq!(AccountId::from_u64(account.as_u64()), account);
        }
    }

    #[test]
    fn un_nom_et_un_mot_de_passe_ordinaires_sont_acceptes() {
        assert_eq!(
            validate_credentials("morgann", "mot-de-passe-solide"),
            Ok(())
        );
        assert_eq!(validate_credentials("a_b-9", "0123456789"), Ok(()));
    }

    #[test]
    fn les_bornes_de_longueur_sont_appliquees() {
        assert_eq!(
            validate_credentials("ab", "mot-de-passe-solide"),
            Err(CredentialError::UsernameTooShort)
        );
        assert_eq!(
            validate_credentials(&"a".repeat(MAX_USERNAME_LEN + 1), "mot-de-passe-solide"),
            Err(CredentialError::UsernameTooLong)
        );
        assert_eq!(
            validate_credentials("morgann", "court"),
            Err(CredentialError::PasswordTooShort)
        );
        assert_eq!(
            validate_credentials("morgann", &"x".repeat(MAX_PASSWORD_LEN + 1)),
            Err(CredentialError::PasswordTooLong)
        );
    }

    #[test]
    fn les_bornes_exactes_passent() {
        assert_eq!(
            validate_credentials(&"a".repeat(MIN_USERNAME_LEN), &"x".repeat(MIN_PASSWORD_LEN)),
            Ok(())
        );
        assert_eq!(
            validate_credentials(&"a".repeat(MAX_USERNAME_LEN), &"x".repeat(MAX_PASSWORD_LEN)),
            Ok(())
        );
    }

    #[test]
    fn les_homographes_unicode_sont_refuses() {
        // « а » cyrillique (U+0430) se lit comme un « a » latin : deux comptes
        // visuellement identiques permettraient l'usurpation.
        assert_eq!(
            validate_credentials("morgаnn", "mot-de-passe-solide"),
            Err(CredentialError::UsernameHasInvalidCharacters)
        );
        assert_eq!(
            validate_credentials("mor gann", "mot-de-passe-solide"),
            Err(CredentialError::UsernameHasInvalidCharacters)
        );
    }

    #[test]
    fn un_mot_de_passe_accepte_tout_caractere() {
        // Aucune restriction de jeu de caracteres cote mot de passe : elle
        // reduirait l'espace de recherche sans rien apporter.
        assert_eq!(
            validate_credentials("morgann", "🐺 élève-guerrier 한량"),
            Ok(())
        );
    }

    #[test]
    fn un_hachage_se_verifie_contre_son_mot_de_passe() {
        let hash = hash_password("mot-de-passe-solide").unwrap();
        assert!(verify_password("mot-de-passe-solide", &hash));
        assert!(!verify_password("autre-mot-de-passe", &hash));
    }

    #[test]
    fn deux_hachages_du_meme_mot_de_passe_different() {
        // Sels distincts : sinon deux comptes partageant un mot de passe se
        // reconnaissent dans la base, et une table precalculee devient utile.
        let first = hash_password("mot-de-passe-solide").unwrap();
        let second = hash_password("mot-de-passe-solide").unwrap();
        assert_ne!(first, second);
        assert!(verify_password("mot-de-passe-solide", &first));
        assert!(verify_password("mot-de-passe-solide", &second));
    }

    #[test]
    fn un_hachage_illisible_refuse_sans_paniquer() {
        assert!(!verify_password("peu-importe", "pas un hachage"));
        assert!(!verify_password("peu-importe", ""));
    }

    #[test]
    fn la_verification_factice_ne_valide_jamais_rien() {
        // Elle ne sert qu'a consommer du temps ; elle ne doit surtout pas
        // devenir un chemin d'authentification.
        verify_against_dummy("mot-de-passe-inexistant");
        assert!(!DUMMY_HASH.is_empty());
    }
}
