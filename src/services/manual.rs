use crate::{domain::*, infrastructure::storage::SavedData};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

pub fn save(data: &mut SavedData, mut item: ManualPending) -> Result<Uuid> {
    item.validate()?;
    item.updated_at = Utc::now();
    if let Some(existing) = data.manual_pending.iter_mut().find(|p| p.id == item.id) {
        item.created_at = existing.created_at;
        *existing = item.clone();
    } else {
        if data.manual_pending.len() >= 1000 {
            bail!("Limite de 1000 pendências atingido");
        }
        data.manual_pending.push(item.clone());
    }
    Ok(item.id)
}
pub fn register(data: &mut SavedData, item: ManualPending) -> Result<Uuid> {
    if let Some(existing) = data.manual_pending.iter().find(|p| {
        p.profile == item.profile
            && ((item.app_id.is_some() && p.app_id == item.app_id)
                || (item.app_id.is_none()
                    && p.app_id.is_none()
                    && normalized_name(&p.name) == normalized_name(&item.name)
                    && p.website == item.website))
    }) {
        let mut updated = item;
        updated.id = existing.id;
        updated.created_at = existing.created_at;
        updated.status = ManualStatus::Pending;
        save(data, updated)
    } else {
        save(data, item)
    }
}
pub fn set_completed(data: &mut SavedData, id: Uuid, completed: bool) -> Result<()> {
    let item = data
        .manual_pending
        .iter_mut()
        .find(|p| p.id == id)
        .context("Pendência não encontrada")?;
    item.set_completed(completed);
    Ok(())
}
