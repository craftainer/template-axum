//! `HeroReadXml` -- the XML sibling router's (`docs/adrs/0014`) response
//! shape. A thin mirror of `views::hero::HeroRead`, not a second copy of
//! any CRUD/validation/ownership logic: the only reason it exists
//! separately is that Rust's `serde` attributes are attached to the type
//! itself, so achieving `xml_codec.py`'s `to_xml`'s "omit a `None` field
//! entirely rather than rendering it" for XML *without* also changing
//! `HeroRead`'s existing JSON behavior (which renders `null` explicitly)
//! needs its own `#[serde(skip_serializing_if = ...)]` attributes on a
//! distinct type. `HeroReadXml::from(hero::Model)` is the only logic here
//! -- a field-for-field copy, immediately below `HeroRead::from`'s own.

use chrono::NaiveDateTime;
use serde::Serialize;

use crate::models::hero;

#[derive(Debug, Serialize)]
pub struct HeroReadXml {
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub powers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_level: Option<i32>,
    pub owner_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl From<hero::Model> for HeroReadXml {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_model_copies_every_field() {
        let now = chrono::Utc::now().naive_utc();
        let model = hero::Model {
            id: 1,
            name: Some("Spectra".to_string()),
            powers: Some(vec!["flight".to_string()]),
            power_level: Some(5),
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let xml = HeroReadXml::from(model);
        assert_eq!(xml.id, 1);
        assert_eq!(xml.name.as_deref(), Some("Spectra"));
        assert_eq!(xml.owner_id, "alice");
        assert!(xml.archived_at.is_none());
    }

    #[test]
    fn serializes_with_none_fields_omitted() {
        let now = chrono::Utc::now().naive_utc();
        let xml = HeroReadXml {
            id: 1,
            name: None,
            powers: None,
            power_level: None,
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let rendered = quick_xml::se::to_string_with_root("hero", &xml).unwrap();
        assert!(!rendered.contains("<name>"));
        assert!(!rendered.contains("<power_level>"));
        assert!(!rendered.contains("<archived_at>"));
        assert!(rendered.contains("<owner_id>alice</owner_id>"));
    }

    #[test]
    fn serializes_a_list_field_as_repeated_elements() {
        let now = chrono::Utc::now().naive_utc();
        let xml = HeroReadXml {
            id: 1,
            name: Some("Spectra".to_string()),
            powers: Some(vec!["flight".to_string(), "strength".to_string()]),
            power_level: None,
            owner_id: "alice".to_string(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let rendered = quick_xml::se::to_string_with_root("hero", &xml).unwrap();
        assert_eq!(rendered.matches("<powers>").count(), 2);
    }
}
