use http::{HeaderMap, HeaderValue, Request, header};
use std::collections::HashMap;

use crate::{BoxBody, BoxFuture, Error, extract::FromRequest};

/// Extracts parsed key-value cookies from the incoming `Cookie` header.
#[derive(Debug, Clone, Default)]
pub struct Cookies(pub HashMap<String, String>);

impl Cookies {
    /// Retrieves a cookie value by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|s| s.as_str())
    }
}

impl FromRequest for Cookies {
    fn extract(
        req: Request<BoxBody>,
    ) -> BoxFuture<'static, Result<(Self, Request<BoxBody>), Error>> {
        Box::pin(async move {
            let mut map = HashMap::new();

            if let Some(cookie_header) = req.headers().get(header::COOKIE)
                && let Ok(raw) = cookie_header.to_str()
            {
                for pair in raw.split(';') {
                    if let Some((k, v)) = pair.split_once('=') {
                        map.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
            }

            Ok((Cookies(map), req))
        })
    }
}

/// SameSite cookie policy options.
#[derive(Debug, Clone, Copy)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// Builder for generating standards-compliant `Set-Cookie` headers.
#[derive(Debug, Clone)]
pub struct Cookie {
    name: String,
    value: String,
    path: Option<String>,
    http_only: bool,
    secure: bool,
    same_site: Option<SameSite>,
    max_age_secs: Option<i64>,
}

impl Cookie {
    /// Creates a new cookie with sensible defaults (`Path=/`, `HttpOnly=true`, `SameSite=Lax`).
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            path: Some("/".to_string()),
            http_only: true,
            secure: false,
            same_site: Some(SameSite::Lax),
            max_age_secs: None,
        }
    }

    /// Creates an expired cookie to clear an existing browser cookie immediately.
    pub fn removal(name: impl Into<String>) -> Self {
        Self::new(name, "").max_age(0)
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn http_only(mut self, enabled: bool) -> Self {
        self.http_only = enabled;
        self
    }

    pub fn secure(mut self, enabled: bool) -> Self {
        self.secure = enabled;
        self
    }

    pub fn same_site(mut self, policy: SameSite) -> Self {
        self.same_site = Some(policy);
        self
    }

    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age_secs = Some(seconds);
        self
    }

    /// Formats the cookie into a `Set-Cookie` header string.
    pub fn to_header_string(&self) -> String {
        let mut parts = vec![format!("{}={}", self.name, self.value)];

        if let Some(ref p) = self.path {
            parts.push(format!("Path={}", p));
        }
        if let Some(age) = self.max_age_secs {
            parts.push(format!("Max-Age={}", age));
        }
        if self.http_only {
            parts.push("HttpOnly".to_string());
        }
        if self.secure {
            parts.push("Secure".to_string());
        }
        if let Some(site) = self.same_site {
            let s = match site {
                SameSite::Strict => "Strict",
                SameSite::Lax => "Lax",
                SameSite::None => "None",
            };
            parts.push(format!("SameSite={}", s));
        }

        parts.join("; ")
    }

    /// Appends this cookie onto an HTTP `HeaderMap`.
    pub fn append_to_headers(&self, headers: &mut HeaderMap) {
        if let Ok(val) = HeaderValue::from_str(&self.to_header_string()) {
            headers.append(header::SET_COOKIE, val);
        }
    }
}
