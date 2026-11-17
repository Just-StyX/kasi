use crate::ApplicationBuilder;
use std::path::Path;

/// Trait implemented automatically via `#[derive(FromEnv)]` for strongly-typed environment configuration.
pub trait FromEnv: Sized + Clone + Send + Sync + 'static {
    /// Loads and parses configuration fields from process environment variables.
    fn from_env() -> Result<Self, String>;
}

/// Parses a `.env` file if present and sets any unset environment variables.
pub fn load_dotenv(path: impl AsRef<Path>) {
    if let Ok(contents) = std::fs::read_to_string(path) {
        for raw_line in contents.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, val)) = line.split_once('=') {
                let k = key.trim();
                let v = val.trim().trim_matches('"').trim_matches('\'');
                if std::env::var_os(k).is_none() {
                    // Note: Safe during single-threaded startup before spawning worker threads
                    unsafe {
                        std::env::set_var(k, v);
                    }
                }
            }
        }
    }
}

impl ApplicationBuilder {
    /// Loads `.env` (if present), parses `T::from_env()`, and registers `T` as a shared `Extension<T>`.
    pub fn with_env_config<T: FromEnv>(self) -> Result<Self, String> {
        load_dotenv(".env");
        let cfg = T::from_env()?;
        Ok(self.extension(cfg))
    }
}
