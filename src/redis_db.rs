use crate::db::{QueryResult, TableName};
use crate::redis_browser::{BrowserSnapshot, KeyFilter, KeyMeta};
use anyhow::{Context, Result, bail};
use redis::{Value, aio::MultiplexedConnection};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const KEY_LIMIT: usize = 10_000;
const PREVIEW_LIMIT: usize = 200;
pub struct RedisStore {
    connection: MultiplexedConnection,
    keys: BTreeMap<String, Vec<u8>>,
    cursor: u64,
    complete: bool,
    pattern: String,
    filter: KeyFilter,
    metadata: BTreeMap<String, KeyMeta>,
    scanned: usize,
    total: Option<usize>,
}
impl RedisStore {
    pub async fn connect(raw: &str) -> Result<Self> {
        let url = if let Some(rest) = raw
            .strip_prefix("valkey://")
            .or_else(|| raw.strip_prefix("keydb://"))
        {
            format!("redis://{rest}")
        } else {
            raw.to_owned()
        };
        let client = redis::Client::open(url)?;
        let mut connection = client.get_multiplexed_async_connection().await?;
        connection.set_response_timeout(Duration::from_secs(15));
        let _: String = redis::cmd("PING").query_async(&mut connection).await?;
        Ok(Self {
            connection,
            keys: BTreeMap::new(),
            cursor: 0,
            complete: false,
            pattern: "*".into(),
            filter: KeyFilter::default(),
            metadata: BTreeMap::new(),
            scanned: 0,
            total: None,
        })
    }
    pub fn set_filter(&mut self, filter: KeyFilter) -> Result<()> {
        filter.validate()?;
        if self.filter != filter {
            self.set_pattern(&filter.pattern);
            self.filter = filter;
        }
        Ok(())
    }
    pub fn restart(&mut self) {
        let pattern = self.pattern.clone();
        self.set_pattern(&pattern);
    }
    pub fn snapshot(&self) -> BrowserSnapshot {
        BrowserSnapshot {
            keys: self
                .metadata
                .values()
                .filter(|k| self.filter.matches(k))
                .cloned()
                .collect(),
            scanned: self.scanned,
            total: self.total,
            complete: self.complete,
            capped: self.keys.len() >= KEY_LIMIT,
        }
    }
    pub fn set_pattern(&mut self, pattern: &str) {
        self.pattern = if pattern.is_empty() {
            "*".into()
        } else {
            pattern.into()
        };
        self.keys.clear();
        self.metadata.clear();
        self.scanned = 0;
        self.filter.pattern = self.pattern.clone();
        self.cursor = 0;
        self.complete = false;
    }
    pub fn summary(&self) -> String {
        format!(
            "{} keys · MATCH {} · {}",
            self.snapshot().keys.len(),
            self.pattern,
            if self.keys.len() >= KEY_LIMIT {
                "10,000-key limit; use F4 to narrow filter"
            } else if self.complete {
                "scan complete; F3 rescans"
            } else {
                "more keys available; F3 continues scan"
            }
        )
    }
    pub async fn keys(&mut self) -> Result<Vec<TableName>> {
        if self.complete {
            self.restart();
        }
        self.total = redis::cmd("DBSIZE")
            .query_async(&mut self.connection)
            .await
            .ok();
        for _ in 0..5 {
            if self.keys.len() >= KEY_LIMIT {
                break;
            }
            let mut cmd = redis::cmd("SCAN");
            cmd.arg(self.cursor)
                .arg("MATCH")
                .arg(&self.pattern)
                .arg("COUNT")
                .arg(200);
            if !self.filter.kind.is_empty() {
                cmd.arg("TYPE").arg(&self.filter.kind);
            }
            let (next, keys): (u64, Vec<Vec<u8>>) = cmd.query_async(&mut self.connection).await?;
            self.cursor = next;
            self.scanned += keys.len();
            let keys = keys
                .into_iter()
                .take(KEY_LIMIT - self.keys.len())
                .collect::<Vec<_>>();
            if !keys.is_empty() {
                let mut pipe = redis::pipe();
                for key in &keys {
                    pipe.cmd("TYPE").arg(key).cmd("PTTL").arg(key);
                }
                let values: Vec<Value> = pipe.query_async(&mut self.connection).await?;
                for (i, key) in keys.into_iter().enumerate() {
                    let label = key_label(&key);
                    let kind: String = redis::from_redis_value(&values[2 * i])?;
                    let ttl_ms: i64 = redis::from_redis_value(&values[2 * i + 1])?;
                    let name = std::str::from_utf8(&key)
                        .map(str::to_owned)
                        .unwrap_or_else(|_| label.clone());
                    self.metadata.insert(
                        label.clone(),
                        KeyMeta {
                            label: label.clone(),
                            name,
                            kind,
                            ttl_ms,
                        },
                    );
                    self.keys.insert(label, key);
                }
            }
            if self.cursor == 0 {
                self.complete = true;
                break;
            }
        }
        Ok(self
            .snapshot()
            .keys
            .into_iter()
            .map(|key| TableName {
                schema: String::new(),
                name: key.label,
            })
            .collect())
    }
    pub async fn execute(&mut self, text: &str) -> Result<QueryResult> {
        // One shell-quoted command per line. Parse the whole script before any writes.
        let commands = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(shell_words::split)
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Invalid command quoting")?;
        if commands.is_empty() {
            bail!("Enter a Redis command first");
        }
        for args in &commands {
            let name = args.first().context("Empty command")?.to_ascii_uppercase();
            if matches!(
                name.as_str(),
                "SUBSCRIBE"
                    | "PSUBSCRIBE"
                    | "SSUBSCRIBE"
                    | "MONITOR"
                    | "SYNC"
                    | "PSYNC"
                    | "HELLO"
                    | "MULTI"
                    | "EXEC"
                    | "DISCARD"
                    | "WATCH"
                    | "UNWATCH"
                    | "QUIT"
                    | "AUTH"
                    | "RESET"
            ) {
                bail!(
                    "{name} changes connection/protocol state or streams indefinitely; use redis-cli for this command"
                );
            }
            if name == "SELECT" {
                bail!(
                    "Choose the logical database in Connections so history and the key browser stay in sync"
                );
            }
        }
        let start = Instant::now();
        let mut result = QueryResult {
            columns: vec!["Command".into(), "Response".into()],
            ..Default::default()
        };
        for args in commands {
            let mut cmd = redis::cmd(&args[0]);
            cmd.arg(&args[1..]);
            let value: Value = cmd.query_async(&mut self.connection).await?;
            result.statements += 1;
            if result.rows.len() < crate::db::ROW_LIMIT {
                result
                    .rows
                    .push(vec![args[0].to_ascii_uppercase(), format_value(&value)]);
            } else {
                result.truncated = true;
            }
        }
        result.elapsed_ms = start.elapsed().as_millis();
        Ok(result)
    }
    pub async fn inspect(&mut self, label: &str) -> Result<QueryResult> {
        let key = self
            .keys
            .get(label)
            .context("Key is no longer in the browser; refresh first")?
            .clone();
        let start = Instant::now();
        let kind: String = redis::cmd("TYPE")
            .arg(&key)
            .query_async(&mut self.connection)
            .await?;
        let ttl: i64 = redis::cmd("PTTL")
            .arg(&key)
            .query_async(&mut self.connection)
            .await?;
        let mut result = QueryResult {
            columns: vec!["Property / item".into(), "Value".into()],
            rows: vec![
                vec!["Key".into(), label.into()],
                vec!["Type".into(), kind.clone()],
                vec![
                    "TTL".into(),
                    match ttl {
                        -1 => "No expiry".into(),
                        -2 => "Key expired or was deleted".into(),
                        n => format!("{n} ms"),
                    },
                ],
            ],
            ..Default::default()
        };
        match kind.as_str() {
            "none" => {}
            "string" => {
                let size: usize = redis::cmd("STRLEN")
                    .arg(&key)
                    .query_async(&mut self.connection)
                    .await?;
                let data: Vec<u8> = redis::cmd("GETRANGE")
                    .arg(&key)
                    .arg(0)
                    .arg(65535)
                    .query_async(&mut self.connection)
                    .await?;
                result
                    .rows
                    .push(vec!["Length (bytes)".into(), size.to_string()]);
                let value = String::from_utf8(data.clone())
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .map(|v| serde_json::to_string_pretty(&v).unwrap())
                    .unwrap_or_else(|| bytes_text(&data));
                result.rows.push(vec!["Value".into(), value]);
                result.truncated = size > 65536;
            }
            "list" => {
                let size: usize = redis::cmd("LLEN")
                    .arg(&key)
                    .query_async(&mut self.connection)
                    .await?;
                let values: Vec<Vec<u8>> = redis::cmd("LRANGE")
                    .arg(&key)
                    .arg(0)
                    .arg(PREVIEW_LIMIT - 1)
                    .query_async(&mut self.connection)
                    .await?;
                result.rows.push(vec!["Length".into(), size.to_string()]);
                result.rows.extend(
                    values
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| vec![i.to_string(), bytes_text(&v)]),
                );
                result.truncated = size > PREVIEW_LIMIT;
            }
            "hash" | "set" | "zset" => {
                let cmd = match kind.as_str() {
                    "hash" => "HSCAN",
                    "set" => "SSCAN",
                    _ => "ZSCAN",
                };
                let (_, items): (u64, Vec<Vec<u8>>) = redis::cmd(cmd)
                    .arg(&key)
                    .arg(0)
                    .arg("COUNT")
                    .arg(PREVIEW_LIMIT)
                    .query_async(&mut self.connection)
                    .await?;
                let length_cmd = match kind.as_str() {
                    "hash" => "HLEN",
                    "set" => "SCARD",
                    _ => "ZCARD",
                };
                let size: usize = redis::cmd(length_cmd)
                    .arg(&key)
                    .query_async(&mut self.connection)
                    .await?;
                result.rows.push(vec!["Length".into(), size.to_string()]);
                if kind == "set" {
                    result.rows.extend(
                        items
                            .iter()
                            .take(PREVIEW_LIMIT)
                            .enumerate()
                            .map(|(i, v)| vec![i.to_string(), bytes_text(v)]),
                    );
                } else {
                    result.rows.extend(
                        items
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .take(PREVIEW_LIMIT)
                            .map(|p| vec![bytes_text(&p[0]), bytes_text(&p[1])]),
                    );
                }
                // SCAN COUNT is a hint, so advertise a preview rather than a complete collection.
                result.truncated = size > result.rows.len().saturating_sub(4);
            }
            "stream" => {
                let size: usize = redis::cmd("XLEN")
                    .arg(&key)
                    .query_async(&mut self.connection)
                    .await?;
                let value: Value = redis::cmd("XRANGE")
                    .arg(&key)
                    .arg("-")
                    .arg("+")
                    .arg("COUNT")
                    .arg(PREVIEW_LIMIT)
                    .query_async(&mut self.connection)
                    .await?;
                result.rows.push(vec!["Length".into(), size.to_string()]);
                if let Value::Array(entries) = value {
                    for entry in entries {
                        if let Value::Array(parts) = entry
                            && parts.len() == 2
                        {
                            result
                                .rows
                                .push(vec![format_value(&parts[0]), format_value(&parts[1])]);
                        }
                    }
                }
                result.truncated = size > PREVIEW_LIMIT;
            }
            _ => result.rows.push(vec![
                "Preview".into(),
                "Module type: inspect using commands in the editor".into(),
            ]),
        }
        result.elapsed_ms = start.elapsed().as_millis();
        Ok(result)
    }
}
fn bytes_text(value: &[u8]) -> String {
    match std::str::from_utf8(value) {
        Ok(s) => s.into(),
        Err(_) => format!(
            "0x{}",
            value.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    }
}
fn key_label(value: &[u8]) -> String {
    match std::str::from_utf8(value) {
        Ok(s) => serde_json::to_string(s).unwrap(),
        Err(_) => bytes_text(value),
    }
}
fn format_value(value: &Value) -> String {
    match value {
        Value::Nil => "(nil)".into(),
        Value::Okay => "OK".into(),
        Value::Int(n) => n.to_string(),
        Value::BulkString(s) => bytes_text(s),
        Value::SimpleString(s) => s.clone(),
        Value::Array(values) | Value::Set(values) => format!(
            "[{}]",
            values
                .iter()
                .map(format_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Map(pairs) => format!(
            "{{{}}}",
            pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", format_value(k), format_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => format!("{other:?}"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn collect_keys(store: &mut RedisStore) -> Vec<TableName> {
        let mut keys = store.keys().await.unwrap();
        for _ in 0..40 {
            if store.snapshot().complete {
                return keys;
            }
            keys = store.keys().await.unwrap();
        }
        panic!("Test scan did not complete");
    }
    #[test]
    fn binary_key_labels_cannot_collide_with_text() {
        assert_ne!(key_label(&[255]), key_label(b"0xff"));
        assert_eq!(bytes_text(&[0xff, 0]), "0xff00");
    }
    #[tokio::test]
    #[ignore = "requires DBRUST_TEST_REDIS_URL pointing to a dedicated test server"]
    async fn redis_types_scan_commands_and_expiry() {
        let url = std::env::var("DBRUST_TEST_REDIS_URL").unwrap();
        let mut store = RedisStore::connect(&url).await.unwrap();
        store.execute("SET dbrust:string '{\"hello\":42}'\nHSET dbrust:hash name alice\nRPUSH dbrust:list a b\nSADD dbrust:set one two\nZADD dbrust:zset 2 alice\nXADD dbrust:stream '*' name alice\nPEXPIRE dbrust:string 60000").await.unwrap();
        store.set_pattern("dbrust:*");
        let keys = collect_keys(&mut store).await;
        assert_eq!(keys.len(), 6);
        for key in keys {
            let r = store.inspect(&key.name).await.unwrap();
            assert!(r.rows.len() > 3);
            assert!(!r.rows[1][1].is_empty());
        }
        let r = store.execute("GET dbrust:string").await.unwrap();
        assert_eq!(r.rows[0][1], "{\"hello\":42}");
        assert!(
            store
                .execute("SET should:not:exist 1\nMULTI")
                .await
                .is_err()
        );
        assert_eq!(
            store.execute("EXISTS should:not:exist").await.unwrap().rows[0][1],
            "0"
        );
        assert!(store.execute("SET broken '").await.is_err());
        store.set_pattern("no-such-key:*");
        assert!(collect_keys(&mut store).await.is_empty());
    }
    #[tokio::test]
    #[ignore = "requires DBRUST_TEST_REDIS_URL pointing to a dedicated test server"]
    async fn redis_combined_filters_and_scan_continuation() {
        use crate::redis_browser::ExpiryFilter;
        let url = std::env::var("DBRUST_TEST_REDIS_URL").unwrap();
        let mut store = RedisStore::connect(&url).await.unwrap();
        store.execute("SET filters:temporary value PX 60000\nSET filters:persistent value\nHSET filters:hash field value").await.unwrap();
        store
            .set_filter(KeyFilter {
                pattern: "filters:*".into(),
                kind: "hash".into(),
                expiry: ExpiryFilter::Persistent,
                ..Default::default()
            })
            .unwrap();
        let keys = collect_keys(&mut store).await;
        assert_eq!(keys.len(), 1);
        assert!(keys[0].name.contains("hash"));
        store
            .set_filter(KeyFilter {
                pattern: "filters:*".into(),
                expiry: ExpiryFilter::Expiring,
                min_ttl: Some(30),
                max_ttl: Some(90),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(collect_keys(&mut store).await.len(), 1);
        store
            .set_filter(KeyFilter {
                pattern: "filters:*".into(),
                expiry: ExpiryFilter::Expiring,
                max_ttl: Some(5),
                ..Default::default()
            })
            .unwrap();
        assert!(collect_keys(&mut store).await.is_empty());
        let mut pipe = redis::pipe();
        for i in 0..1500 {
            pipe.cmd("SET").arg(format!("paging:{i}")).arg("v").ignore();
        }
        let _: () = pipe.query_async(&mut store.connection).await.unwrap();
        store
            .set_filter(KeyFilter {
                pattern: "paging:*".into(),
                ..Default::default()
            })
            .unwrap();
        let mut keys = store.keys().await.unwrap();
        assert!(!store.snapshot().complete);
        assert!(keys.len() < 1500);
        for _ in 0..10 {
            if store.snapshot().complete {
                break;
            }
            keys = store.keys().await.unwrap();
        }
        assert!(store.snapshot().complete);
        assert_eq!(keys.len(), 1500);
        assert_eq!(store.snapshot().keys.len(), 1500);
        store.restart();
        assert_eq!(store.snapshot().scanned, 0);
        assert!(store.snapshot().keys.is_empty());
    }
}
