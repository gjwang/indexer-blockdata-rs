use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey, Algorithm};
use serde::{Deserialize, Serialize};
use chrono::Utc;

/// Claims for Centrifugo connection token
#[derive(Debug, Serialize, Deserialize)]
pub struct CentrifugoClaims {
    /// Subject (user ID) - CRITICAL for user# channel validation
    pub sub: String,
    /// Expiration time (Unix timestamp)
    pub exp: i64,
    /// Issued at (Unix timestamp)
    pub iat: i64,
    /// Optional: pre-authorized channels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<String>>,
}

/// Token generator for Centrifugo authentication
pub struct CentrifugoTokenGenerator {
    secret_key: String,
}

impl CentrifugoTokenGenerator {
    /// Create a new token generator with the HMAC secret key
    pub fn new(secret_key: String) -> Self {
        Self { secret_key }
    }

    /// Generate a connection token for a user
    /// 
    /// # Arguments
    /// * `user_id` - The unique user identifier
    /// * `ttl_seconds` - Token time-to-live in seconds (default: 3600 = 1 hour)
    /// * `channels` - Optional list of pre-authorized channels
    pub fn generate_token(
        &self,
        user_id: &str,
        ttl_seconds: Option<i64>,
        channels: Option<Vec<String>>,
    ) -> Result<String, jsonwebtoken::errors::Error> {
        let now = Utc::now().timestamp();
        let ttl = ttl_seconds.unwrap_or(3600); // Default 1 hour
        let expiration = now + ttl;

        let claims = CentrifugoClaims {
            sub: user_id.to_string(),
            exp: expiration,
            iat: now,
            channels,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret_key.as_bytes()),
        )?;

        Ok(token)
    }

    /// Generate a token with default channels for a user
    /// Automatically includes balance, orders, and positions channels
    pub fn generate_token_with_default_channels(
        &self,
        user_id: &str,
        ttl_seconds: Option<i64>,
    ) -> Result<String, jsonwebtoken::errors::Error> {
        let channels = vec![
            format!("user:{}#balance", user_id),
            format!("user:{}#orders", user_id),
            format!("user:{}#positions", user_id),
        ];

        self.generate_token(user_id, ttl_seconds, Some(channels))
    }

    /// Verify and decode a Centrifugo token
    pub fn verify_token(&self, token: &str) -> Result<CentrifugoClaims, jsonwebtoken::errors::Error> {
        let validation = Validation::new(Algorithm::HS256);
        let token_data = decode::<CentrifugoClaims>(
            token,
            &DecodingKey::from_secret(self.secret_key.as_bytes()),
            &validation,
        )?;

        Ok(token_data.claims)
    }
}

/// Response structure for token endpoint
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub centrifugo_token: String,
    pub expires_in: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_generation() {
        let generator = CentrifugoTokenGenerator::new(
            "my_super_secret_key_which_is_very_long_and_secure_enough_for_hs256".to_string()
        );

        let token = generator.generate_token("12345", Some(3600), None).unwrap();
        assert!(!token.is_empty());

        // Verify the token
        let claims = generator.verify_token(&token).unwrap();
        assert_eq!(claims.sub, "12345");
    }

    #[test]
    fn test_token_with_channels() {
        let generator = CentrifugoTokenGenerator::new(
            "my_super_secret_key_which_is_very_long_and_secure_enough_for_hs256".to_string()
        );

        let token = generator.generate_token_with_default_channels("12345", Some(3600)).unwrap();
        let claims = generator.verify_token(&token).unwrap();

        assert_eq!(claims.sub, "12345");
        assert!(claims.channels.is_some());
        
        let channels = claims.channels.unwrap();
        assert!(channels.contains(&"user:12345#balance".to_string()));
        assert!(channels.contains(&"user:12345#orders".to_string()));
        assert!(channels.contains(&"user:12345#positions".to_string()));
    }

    #[test]
    fn test_invalid_token() {
        let generator = CentrifugoTokenGenerator::new(
            "my_super_secret_key_which_is_very_long_and_secure_enough_for_hs256".to_string()
        );

        let result = generator.verify_token("invalid.token.here");
        assert!(result.is_err());
    }
}
