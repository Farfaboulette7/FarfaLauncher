//! Erreurs de l'application.
//!
//! Chaque erreur porte un `code` stable (que l'interface utilise pour réagir,
//! ex. retour à l'écran de connexion sur `auth_invalid`) et un message en
//! français compréhensible par un non-informaticien. Le détail technique ne
//! part que dans les logs.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Adresse du serveur invalide.")]
    InvalidServerUrl,
    #[error("Pour votre sécurité, l'adresse du serveur doit commencer par https:// (http:// n'est accepté que sur le réseau local).")]
    InsecureServerUrl,
    #[error("Aucun serveur RomM n'a répondu à cette adresse. Vérifiez l'adresse et votre connexion Internet.")]
    ServerUnreachable,
    #[error("Ce serveur ne semble pas être un serveur RomM.")]
    NotRomm,
    #[error("Aucun serveur n'est configuré.")]
    NoServer,
    #[error("Votre session a expiré ou votre accès a été retiré. Reconnectez-vous.")]
    AuthInvalid,
    #[error("Votre compte n'a pas les droits nécessaires. Demandez à l'administrateur du serveur.")]
    Forbidden,
    #[error("Ce code d'appairage est invalide ou a expiré. Générez-en un nouveau dans RomM.")]
    PairCodeInvalid,
    #[error("Trop de tentatives. Patientez une minute puis réessayez.")]
    RateLimited,
    #[error("Ce token n'a pas un format valide (il doit commencer par rmm_).")]
    TokenFormat,
    #[error("Le serveur a refusé ce token. Il a peut-être été révoqué ou a expiré.")]
    TokenRejected,
    #[error("Impossible d'accéder au trousseau sécurisé du système.")]
    Keychain,
    #[error("Le serveur a renvoyé une erreur inattendue. Réessayez plus tard.")]
    Server,
    #[error("Une erreur inattendue est survenue. Les détails sont dans les logs.")]
    Internal,
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::InvalidServerUrl => "invalid_server_url",
            AppError::InsecureServerUrl => "insecure_server_url",
            AppError::ServerUnreachable => "server_unreachable",
            AppError::NotRomm => "not_romm",
            AppError::NoServer => "no_server",
            AppError::AuthInvalid => "auth_invalid",
            AppError::Forbidden => "forbidden",
            AppError::PairCodeInvalid => "pair_code_invalid",
            AppError::RateLimited => "rate_limited",
            AppError::TokenFormat => "token_format",
            AppError::TokenRejected => "token_rejected",
            AppError::Keychain => "keychain",
            AppError::Server => "server",
            AppError::Internal => "internal",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Journalise le détail technique et renvoie l'erreur « utilisateur ».
pub fn log_err<E: std::fmt::Display>(err: AppError, context: &str, detail: E) -> AppError {
    tracing::error!("{context}: {detail}");
    err
}
