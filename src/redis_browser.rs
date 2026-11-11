use anyhow::{Result, bail};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ExpiryFilter {
    #[default]
    Any,
    Persistent,
    Expiring,
}
impl ExpiryFilter {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Any => "Any TTL",
            Self::Persistent => "No expiry",
            Self::Expiring => "Expiring",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            Self::Any => Self::Persistent,
            Self::Persistent => Self::Expiring,
            Self::Expiring => Self::Any,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyFilter {
    pub pattern: String,
    pub kind: String,
    pub expiry: ExpiryFilter,
    pub min_ttl: Option<u64>,
    pub max_ttl: Option<u64>,
}
impl Default for KeyFilter {
    fn default() -> Self {
        Self {
            pattern: "*".into(),
            kind: String::new(),
            expiry: ExpiryFilter::Any,
            min_ttl: None,
            max_ttl: None,
        }
    }
}
impl KeyFilter {
    pub const TYPES: [&'static str; 7] = ["", "string", "hash", "list", "set", "zset", "stream"];
    pub fn validate(&self) -> Result<()> {
        if !Self::TYPES.contains(&self.kind.as_str()) {
            bail!("Choose a supported key type");
        }
        if self
            .min_ttl
            .zip(self.max_ttl)
            .is_some_and(|(min, max)| min > max)
        {
            bail!("Minimum TTL must not exceed maximum TTL");
        }
        if self.expiry == ExpiryFilter::Persistent
            && (self.min_ttl.is_some() || self.max_ttl.is_some())
        {
            bail!("TTL bounds apply to expiring keys; choose Any TTL or Expiring");
        }
        Ok(())
    }
    pub fn matches(&self, key: &KeyMeta) -> bool {
        if key.ttl_ms == -2 || (!self.kind.is_empty() && self.kind != key.kind) {
            return false;
        }
        if self.expiry == ExpiryFilter::Persistent && key.ttl_ms != -1 {
            return false;
        }
        if self.expiry == ExpiryFilter::Expiring && key.ttl_ms < 0 {
            return false;
        }
        if (self.min_ttl.is_some() || self.max_ttl.is_some()) && key.ttl_ms < 0 {
            return false;
        }
        let ttl = key.ttl_ms.max(0) as u64;
        !self.min_ttl.is_some_and(|n| ttl < n.saturating_mul(1000))
            && !self.max_ttl.is_some_and(|n| ttl > n.saturating_mul(1000))
    }
}
#[derive(Clone, Debug)]
pub struct KeyMeta {
    pub label: String,
    pub name: String,
    pub kind: String,
    pub ttl_ms: i64,
}
impl KeyMeta {
    pub fn ttl(&self) -> String {
        match self.ttl_ms {
            -1 => "∞".into(),
            -2 => "gone".into(),
            n => format!("{}s", n.max(0) / 1000),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct BrowserSnapshot {
    pub keys: Vec<KeyMeta>,
    pub scanned: usize,
    pub total: Option<usize>,
    pub complete: bool,
    pub capped: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeySort {
    #[default]
    Name,
    Type,
    Ttl,
}
impl KeySort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Type,
            Self::Type => Self::Ttl,
            Self::Ttl => Self::Name,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Type => "Type",
            Self::Ttl => "TTL",
        }
    }
}
#[derive(Clone, Debug)]
pub enum BrowserEntry {
    Folder(String, usize),
    Key(KeyMeta),
}
impl BrowserEntry {
    pub fn label(&self) -> String {
        match self {
            Self::Folder(name, n) => format!("▸ {name} ({n})"),
            Self::Key(key) => format!("{:<6} {:>7}  {}", key.kind, key.ttl(), key.name),
        }
    }
}
pub fn entries(
    snapshot: &BrowserSnapshot,
    tree: bool,
    prefix: &str,
    sort: KeySort,
    descending: bool,
) -> Vec<BrowserEntry> {
    let mut keys = snapshot
        .keys
        .iter()
        .filter(|k| k.name.starts_with(prefix))
        .cloned()
        .collect::<Vec<_>>();
    keys.sort_by(|a, b| {
        let order = match sort {
            KeySort::Name => a.name.cmp(&b.name),
            KeySort::Type => a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)),
            KeySort::Ttl => {
                let ttl = |k: &KeyMeta| if k.ttl_ms < 0 { i64::MAX } else { k.ttl_ms };
                ttl(a).cmp(&ttl(b)).then(a.name.cmp(&b.name))
            }
        };
        if descending { order.reverse() } else { order }
    });
    if !tree {
        return keys.into_iter().map(BrowserEntry::Key).collect();
    }
    let mut folders = std::collections::BTreeMap::<String, usize>::new();
    let mut result = vec![];
    for key in keys {
        let rest = &key.name[prefix.len()..];
        if let Some(i) = rest.find(':') {
            let folder = format!("{prefix}{}", &rest[..=i]);
            *folders.entry(folder).or_default() += 1;
        } else {
            result.push(BrowserEntry::Key(key));
        }
    }
    let mut grouped = folders
        .into_iter()
        .map(|(f, n)| BrowserEntry::Folder(f, n))
        .collect::<Vec<_>>();
    grouped.extend(result);
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(name: &str, kind: &str, ttl_ms: i64) -> KeyMeta {
        KeyMeta {
            label: format!("\"{name}\""),
            name: name.into(),
            kind: kind.into(),
            ttl_ms,
        }
    }
    #[test]
    fn filters_validate_and_expiry_ranges_are_exact() {
        let mut f = KeyFilter {
            expiry: ExpiryFilter::Expiring,
            min_ttl: Some(10),
            max_ttl: Some(20),
            ..Default::default()
        };
        assert!(f.matches(&key("a", "hash", 15000)));
        assert!(!f.matches(&key("b", "hash", -1)));
        assert!(!f.matches(&key("c", "hash", 9999)));
        f.kind = "string".into();
        assert!(!f.matches(&key("d", "hash", 15000)));
        f.min_ttl = Some(30);
        assert!(f.validate().is_err());
        f.min_ttl = None;
        f.expiry = ExpiryFilter::Persistent;
        assert!(f.validate().is_err());
    }
    #[test]
    fn namespaces_and_sorting_preserve_key_identity() {
        let s = BrowserSnapshot {
            keys: vec![
                key("user:2", "hash", 10000),
                key("user:1", "string", -1),
                key("plain", "set", 5000),
            ],
            ..Default::default()
        };
        let rows = entries(&s, true, "", KeySort::Name, false);
        assert!(matches!(&rows[0],BrowserEntry::Folder(f,2) if f=="user:"));
        assert_eq!(entries(&s, true, "user:", KeySort::Name, false).len(), 2);
        assert!(
            matches!(&entries(&s,false,"",KeySort::Ttl,false)[0],BrowserEntry::Key(k) if k.name=="plain")
        );
    }
}
