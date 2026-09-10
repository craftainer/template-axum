//! Hero form DTO -- the `application/x-www-form-urlencoded` shape
//! `controllers::heroes_web`'s no-JS `<form>` submits (FR-0034), converted
//! into the existing `views::hero::HeroCreate`/`HeroUpdate` so form
//! submissions run through exactly the same validation/CRUD path the JSON
//! API does (same reasoning as `views::hero_v1`'s conversion into v2).
//!
//! A plain HTML form has no way to submit a `Vec<String>` field, so
//! `powers` is one comma-separated text input, split/trimmed/filtered here
//! -- and no way to submit `Option<T>::None` distinctly from an empty
//! string, so an empty `power_level` field means "no power level", not
//! "leave unchanged" (a form always submits every field, so there is no
//! partial-update case to distinguish here the way JSON `PATCH` has).

use crate::generic::views::FieldError;
use crate::hero::views::hero::{HeroCreate, HeroUpdate};

#[derive(Debug, serde::Deserialize)]
pub struct HeroFormFields {
    #[serde(default)]
    pub name: String,
    /// Comma-separated powers, e.g. `"flight, strength"`.
    #[serde(default)]
    pub powers: String,
    /// Empty string means "no power level" -- parsed below, not by serde,
    /// so a non-numeric value becomes a normal field-level `FieldError`
    /// rather than a raw 400 from a failed `Option<i32>` deserialize.
    #[serde(default)]
    pub power_level: String,
}

fn parse_powers(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|power| !power.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_power_level(raw: &str) -> Result<Option<i32>, FieldError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<i32>()
        .map(Some)
        .map_err(|_| FieldError::new("power_level", "must be a whole number"))
}

impl HeroFormFields {
    pub fn into_hero_create(self) -> Result<HeroCreate, Vec<FieldError>> {
        let power_level = parse_power_level(&self.power_level);
        let mut field_errors = Vec::new();
        let power_level = match power_level {
            Ok(value) => value,
            Err(err) => {
                field_errors.push(err);
                None
            }
        };
        let create = HeroCreate {
            name: self.name,
            powers: parse_powers(&self.powers),
            power_level,
        };
        if let Err(mut errors) = create.validate() {
            field_errors.append(&mut errors);
        }
        if field_errors.is_empty() {
            Ok(create)
        } else {
            Err(field_errors)
        }
    }

    /// A form submission always carries every field, so this is a full
    /// replace, not a partial update -- every field maps to `Some(...)`.
    pub fn into_hero_update(self) -> Result<HeroUpdate, Vec<FieldError>> {
        let power_level = parse_power_level(&self.power_level);
        let mut field_errors = Vec::new();
        let power_level = match power_level {
            Ok(value) => value,
            Err(err) => {
                field_errors.push(err);
                None
            }
        };
        let update = HeroUpdate {
            name: Some(self.name),
            powers: Some(parse_powers(&self.powers)),
            power_level,
        };
        if let Err(mut errors) = update.validate() {
            field_errors.append(&mut errors);
        }
        if field_errors.is_empty() {
            Ok(update)
        } else {
            Err(field_errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_trims_and_drops_empty_powers() {
        let fields = HeroFormFields {
            name: "Spectra".to_string(),
            powers: " flight ,, strength".to_string(),
            power_level: "5".to_string(),
        };
        let create = fields.into_hero_create().unwrap();
        assert_eq!(
            create.powers,
            vec!["flight".to_string(), "strength".to_string()]
        );
        assert_eq!(create.power_level, Some(5));
    }

    #[test]
    fn empty_power_level_means_none() {
        let fields = HeroFormFields {
            name: "Spectra".to_string(),
            powers: "flight".to_string(),
            power_level: "".to_string(),
        };
        let create = fields.into_hero_create().unwrap();
        assert_eq!(create.power_level, None);
    }

    #[test]
    fn non_numeric_power_level_is_a_field_error() {
        let fields = HeroFormFields {
            name: "Spectra".to_string(),
            powers: "flight".to_string(),
            power_level: "not-a-number".to_string(),
        };
        let errors = fields.into_hero_create().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "power_level"));
    }

    #[test]
    fn empty_name_and_powers_collects_both_errors() {
        let fields = HeroFormFields {
            name: "".to_string(),
            powers: "".to_string(),
            power_level: "".to_string(),
        };
        let errors = fields.into_hero_create().unwrap_err();
        assert!(errors.iter().any(|e| e.field == "name"));
        assert!(errors.iter().any(|e| e.field == "powers"));
    }

    #[test]
    fn into_hero_update_replaces_every_field() {
        let fields = HeroFormFields {
            name: "Renamed".to_string(),
            powers: "stealth".to_string(),
            power_level: "9".to_string(),
        };
        let update = fields.into_hero_update().unwrap();
        assert_eq!(update.name, Some("Renamed".to_string()));
        assert_eq!(update.powers, Some(vec!["stealth".to_string()]));
        assert_eq!(update.power_level, Some(9));
    }

    #[test]
    fn into_hero_update_rejects_a_non_numeric_power_level() {
        let fields = HeroFormFields {
            name: "Renamed".to_string(),
            powers: "stealth".to_string(),
            power_level: "not-a-number".to_string(),
        };
        let errors = fields
            .into_hero_update()
            .expect_err("a non-numeric power level is invalid");
        assert!(errors.iter().any(|e| e.field == "power_level"));
    }

    #[test]
    fn into_hero_update_rejects_an_empty_name() {
        let fields = HeroFormFields {
            name: "".to_string(),
            powers: "stealth".to_string(),
            power_level: "5".to_string(),
        };
        let errors = fields
            .into_hero_update()
            .expect_err("an empty name is invalid");
        assert!(errors.iter().any(|e| e.field == "name"));
    }
}
