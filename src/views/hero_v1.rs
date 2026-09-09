//! Hero v1 DTOs -- the deprecated-compat sibling of `views::hero`'s v2
//! shape (`docs/adrs/0017`, FR-0031/FR-0032). v1 has a single
//! `superpower: String` field where v2 has `powers: Vec<String>`; every
//! other field (`name`, `power_level`) is identical. There is no separate
//! storage for v1 -- it's a lossy view onto the exact same
//! `HeroSeaOrmRepository`/`HeroMemoryRepository` v2 data `controllers::
//! heroes` already serves, converted at the boundary:
//!
//! - v2 -> v1 (read): `superpower` is `powers[0]` -- lossy when a record
//!   has more than one power (deliberate; v1 never had a way to represent
//!   more than one), and `None` when `powers` is empty/`None`.
//! - v1 -> v2 (create): `superpower` wraps into a single-element
//!   `powers` list.
//! - v1 -> v2 (update): `superpower` maps to `powers` only when the
//!   client actually supplied it (`Some`) -- an omitted `superpower`
//!   leaves the record's existing `powers` untouched (FR-0004's own
//!   "omitted field is left unchanged" rule, preserved across the
//!   conversion).

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::models::hero;
use crate::views::hero::{HeroCreate, HeroUpdate};
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

/// `POST /crud/v1/heroes/v1/json` request body.
#[derive(Debug, Deserialize)]
pub struct HeroCreateV1 {
    pub name: String,
    pub superpower: String,
    #[serde(default)]
    pub power_level: Option<i32>,
}

impl HeroCreateV1 {
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();
        validate_text_field("name", &self.name, &mut errors);
        validate_text_field("superpower", &self.superpower, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl From<HeroCreateV1> for HeroCreate {
    fn from(v1: HeroCreateV1) -> Self {
        HeroCreate {
            name: v1.name,
            powers: vec![v1.superpower],
            power_level: v1.power_level,
        }
    }
}

/// `PATCH /crud/v1/heroes/v1/json?id=` request body -- every field
/// optional, same "omitted means unchanged" rule as v2's `HeroUpdate`.
#[derive(Debug, Deserialize, Default)]
pub struct HeroUpdateV1 {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub superpower: Option<String>,
    #[serde(default)]
    pub power_level: Option<i32>,
}

impl HeroUpdateV1 {
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();
        if let Some(name) = &self.name {
            validate_text_field("name", name, &mut errors);
        }
        if let Some(superpower) = &self.superpower {
            validate_text_field("superpower", superpower, &mut errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl From<HeroUpdateV1> for HeroUpdate {
    fn from(v1: HeroUpdateV1) -> Self {
        HeroUpdate {
            name: v1.name,
            powers: v1.superpower.map(|superpower| vec![superpower]),
            power_level: v1.power_level,
        }
    }
}

/// The v1 read/response shape -- lossy on `powers.len() > 1` (see module
/// doc).
#[derive(Debug, Serialize)]
pub struct HeroReadV1 {
    pub id: i32,
    pub name: Option<String>,
    pub superpower: Option<String>,
    pub power_level: Option<i32>,
    pub owner_id: String,
    pub archived_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl From<hero::Model> for HeroReadV1 {
    fn from(model: hero::Model) -> Self {
        Self {
            id: model.id,
            name: model.name,
            superpower: model.powers.and_then(|powers| powers.into_iter().next()),
            power_level: model.power_level,
            owner_id: model.owner_id,
            archived_at: model.archived_at,
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_create() -> HeroCreateV1 {
        HeroCreateV1 {
            name: "Spectra".to_string(),
            superpower: "flight".to_string(),
            power_level: Some(5),
        }
    }

    #[test]
    fn create_accepts_a_valid_payload() {
        assert!(valid_create().validate().is_ok());
    }

    #[test]
    fn create_rejects_empty_superpower() {
        let mut hero = valid_create();
        hero.superpower = String::new();
        let errors = hero.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "superpower"));
    }

    #[test]
    fn create_into_hero_create_wraps_superpower_into_a_single_element_powers_list() {
        let v2: HeroCreate = valid_create().into();
        assert_eq!(v2.powers, vec!["flight".to_string()]);
        assert_eq!(v2.name, "Spectra");
        assert_eq!(v2.power_level, Some(5));
    }

    #[test]
    fn update_with_no_fields_is_valid() {
        assert!(HeroUpdateV1::default().validate().is_ok());
    }

    #[test]
    fn update_into_hero_update_maps_superpower_to_powers_when_present() {
        let v1 = HeroUpdateV1 {
            superpower: Some("stealth".to_string()),
            ..Default::default()
        };
        let v2: HeroUpdate = v1.into();
        assert_eq!(v2.powers, Some(vec!["stealth".to_string()]));
    }

    #[test]
    fn update_into_hero_update_leaves_powers_unset_when_superpower_is_omitted() {
        let v1 = HeroUpdateV1 {
            name: Some("Renamed".to_string()),
            ..Default::default()
        };
        let v2: HeroUpdate = v1.into();
        assert_eq!(
            v2.powers, None,
            "an omitted superpower must not clobber existing powers"
        );
    }

    #[test]
    fn read_from_model_takes_the_first_power_as_superpower() {
        let now = chrono::Utc::now().naive_utc();
        let model = hero::Model {
            id: 1,
            name: Some("Spectra".to_string()),
            powers: Some(vec!["flight".to_string(), "strength".to_string()]),
            power_level: Some(5),
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let v1 = HeroReadV1::from(model);
        assert_eq!(v1.superpower.as_deref(), Some("flight"));
    }

    #[test]
    fn read_from_model_with_no_powers_has_no_superpower() {
        let now = chrono::Utc::now().naive_utc();
        let model = hero::Model {
            id: 1,
            name: Some("Spectra".to_string()),
            powers: None,
            power_level: None,
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let v1 = HeroReadV1::from(model);
        assert!(v1.superpower.is_none());
    }
}
