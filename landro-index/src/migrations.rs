//! Database migration system for schema upgrades and downgrades
//!
//! This module provides a robust migration system that can handle schema evolution
//! over time while preserving data integrity and supporting rollbacks.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use tracing::{info, warn};

use crate::errors::{IndexError, Result};

/// A single database migration
pub struct Migration {
    /// Migration version number
    pub version: u32,
    /// Description of what this migration does
    pub description: String,
    /// SQL statements to apply the migration
    pub up: Vec<String>,
    /// SQL statements to rollback the migration (optional)
    pub down: Option<Vec<String>>,
}

impl Migration {
    /// Create a new migration
    pub fn new(
        version: u32,
        description: impl Into<String>,
        up: Vec<String>,
        down: Option<Vec<String>>,
    ) -> Self {
        Self {
            version,
            description: description.into(),
            up,
            down,
        }
    }

    /// Apply this migration to a database connection
    pub fn apply(&self, conn: &mut Connection) -> Result<()> {
        info!("Applying migration {}: {}", self.version, self.description);

        let tx = conn.transaction().map_err(IndexError::Database)?;

        // Execute all migration statements
        for statement in &self.up {
            tx.execute_batch(statement).map_err(IndexError::Database)?;
        }

        // Record that this migration has been applied
        tx.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            params![self.version],
        )
        .map_err(IndexError::Database)?;

        tx.commit().map_err(IndexError::Database)?;

        info!("Migration {} applied successfully", self.version);
        Ok(())
    }

    /// Rollback this migration from a database connection
    pub fn rollback(&self, conn: &mut Connection) -> Result<()> {
        let down_statements = self.down.as_ref().ok_or_else(|| {
            IndexError::DatabaseError(format!(
                "Migration {} does not support rollback",
                self.version
            ))
        })?;

        info!(
            "Rolling back migration {}: {}",
            self.version, self.description
        );

        let tx = conn.transaction().map_err(IndexError::Database)?;

        // Execute all rollback statements in reverse order
        for statement in down_statements.iter().rev() {
            tx.execute_batch(statement).map_err(IndexError::Database)?;
        }

        // Remove this migration from the version table
        tx.execute(
            "DELETE FROM schema_version WHERE version = ?1",
            params![self.version],
        )
        .map_err(IndexError::Database)?;

        tx.commit().map_err(IndexError::Database)?;

        info!("Migration {} rolled back successfully", self.version);
        Ok(())
    }
}

/// Database migration manager
pub struct MigrationManager {
    migrations: HashMap<u32, Migration>,
    target_version: u32,
}

impl MigrationManager {
    /// Create a new migration manager
    pub fn new(target_version: u32) -> Self {
        let mut manager = Self {
            migrations: HashMap::new(),
            target_version,
        };

        // Register all migrations
        manager.register_migrations();
        manager
    }

    /// Register all available migrations
    fn register_migrations(&mut self) {
        // Migration 1: Initial schema (this is handled by the schema.rs directly)
        // Future migrations would be added here

        // Example migration 2: Add new index (this would be for future schema changes)
        /*
        self.add_migration(Migration::new(
            2,
            "Add performance index on files.size",
            vec![
                "CREATE INDEX IF NOT EXISTS idx_files_size_desc ON files(size DESC)".to_string(),
            ],
            Some(vec![
                "DROP INDEX IF EXISTS idx_files_size_desc".to_string(),
            ]),
        ));
        */
    }

    /// Add a migration to the manager
    pub fn add_migration(&mut self, migration: Migration) {
        let version = migration.version;
        if self.migrations.insert(version, migration).is_some() {
            warn!("Replacing existing migration for version {}", version);
        }
    }

    /// Get the current database schema version
    pub fn get_current_version(&self, conn: &Connection) -> Result<u32> {
        let version = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get::<_, Option<u32>>(0)
            })
            .map_err(IndexError::Database)?
            .unwrap_or(0);

        Ok(version)
    }

    /// Check if database needs migration
    pub fn needs_migration(&self, conn: &Connection) -> Result<bool> {
        let current_version = self.get_current_version(conn)?;
        Ok(current_version != self.target_version)
    }

    /// Migrate database to target version
    pub fn migrate(&self, conn: &mut Connection) -> Result<()> {
        let current_version = self.get_current_version(conn)?;

        if current_version == self.target_version {
            info!(
                "Database is already at target version {}",
                self.target_version
            );
            return Ok(());
        }

        if current_version > self.target_version {
            return self.migrate_down(conn, current_version, self.target_version);
        } else {
            return self.migrate_up(conn, current_version, self.target_version);
        }
    }

    /// Migrate database up to a specific version
    fn migrate_up(&self, conn: &mut Connection, from: u32, to: u32) -> Result<()> {
        info!("Migrating database from version {} to {}", from, to);

        let mut versions: Vec<u32> = self
            .migrations
            .keys()
            .filter(|&&v| v > from && v <= to)
            .cloned()
            .collect();

        versions.sort();

        for version in versions {
            if let Some(migration) = self.migrations.get(&version) {
                migration.apply(conn)?;
            } else {
                return Err(IndexError::DatabaseError(format!(
                    "Missing migration for version {}",
                    version
                )));
            }
        }

        info!("Successfully migrated to version {}", to);
        Ok(())
    }

    /// Migrate database down to a specific version
    fn migrate_down(&self, conn: &mut Connection, from: u32, to: u32) -> Result<()> {
        info!("Rolling back database from version {} to {}", from, to);

        let mut versions: Vec<u32> = self
            .migrations
            .keys()
            .filter(|&&v| v <= from && v > to)
            .cloned()
            .collect();

        // Sort in descending order for rollback
        versions.sort_by(|a, b| b.cmp(a));

        for version in versions {
            if let Some(migration) = self.migrations.get(&version) {
                migration.rollback(conn)?;
            } else {
                return Err(IndexError::DatabaseError(format!(
                    "Missing migration for version {}",
                    version
                )));
            }
        }

        info!("Successfully rolled back to version {}", to);
        Ok(())
    }

    /// Get list of pending migrations
    pub fn get_pending_migrations(&self, conn: &Connection) -> Result<Vec<u32>> {
        let current_version = self.get_current_version(conn)?;

        let mut pending: Vec<u32> = self
            .migrations
            .keys()
            .filter(|&&v| v > current_version && v <= self.target_version)
            .cloned()
            .collect();

        pending.sort();
        Ok(pending)
    }

    /// Validate migration consistency
    pub fn validate(&self) -> Result<()> {
        // Check for version gaps
        let mut versions: Vec<u32> = self.migrations.keys().cloned().collect();
        versions.sort();

        let mut expected_version = 1;
        for version in versions {
            if version != expected_version {
                return Err(IndexError::DatabaseError(format!(
                    "Migration version gap: expected {}, found {}",
                    expected_version, version
                )));
            }
            expected_version += 1;
        }

        // Ensure target version exists if we have migrations
        if !self.migrations.is_empty() && !self.migrations.contains_key(&self.target_version) {
            return Err(IndexError::DatabaseError(format!(
                "Target version {} not found in migrations",
                self.target_version
            )));
        }

        Ok(())
    }
}

/// Helper to run migrations during database initialization
pub fn run_migrations(conn: &mut Connection, target_version: u32) -> Result<()> {
    let manager = MigrationManager::new(target_version);

    // Validate migration consistency
    manager.validate()?;

    // Run migrations if needed
    if manager.needs_migration(conn)? {
        manager.migrate(conn)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn create_test_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();

        // Create schema_version table for testing
        conn.execute(
            "CREATE TABLE schema_version (
                version INTEGER PRIMARY KEY,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .unwrap();

        conn
    }

    #[test]
    fn test_migration_creation() {
        let migration = Migration::new(
            1,
            "Test migration",
            vec!["CREATE TABLE test (id INTEGER)".to_string()],
            Some(vec!["DROP TABLE test".to_string()]),
        );

        assert_eq!(migration.version, 1);
        assert_eq!(migration.description, "Test migration");
        assert_eq!(migration.up.len(), 1);
        assert!(migration.down.is_some());
    }

    #[test]
    fn test_migration_apply() {
        let mut conn = create_test_connection();

        let migration = Migration::new(
            1,
            "Create test table",
            vec!["CREATE TABLE test (id INTEGER PRIMARY KEY)".to_string()],
            None,
        );

        migration.apply(&mut conn).unwrap();

        // Check that table was created
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Check that version was recorded
        let version: u32 = conn
            .query_row(
                "SELECT version FROM schema_version WHERE version = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_migration_rollback() {
        let mut conn = create_test_connection();

        let migration = Migration::new(
            1,
            "Create test table",
            vec!["CREATE TABLE test (id INTEGER PRIMARY KEY)".to_string()],
            Some(vec!["DROP TABLE test".to_string()]),
        );

        // Apply then rollback
        migration.apply(&mut conn).unwrap();
        migration.rollback(&mut conn).unwrap();

        // Check that table was dropped
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);

        // Check that version was removed
        let result = conn.query_row(
            "SELECT version FROM schema_version WHERE version = 1",
            [],
            |row| row.get::<_, u32>(0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_migration_manager() {
        let mut manager = MigrationManager::new(2);

        manager.add_migration(Migration::new(
            1,
            "First migration",
            vec!["CREATE TABLE users (id INTEGER)".to_string()],
            Some(vec!["DROP TABLE users".to_string()]),
        ));

        manager.add_migration(Migration::new(
            2,
            "Second migration",
            vec!["ALTER TABLE users ADD COLUMN name TEXT".to_string()],
            Some(vec!["ALTER TABLE users DROP COLUMN name".to_string()]),
        ));

        let mut conn = create_test_connection();

        // Should need migration
        assert!(manager.needs_migration(&conn).unwrap());

        // Check pending migrations
        let pending = manager.get_pending_migrations(&conn).unwrap();
        assert_eq!(pending, vec![1, 2]);

        // Run migrations
        manager.migrate(&mut conn).unwrap();

        // Should no longer need migration
        assert!(!manager.needs_migration(&conn).unwrap());

        // Check current version
        assert_eq!(manager.get_current_version(&conn).unwrap(), 2);
    }

    #[test]
    fn test_migration_validation() {
        let mut manager = MigrationManager::new(2);

        // Add migration with gap (missing version 1)
        manager.add_migration(Migration::new(
            2,
            "Second migration",
            vec!["CREATE TABLE test (id INTEGER)".to_string()],
            None,
        ));

        // Should fail validation
        assert!(manager.validate().is_err());
    }
}
