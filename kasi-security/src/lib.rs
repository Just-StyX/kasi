use http::{Request, header};
use kasi_core::BoxBody;
use kasi_core::{error::Error, extract::FromRequest};
use openidconnect::IssuerUrl;
use openidconnect::JsonWebKey;
use openidconnect::core::{CoreJsonWebKeySet, CoreProviderMetadata};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredRole(pub &'static str);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuth2<const N: usize>(pub [&'static str; N]);

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: uuid::Uuid,
    pub email: String,
    pub active_roles: Vec<String>,
    pub scopes: Vec<String>,
}

// Representing standard OIDC access token payloads
#[derive(Debug, Deserialize, Serialize)]
struct OidcStandardClaims {
    sub: String,
    email: Option<String>,
    #[serde(default)]
    scope: String,
    // Keycloak typically bundles realm roles inside 'realm_access'
    #[serde(default)]
    realm_access: RealmAccess,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RealmAccess {
    #[serde(default)]
    roles: Vec<String>,
}

pub struct OidcConfig {
    jwks: CoreJsonWebKeySet,
    expected_issuer: String,
}

struct MockClaims {
    sub: uuid::Uuid,
    email: String,
    roles: Vec<String>,
    scopes: Vec<String>,
}

impl RequiredRole {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl<const N: usize> OAuth2<N> {
    pub fn as_slice(&self) -> &[&'static str] {
        &self.0
    }
}

struct MockJwtEngine;
impl MockJwtEngine {
    fn decode_and_verify(token: &str) -> Result<MockClaims, &'static str> {
        if token == "invalid" {
            return Err("Token Expired or Revoked");
        }

        Ok(MockClaims {
            sub: uuid::Uuid::new_v4(),
            email: "developer_context@kasi.local".to_string(),
            roles: vec!["User".to_string(), "Admin".to_string()],
            scopes: vec!["read:profile".to_string(), "write:system".to_string()],
        })
    }
}

impl OidcConfig {
    pub async fn discover(
        issuer_url: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let issuer = IssuerUrl::new(issuer_url.to_string())?;
        let http_client = openidconnect::reqwest::ClientBuilder::new()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .build()?;

        let provider_metadata = CoreProviderMetadata::discover_async(issuer, &http_client).await?;

        Ok(Self {
            jwks: provider_metadata.jwks().clone(),
            expected_issuer: issuer_url.to_string(),
        })
    }
}

impl FromRequest for AuthContext {
    fn extract(
        req: Request<BoxBody>,
    ) -> kasi_core::BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let auth_ctx = Self::verify(&req, &[], &[]).await?;

            Ok((auth_ctx, req))
        })
    }
}

impl AuthContext {
    pub async fn password_verify(
        req: &Request<BoxBody>, // FIX: Changed from &Request<Incoming>
        required_roles: &[&str],
        required_scopes: &[&str],
    ) -> Result<Self, Error> {
        let auth_header = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                Error::Unauthorized("Missing Authorization bearer token header parameter.".into())
            })?;

        if !auth_header.starts_with("Bearer ") {
            return Err(Error::BadRequest(
                "Authorization scheme must be formatted as 'Bearer <Token>'".into(),
            ));
        }

        let token_string = &auth_header[7..];

        let claims = MockJwtEngine::decode_and_verify(token_string)
            .map_err(|e| Error::Unauthorized(format!("Invalid access token signature: {}", e)))?;

        for expected_scope in required_scopes {
            if !claims.scopes.iter().any(|s| s == expected_scope) {
                return Err(Error::Unauthorized(format!(
                    "Insufficient access privileges. Missing OAuth2 scope: '{}'",
                    expected_scope
                )));
            }
        }

        for expected_role in required_roles {
            if !claims.roles.iter().any(|r| r == expected_role) {
                return Err(Error::Unauthorized(format!(
                    "Access Denied. Identity lacks required role: '{}'",
                    expected_role
                )));
            }
        }

        Ok(AuthContext {
            user_id: claims.sub,
            email: claims.email,
            active_roles: claims.roles,
            scopes: claims.scopes,
        })
    }

    pub async fn verify(
        req: &Request<BoxBody>, // FIX: Changed from &Request<Incoming>
        required_roles: &[&str],
        required_scopes: &[&str],
    ) -> Result<Self, Error> {
        // 1. Get OidcConfig from Request Extensions container
        let oidc_config = req
            .extensions()
            .get::<std::sync::Arc<OidcConfig>>()
            .ok_or_else(|| {
                Error::Internal(
                    "OidcConfig engine not initialized in application context extensions.".into(),
                )
            })?;

        // 2. Parse Authorization Header
        let auth_header = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                Error::Unauthorized("Missing Authorization bearer token header parameter.".into())
            })?;

        if !auth_header.starts_with("Bearer ") {
            return Err(Error::BadRequest(
                "Authorization scheme must be formatted as 'Bearer <Token>'".into(),
            ));
        }
        let token_string = &auth_header[7..];

        // 3. SECURE LOCAL DECODING & SIGNATURE VALIDATION
        let structural_jwt = jsonwebtoken::decode_header(token_string)
            .map_err(|e| Error::Unauthorized(format!("Malformed JWT Header structure: {}", e)))?;

        let kid = structural_jwt
            .kid
            .ok_or_else(|| Error::Unauthorized("Missing Key ID (kid) header parameter.".into()))?;
        let jwk = oidc_config
            .jwks
            .keys()
            .iter()
            .find(|k| k.key_id().map(|id| id.as_str()) == Some(&kid))
            .ok_or_else(|| {
                Error::Unauthorized(
                    "Token signature signed by unverified issuer key metadata.".into(),
                )
            })?;

        let jwk_json_string = serde_json::to_string(jwk).map_err(|e| {
            Error::Internal(format!(
                "Failed to serialize provider key component layout: {}",
                e
            ))
        })?;

        let jsonwebtoken_jwk: jsonwebtoken::jwk::Jwk = serde_json::from_str(&jwk_json_string)
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to map provider key structure into JWT boundaries: {}",
                    e
                ))
            })?;

        let decoding_key = jsonwebtoken::DecodingKey::from_jwk(&jsonwebtoken_jwk).map_err(|e| {
            Error::Internal(format!("Failed to parse provider verification key: {}", e))
        })?;

        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&[&oidc_config.expected_issuer]);
        validation.validate_aud = false;

        let token_data =
            jsonwebtoken::decode::<OidcStandardClaims>(token_string, &decoding_key, &validation)
                .map_err(|e| {
                    Error::Unauthorized(format!(
                        "Cryptographic signature verification failed: {}",
                        e
                    ))
                })?;

        let claims = token_data.claims;

        // 4. Transform parsed scope string array into a vector
        let claims_scopes: Vec<String> = claims
            .scope
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let claims_roles = claims.realm_access.roles;

        // 5. Evaluate guards compiled into the macro attributes
        for expected_scope in required_scopes {
            if !claims_scopes.iter().any(|s| s == expected_scope) {
                return Err(Error::Unauthorized(format!(
                    "Insufficient token privileges. Missing scope: '{}'",
                    expected_scope
                )));
            }
        }

        for expected_role in required_roles {
            if !claims_roles.iter().any(|r| r == expected_role) {
                return Err(Error::Unauthorized(format!(
                    "Identity access blocked. Missing role: '{}'",
                    expected_role
                )));
            }
        }

        // 6. Map to clean platform types
        let parsed_uuid = uuid::Uuid::parse_str(&claims.sub).unwrap_or_else(|_| uuid::Uuid::nil());

        Ok(AuthContext {
            user_id: parsed_uuid,
            email: claims.email.unwrap_or_default(),
            active_roles: claims_roles,
            scopes: claims_scopes,
        })
    }

    pub fn default_anonymous() -> Self {
        Self {
            user_id: uuid::Uuid::nil(),
            email: "anonymous@kasi.local".into(),
            active_roles: vec![],
            scopes: vec![],
        }
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.active_roles.iter().any(|r| r == role)
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}
