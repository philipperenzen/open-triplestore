use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// JWT configuration.
#[derive(Clone)]
pub struct JwtConfig {
    pub secret: String,
    pub access_expiry_minutes: u64,
    pub refresh_expiry_days: u64,
}

impl JwtConfig {
    pub fn new(secret: String, access_expiry_minutes: u64, refresh_expiry_days: u64) -> Self {
        Self {
            secret,
            access_expiry_minutes,
            refresh_expiry_days,
        }
    }
}

/// JWT claims.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,
    /// Username
    pub username: String,
    /// System role (super_admin, admin, user)
    pub role: String,
    /// Token type (access, refresh)
    pub token_type: String,
    /// Issued at (Unix timestamp)
    pub iat: u64,
    /// Expiration (Unix timestamp)
    pub exp: u64,
}

/// Issue a short-lived access token.
pub fn issue_access_token(
    config: &JwtConfig,
    user_id: &str,
    username: &str,
    role: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp() as u64;
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        token_type: "access".to_string(),
        iat: now,
        exp: now + config.access_expiry_minutes * 60,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )?;

    Ok(token)
}

/// Issue a longer-lived refresh token.
pub fn issue_refresh_token(
    config: &JwtConfig,
    user_id: &str,
    username: &str,
    role: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp() as u64;
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        token_type: "refresh".to_string(),
        iat: now,
        exp: now + config.refresh_expiry_days * 86400,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.secret.as_bytes()),
    )?;

    Ok(token)
}

/// Verify and decode a JWT token.
pub fn verify_token(config: &JwtConfig, token: &str) -> anyhow::Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )?;

    Ok(token_data.claims)
}

/// Generate a random API token string with `ots_` prefix.
pub fn generate_api_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
    format!("ots_{}", encoded)
}

/// Hash a token (API token or refresh token) with SHA-256.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// True for well-known default/placeholder JWT secrets (and the empty string) that must never
/// sign tokens in a real deployment — a public secret makes every session token forgeable.
/// `main` warns on these and refuses to start when production cookies are enabled.
pub fn is_weak_jwt_secret(secret: &str) -> bool {
    const WEAK: &[&str] = &[
        "",
        "change-me-in-production",
        "change-me",
        "changeme",
        "dev-secret-change-me",
        "secret",
        "changemechangeme",
    ];
    WEAK.contains(&secret.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_and_verify_access_token() {
        let config = JwtConfig::new("test-secret-key".to_string(), 30, 30);
        let token = issue_access_token(&config, "user-123", "alice", "user").unwrap();
        let claims = verify_token(&config, &token).unwrap();
        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.username, "alice");
        assert_eq!(claims.role, "user");
        assert_eq!(claims.token_type, "access");
    }

    #[test]
    fn test_issue_and_verify_refresh_token() {
        let config = JwtConfig::new("test-secret-key".to_string(), 30, 30);
        let token = issue_refresh_token(&config, "user-123", "alice", "admin").unwrap();
        let claims = verify_token(&config, &token).unwrap();
        assert_eq!(claims.token_type, "refresh");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_invalid_token() {
        let config = JwtConfig::new("test-secret-key".to_string(), 30, 30);
        let result = verify_token(&config, "invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_api_token_generation() {
        let token = generate_api_token();
        assert!(token.starts_with("ots_"));
        assert!(token.len() > 10);

        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64); // SHA-256 hex
    }
}

#[cfg(test)]
mod jwt_security_tests {
    use super::is_weak_jwt_secret;

    #[test]
    fn weak_jwt_secrets_are_flagged() {
        for s in [
            "",
            "change-me-in-production",
            "  changeme ",
            "dev-secret-change-me",
            "secret",
        ] {
            assert!(
                is_weak_jwt_secret(s),
                "{s:?} must be rejected as a weak/default JWT secret"
            );
        }
    }

    #[test]
    fn strong_jwt_secret_is_allowed() {
        // e.g. `openssl rand -hex 32`
        assert!(!is_weak_jwt_secret(
            "9f3c2a1be7d04f5a8c6b0e2d4a6f8c1e3b5d7f9a1c3e5b7d9f1a3c5e7b9d1f3a5"
        ));
    }
}
