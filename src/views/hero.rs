//! Hero v2 DTOs -- translation of template-fastapi's `views/hero_v2.py`.
//! Validation rules (FR-0003): `name` 1-200 chars; `powers` a list of at
//! least one string, each 1-200 chars; `power_level` unconstrained
//! (nullable integer).
//!
//! Deviation from the Python original, noted here rather than silently:
//! `HeroV2Update` here treats an omitted field as "leave unchanged" the
//! same way (FR-0004), but does not support explicitly nulling out a
//! previously-set field via PATCH (Pydantic's `exclude_unset` can tell
//! "field omitted" from "field explicitly null"; a plain `Option<T>` in
//! serde cannot without a double-`Option` deserializer this port doesn't
//! pull in for phase 2). `Draftable`/`Schedulable`/`Lockable` fields
//! (`is_draft`, `publish_at`, `unpublish_at`, `is_locked`) are out of
//! scope per this phase's plan and have no equivalent here.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::models::hero;
use crate::views::FieldError;

const MAX_LEN: usize = 200;

fn validate_text_field(field: &'static str, value: &str, errors: &mut Vec<FieldError>) {
    let len = value.chars().count();
    if !(1..=MAX_LEN).contains(&len) {
        errors.push(FieldError::new(
            field,
            format!("must be 1-{MAX_LEN} characters"),
        ));
    }
}

fn validate_powers(powers: &[String], errors: &mut Vec<FieldError>) {
    if powers.is_empty() {
        errors.push(FieldError::new("powers", "must contain at least one power"));
        return;
    }
    for (index, power) in powers.iter().enumerate() {
        let len = power.chars().count();
        if !(1..=MAX_LEN).contains(&len) {
            errors.push(FieldError::new(
                "powers",
                format!("powers[{index}] must be 1-{MAX_LEN} characters"),
            ));
        }
    }
}

/// `POST /crud/v1/heroes/v2/json` request body.
#[derive(Debug, Deserialize)]
pub struct HeroCreate {
    pub name: String,
    pub powers: Vec<String>,
    pub power_level: Option<i32>,
}

impl HeroCreate {
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();
        validate_text_field("name", &self.name, &mut errors);
        validate_powers(&self.powers, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// `PATCH /crud/v1/heroes/v2/json?id=` request body -- every field
/// optional; an omitted field is left unchanged (FR-0004).
#[derive(Debug, Deserialize, Default)]
pub struct HeroUpdate {
    pub name: Option<String>,
    pub powers: Option<Vec<String>>,
    pub power_level: Option<i32>,
}

impl HeroUpdate {
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();
        if let Some(name) = &self.name {
            validate_text_field("name", name, &mut errors);
        }
        if let Some(powers) = &self.powers {
            validate_powers(powers, &mut errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// The read/response shape -- `GET`/`POST`/`PATCH` all return this.
#[derive(Debug, Serialize)]
pub struct HeroRead {
    pub id: i32,
    pub name: Option<String>,
    pub powers: Option<Vec<String>>,
    pub power_level: Option<i32>,
    pub owner_id: String,
    pub archived_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl From<hero::Model> for HeroRead {
    fn from(model: hero::Model) -> Self {
        Self {
            id: model.id,
            name: model.name,
            powers: model.powers,
            power_level: model.power_level,
            owner_id: model.owner_id,
            archived_at: model.archived_at,
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

/// `GET` list-query parameters.
#[derive(Debug, Deserialize)]
pub struct HeroListQuery {
    #[serde(default)]
    pub id: Option<i32>,
    #[serde(default)]
    pub skip: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub include_archived: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_create() -> HeroCreate {
        HeroCreate {
            name: "Spectra".to_string(),
            powers: vec!["flight".to_string()],
            power_level: Some(5),
        }
    }

    // -- HeroCreate::validate -- every FR-0003 rule, both directions.

    #[test]
    fn create_accepts_a_valid_payload() {
        assert!(valid_create().validate().is_ok());
    }

    #[test]
    fn create_accepts_power_level_none() {
        let mut hero = valid_create();
        hero.power_level = None;
        assert!(hero.validate().is_ok());
    }

    #[test]
    fn create_rejects_empty_name() {
        let mut hero = valid_create();
        hero.name = String::new();
        let errors = hero.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn create_rejects_name_over_200_chars() {
        let mut hero = valid_create();
        hero.name = "x".repeat(201);
        let errors = hero.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn create_accepts_name_at_boundary_lengths() {
        let mut hero = valid_create();
        hero.name = "x".to_string();
        assert!(hero.validate().is_ok());
        hero.name = "x".repeat(200);
        assert!(hero.validate().is_ok());
    }

    #[test]
    fn create_counts_name_length_in_chars_not_bytes() {
        // Each "é" here is 2 bytes but 1 char -- 200 chars must still pass.
        let mut hero = valid_create();
        hero.name = "é".repeat(200);
        assert!(hero.validate().is_ok());
        hero.name = "é".repeat(201);
        assert!(hero.validate().is_err());
    }

    #[test]
    fn create_rejects_empty_powers_list() {
        let mut hero = valid_create();
        hero.powers = vec![];
        let errors = hero.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "powers"));
    }

    #[test]
    fn create_rejects_a_too_long_power_entry() {
        let mut hero = valid_create();
        hero.powers = vec!["x".repeat(201)];
        let errors = hero.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "powers"));
    }

    #[test]
    fn create_rejects_an_empty_power_entry() {
        let mut hero = valid_create();
        hero.powers = vec![String::new()];
        let errors = hero.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "powers"));
    }

    #[test]
    fn create_accepts_multiple_valid_powers() {
        let mut hero = valid_create();
        hero.powers = vec!["flight".to_string(), "strength".to_string()];
        assert!(hero.validate().is_ok());
    }

    #[test]
    fn create_collects_multiple_errors_at_once() {
        let hero = HeroCreate {
            name: String::new(),
            powers: vec![],
            power_level: None,
        };
        let errors = hero.validate().unwrap_err();
        assert_eq!(errors.len(), 2);
    }

    // -- HeroUpdate::validate -- omitted fields are always valid (FR-0004);
    // present fields obey the same FR-0003 rules as create.

    #[test]
    fn update_with_no_fields_is_valid() {
        assert!(HeroUpdate::default().validate().is_ok());
    }

    #[test]
    fn update_validates_name_when_present() {
        let update = HeroUpdate {
            name: Some(String::new()),
            ..Default::default()
        };
        let errors = update.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn update_validates_powers_when_present() {
        let update = HeroUpdate {
            powers: Some(vec![]),
            ..Default::default()
        };
        let errors = update.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "powers"));
    }

    #[test]
    fn update_leaves_power_level_unconstrained() {
        let update = HeroUpdate {
            power_level: Some(-999),
            ..Default::default()
        };
        assert!(update.validate().is_ok());
    }

    #[test]
    fn update_accepts_only_name_changing() {
        let update = HeroUpdate {
            name: Some("New Name".to_string()),
            ..Default::default()
        };
        assert!(update.validate().is_ok());
    }

    // -- HeroRead::from -- the ORM model -> response DTO mapping.

    #[test]
    fn hero_read_from_model_copies_every_field() {
        let now = chrono::Utc::now().naive_utc();
        let model = hero::Model {
            id: 42,
            name: Some("Spectra".to_string()),
            powers: Some(vec!["flight".to_string()]),
            power_level: Some(7),
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let read = HeroRead::from(model);
        assert_eq!(read.id, 42);
        assert_eq!(read.name.as_deref(), Some("Spectra"));
        assert_eq!(read.owner_id, "alice");
        assert_eq!(read.power_level, Some(7));
        assert!(read.archived_at.is_none());
    }
}
