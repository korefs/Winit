use crate::{domain::*, infrastructure::storage::SavedData};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDocument {
    pub schema_version: u32,
    pub apps: Vec<CustomApp>,
}
pub fn validate(apps: &[CustomApp]) -> Result<()> {
    if apps.len() > 500 {
        bail!("O catálogo suporta até 500 aplicativos personalizados");
    }
    let (mut ids, mut names, mut origins) = (HashSet::new(), HashSet::new(), HashSet::new());
    for app in apps {
        app.validate()?;
        if !ids.insert(app.id)
            || !names.insert(normalized_name(&app.name))
            || !origins.insert(app.origin_key())
        {
            bail!("Cadastro duplicado no catálogo: {}", app.name);
        }
    }
    if serde_json::to_vec_pretty(&CatalogDocument {
        schema_version: 1,
        apps: apps.to_vec(),
    })?
    .len()
        > 4 * 1024 * 1024
    {
        bail!("O catálogo excede 4 MB");
    }
    Ok(())
}
pub fn save(data: &mut SavedData, app: CustomApp) -> Result<()> {
    app.validate()?;
    let mut apps = data.custom_apps.clone();
    if let Some(i) = apps.iter().position(|a| a.id == app.id) {
        apps[i] = app.clone();
    } else {
        apps.push(app.clone());
    }
    validate(&apps)?;
    data.custom_apps = apps;
    for profile in &mut data.profiles {
        for embedded in &mut profile.custom_apps {
            if embedded.id == app.id {
                *embedded = app.clone();
            }
        }
    }
    Ok(())
}
pub fn remove(data: &mut SavedData, ids: &HashSet<Uuid>) -> Result<()> {
    if data.history.iter().any(|o| {
        matches!(o.state, OperationState::Pending | OperationState::Running)
            && o.request
                .custom_app
                .as_ref()
                .is_some_and(|a| ids.contains(&a.id))
    }) {
        bail!("Aguarde ou cancele as operações desses aplicativos antes de excluir");
    }
    data.custom_apps.retain(|a| !ids.contains(&a.id));
    let keep = |p: &PackageRef| {
        p.provider != ProviderId::Custom
            || Uuid::parse_str(&p.id)
                .map(|id| !ids.contains(&id))
                .unwrap_or(true)
    };
    data.favorites.retain(keep);
    for p in &mut data.profiles {
        p.packages.retain(keep);
        p.custom_apps.retain(|a| !ids.contains(&a.id));
    }
    // Pendências e histórico são snapshots independentes; não são apagados em cascata.
    Ok(())
}
pub fn add_example(data: &mut SavedData) -> Result<Uuid> {
    let seed = CustomApp::duodesk_example();
    if let Some(existing) = data.custom_apps.iter().find(|a| {
        a.id == seed.id
            || a.origin_key() == seed.origin_key()
            || normalized_name(&a.name) == normalized_name(&seed.name)
    }) {
        return Ok(existing.id);
    }
    let id = seed.id;
    save(data, seed)?;
    Ok(id)
}
pub fn get<'a>(data: &'a SavedData, id: &str) -> Result<&'a CustomApp> {
    let id = Uuid::parse_str(id)?;
    data.custom_apps.iter().find(|a|a.id==id).context("Aplicativo personalizado não encontrado. Importe seu catálogo ou a definição incorporada no perfil.")
}
pub fn export(path: &Path, apps: &[CustomApp]) -> Result<()> {
    validate(apps)?;
    crate::infrastructure::storage::atomic_write(
        path,
        &serde_json::to_vec_pretty(&CatalogDocument {
            schema_version: 1,
            apps: apps.to_vec(),
        })?,
    )
}
pub fn parse(json: &str) -> Result<CatalogDocument> {
    if json.len() > 4 * 1024 * 1024 {
        bail!("O catálogo excede 4 MB");
    }
    let doc: CatalogDocument = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .context("JSON de catálogo inválido")?;
    if doc.schema_version != 1 {
        bail!("Versão de catálogo não suportada");
    }
    validate(&doc.apps)?;
    Ok(doc)
}
pub fn import(path: &Path) -> Result<CatalogDocument> {
    if std::fs::metadata(path)?.len() > 4 * 1024 * 1024 {
        bail!("Catálogo maior que 4 MB");
    }
    parse(&std::fs::read_to_string(path)?)
}
pub fn merge(data: &mut SavedData, document: CatalogDocument) -> Result<()> {
    if document.schema_version != 1 {
        bail!("Versão de catálogo não suportada");
    }
    validate(&document.apps)?;
    let mut copy = data.clone();
    for app in document.apps {
        save(&mut copy, app)?;
    }
    *data = copy;
    Ok(())
}
pub fn hydrate_profile(profile: &mut Profile, apps: &[CustomApp]) -> Result<()> {
    let mut embedded = vec![];
    for p in &profile.packages {
        if p.provider == ProviderId::Custom {
            let id = Uuid::parse_str(&p.id)?;
            let app=apps.iter().find(|a|a.id==id).or_else(||profile.custom_apps.iter().find(|a|a.id==id)).context("Perfil contém um aplicativo personalizado ausente do catálogo e sem definição incorporada")?;
            embedded.push(app.clone());
        }
    }
    profile.custom_apps = embedded;
    profile.validate()
}
pub fn import_profile_definitions(data: &mut SavedData, profile: &mut Profile) -> Result<()> {
    profile.validate()?;
    let mut copy = data.clone();
    let mut imported = profile.clone();
    for app in &profile.custom_apps {
        // Cadastros locais têm precedência. Equivalentes com outro UUID são reconciliados.
        let existing = copy
            .custom_apps
            .iter()
            .find(|a| a.id == app.id || a.origin_key() == app.origin_key())
            .map(|a| a.id);
        if let Some(id) = existing {
            for reference in &mut imported.packages {
                if reference.provider == ProviderId::Custom
                    && reference.id.eq_ignore_ascii_case(&app.id.to_string())
                {
                    reference.id = id.to_string();
                }
            }
        } else {
            save(&mut copy, app.clone())?;
        }
    }
    hydrate_profile(&mut imported, &copy.custom_apps)?;
    *data = copy;
    *profile = imported;
    Ok(())
}
