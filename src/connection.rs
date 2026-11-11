use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedConnection {
    pub url: String,
    pub needs_password: bool,
}

impl SavedConnection {
    pub fn from_url(raw: &str) -> Result<Self> {
        if raw.starts_with("sqlite:") {
            return Ok(Self {
                url: raw.into(),
                needs_password: false,
            });
        }
        let mut url = Url::parse(raw).context("Invalid connection URL")?;
        if !matches!(
            url.scheme(),
            "postgres" | "postgresql" | "mysql" | "redis" | "rediss" | "valkey" | "keydb"
        ) {
            bail!(
                "Use postgres://, mysql://, sqlite:, redis://, rediss://, valkey://, or keydb://"
            );
        }
        let mut secret = url.password().is_some();
        url.set_password(None)
            .map_err(|_| anyhow::anyhow!("Invalid URL"))?;
        let params: Vec<(String, String)> = url
            .query_pairs()
            .filter_map(|(k, v)| {
                if matches!(
                    k.to_ascii_lowercase().as_str(),
                    "password" | "pass" | "sslpassword"
                ) {
                    secret = true;
                    None
                } else {
                    Some((k.into_owned(), v.into_owned()))
                }
            })
            .collect();
        url.set_query(None);
        if !params.is_empty() {
            url.query_pairs_mut().extend_pairs(params);
        }
        Ok(Self {
            url: url.into(),
            needs_password: secret,
        })
    }

    pub fn with_password(&self, password: &str) -> Result<String> {
        if !self.needs_password {
            return Ok(self.url.clone());
        }
        let mut url = Url::parse(&self.url)?;
        url.set_password(Some(&encode_component(password)))
            .map_err(|_| anyhow::anyhow!("Invalid URL"))?;
        Ok(url.into())
    }
}

pub fn manual_url(
    driver: &str,
    host: &str,
    port: &str,
    db: &str,
    user: &str,
    password: &str,
) -> Result<String> {
    if !matches!(
        driver,
        "postgres" | "mysql" | "sqlite" | "redis" | "rediss" | "valkey" | "keydb"
    ) {
        bail!("Driver must be postgres, mysql, sqlite, redis, rediss, valkey, or keydb");
    }
    if driver == "sqlite" {
        if db.is_empty() {
            bail!("Enter a SQLite path or :memory:");
        }
        return Ok(if db == ":memory:" {
            "sqlite::memory:".into()
        } else {
            format!("sqlite://{db}?mode=rwc")
        });
    }
    let redis = matches!(driver, "redis" | "rediss" | "valkey" | "keydb");
    if host.is_empty() || (!redis && (db.is_empty() || user.is_empty())) {
        bail!("Host, database, and username are required");
    }
    let mut url = Url::parse(&format!("{driver}://localhost"))?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    url.set_host(Some(&host)).context("Invalid host")?;
    let port = if port.is_empty() {
        if driver == "mysql" {
            3306
        } else if redis {
            6379
        } else {
            5432
        }
    } else {
        port.parse::<u16>().context("Port must be 1–65535")?
    };
    if port == 0 {
        bail!("Port must be 1–65535");
    }
    url.set_port(Some(port))
        .map_err(|_| anyhow::anyhow!("Invalid port"))?;
    url.set_username(&encode_component(user))
        .map_err(|_| anyhow::anyhow!("Invalid username"))?;
    if !redis || !password.is_empty() {
        url.set_password(Some(&encode_component(password)))
            .map_err(|_| anyhow::anyhow!("Invalid password"))?;
    }
    if redis {
        let index = if db.is_empty() {
            0
        } else {
            db.parse::<u32>()
                .context("Redis database must be a nonnegative number")?
        };
        url.set_path(&index.to_string());
    } else {
        url.set_path(&encode_component(db));
    }
    Ok(url.into())
}

fn encode_component(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

#[derive(Default)]
pub struct History {
    pub entries: Vec<SavedConnection>,
    pub path: PathBuf,
}
impl History {
    pub fn load() -> Result<Self> {
        let path = ProjectDirs::from("", "", "dbrust")
            .context("Cannot locate config directory")?
            .config_dir()
            .join("connections.json");
        Self::load_from(path)
    }
    pub fn load_from(path: PathBuf) -> Result<Self> {
        let entries = match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str::<Vec<SavedConnection>>(&data)
                .context("Invalid connection history JSON")?
                .into_iter()
                .map(|entry| {
                    let mut clean = SavedConnection::from_url(&entry.url)?;
                    clean.needs_password |= entry.needs_password;
                    Ok(clean)
                })
                .collect::<Result<Vec<_>>>()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(e.into()),
        };
        Ok(Self { entries, path })
    }
    pub fn remember(&mut self, entry: SavedConnection) -> Result<()> {
        self.entries.retain(|e| e.url != entry.url);
        self.entries.insert(0, entry);
        self.entries.truncate(20);
        let parent = self.path.parent().context("Invalid history path")?;
        fs::create_dir_all(parent)?;
        let temp = self.path.with_extension("json.tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&temp)?;
        file.write_all(serde_json::to_string_pretty(&self.entries)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(temp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_removed_and_reconnect_is_encoded() {
        let saved = SavedConnection::from_url(
            "postgres://alice:secret@localhost/db?password=other&sslmode=require",
        )
        .unwrap();
        assert!(!saved.url.contains("secret"));
        assert!(!saved.url.contains("other"));
        assert!(saved.needs_password);
        let restored = Url::parse(&saved.with_password("a@b:c/d").unwrap()).unwrap();
        assert_eq!(restored.password(), Some("a%40b%3Ac%2Fd"));
        assert!(saved.url.contains("sslmode=require"));
    }
    #[test]
    fn manual_fields_encode_credentials() {
        let url = manual_url("mysql", "localhost", "", "my db", "a@b", "p/q").unwrap();
        assert!(url.contains("a%40b:p%2Fq"));
        assert!(url.contains(":3306/my%20db"));
        assert!(manual_url("postgres", "localhost", "0", "db", "u", "p").is_err());
    }
    #[test]
    fn history_roundtrip_deduplicates() {
        let path = std::env::temp_dir().join(format!(
            "dbrust-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut history = History::load_from(path.clone()).unwrap();
        let saved = SavedConnection::from_url("mysql://u:p@localhost/db").unwrap();
        history.remember(saved.clone()).unwrap();
        history.remember(saved).unwrap();
        let loaded = History::load_from(path.clone()).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.entries[0].needs_password);
        assert!(!fs::read_to_string(&path).unwrap().contains("u:p"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn literal_percent_and_ipv6_are_preserved() {
        let raw = manual_url("postgres", "::1", "5432", "db/name", "user%20", "p%2Fword").unwrap();
        let url = Url::parse(&raw).unwrap();
        assert_eq!(url.host_str(), Some("[::1]"));
        assert_eq!(url.username(), "user%2520");
        assert_eq!(url.password(), Some("p%252Fword"));
        assert_eq!(url.path(), "/db%2Fname");
        let saved = SavedConnection::from_url(&raw).unwrap();
        assert_eq!(
            Url::parse(&saved.with_password("p%2Fword").unwrap())
                .unwrap()
                .password(),
            Some("p%252Fword")
        );
    }
    #[test]
    fn redis_manual_defaults_and_password_history() {
        for driver in ["redis", "rediss", "valkey", "keydb"] {
            let raw = manual_url(driver, "localhost", "", "", "", "").unwrap();
            let parsed = Url::parse(&raw).unwrap();
            assert_eq!(parsed.port(), Some(6379));
            assert_eq!(parsed.path(), "/0");
            assert!(parsed.password().is_none());
            let raw = manual_url(driver, "localhost", "", "2", "default", "p%word").unwrap();
            let saved = SavedConnection::from_url(&raw).unwrap();
            assert!(saved.needs_password);
            assert!(!saved.url.contains("p%25word"));
            assert_eq!(
                Url::parse(&saved.with_password("p%word").unwrap())
                    .unwrap()
                    .password(),
                Some("p%25word")
            );
        }
        assert!(manual_url("redis", "localhost", "", "abc", "", "").is_err());
    }
}
