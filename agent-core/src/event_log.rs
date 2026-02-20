use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EventType {
    FactAsserted,
    FactRetracted,
    FactSuggested,
    FactSuggestionResolved,
    PlanSelected,
    ActionIntent,
    ActionResult,
    Escalated,
    EscalationResponded,
    Resolved,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: Option<i64>,
    pub incident_id: String,
    pub event_type: EventType,
    pub description: String,
    pub details: Option<serde_json::Value>,
    pub timestamp: String,
}

#[derive(Clone)]
pub struct EventLog {
    db_path: Arc<PathBuf>,
}

impl EventLog {
    pub fn open(path: &str) -> Result<Self, String> {
        let db_path = PathBuf::from(path);
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }

        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                incident_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                description TEXT NOT NULL,
                details TEXT,
                timestamp TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_incident ON events(incident_id);
            CREATE INDEX IF NOT EXISTS idx_events_ts ON events(timestamp);
            ",
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            db_path: Arc::new(db_path),
        })
    }

    pub fn append(&self, event: &Event) -> Result<i64, String> {
        let conn = Connection::open(&*self.db_path).map_err(|e| e.to_string())?;
        let event_type = serde_json::to_string(&event.event_type).map_err(|e| e.to_string())?;
        let details = event
            .details
            .as_ref()
            .map(|d| serde_json::to_string(d).map_err(|e| e.to_string()))
            .transpose()?;

        conn.execute(
            "INSERT INTO events (incident_id, event_type, description, details, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.incident_id,
                event_type,
                event.description,
                details,
                event.timestamp,
            ],
        )
        .map_err(|e| e.to_string())?;

        Ok(conn.last_insert_rowid())
    }

    pub fn events_for_incident(&self, incident_id: &str) -> Result<Vec<Event>, String> {
        let conn = Connection::open(&*self.db_path).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, incident_id, event_type, description, details, timestamp
                 FROM events
                 WHERE incident_id = ?1
                 ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![incident_id], map_row)
            .map_err(|e| e.to_string())?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|e| e.to_string())?);
        }
        Ok(events)
    }

    pub fn events_after(&self, after_id: i64) -> Result<Vec<Event>, String> {
        let conn = Connection::open(&*self.db_path).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, incident_id, event_type, description, details, timestamp
                 FROM events
                 WHERE id > ?1
                 ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![after_id], map_row)
            .map_err(|e| e.to_string())?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|e| e.to_string())?);
        }
        Ok(events)
    }

    pub fn active_incidents(&self) -> Result<Vec<String>, String> {
        let conn = Connection::open(&*self.db_path).map_err(|e| e.to_string())?;
        let mut all = BTreeSet::new();
        let mut resolved = BTreeSet::new();

        let mut stmt = conn
            .prepare("SELECT DISTINCT incident_id FROM events")
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for id in ids {
            all.insert(id.map_err(|e| e.to_string())?);
        }

        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT incident_id FROM events
                 WHERE event_type = ?1",
            )
            .map_err(|e| e.to_string())?;
        let resolved_type = serde_json::to_string(&EventType::Resolved).map_err(|e| e.to_string())?;
        let resolved_ids = stmt
            .query_map(params![resolved_type], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;

        for id in resolved_ids {
            resolved.insert(id.map_err(|e| e.to_string())?);
        }

        Ok(all.difference(&resolved).cloned().collect())
    }

    pub fn all_incidents(&self) -> Result<Vec<String>, String> {
        let conn = Connection::open(&*self.db_path).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT incident_id
                 FROM events
                 GROUP BY incident_id
                 ORDER BY MAX(id) DESC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn latest_event_id(&self) -> Result<Option<i64>, String> {
        let conn = Connection::open(&*self.db_path).map_err(|e| e.to_string())?;
        conn.query_row("SELECT MAX(id) FROM events", [], |row| row.get::<_, Option<i64>>(0))
            .optional()
            .map_err(|e| e.to_string())
            .map(|v| v.flatten())
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let event_type_str: String = row.get(2)?;
    let details_str: Option<String> = row.get(4)?;

    let event_type: EventType = serde_json::from_str(&event_type_str).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })?;

    let details = details_str
        .map(|s| {
            serde_json::from_str(&s).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
            })
        })
        .transpose()?;

    Ok(Event {
        id: row.get(0)?,
        incident_id: row.get(1)?,
        event_type,
        description: row.get(3)?,
        details,
        timestamp: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_path(name: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        format!("/tmp/rig-bdi-tests/{name}-{nanos}.db")
    }

    #[test]
    fn append_and_query_roundtrip() {
        let log = EventLog::open(&db_path("roundtrip")).expect("open");
        let id = log
            .append(&Event {
                id: None,
                incident_id: "inc-a".into(),
                event_type: EventType::FactAsserted,
                description: "fact".into(),
                details: Some(serde_json::json!({"k": "v"})),
                timestamp: "1".into(),
            })
            .expect("append");

        assert!(id > 0);

        let events = log.events_for_incident("inc-a").expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].incident_id, "inc-a");
        assert!(matches!(events[0].event_type, EventType::FactAsserted));
        assert_eq!(events[0].details, Some(serde_json::json!({"k": "v"})));
    }

    #[test]
    fn events_after_tracks_incremental_stream() {
        let log = EventLog::open(&db_path("events-after")).expect("open");
        let a = log
            .append(&Event {
                id: None,
                incident_id: "inc-a".into(),
                event_type: EventType::FactAsserted,
                description: "fact".into(),
                details: None,
                timestamp: "1".into(),
            })
            .expect("append a");
        let b = log
            .append(&Event {
                id: None,
                incident_id: "inc-a".into(),
                event_type: EventType::Resolved,
                description: "resolved".into(),
                details: None,
                timestamp: "2".into(),
            })
            .expect("append b");

        let events = log.events_after(a).expect("events after");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, Some(b));
    }

    #[test]
    fn active_incidents_excludes_resolved() {
        let log = EventLog::open(&db_path("active")).expect("open");
        for event in [
            Event {
                id: None,
                incident_id: "inc-1".into(),
                event_type: EventType::FactAsserted,
                description: "fact".into(),
                details: None,
                timestamp: "1".into(),
            },
            Event {
                id: None,
                incident_id: "inc-1".into(),
                event_type: EventType::Resolved,
                description: "resolved".into(),
                details: None,
                timestamp: "2".into(),
            },
            Event {
                id: None,
                incident_id: "inc-2".into(),
                event_type: EventType::FactAsserted,
                description: "fact".into(),
                details: None,
                timestamp: "3".into(),
            },
        ] {
            log.append(&event).expect("append");
        }

        let active = log.active_incidents().expect("active");
        assert_eq!(active, vec!["inc-2".to_string()]);
    }
}
