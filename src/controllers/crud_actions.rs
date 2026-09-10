//! Shared id/filter/bulk decision logic for the CRUD router -- port of
//! `controllers/crud_actions.py`. Only `heroes.rs` calls this today (this
//! port has one resource so far), but it's generic over any `R:
//! Repository` rather than Hero-specific, so a future sibling router
//! (item 4's XML router) reuses the same decision logic instead of
//! duplicating it -- the same role `crud_actions.py` plays for
//! `build_json_router`/`build_xml_router` in the reference.

use crate::crud::CrudService;
use crate::models::HasId;
use crate::problem_details::AppError;
use crate::repositories::filtering::{FilterClause, SortClause};
use crate::repositories::Repository;
use crate::views::bulk::{BulkDeleteResult, BulkUpdateResult};
use crate::views::FieldError;

/// A resolved `list`/`get`: one record (an `?id=` request) or a filtered/
/// sorted page of them.
pub enum ListOrGet<M> {
    One(M),
    Many(Vec<M>),
}

/// A resolved update: one record (an `?id=` request) or the result of a
/// bulk update over the given filters.
#[derive(Debug)]
pub enum UpdateOutcome<M> {
    One(M),
    Bulk(BulkUpdateResult),
}

/// A resolved delete: one record deleted (an `?id=` request, -> 204) or
/// the result of a bulk delete over the given filters.
#[derive(Debug)]
pub enum DeleteOutcome {
    One,
    Bulk(BulkDeleteResult),
}

const NO_TARGET_FIELD: &str = "filter";

fn no_target_error() -> AppError {
    AppError::UnprocessableEntity(vec![FieldError::new(
        NO_TARGET_FIELD,
        "id or at least one filter is required for a bulk action",
    )])
}

fn too_many_matched_error(matched: u64, max_matched: u64) -> AppError {
    AppError::UnprocessableEntity(vec![FieldError::new(
        NO_TARGET_FIELD,
        format!(
            "bulk action would affect {matched} records, over the {max_matched}-record limit -- narrow the filters"
        ),
    )])
}

fn not_found(id: i32) -> AppError {
    AppError::NotFound(format!("record {id} not found"))
}

/// Return one record by id, or a filtered/sorted page of matching records.
pub async fn resolve_list_or_get<R>(
    crud: &CrudService<R>,
    id: Option<i32>,
    skip: u64,
    limit: u64,
    include_archived: bool,
    filters: Vec<FilterClause>,
    sort: Vec<SortClause>,
) -> Result<ListOrGet<R::Model>, AppError>
where
    R: Repository,
{
    if let Some(id) = id {
        let record = crud.get(id, include_archived).await?;
        let record = record.ok_or_else(|| not_found(id))?;
        return Ok(ListOrGet::One(record));
    }
    let records = crud
        .list(skip, limit, include_archived, filters, sort)
        .await?;
    Ok(ListOrGet::Many(records))
}

/// Update one record by id, or bulk-update every record matching the
/// given filters (scoped to `owner_id`, capped by `max_matched`).
pub async fn resolve_update<R>(
    crud: &CrudService<R>,
    id: Option<i32>,
    owner_id: &str,
    filters: Vec<FilterClause>,
    data: R::Update,
    max_matched: u64,
) -> Result<UpdateOutcome<R::Model>, AppError>
where
    R: Repository,
    R::Model: HasId,
{
    if let Some(id) = id {
        let updated = resolve_update_by_id(crud, id, owner_id, data).await?;
        return Ok(UpdateOutcome::One(updated));
    }
    if filters.is_empty() {
        return Err(no_target_error());
    }
    let matched = crud.count(&filters, false).await?;
    if matched > max_matched {
        return Err(too_many_matched_error(matched, max_matched));
    }
    let updated = crud.update_many(owner_id, filters, data).await?;
    let ids = updated.iter().map(HasId::id).collect();
    Ok(UpdateOutcome::Bulk(BulkUpdateResult {
        matched: updated.len(),
        ids,
    }))
}

/// Update one record by id -- the id-only half of `resolve_update`,
/// factored out for callers (`heroes_web.rs`) that only ever update by
/// id and would otherwise have to handle an `UpdateOutcome::Bulk` arm
/// their own logic can never produce.
pub async fn resolve_update_by_id<R>(
    crud: &CrudService<R>,
    id: i32,
    owner_id: &str,
    data: R::Update,
) -> Result<R::Model, AppError>
where
    R: Repository,
{
    let updated = crud.update(id, owner_id, data).await?;
    updated.ok_or_else(|| not_found(id))
}

/// Delete one record by id, or bulk-delete every record matching the
/// given filters (scoped to `owner_id`, capped by `max_matched`).
pub async fn resolve_delete<R>(
    crud: &CrudService<R>,
    id: Option<i32>,
    owner_id: &str,
    filters: Vec<FilterClause>,
    max_matched: u64,
) -> Result<DeleteOutcome, AppError>
where
    R: Repository,
    R::Model: HasId,
{
    if let Some(id) = id {
        let deleted = crud.delete(id, owner_id).await?;
        if !deleted {
            return Err(not_found(id));
        }
        return Ok(DeleteOutcome::One);
    }
    if filters.is_empty() {
        return Err(no_target_error());
    }
    let matched = crud.count(&filters, false).await?;
    if matched > max_matched {
        return Err(too_many_matched_error(matched, max_matched));
    }
    let deleted = crud.delete_many(owner_id, filters).await?;
    let ids = deleted.iter().map(HasId::id).collect();
    Ok(DeleteOutcome::Bulk(BulkDeleteResult {
        matched: deleted.len(),
        ids,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crud::CrudService;
    use crate::repositories::filtering::{FilterOp, FilterValue};
    use crate::repositories::hero_memory::HeroMemoryRepository;
    use crate::views::hero::{HeroCreate, HeroUpdate};

    fn service() -> CrudService<HeroMemoryRepository> {
        CrudService::new(HeroMemoryRepository::new())
    }

    fn filter() -> Vec<FilterClause> {
        vec![FilterClause {
            field: "owner_id".to_string(),
            op: FilterOp::Eq,
            value: FilterValue::Str("alice".to_string()),
        }]
    }

    async fn seed(crud: &CrudService<HeroMemoryRepository>, n: usize) {
        for i in 0..n {
            crud.create(
                "alice",
                HeroCreate {
                    name: format!("Hero {i}"),
                    powers: vec!["flight".to_string()],
                    power_level: None,
                },
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn resolve_delete_by_id_returns_not_found_when_missing() {
        let crud = service();
        let result = resolve_delete(&crud, Some(999), "alice", vec![], 1000).await;
        let err = result.expect_err("deleting a nonexistent id must 404");
        assert!(matches!(err, AppError::NotFound(_)));
    }

    #[tokio::test]
    async fn resolve_update_rejects_a_bulk_action_over_the_max_matched_cap() {
        let crud = service();
        seed(&crud, 3).await;
        let result = resolve_update(
            &crud,
            None,
            "alice",
            filter(),
            HeroUpdate {
                name: Some("Renamed".to_string()),
                ..Default::default()
            },
            2,
        )
        .await;
        let err = result.expect_err("matching more records than max_matched must be rejected");
        assert!(matches!(err, AppError::UnprocessableEntity(_)));
    }

    #[tokio::test]
    async fn resolve_delete_rejects_a_bulk_action_over_the_max_matched_cap() {
        let crud = service();
        seed(&crud, 3).await;
        let result = resolve_delete(&crud, None, "alice", filter(), 2).await;
        let err = result.expect_err("matching more records than max_matched must be rejected");
        assert!(matches!(err, AppError::UnprocessableEntity(_)));
    }
}
