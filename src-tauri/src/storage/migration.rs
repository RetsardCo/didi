use serde_json::Value;

use crate::storage::errors::StorageError;
use crate::storage::schema::{AppConfigV1, CURRENT_SCHEMA_VERSION};

/// Strictly inspects `schema_version` in a parsed JSON configuration value.
///
/// Returns `Ok(None)` only when `"schema_version"` is absent on a JSON object (legacy v0).
/// Rejects strings, floats, negatives, nulls, and out-of-range numbers as `InvalidSchemaVersion`
/// instead of coercing them to `0`.
pub fn inspect_schema_version(value: &Value) -> Result<Option<u32>, StorageError> {
    let Some(map) = value.as_object() else {
        return Err(StorageError::InvalidSchemaVersion);
    };

    match map.get("schema_version") {
        None => Ok(None),
        Some(Value::Number(num)) => {
            let Some(u) = num.as_u64() else {
                return Err(StorageError::InvalidSchemaVersion);
            };
            let ver = u32::try_from(u).map_err(|_| StorageError::InvalidSchemaVersion)?;
            Ok(Some(ver))
        }
        Some(_) => Err(StorageError::InvalidSchemaVersion),
    }
}

/// Inspects and migrates configuration JSON bytes into a validated `AppConfigV1`.
///
/// Refuses to overwrite or deserialize any configuration where `schema_version > CURRENT_SCHEMA_VERSION`
/// (ADR D-020, Section 4.4).
pub fn parse_and_migrate_config(raw_json: &[u8]) -> Result<AppConfigV1, StorageError> {
    let value: Value = serde_json::from_slice(raw_json)?;

    let schema_version: u32 = inspect_schema_version(&value)?.unwrap_or_default();

    if schema_version > CURRENT_SCHEMA_VERSION {
        return Err(StorageError::UnsupportedFutureSchema {
            found: schema_version,
            supported_max: CURRENT_SCHEMA_VERSION,
        });
    }

    if schema_version == CURRENT_SCHEMA_VERSION {
        let config: AppConfigV1 = serde_json::from_value(value)?;
        config.validate().map_err(StorageError::Validation)?;
        return Ok(config);
    }

    // Future schema migration hooks (schema_version < CURRENT_SCHEMA_VERSION)
    // E.g. migrate_v0_to_v1 if v0 config exists
    migrate_legacy_to_current(value, schema_version)
}

fn migrate_legacy_to_current(
    mut value: Value,
    found_version: u32,
) -> Result<AppConfigV1, StorageError> {
    if found_version == 0 {
        // Upgrade v0 (unversioned legacy draft) by injecting schema_version = 1
        if let Value::Object(ref mut map) = value {
            map.insert(
                "schema_version".to_owned(),
                Value::Number(CURRENT_SCHEMA_VERSION.into()),
            );
        }
        let config: AppConfigV1 = serde_json::from_value(value)?;
        config.validate().map_err(StorageError::Validation)?;
        return Ok(config);
    }

    Err(StorageError::UnsupportedFutureSchema {
        found: found_version,
        supported_max: CURRENT_SCHEMA_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_v1_config() -> Result<(), Box<dyn std::error::Error>> {
        let config = AppConfigV1::default_config();
        let bytes = serde_json::to_vec_pretty(&config)?;
        let parsed = parse_and_migrate_config(&bytes)?;
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.active_profile_id, "default");
        Ok(())
    }

    #[test]
    fn rejects_unsupported_future_schema_version() -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(AppConfigV1::default_config())?;
        value["schema_version"] = serde_json::json!(2);

        let bytes = serde_json::to_vec(&value)?;
        let result = parse_and_migrate_config(&bytes);
        match result {
            Err(StorageError::UnsupportedFutureSchema {
                found,
                supported_max,
            }) => {
                assert_eq!(found, 2);
                assert_eq!(supported_max, 1);
            }
            Ok(_) | Err(_) => {
                return Err("expected UnsupportedFutureSchema error".into());
            }
        }
        Ok(())
    }

    #[test]
    fn migrates_legacy_v0_config() -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(AppConfigV1::default_config())?;
        if let Value::Object(ref mut map) = value {
            map.remove("schema_version");
        }

        let bytes = serde_json::to_vec(&value)?;
        let migrated = parse_and_migrate_config(&bytes)?;
        assert_eq!(migrated.schema_version, 1);
        Ok(())
    }

    #[test]
    fn rejects_malformed_string_float_and_negative_schema_versions(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for bad_version in [
            serde_json::json!("2"),
            serde_json::json!("1"),
            serde_json::json!(2.0),
            serde_json::json!(1.5),
            serde_json::json!(-1),
            serde_json::json!(null),
            serde_json::json!(true),
        ] {
            let mut value = serde_json::to_value(AppConfigV1::default_config())?;
            value["schema_version"] = bad_version.clone();
            let bytes = serde_json::to_vec(&value)?;
            let res = parse_and_migrate_config(&bytes);
            assert!(
                matches!(res, Err(StorageError::InvalidSchemaVersion)),
                "expected InvalidSchemaVersion for {bad_version}, got {res:?}"
            );
        }
        Ok(())
    }
}
