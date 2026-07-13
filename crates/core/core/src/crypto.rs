use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Deserialization error: {0}")]
    TomlDeserialization(#[from] toml::de::Error),
    #[error("Invalid key format")]
    InvalidKeyFormat,
    #[error("Key generation failed")]
    KeyGenerationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentKeyPair {
    pub public_key: String,
    pub secret_key: String,
    pub key_id: String,
    pub created_at: i64,
}

impl AgentKeyPair {
    pub fn generate() -> Result<Self, CryptoError> {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();

        let key_id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().timestamp();

        Ok(AgentKeyPair {
            public_key: hex::encode(verifying_key.as_bytes()),
            secret_key: hex::encode(signing_key.as_bytes()),
            key_id,
            created_at,
        })
    }

    pub fn get_verifying_key(&self) -> Result<VerifyingKey, CryptoError> {
        let public_bytes =
            hex::decode(&self.public_key).map_err(|_| CryptoError::InvalidKeyFormat)?;
        if public_bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat);
        }
        let mut public_key_array = [0u8; 32];
        public_key_array.copy_from_slice(&public_bytes);
        VerifyingKey::from_bytes(&public_key_array).map_err(|_| CryptoError::InvalidKeyFormat)
    }

    pub fn get_signing_key(&self) -> Result<SigningKey, CryptoError> {
        let secret_bytes =
            hex::decode(&self.secret_key).map_err(|_| CryptoError::InvalidKeyFormat)?;
        if secret_bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat);
        }
        let mut secret_key_array = [0u8; 32];
        secret_key_array.copy_from_slice(&secret_bytes);
        Ok(SigningKey::from_bytes(&secret_key_array))
    }

    pub fn save_to_file(&self, path: &PathBuf) -> Result<(), CryptoError> {
        let json = serde_json::to_string_pretty(self)?;

        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
        }

        #[cfg(not(unix))]
        {
            // On Windows, file permissions are controlled by ACLs.
            // This write uses default ACL (user-only access on typical single-user systems).
            // For multi-user deployments, configure NTFS permissions manually or use
            // a credential manager like Windows DPAPI.
            fs::write(path, json)?;
        }

        Ok(())
    }

    pub fn load_from_file(path: &PathBuf) -> Result<Self, CryptoError> {
        let json = fs::read_to_string(path)?;
        let keypair: AgentKeyPair = serde_json::from_str(&json)?;
        Ok(keypair)
    }

    pub fn ensure_master_key() -> Result<Self, CryptoError> {
        // Strategy 1: Environment variables
        if let Ok(secret_key) = std::env::var("SAVANT_MASTER_SECRET_KEY") {
            if let Ok(public_key) = std::env::var("SAVANT_MASTER_PUBLIC_KEY") {
                let key_id =
                    std::env::var("SAVANT_MASTER_KEY_ID").unwrap_or_else(|_| "env-key".to_string());
                let created_at = chrono::Utc::now().timestamp();

                return Ok(AgentKeyPair {
                    public_key,
                    secret_key,
                    key_id,
                    created_at,
                });
            }
        }

        // Strategy 2: Load .env from current working directory
        if let Err(e) = dotenvy::dotenv() {
            tracing::warn!("[core::crypto] Failed to load .env from cwd: {}", e);
        }
        if let Ok(secret_key) = std::env::var("SAVANT_MASTER_SECRET_KEY") {
            if let Ok(public_key) = std::env::var("SAVANT_MASTER_PUBLIC_KEY") {
                let key_id =
                    std::env::var("SAVANT_MASTER_KEY_ID").unwrap_or_else(|_| "cwd-key".to_string());
                return Ok(AgentKeyPair {
                    public_key,
                    secret_key,
                    key_id,
                    created_at: chrono::Utc::now().timestamp(),
                });
            }
        }

        // Strategy 3: Load .env from exe directory (for installed apps)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let env_path = exe_dir.join(".env");
                if env_path.exists() {
                    if let Err(e) = dotenvy::from_path(&env_path) {
                        tracing::warn!("[core::crypto] Failed to load .env from exe dir: {}", e);
                    }
                    if let Ok(secret_key) = std::env::var("SAVANT_MASTER_SECRET_KEY") {
                        if let Ok(public_key) = std::env::var("SAVANT_MASTER_PUBLIC_KEY") {
                            let key_id = std::env::var("SAVANT_MASTER_KEY_ID")
                                .unwrap_or_else(|_| "exe-key".to_string());
                            return Ok(AgentKeyPair {
                                public_key,
                                secret_key,
                                key_id,
                                created_at: chrono::Utc::now().timestamp(),
                            });
                        }
                    }
                }
            }
        }

        // Strategy 4: Load from persistent key file in config directory
        if let Some(key_path) = Self::key_file_path() {
            if key_path.exists() {
                if let Ok(keypair) = Self::load_from_file(&key_path) {
                    tracing::info!("✅ Loaded master key from {:?}", key_path);
                    return Ok(keypair);
                }
            }
        }

        // Strategy 5: Auto-generate and persist to config directory
        // NOTE: Auto-generation is a convenience fallback for development.
        // Production deployments should explicitly configure keys via environment variables.
        tracing::warn!("[core::crypto] No master key configured. Auto-generating a development key. Set SAVANT_MASTER_SECRET_KEY/SAVANT_MASTER_PUBLIC_KEY for production.");
        let generated_key = Self::generate()?;
        let key_id_short = &generated_key.key_id[..generated_key.key_id.len().min(8)];
        tracing::info!(
            "[core::crypto] Generated development master key: {}...",
            key_id_short
        );

        // Persist to config directory so it survives restarts
        if let Some(key_path) = Self::key_file_path() {
            if let Some(parent) = key_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!("[core::crypto] Failed to create key directory: {}", e);
                }
            }
            if let Err(e) = generated_key.save_to_file(&key_path) {
                tracing::warn!("⚠️  Failed to persist master key to {:?}: {}", key_path, e);
            } else {
                tracing::info!("✅ Master key persisted to {:?}", key_path);
            }
        }

        Ok(generated_key)
    }

    /// Returns the platform-appropriate path for the master key file.
    /// Windows: %APPDATA%/savant/master_key.json
    /// Unix: ~/.config/savant/master_key.json
    fn key_file_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            std::env::var("APPDATA").ok().map(|appdata| {
                PathBuf::from(appdata)
                    .join("savant")
                    .join("master_key.json")
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::env::var("HOME").ok().map(|home| {
                PathBuf::from(home)
                    .join(".config")
                    .join("savant")
                    .join("master_key.json")
            })
        }
    }

    pub fn sign_message(&self, message: &str) -> Result<String, CryptoError> {
        let secret_bytes =
            hex::decode(&self.secret_key).map_err(|_| CryptoError::InvalidKeyFormat)?;

        if secret_bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat);
        }

        let mut secret_key_array = [0u8; 32];
        secret_key_array.copy_from_slice(&secret_bytes);

        let signing_key = SigningKey::from_bytes(&secret_key_array);

        let signature = signing_key.sign(message.as_bytes());

        Ok(hex::encode(signature.to_bytes()))
    }

    pub fn verify_message(&self, message: &str, signature: &str) -> Result<bool, CryptoError> {
        let public_bytes =
            hex::decode(&self.public_key).map_err(|_| CryptoError::InvalidKeyFormat)?;

        if public_bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat);
        }

        let mut public_key_array = [0u8; 32];
        public_key_array.copy_from_slice(&public_bytes);

        let verifying_key = match VerifyingKey::from_bytes(&public_key_array) {
            Ok(key) => key,
            Err(_) => return Err(CryptoError::InvalidKeyFormat),
        };

        let sig_bytes = hex::decode(signature).map_err(|_| CryptoError::InvalidKeyFormat)?;

        if sig_bytes.len() != 64 {
            return Err(CryptoError::InvalidKeyFormat);
        }

        let mut signature_bytes = [0u8; 64];
        signature_bytes.copy_from_slice(&sig_bytes);

        let signature = Signature::from_bytes(&signature_bytes);

        match verifying_key.verify(message.as_bytes(), &signature) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

pub fn get_openrouter_api_key() -> Result<String, CryptoError> {
    // Try environment variable first
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
        return Ok(key);
    }

    // Try config file
    let config_path = PathBuf::from("config/api_keys.toml");
    if config_path.exists() {
        // Check file permissions on Unix - warn if world-readable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&config_path) {
                let mode = metadata.permissions().mode();
                if mode & 0o004 != 0 {
                    tracing::warn!(
                        "config/api_keys.toml has world-readable permissions ({:o}). \
                         Consider restricting to 0o600.",
                        mode & 0o777
                    );
                }
            }
        }

        let content = fs::read_to_string(&config_path)?;
        let config: toml::Value = toml::from_str(&content)?;
        if let Some(key) = config
            .get("openrouter")
            .and_then(|v| v.get("api_key"))
            .and_then(|v| v.as_str())
        {
            return Ok(key.to_string());
        }
    }

    // Production: Fail loudly if no API key is configured
    tracing::error!("No OpenRouter API key found. Set OPENROUTER_API_KEY environment variable or add to config/api_keys.toml");
    Err(CryptoError::InvalidKeyFormat)
}
