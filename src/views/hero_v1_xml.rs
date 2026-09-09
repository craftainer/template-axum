//! `HeroReadV1Xml` -- the Hero v1 compat router's XML sibling response
//! shape (`docs/adrs/0014`'s pattern, extended to v1 by `docs/adrs/0017`
//! and FR-0032/`FR-0027`). Field-for-field mirror of `views::hero_v1::
//! HeroReadV1`, same reasoning as `views::hero_xml::HeroReadXml`: a
//! `#[serde(skip_serializing_if = ...)]` attribute is attached to the
//! type, so omitting `None` fields from XML without changing v1's JSON
//! response needs its own type. `HeroReadV1Xml::from(hero::Model)` is the
//! only logic here.

use chrono::NaiveDateTime;
use serde::Serialize;

use crate::models::hero;

#[derive(Debug, Serialize)]
pub struct HeroReadV1Xml {
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superpower: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_level: Option<i32>,
    pub owner_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl From<hero::Model> for HeroReadV1Xml {
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

    #[test]
    fn from_model_takes_the_first_power_as_superpower() {
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
        let xml = HeroReadV1Xml::from(model);
        assert_eq!(xml.superpower.as_deref(), Some("flight"));
    }

    #[test]
    fn serializes_with_none_fields_omitted() {
        let now = chrono::Utc::now().naive_utc();
        let xml = HeroReadV1Xml {
            id: 1,
            name: None,
            superpower: None,
            power_level: None,
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let rendered = quick_xml::se::to_string_with_root("hero", &xml).unwrap();
        assert!(!rendered.contains("<name>"));
        assert!(!rendered.contains("<superpower>"));
        assert!(rendered.contains("<owner_id>alice</owner_id>"));
    }
}
