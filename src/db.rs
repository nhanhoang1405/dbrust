use anyhow::{Result, bail};
use futures_util::TryStreamExt;
use sqlx::{Column, Connection, Either, Executor, Row, TypeInfo, ValueRef};
use std::time::Instant;

pub const ROW_LIMIT: usize = 10_000;
pub enum Database {
    Postgres(sqlx::PgConnection),
    Mysql(sqlx::MySqlConnection),
    Sqlite(sqlx::SqliteConnection),
}
#[derive(Clone, Debug)]
pub struct TableName {
    pub schema: String,
    pub name: String,
}
impl TableName {
    pub fn label(&self) -> String {
        if self.schema.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }
    pub fn preview_sql(&self, mysql: bool) -> String {
        let q = if mysql { '`' } else { '"' };
        let quote = |s: &str| format!("{q}{}{q}", s.replace(q, &format!("{q}{q}")));
        let name = if self.schema.is_empty() {
            quote(&self.name)
        } else {
            format!("{}.{}", quote(&self.schema), quote(&self.name))
        };
        format!("SELECT * FROM {name} LIMIT 1000;")
    }
}
#[derive(Default, Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub affected: u64,
    pub elapsed_ms: u128,
    pub truncated: bool,
    pub statements: usize,
}

macro_rules! cell {
    ($row:expr, $i:expr $(, $extra:ty)*) => {{
        let row = $row;
        let i = $i;
        if row.try_get_raw(i)?.is_null() { "NULL".to_string() }
        else if let Ok(v) = row.try_get::<String,_>(i) { v }
        else if let Ok(v) = row.try_get::<i64,_>(i) { v.to_string() }
        else if let Ok(v) = row.try_get::<i32,_>(i) { v.to_string() }
        else if let Ok(v) = row.try_get::<i16,_>(i) { v.to_string() }
        else if let Ok(v) = row.try_get::<f64,_>(i) { v.to_string() }
        else if let Ok(v) = row.try_get::<f32,_>(i) { v.to_string() }
        else if let Ok(v) = row.try_get::<bool,_>(i) { v.to_string() }
        $(else if let Ok(v) = row.try_get::<$extra,_>(i) { v.to_string() })*
        else if let Ok(v) = row.try_get::<Vec<u8>,_>(i) {
            format!("0x{}", v.iter().map(|b| format!("{b:02x}")).collect::<String>())
        } else { format!("<{}: unsupported display>", row.column(i).type_info().name()) }
    }};
}
macro_rules! execute {
    ($conn:expr, $sql:expr $(, $extra:ty)*) => {{
        let mut result = QueryResult::default();
        let mut stream = sqlx::raw_sql($sql).fetch_many(&mut *$conn);
        let mut new_set = true;
        while let Some(item) = stream.try_next().await? {
            match item {
                Either::Left(done) => {
                    result.affected += done.rows_affected();
                    result.statements += 1;
                    new_set = true;
                }
                Either::Right(row) => {
                    if new_set {
                        result.columns = row.columns().iter().map(|c| c.name().to_owned()).collect();
                        result.rows.clear();
                        result.truncated = false;
                        new_set = false;
                    }
                    if result.rows.len() < ROW_LIMIT {
                        let mut values = Vec::with_capacity(row.len());
                        for i in 0..row.len() { values.push(cell!(&row, i $(, $extra)*)); }
                        result.rows.push(values);
                    } else { result.truncated = true; }
                }
            }
        }
        drop(stream);
        // Describe only a successful, single SELECT. Preparing a multi-statement
        // script on PostgreSQL would itself fail and abort an open transaction.
        if result.columns.is_empty() && result.statements == 1
            && $sql.split_whitespace().next().is_some_and(|word| word.eq_ignore_ascii_case("select")) {
            if let Ok(description) = (&mut *$conn).describe($sql).await {
                result.columns = description.columns().iter().map(|c| c.name().to_owned()).collect();
            }
        }
        result
    }};
}
impl Database {
    pub async fn connect(url: &str) -> Result<Self> {
        Ok(
            if url.starts_with("postgres:") || url.starts_with("postgresql:") {
                Self::Postgres(sqlx::PgConnection::connect(url).await?)
            } else if url.starts_with("mysql:") {
                Self::Mysql(sqlx::MySqlConnection::connect(url).await?)
            } else if url.starts_with("sqlite:") {
                Self::Sqlite(sqlx::SqliteConnection::connect(url).await?)
            } else {
                bail!("Unsupported URL scheme")
            },
        )
    }
    pub async fn execute(&mut self, sql: &str) -> Result<QueryResult> {
        if sql.trim().is_empty() {
            bail!("Write a query first");
        }
        let start = Instant::now();
        let mut result = match self {
            Self::Postgres(c) => execute!(
                c,
                sql,
                chrono::NaiveDateTime,
                chrono::DateTime<chrono::Utc>,
                chrono::NaiveDate,
                chrono::NaiveTime,
                uuid::Uuid,
                serde_json::Value,
                rust_decimal::Decimal
            ),
            Self::Mysql(c) => execute!(
                c,
                sql,
                u64,
                u32,
                u16,
                u8,
                chrono::NaiveDateTime,
                chrono::NaiveDate,
                chrono::NaiveTime,
                serde_json::Value,
                rust_decimal::Decimal
            ),
            Self::Sqlite(c) => execute!(
                c,
                sql,
                chrono::NaiveDateTime,
                chrono::NaiveDate,
                chrono::NaiveTime
            ),
        };
        result.elapsed_ms = start.elapsed().as_millis();
        Ok(result)
    }
    pub async fn tables(&mut self) -> Result<Vec<TableName>> {
        let sql = match self {
            Self::Postgres(_) => {
                "SELECT table_schema, table_name FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY table_schema, table_name"
            }
            Self::Mysql(_) => {
                "SELECT table_schema, table_name FROM information_schema.tables WHERE table_schema = DATABASE() ORDER BY table_name"
            }
            Self::Sqlite(_) => {
                "SELECT '' AS table_schema, name AS table_name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name"
            }
        };
        let result = self.execute(sql).await?;
        Ok(result
            .rows
            .into_iter()
            .map(|r| TableName {
                schema: r[0].clone(),
                name: r[1].clone(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn sqlite_full_session_and_multi_statement() {
        let mut db = Database::connect("sqlite::memory:").await.unwrap();
        let r = db.execute("CREATE TABLE \"a b\" (id INTEGER, title TEXT, data BLOB); INSERT INTO \"a b\" VALUES (42, 'hello 🌍', X'00FF'); SELECT *, NULL AS missing FROM \"a b\";").await.unwrap();
        assert_eq!(r.rows[0], ["42", "hello 🌍", "0x00ff", "NULL"]);
        assert_eq!(r.columns, ["id", "title", "data", "missing"]);
        assert_eq!(r.statements, 3);
        let tables = db.tables().await.unwrap();
        assert_eq!(tables[0].name, "a b");
        assert!(db.execute(&tables[0].preview_sql(false)).await.is_ok());
        assert!(db.execute("SELECT invalid FROM missing").await.is_err());
        assert!(db.execute("SELECT 1").await.is_ok());
    }
    #[tokio::test]
    async fn result_memory_is_bounded() {
        let mut db = Database::connect("sqlite::memory:").await.unwrap();
        let r = db.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10005) SELECT x FROM n").await.unwrap();
        assert_eq!(r.rows.len(), ROW_LIMIT);
        assert!(r.truncated);
    }
    #[test]
    fn identifiers_are_quoted() {
        let t = TableName {
            schema: "a\"b".into(),
            name: "x`y".into(),
        };
        assert_eq!(
            t.preview_sql(false),
            "SELECT * FROM \"a\"\"b\".\"x`y\" LIMIT 1000;"
        );
        assert_eq!(
            t.preview_sql(true),
            "SELECT * FROM `a\"b`.`x``y` LIMIT 1000;"
        );
    }
    #[tokio::test]
    async fn empty_select_preserves_column_headers() {
        let mut db = Database::connect("sqlite::memory:").await.unwrap();
        let r = db
            .execute("SELECT 1 AS id, 'value' AS name WHERE 0")
            .await
            .unwrap();
        assert!(r.rows.is_empty());
        assert_eq!(r.columns, ["id", "name"]);
    }
    #[tokio::test]
    #[ignore = "requires DBRUST_TEST_POSTGRES_URL"]
    async fn postgres_session_when_configured() {
        let url = std::env::var("DBRUST_TEST_POSTGRES_URL").expect("Set DBRUST_TEST_POSTGRES_URL");
        let mut db = Database::connect(&url).await.unwrap();
        let r=db.execute("SELECT 42::bigint AS id, true AS enabled, 1.25::numeric AS amount, '{\"a\":1}'::jsonb AS doc, '2026-11-10'::date AS day, NULL::text AS missing").await.unwrap();
        assert_eq!(
            r.rows[0],
            ["42", "true", "1.25", "{\"a\":1}", "2026-11-10", "NULL"]
        );
        let r=db.execute("CREATE TEMP TABLE dbrust_test (id integer); INSERT INTO dbrust_test VALUES (7); SELECT * FROM dbrust_test;").await.unwrap();
        assert_eq!(r.rows[0][0], "7");
        assert!(db.execute("SELECT broken FROM nowhere").await.is_err());
        assert_eq!(db.execute("SELECT 1 AS ok").await.unwrap().rows[0][0], "1");
        assert!(db.tables().await.is_ok());
        db.execute("BEGIN").await.unwrap();
        db.execute("INSERT INTO dbrust_test VALUES (8); INSERT INTO dbrust_test VALUES (9);")
            .await
            .unwrap();
        db.execute("COMMIT").await.unwrap();
        assert_eq!(
            db.execute("SELECT count(*) FROM dbrust_test")
                .await
                .unwrap()
                .rows[0][0],
            "3"
        );
    }
    #[tokio::test]
    #[ignore = "requires DBRUST_TEST_MYSQL_URL"]
    async fn mysql_session_when_configured() {
        let url = std::env::var("DBRUST_TEST_MYSQL_URL").expect("Set DBRUST_TEST_MYSQL_URL");
        let mut db = Database::connect(&url).await.unwrap();
        let r = db
            .execute("SELECT 42 AS id, CAST(1.25 AS DECIMAL(5,2)) AS amount, NULL AS missing")
            .await
            .unwrap();
        assert_eq!(r.rows[0], ["42", "1.25", "NULL"]);
        assert!(db.tables().await.is_ok());
    }
}
