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
